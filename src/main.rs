use std::{collections::BTreeMap, convert::TryInto, env, path::PathBuf, sync::Arc};

use counter::Counter;
use dotenv::dotenv;
use glob::glob;
use moka::future::Cache;
use once_cell::sync::Lazy;
use rand::{prelude::StdRng, seq::SliceRandom, SeedableRng};
use regex::Regex;
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    framework::{
        standard::{
            macros::{command, group},
            CommandResult,
        },
        StandardFramework,
    },
    model::{channel::Message, gateway::Ready, misc::Mentionable},
    prelude::*,
    Result as SerenityResult,
};
use songbird::{
    input::{
        self,
        cached::{Compressed, Memory},
        Input,
    },
    tracks::create_player,
    SerenityInit,
};

struct SoundDetail {
    path: PathBuf,
    sample_rate_hz: u32,
    is_stereo: bool,
}

impl SoundDetail {
    fn new(path: PathBuf, sample_rate_hz: u32, is_stereo: bool) -> Self {
        Self {
            path,
            sample_rate_hz,
            is_stereo,
        }
    }
}

static SAY_REG: Lazy<Mutex<Regex>> =
    Lazy::new(|| Mutex::new(Regex::new(r"^\s*([-_!^~0-9a-zA-Z]+)\s*(@?(\d{2,3}))?$").unwrap()));

// TODO: store various details such as length
static SOUND_DETAILS: Lazy<Mutex<BTreeMap<String, SoundDetail>>> = Lazy::new(|| {
    let mut path_map = BTreeMap::new();
    for path in (glob(&format!("{}/*.mp3", env::var("SOUND_DIR").unwrap())).unwrap()).flatten() {
        let data = mp3_metadata::read_from_file(path.clone());
        if data.is_err() {
            println!("invalid: {:?}", path);
        }

        let freqs: Counter<_> = data
            .as_ref()
            .unwrap()
            .frames
            .iter()
            .map(|f| f.sampling_freq)
            .collect();
        let sample_rate_hz = freqs.most_common()[0].0 as u32;

        let chan_types: Counter<_> = data
            .unwrap()
            .frames
            .iter()
            .map(|f| f.chan_type == mp3_metadata::ChannelType::SingleChannel)
            .collect();
        let is_stereo = !chan_types.most_common()[0].0;

        path_map.insert(
            path.file_stem().unwrap().to_str().unwrap().to_string(),
            SoundDetail::new(path, sample_rate_hz, is_stereo),
        );
    }
    Mutex::new(path_map)
});

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // saysound-spam channel
        if msg.channel_id != 921678977662332928 {
            return;
        }

        let caps = { SAY_REG.lock().await.captures(&msg.content) };
        if caps.is_none() {
            return;
        }
        let caps = caps.unwrap();

        let guild = msg.guild(&ctx.cache).await.unwrap();
        let guild_id = guild.id;

        let manager = songbird::get(&ctx)
            .await
            .expect("Songbird Voice client placed in at initialisation.")
            .clone();

        if let Some(handler_lock) = manager.get(guild_id) {
            if let Some(name) = caps.get(1).map(|m| m.as_str().to_string()) {
                let speed = caps
                    .get(3)
                    .map(|m| m.as_str().parse().unwrap())
                    .unwrap_or(100);
                let sound = SoundInfo::new(name.clone(), speed);

                let sources_lock = ctx
                    .data
                    .read()
                    .await
                    .get::<SoundStore>()
                    .cloned()
                    .expect("Sound cache was installed at startup.");
                let sources = sources_lock.lock().await;

                let details = SOUND_DETAILS.lock().await;

                if let Some(source) = sources.get(&sound) {
                    let (mut audio, _audio_handle) = create_player((&*source).into());
                    audio.set_volume(0.05);
                    let mut handler = handler_lock.lock().await;
                    handler.play(audio);
                } else if let Some(detail) = details.get(&name) {
                    let audio_filters = [
                        format!("asetrate={}*{}/100", detail.sample_rate_hz, speed),
                        format!("aresample={}", detail.sample_rate_hz),
                    ];
                    let mem = Memory::new(
                        input::ffmpeg_optioned(
                            detail.path.clone(),
                            &[],
                            &[
                                "-f",
                                "s16le",
                                "-ac",
                                if detail.is_stereo { "2" } else { "1" },
                                "-ar",
                                "48000",
                                "-acodec",
                                "pcm_f32le",
                                "-af",
                                &audio_filters.join(","),
                                "-",
                            ],
                        )
                        .await
                        .expect("File should be in root folder."),
                    )
                    .expect("These parameters are well-defined.");
                    let _ = mem.raw.spawn_loader();
                    let source = CachedSound::Uncompressed(mem);
                    let (mut audio, _audio_handle) = create_player((&source).into());
                    audio.set_volume(0.05);
                    let mut handler = handler_lock.lock().await;
                    handler.play(audio);
                    sources.insert(sound, Arc::new(source)).await;
                }
            }
        }
    }
}

enum CachedSound {
    #[allow(dead_code)]
    Compressed(Compressed),

    Uncompressed(Memory),
}

impl From<&CachedSound> for Input {
    fn from(obj: &CachedSound) -> Self {
        use CachedSound::*;
        match obj {
            Compressed(c) => c.new_handle().into(),
            Uncompressed(u) => u
                .new_handle()
                .try_into()
                .expect("Failed to create decoder for Memory source."),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct SoundInfo {
    name: String,
    speed: u32,
}

impl SoundInfo {
    const fn new(name: String, speed: u32) -> Self {
        Self { name, speed }
    }
}

struct SoundStore;

impl TypeMapKey for SoundStore {
    type Value = Arc<Mutex<Cache<SoundInfo, Arc<CachedSound>>>>;
}

struct PathStore;

impl TypeMapKey for PathStore {
    type Value = Arc<Mutex<BTreeMap<String, PathBuf>>>;
}

#[group]
#[commands(deafen, join, leave, mute, undeafen, unmute, s, r, stop)]
struct General;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    dotenv()?;

    // Configure the client with your Discord bot token in the environment.
    let args: Vec<String> = env::args().collect();
    let token = args
        .get(1)
        .map(String::from)
        .unwrap_or(env::var("DISCORD_TOKEN").unwrap());

    let framework = StandardFramework::new()
        .configure(|c| c.prefix("~"))
        .group(&GENERAL_GROUP);

    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Err creating client");

    {
        let mut data = client.data.write().await;
        data.insert::<SoundStore>(Arc::new(Mutex::new(Cache::new(100))));
    }

    let _ = client
        .start()
        .await
        .map_err(|why| println!("Client ended: {:?}", why));

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn deafen(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            check_msg(msg.reply(ctx, "Not in a voice channel").await);

            return Ok(());
        }
    };

    let mut handler = handler_lock.lock().await;

    if handler.is_deaf() {
        check_msg(msg.channel_id.say(&ctx.http, "Already deafened").await);
    } else {
        if let Err(e) = handler.deafen(true).await {
            check_msg(
                msg.channel_id
                    .say(&ctx.http, format!("Failed: {:?}", e))
                    .await,
            );
        }

        check_msg(msg.channel_id.say(&ctx.http, "Deafened").await);
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn join(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let channel_id = guild
        .voice_states
        .get(&msg.author.id)
        .and_then(|voice_state| voice_state.channel_id);

    let connect_to = match channel_id {
        Some(channel) => channel,
        None => {
            check_msg(msg.reply(ctx, "Not in a voice channel").await);

            return Ok(());
        }
    };

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let (_handler_lock, success_reader) = manager.join(guild_id, connect_to).await;

    if let Ok(_reader) = success_reader {
        check_msg(
            msg.channel_id
                .say(&ctx.http, &format!("Joined {}", connect_to.mention()))
                .await,
        );
    } else {
        check_msg(
            msg.channel_id
                .say(&ctx.http, "Error joining the channel")
                .await,
        );
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn leave(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();
    let has_handler = manager.get(guild_id).is_some();

    if has_handler {
        if let Err(e) = manager.remove(guild_id).await {
            check_msg(
                msg.channel_id
                    .say(&ctx.http, format!("Failed: {:?}", e))
                    .await,
            );
        }

        check_msg(msg.channel_id.say(&ctx.http, "Left voice channel").await);
    } else {
        check_msg(msg.reply(ctx, "Not in a voice channel").await);
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn mute(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            check_msg(msg.reply(ctx, "Not in a voice channel").await);

            return Ok(());
        }
    };

    let mut handler = handler_lock.lock().await;

    if handler.is_mute() {
        check_msg(msg.channel_id.say(&ctx.http, "Already muted").await);
    } else {
        if let Err(e) = handler.mute(true).await {
            check_msg(
                msg.channel_id
                    .say(&ctx.http, format!("Failed: {:?}", e))
                    .await,
            );
        }

        check_msg(msg.channel_id.say(&ctx.http, "Now muted").await);
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn undeafen(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;

        if let Err(e) = handler.deafen(false).await {
            check_msg(
                msg.channel_id
                    .say(&ctx.http, format!("Failed: {:?}", e))
                    .await,
            );
        }

        check_msg(msg.channel_id.say(&ctx.http, "Undeafened").await);
    } else {
        check_msg(
            msg.channel_id
                .say(&ctx.http, "Not in a voice channel to undeafen in")
                .await,
        );
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn unmute(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;
    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;

        if let Err(e) = handler.mute(false).await {
            check_msg(
                msg.channel_id
                    .say(&ctx.http, format!("Failed: {:?}", e))
                    .await,
            );
        }

        check_msg(msg.channel_id.say(&ctx.http, "Unmuted").await);
    } else {
        check_msg(
            msg.channel_id
                .say(&ctx.http, "Not in a voice channel to unmute in")
                .await,
        );
    }

    Ok(())
}

/// Checks that a message successfully sent; if not, then logs why to stdout.
fn check_msg(result: SerenityResult<Message>) {
    if let Err(why) = result {
        println!("Error sending message: {:?}", why);
    }
}

#[command]
async fn s(ctx: &Context, msg: &Message) -> CommandResult {
    if let Some(query) = msg.content.split_whitespace().collect::<Vec<_>>().get(1) {
        let lock = SOUND_DETAILS.lock().await;
        let mut sims: Vec<_> = lock
            .keys()
            .map(|k| (k, strsim::jaro_winkler(query, &k.to_lowercase())))
            .collect();
        sims.sort_by(|(_, d1), (_, d2)| d2.partial_cmp(d1).unwrap());
        check_msg(
            msg.channel_id
                .say(
                    &ctx.http,
                    &sims[..10]
                        .iter()
                        .cloned()
                        .map(|(name, _)| name)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .await,
        );
    }
    Ok(())
}

#[command]
async fn r(ctx: &Context, msg: &Message) -> CommandResult {
    let speed = msg
        .content
        .split_whitespace()
        .collect::<Vec<_>>()
        .get(1)
        .map(|s| s.to_owned())
        .unwrap_or("100")
        .parse::<u32>()
        .unwrap_or(100);
    let lock = SOUND_DETAILS.lock().await;
    let names: Vec<_> = lock.keys().collect();
    let mut rng: StdRng = SeedableRng::from_entropy();
    if let Some(mut result) = names.choose(&mut rng).map(|r| r.to_string()) {
        if speed != 100 {
            result += &format!(" {}", speed);
        }
        check_msg(msg.channel_id.say(&ctx.http, result).await);
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn stop(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();
    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            check_msg(msg.reply(ctx, "Not in a voice channel").await);

            return Ok(());
        }
    };
    let mut handler = handler_lock.lock().await;
    handler.stop();

    Ok(())
}