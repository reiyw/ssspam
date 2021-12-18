use std::{collections::BTreeMap, convert::TryInto, env, path::PathBuf, sync::Arc};

use dotenv::dotenv;
use glob::glob;
use moka::future::Cache;
use once_cell::sync::Lazy;
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

static SAY_REG: Lazy<Mutex<Regex>> =
    Lazy::new(|| Mutex::new(Regex::new(r"^\s*([-_!^~0-9a-zA-Z]+)\s*(@?(\d{2,3}))?$").unwrap()));

// TODO: store various details such as length
static SOUND_DETAILS: Lazy<Mutex<BTreeMap<String, PathBuf>>> = Lazy::new(|| {
    let mut path_map = BTreeMap::new();
    for path in (glob(&format!("{}/*.mp3", env::var("SOUND_DIR").unwrap())).unwrap()).flatten() {
        path_map.insert(
            path.file_stem().unwrap().to_str().unwrap().to_string(),
            path,
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

                let paths = SOUND_DETAILS.lock().await;

                if let Some(source) = sources.get(&sound) {
                    let (mut audio, _audio_handle) = create_player((&*source).into());
                    audio.set_volume(0.05);
                    let mut handler = handler_lock.lock().await;
                    handler.play_only(audio);
                } else if let Some(path) = paths.get(&name) {
                    let mem = Memory::new(
                        input::ffmpeg_optioned(
                            path,
                            &[],
                            &[
                                "-f",
                                "s16le",
                                "-ac",
                                "2",
                                "-ar",
                                "48000",
                                "-acodec",
                                "pcm_f32le",
                                "-af",
                                &format!("asetrate=44100*{}/100,aresample=44100", speed),
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
                    handler.play_only(audio);
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
#[commands(deafen, join, leave, mute, undeafen, unmute, s)]
struct General;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    dotenv()?;

    // Configure the client with your Discord bot token in the environment.
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

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

        // TODO: make static
        // TODO: store various details such as length
        let mut path_map = BTreeMap::new();

        for path in (glob(&format!("{}/*.mp3", env::var("SOUND_DIR")?))?).flatten() {
            path_map.insert(
                path.file_stem().unwrap().to_str().unwrap().to_string(),
                path,
            );
        }

        data.insert::<PathStore>(Arc::new(Mutex::new(path_map)));
    }

    {
        let mut data = client.data.write().await;
        data.insert::<SoundStore>(Arc::new(Mutex::new(Cache::new(1_000))));
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
        let paths_lock = ctx
            .data
            .read()
            .await
            .get::<PathStore>()
            .cloned()
            .expect("Path cache was installed at startup.");
        let paths = paths_lock.lock().await;
        let mut paths: Vec<_> = paths
            .keys()
            .map(|k| (k, strsim::jaro_winkler(query, &k.to_lowercase())))
            .collect();
        paths.sort_by(|(_, d1), (_, d2)| d2.partial_cmp(d1).unwrap());
        check_msg(
            msg.channel_id
                .say(
                    &ctx.http,
                    &paths[..10]
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
