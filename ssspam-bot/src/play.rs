use std::{
    cmp,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use moka::sync::Cache;
use parking_lot::RwLock;
use serenity::{client::Context, model::id::GuildId, prelude::TypeMapKey};
use songbird::{
    input::cached::Memory,
    tracks::{Track, TrackHandle},
    Call,
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::{sslang::Action, SayCommand, SayCommands, SoundFile, SoundStorage};

static MAX_PLAYABLE_DURATION: Duration = Duration::from_secs(180);
static VOLUME: f32 = 0.05;

#[derive(Clone)]
pub struct SaySoundCache {
    cache: Cache<SayCommand, Arc<DecodedSaySound>>,
}

impl SaySoundCache {
    pub fn new(max_capacity: u64, time_to_idle: Duration) -> Self {
        Self {
            cache: Cache::builder()
                .max_capacity(max_capacity)
                .time_to_idle(time_to_idle)
                .build(),
        }
    }

    fn get(&self, say_command: &SayCommand) -> Option<Arc<DecodedSaySound>> {
        self.cache.get(say_command)
    }

    fn insert(&mut self, say_command: SayCommand, say_sound: Arc<DecodedSaySound>) {
        self.cache.insert(say_command, say_sound);
    }

    pub fn clean(&mut self) {
        self.cache.invalidate_all();
    }
}

impl TypeMapKey for SaySoundCache {
    type Value = Arc<RwLock<Self>>;
}

#[derive(Clone)]
struct DecodedSaySound {
    decoded_data: Memory,

    /// Duration to block until next say sound is played.
    blocking_duration: Duration,

    /// Duration of this say sound.
    playing_duration: Duration,
}

impl DecodedSaySound {
    #[tracing::instrument]
    async fn from_command_and_file(command: &SayCommand, file: &SoundFile) -> anyhow::Result<Self> {
        let decoded_data = decode(command, file).await?;

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

        Ok(Self {
            decoded_data,
            blocking_duration,
            playing_duration,
        })
    }
}

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

    let ffmpeg_out = Command::new("ffmpeg")
        .args([
            "-ss",
            &format!("{}ms", command.start),
            "-t",
            &t_opt_value,
            "-i",
            &file.path.to_str().unwrap(),
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
        .output()?;
    Ok(Memory::new(ffmpeg_out.stdout.into()).await?)
}

#[tracing::instrument]
async fn process_say_commands(
    say_commands: SayCommands,
    ctx: &Context,
) -> anyhow::Result<Vec<Arc<DecodedSaySound>>> {
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

    let mut decoded_sounds = Vec::new();
    for say_command in say_commands.into_iter() {
        if let Some(decoded) = cache.read().get(&say_command) {
            decoded_sounds.push(decoded);
            continue;
        }

        let sound_file = { storage.read().get(&say_command.name) };
        if let Some(sound_file) = sound_file {
            let decoded =
                match DecodedSaySound::from_command_and_file(&say_command, &sound_file).await {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        warn!("Error decoding: {e:?}");
                        continue;
                    }
                };
            let decoded = Arc::new(decoded);
            cache.write().insert(say_command, Arc::clone(&decoded));

            decoded_sounds.push(decoded);
        }
    }

    Ok(decoded_sounds)
}

#[tracing::instrument]
pub async fn play_say_commands(
    say_commands: SayCommands,
    ctx: &Context,
    guild_id: GuildId,
) -> anyhow::Result<()> {
    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = manager
        .get(guild_id)
        .context("Could not get the call handler for the given guild")?;

    let decoded_sounds = process_say_commands(say_commands, ctx).await?;

    let mut track_handles = Vec::new();
    let mut estimated_duration = Duration::from_secs(0);
    let mut elapsed = Duration::from_secs(0);
    let timeout_result = {
        let task = send_tracks(
            decoded_sounds,
            handler_lock,
            &mut track_handles,
            &mut estimated_duration,
            &mut elapsed,
        );
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
    decoded_sounds: Vec<Arc<DecodedSaySound>>,
    handler_lock: Arc<Mutex<Call>>,
    track_handles: &mut Vec<TrackHandle>,
    estimated_duration: &mut Duration,
    elapsed: &mut Duration,
) -> anyhow::Result<()> {
    for decoded_sound in decoded_sounds {
        *estimated_duration = cmp::max(
            *estimated_duration,
            *elapsed + decoded_sound.playing_duration,
        );

        let track_handle =
            play_sound(&decoded_sound.decoded_data, handler_lock.clone(), VOLUME).await;

        *elapsed += decoded_sound.blocking_duration;
        tokio::time::sleep(decoded_sound.blocking_duration).await;

        (*track_handles).push(track_handle);
    }
    Ok(())
}

pub async fn play_sound(mem: &Memory, handler_lock: Arc<Mutex<Call>>, volume: f32) -> TrackHandle {
    let mut handler = handler_lock.lock().await;

    handler.play(Track::new(mem.new_handle().into()).volume(volume))
}
