//! Example demonstrating how to store and convert audio streams which you
//! either want to reuse between servers, or to seek/loop on. See `join`, and `ting`.
//!
//! Requires the "cache", "standard_framework", and "voice" features be enabled in your
//! Cargo.toml, like so:
//!
//! ```toml
//! [dependencies.serenity]
//! git = "https://github.com/serenity-rs/serenity.git"
//! features = ["cache", "framework", "standard_framework", "voice"]
//! ```
use std::{
    collections::HashMap,
    convert::TryInto,
    env,
    path::PathBuf,
    sync::{Arc, Weak},
};

use dotenv::dotenv;
use glob::glob;

use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    framework::{
        standard::{
            macros::{command, group},
            Args, CommandResult,
        },
        StandardFramework,
    },
    model::{channel::Message, gateway::Ready, misc::Mentionable},
    prelude::Mutex,
    Result as SerenityResult,
};

use songbird::{
    driver::Bitrate,
    input::{
        self,
        cached::{Compressed, Memory},
        Input,
    },
    Call, Event, EventContext, EventHandler as VoiceEventHandler, SerenityInit, TrackEvent,
};
use songbird::{driver::Driver, ffmpeg, tracks::create_player};

// This imports `typemap`'s `Key` as `TypeMapKey`.
use serenity::prelude::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let guild = msg.guild(&ctx.cache).await.unwrap();
        let guild_id = guild.id;

        let manager = songbird::get(&ctx)
            .await
            .expect("Songbird Voice client placed in at initialisation.")
            .clone();

        if let Some(handler_lock) = manager.get(guild_id) {
            let mut handler = handler_lock.lock().await;

            let sources_lock = ctx
                .data
                .read()
                .await
                .get::<SoundStore>()
                .cloned()
                .expect("Sound cache was installed at startup.");
            let mut sources = sources_lock.lock().await;

            let paths_lock = ctx
                .data
                .read()
                .await
                .get::<PathStore>()
                .cloned()
                .expect("Path cache was installed at startup.");
            let paths = paths_lock.lock().await;

            let mut iter = msg.content.split_ascii_whitespace();
            if let Some(sound_name) = iter.next() {
                if let Some(source) = sources.get(sound_name) {
                    // let handle = handler.play_source(source.into());
                    let (mut audio, _audio_handle) = create_player(source.into());
                    audio.set_volume(0.05);
                    handler.play_only(audio);
                } else {
                    if let Some(path) = paths.get(sound_name) {
                        let mem = Memory::new(
                            input::ffmpeg(path)
                                .await
                                .expect("File should be in root folder."),
                        )
                        .expect("These parameters are well-defined.");
                        let _ = mem.raw.spawn_loader();
                        // let song_src = Compressed::new(
                        //     input::ffmpeg(path).await.expect("Link may be dead."),
                        //     Bitrate::BitsPerSecond(128),
                        // )
                        // .expect("These parameters are well-defined.");
                        // let _ = song_src.raw.spawn_loader();
                        let source = CachedSound::Uncompressed(mem);
                        // let handle = handler.play_source((&source).into());
                        let (mut audio, _audio_handle) = create_player((&source).into());
                        audio.set_volume(0.05);
                        handler.play_only(audio);
                        sources.insert(sound_name.into(), source);
                    }
                }
            }
        }
    }
}

enum CachedSound {
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

struct SoundStore;

impl TypeMapKey for SoundStore {
    type Value = Arc<Mutex<HashMap<String, CachedSound>>>;
}

struct PathStore;

impl TypeMapKey for PathStore {
    type Value = Arc<Mutex<HashMap<String, PathBuf>>>;
}

#[group]
#[commands(deafen, join, leave, mute, ting, undeafen, unmute)]
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

        let mut path_map = HashMap::new();

        for entry in glob(&format!("{}/*.mp3", env::var("SOUND_DIR")?))? {
            if let Ok(path) = entry {
                path_map.insert(
                    path.file_stem().unwrap().to_str().unwrap().to_string(),
                    path,
                );
            }
        }

        data.insert::<PathStore>(Arc::new(Mutex::new(path_map)));
    }

    {
        let mut data = client.data.write().await;
        data.insert::<SoundStore>(Arc::new(Mutex::new(HashMap::new())));
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

    let (handler_lock, success_reader) = manager.join(guild_id, connect_to).await;

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

struct LoopPlaySound {
    call_lock: Weak<Mutex<Call>>,
    sources: Arc<Mutex<HashMap<String, CachedSound>>>,
}

#[async_trait]
impl VoiceEventHandler for LoopPlaySound {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        if let Some(call_lock) = self.call_lock.upgrade() {
            let src = {
                let sources = self.sources.lock().await;
                sources
                    .get("loop")
                    .expect("Handle placed into cache at startup.")
                    .into()
            };

            let mut handler = call_lock.lock().await;
            let sound = handler.play_source(src);
            let _ = sound.set_volume(0.5);
        }

        None
    }
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
async fn ting(ctx: &Context, msg: &Message, _args: Args) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;

        let sources_lock = ctx
            .data
            .read()
            .await
            .get::<SoundStore>()
            .cloned()
            .expect("Sound cache was installed at startup.");
        let mut sources = sources_lock.lock().await;

        let paths_lock = ctx
            .data
            .read()
            .await
            .get::<PathStore>()
            .cloned()
            .expect("Path cache was installed at startup.");
        let paths = paths_lock.lock().await;

        let mut iter = msg.content.split_ascii_whitespace();
        iter.next();
        if let Some(sound_name) = iter.next() {
            if let Some(source) = sources.get(sound_name) {
                let _sound = handler.play_source(source.into());
            } else {
                if let Some(path) = paths.get(sound_name) {
                    let mem = Memory::new(
                        input::ffmpeg(path)
                            .await
                            .expect("File should be in root folder."),
                    )
                    .expect("These parameters are well-defined.");
                    let _ = mem.raw.spawn_loader();
                    // let song_src = Compressed::new(
                    //     input::ffmpeg(path).await.expect("Link may be dead."),
                    //     Bitrate::BitsPerSecond(128),
                    // )
                    // .expect("These parameters are well-defined.");
                    // let _ = song_src.raw.spawn_loader();
                    let source = CachedSound::Uncompressed(mem);
                    let _handle = handler.play_source((&source).into());
                    sources.insert(sound_name.into(), source);
                }
            }
        }

        // let source = sources
        //     .get("ting")
        //     .expect("Handle placed into cache at startup.");

        // let _sound = handler.play_source(source.into());
    } else {
        check_msg(
            msg.channel_id
                .say(&ctx.http, "Not in a voice channel to play in")
                .await,
        );
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
