use std::{
    cmp,
    process::{Command, Stdio},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use quick_cache::{Weighter, sync::Cache};
use serenity::{async_trait, client::Context, model::id::GuildId, prelude::TypeMapKey};
use songbird::{
    Call, Event, EventContext, EventHandler as VoiceEventHandler,
    constants::TIMESTEP_LENGTH,
    input::{
        ChildContainer, Input,
        cached::Memory,
        codecs::{get_codec_registry, get_probe},
    },
    tracks::{Track, TrackHandle},
};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tracing::{debug, info, warn};

use crate::{SayCommand, SayCommands, SoundFile, SoundStorage, sslang::Action};

static MAX_PLAYABLE_DURATION: Duration = Duration::from_secs(180);
static VOLUME: f32 = 0.05;

/// Bytes that a second of one channel of decoded audio takes: ffmpeg's output is 48 kHz f32.
const DECODED_BYTES_PER_CHANNEL_SECOND: u64 = 48_000 * 4;

/// The ffmpeg processes that may decode at once, across guilds. A message can hold a hundred say
/// sounds, each of which needs its own ffmpeg unless it is cached.
static FFMPEG_SLOTS: Semaphore = Semaphore::const_new(4);

/// The say sounds of a message that are decoded ahead of the one being played, so that each is
/// ready when its turn comes, even in the middle of a message.
const DECODE_AHEAD: usize = 8;

pub struct SaySoundCache {
    cache: Cache<SayCommand, Arc<DecodedSaySound>, DecodedSizeWeighter>,
}

impl SaySoundCache {
    /// A cache of the decoded say sounds that take up to `max_bytes` in all.
    pub fn new(max_bytes: u64) -> Self {
        // The sounds are 2.9 s long on average, 1.1 MB decoded in stereo.
        let estimated_items = (max_bytes / 1_000_000).max(1) as usize;
        Self {
            cache: Cache::with_weighter(estimated_items, max_bytes, DecodedSizeWeighter),
        }
    }

    fn get(&self, say_command: &SayCommand) -> Option<Arc<DecodedSaySound>> {
        self.cache.get(say_command)
    }

    fn insert(&self, say_command: SayCommand, say_sound: Arc<DecodedSaySound>) {
        self.cache.insert(say_command, say_sound);
    }

    pub fn clean(&self) {
        self.cache.clear()
    }
}

impl TypeMapKey for SaySoundCache {
    type Value = Arc<Self>;
}

#[derive(Clone)]
struct DecodedSizeWeighter;

impl Weighter<SayCommand, Arc<DecodedSaySound>> for DecodedSizeWeighter {
    fn weight(&self, _: &SayCommand, sound: &Arc<DecodedSaySound>) -> u64 {
        // An item of weight 0 would never be evicted.
        sound.decoded_bytes.max(1)
    }
}

#[derive(Clone)]
struct DecodedSaySound {
    /// The decoded audio, which ffmpeg may still be writing.
    decoded_data: Memory,

    /// Duration to block until next say sound is played.
    blocking_duration: Duration,

    /// Duration of this say sound.
    playing_duration: Duration,

    /// The bytes that the decoded audio takes once ffmpeg has written it all, estimated from its
    /// duration, since the cache needs them when ffmpeg has only started.
    decoded_bytes: u64,
}

impl DecodedSaySound {
    #[tracing::instrument]
    async fn from_command_and_file(command: &SayCommand, file: &SoundFile) -> anyhow::Result<Self> {
        let playing_duration = {
            let mut dur = cmp::max(
                (file.duration().as_millis() as i64) - command.start as i64,
                0,
            );
            if let Some(n) = command.duration {
                dur = cmp::min(dur, n as i64)
            }
            dur = ((dur as f64) * (100.0 / command.speed as f64)) as i64;
            if command.stop {
                dur = cmp::min(dur, command.wait as i64);
            }

            // Capped at 180 secs during encoding.
            dur = cmp::min(dur, 180 * 1000);

            Duration::from_millis(dur as u64)
        };

        let blocking_duration = match command.action {
            Action::Synthesize => Duration::from_millis(command.wait as u64),
            Action::Concat => playing_duration,
        };

        let decoded_bytes = playing_duration.as_millis() as u64
            * u64::from(file.channel_count())
            * DECODED_BYTES_PER_CHANNEL_SECOND
            / 1000;

        let decoded_data = decode(command, file).await?;

        Ok(Self {
            decoded_data,
            blocking_duration,
            playing_duration,
            decoded_bytes,
        })
    }
}

/// Starts ffmpeg on the say sound and returns its output as ffmpeg writes it: playing it waits only
/// for the beginning of the sound to be decoded, not for all of it.
#[tracing::instrument]
async fn decode(command: &SayCommand, file: &SoundFile) -> anyhow::Result<Memory> {
    let audio_filters = {
        let speed_multiplier = command.speed as f64 / 100.0;
        let pitch_multiplier = command.pitch as f64 / 100.0;
        let asetrate = file.sample_rate_hz() as f64 * speed_multiplier * pitch_multiplier;
        let atempo = 1.0 / pitch_multiplier;
        let mut afs = vec![
            format!("asetrate={asetrate}"),
            format!("atempo={atempo}"),
            format!("aresample={}", file.sample_rate_hz()),
        ];
        if let Some(af) = command.audio_filter.clone() {
            afs.push(af);
        }
        afs
    };

    let t_opt_value = match command.duration {
        Some(dur) => format!("{dur}ms"),
        None if command.stop => format!("{}ms", command.wait),
        None => "0".to_string(),
    };

    // Held until ffmpeg has written all of its output.
    let slot = FFMPEG_SLOTS.acquire().await?;

    let child = Command::new("ffmpeg")
        .args([
            "-ss",
            &format!("{}ms", command.start),
            "-t",
            &t_opt_value,
            "-i",
            (file.path.to_str().unwrap()),
            "-f",
            "wav",
            "-ac",
            &file.channel_count().to_string(),
            "-ar",
            "48000",
            "-acodec",
            "pcm_f32le",
            "-t",
            "180",
            "-af",
            &audio_filters.join(","),
            "-",
        ])
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    // ChildContainer kills and reaps ffmpeg when it is dropped, which Memory does once it has read
    // all of ffmpeg's output.
    let memory = Memory::new(ChildContainer::from(child).into()).await?;

    // Memory reads its source when one of its handles needs more than it holds, and the mixer plays
    // a handle on the thread that mixes every track. Reading all of ffmpeg's output here, as fast
    // as ffmpeg writes it, keeps the mixer's reads to what is already held: ffmpeg decodes a
    // hundred times faster than the sound plays.
    let mut loader = memory.new_handle();
    tokio::task::spawn_blocking(move || {
        loader.raw.load_all();
        drop(slot);
    });

    Ok(memory)
}

/// Decodes the say sounds, or gets them from the cache, and sends them in order, a few ahead of the
/// one being played.
async fn decode_say_commands(
    say_commands: SayCommands,
    cache: Arc<SaySoundCache>,
    storage: Arc<RwLock<SoundStorage>>,
    tx: mpsc::Sender<Arc<DecodedSaySound>>,
) {
    for say_command in say_commands {
        let decoded = if let Some(decoded) = cache.get(&say_command) {
            decoded
        } else {
            let Some(sound_file) = storage.read().unwrap().get(&say_command.name) else {
                continue;
            };
            let decoded =
                match DecodedSaySound::from_command_and_file(&say_command, &sound_file).await {
                    Ok(decoded) => Arc::new(decoded),
                    Err(e) => {
                        warn!("Error decoding: {e:?}");
                        continue;
                    }
                };
            // Cached while ffmpeg still writes it: the same say sound later in the message reads
            // what this one has decoded.
            cache.insert(say_command, Arc::clone(&decoded));
            decoded
        };

        if tx.send(decoded).await.is_err() {
            // Playing has stopped.
            break;
        }
    }
}

#[tracing::instrument]
pub async fn play_say_commands(
    say_commands: SayCommands,
    ctx: &Context,
    guild_id: GuildId,
) -> anyhow::Result<()> {
    let started = Instant::now();

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = manager
        .get(guild_id)
        .context("Could not get the call handler for the given guild")?;

    let cache = ctx
        .data
        .read()
        .await
        .get::<SaySoundCache>()
        .context("Could not get SaySoundCache")?
        .clone();
    let storage = ctx
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();

    let (tx, rx) = mpsc::channel(DECODE_AHEAD);

    let mut track_handles = Vec::new();
    let mut estimated_duration = Duration::from_secs(0);
    let mut elapsed = Duration::from_secs(0);
    let timeout_result = {
        let task = async {
            tokio::join!(
                decode_say_commands(say_commands, cache, storage, tx),
                send_tracks(
                    rx,
                    handler_lock,
                    &mut track_handles,
                    &mut estimated_duration,
                    &mut elapsed,
                    started,
                ),
            )
        };
        tokio::pin!(task);
        tokio::time::timeout(MAX_PLAYABLE_DURATION, &mut task).await
    };
    match timeout_result {
        Ok(_) => {
            if estimated_duration > MAX_PLAYABLE_DURATION {
                let sleep_dur = MAX_PLAYABLE_DURATION - elapsed;
                tokio::time::sleep(sleep_dur).await;
                for track_handle in track_handles.iter() {
                    track_handle.stop().ok();
                }
            }
        }
        Err(_) => {
            for track_handle in track_handles.iter() {
                track_handle.stop().ok();
            }
        }
    }

    Ok(())
}

async fn send_tracks(
    mut rx: mpsc::Receiver<Arc<DecodedSaySound>>,
    handler_lock: Arc<Mutex<Call>>,
    track_handles: &mut Vec<TrackHandle>,
    estimated_duration: &mut Duration,
    elapsed: &mut Duration,
    started: Instant,
) {
    while let Some(decoded_sound) = rx.recv().await {
        *estimated_duration = cmp::max(
            *estimated_duration,
            *elapsed + decoded_sound.playing_duration,
        );

        let decoded_at = started.elapsed();
        match play_sound(&decoded_sound.decoded_data, handler_lock.clone(), VOLUME).await {
            Ok((track_handle, parse_time)) => {
                if track_handles.is_empty() {
                    // Where the time went: decoded_ms until ffmpeg had started (or the cache had
                    // it), then parse_ms waiting for ffmpeg's first output, then the Call's lock.
                    info!(
                        decoded_ms = decoded_at.as_millis(),
                        parse_ms = parse_time.as_millis(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "First say sound handed to the mixer"
                    );
                    // Fires once the track has played for one frame, on the tick that mixed it.
                    track_handle
                        .add_event(Event::Delayed(TIMESTEP_LENGTH), LogFirstFrame { started })
                        .ok();
                }
                track_handles.push(track_handle);
            }
            Err(e) => warn!("Error playing a say sound: {e:?}"),
        }

        *elapsed += decoded_sound.blocking_duration;
        tokio::time::sleep(decoded_sound.blocking_duration).await;
    }
}

/// Logs when the mixer has mixed the first 20 ms frame of a message's first track.
struct LogFirstFrame {
    started: Instant,
}

#[async_trait]
impl VoiceEventHandler for LogFirstFrame {
    async fn act(&self, _: &EventContext<'_>) -> Option<Event> {
        info!(
            elapsed_ms = self.started.elapsed().as_millis(),
            "First say sound mixed"
        );
        Some(Event::Cancel)
    }
}

/// Parses the audio and hands it to the mixer, which mixes it from its next 20 ms tick.
///
/// Audio handed over unparsed, the mixer would send it to its thread pool to be parsed on one tick and
/// take it back on a later one: 45-59 ms in all, against the 20 ms at most that this waits.
///
/// Returns the track and how long parsing it took.
async fn play_sound(
    mem: &Memory,
    handler_lock: Arc<Mutex<Call>>,
    volume: f32,
) -> anyhow::Result<(TrackHandle, Duration)> {
    let parse_started = Instant::now();
    let input = parse(mem).await?;
    let parse_time = parse_started.elapsed();
    let mut handler = handler_lock.lock().await;
    Ok((handler.play(Track::new(input).volume(volume)), parse_time))
}

/// Parses the audio from its beginning, as the mixer would, waiting for ffmpeg to have written its
/// header if it has not yet.
async fn parse(mem: &Memory) -> anyhow::Result<Input> {
    Ok(Input::from(mem.new_handle())
        .make_playable_async(get_codec_registry(), get_probe())
        .await?)
}

/// How often [`keep_warm`] decodes a sound.
const WARM_UP_INTERVAL: Duration = Duration::from_secs(60);

/// A warm-up slower than this is logged at INFO, as a sign that ffmpeg had to be read from disk.
const SLOW_WARM_UP: Duration = Duration::from_millis(50);

/// Decodes and parses a random sound at start and then once a minute, with nobody listening, so that
/// ffmpeg stays in the page cache.
///
/// ffmpeg is a 200 MB static binary. After 10 to 25 minutes without a say sound, the first one
/// waited 0.8-5.5 s for ffmpeg's first output, against the 10-25 ms it takes otherwise, as ffmpeg
/// was read from disk again.
pub async fn keep_warm(storage: Arc<RwLock<SoundStorage>>) {
    let mut interval = tokio::time::interval(WARM_UP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        warm_up(&storage).await;
    }
}

async fn warm_up(storage: &RwLock<SoundStorage>) {
    let Some(file) = storage.read().unwrap().get_random() else {
        return;
    };
    let started = Instant::now();
    let command = SayCommand {
        name: file.name.clone(),
        ..Default::default()
    };
    let result = async { parse(&decode(&command, &file).await?).await }.await;
    let took = started.elapsed();
    match result {
        Ok(_) if took >= SLOW_WARM_UP => info!("Warmed up on {} in {took:?}", file.name),
        Ok(_) => debug!("Warmed up on {} in {took:?}", file.name),
        Err(e) => warn!("Error warming up on {}: {e:?}", file.name),
    }
}
