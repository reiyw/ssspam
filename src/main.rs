use std::{
    collections::{BTreeMap, HashMap},
    convert::TryInto,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use dotenv::dotenv;
use moka::future::Cache;
use once_cell::sync::Lazy;
use rand::{prelude::StdRng, seq::SliceRandom, SeedableRng};
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
    model::{
        channel::Message,
        gateway::Ready,
        id::{ChannelId, GuildId},
        misc::Mentionable,
        prelude::VoiceState,
    },
    prelude::*,
    Result as SerenityResult,
};
use songbird::{
    input::{
        self,
        cached::{Compressed, Memory},
        Input,
    },
    Call, SerenityInit,
};
use structopt::StructOpt;

use ssspambot::{
    load_sounds_try_from_cache,
    parser::{parse_say_commands, SayCommand},
    play_source, prettify_sounds, search_impl, SoundDetail,
};

static SOUND_DETAILS: Lazy<RwLock<BTreeMap<String, SoundDetail>>> =
    Lazy::new(|| RwLock::new(BTreeMap::new()));

async fn play_cmd(
    cmd: SayCommand,
    handler_lock: Arc<tokio::sync::Mutex<Call>>,
    sources_lock: Arc<tokio::sync::Mutex<Cache<SoundInfo, Arc<CachedSound>>>>,
) {
    let sound = SoundInfo::new(cmd.name.clone(), cmd.speed);

    {
        let sources = sources_lock.lock().await;
        if let Some(source) = sources.get(&sound) {
            play_source((&*source).into(), handler_lock.clone()).await;
            return;
        }
    }

    let detail = {
        let details = SOUND_DETAILS.read().await;
        let detail_opt = details.get(&cmd.name);
        if detail_opt.is_none() {
            return;
        }
        detail_opt.unwrap().clone()
    };

    let audio_filters = [
        format!("asetrate={}*{}/100", detail.sample_rate_hz, cmd.speed),
        format!("aresample={}", detail.sample_rate_hz),
    ];
    let mem = Memory::new(
        input::ffmpeg_optioned(
            detail.path,
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
    play_source((&source).into(), handler_lock.clone()).await;

    let sources = sources_lock.lock().await;
    sources.insert(sound, Arc::new(source)).await;
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Allow saysound-spam channel at css server or general channel at my server.
        if !(msg.channel_id == 921678977662332928 || msg.channel_id == 391743739430699010) {
            return;
        }

        let guild = msg.guild(&ctx.cache).await.unwrap();
        let guild_id = guild.id;

        let authors_voice_channel_id = guild
            .voice_states
            .get(&msg.author.id)
            .and_then(|voice_state| voice_state.channel_id);

        let bots_voice_channel_id = ctx
            .data
            .read()
            .await
            .get::<BotJoinningChannel>()
            .cloned()
            .unwrap()
            .lock()
            .await
            .get(&guild_id)
            .cloned();

        if authors_voice_channel_id != bots_voice_channel_id {
            return;
        }

        let cmds = parse_say_commands(&msg.content);
        if cmds.is_err() {
            return;
        }
        let cmds = cmds.unwrap();
        if cmds.is_empty() {
            return;
        }

        let manager = songbird::get(&ctx)
            .await
            .expect("Songbird Voice client placed in at initialisation.")
            .clone();

        let sources_lock = ctx
            .data
            .read()
            .await
            .get::<SoundStore>()
            .cloned()
            .expect("Sound cache was installed at startup.");

        if let Some(handler_lock) = manager.get(guild_id) {
            for cmd in cmds {
                play_cmd(cmd, handler_lock.clone(), sources_lock.clone()).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    async fn voice_state_update(
        &self,
        ctx: Context,
        _: Option<GuildId>,
        old_state: Option<VoiceState>,
        _: VoiceState,
    ) {
        if let Some(old_state) = old_state {
            let guild_id = old_state.guild_id.unwrap();
            let bots_voice_channel_id = ctx
                .data
                .read()
                .await
                .get::<BotJoinningChannel>()
                .cloned()
                .unwrap()
                .lock()
                .await
                .get(&guild_id)
                .cloned();
            if bots_voice_channel_id != old_state.channel_id {
                return;
            }

            if let Some(channel_id) = old_state.channel_id {
                let channel = ctx.cache.guild_channel(channel_id).await.unwrap();
                let members = channel.members(&ctx.cache).await.unwrap();
                if members.len() == 1 && members[0].user.bot {
                    let manager = songbird::get(&ctx)
                        .await
                        .expect("Songbird Voice client placed in at initialisation.")
                        .clone();
                    let has_handler = manager.get(guild_id).is_some();
                    if has_handler {
                        manager.remove(guild_id).await.unwrap();
                    }
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

struct BotJoinningChannel;

impl TypeMapKey for BotJoinningChannel {
    type Value = Arc<Mutex<HashMap<GuildId, ChannelId>>>;
}

#[group]
#[commands(join, leave, mute, unmute, s, st, r, stop)]
struct General;

#[derive(Debug, StructOpt)]
#[structopt(name = "ssspam")]
struct Opt {
    #[structopt(long, env)]
    discord_token: String,

    #[structopt(long, parse(from_os_str), env)]
    sound_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    dotenv().ok();

    let opt = Opt::from_args();

    {
        let mut sound_details = SOUND_DETAILS.write().await;
        *sound_details = load_sounds_try_from_cache(opt.sound_dir);
    }

    let framework = StandardFramework::new()
        .configure(|c| c.prefix("~"))
        .group(&GENERAL_GROUP);

    let mut client = Client::builder(&opt.discord_token)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Err creating client");

    {
        let mut data = client.data.write().await;
        data.insert::<SoundStore>(Arc::new(Mutex::new(Cache::new(50))));
    }

    {
        let mut data = client.data.write().await;
        data.insert::<BotJoinningChannel>(Arc::new(Mutex::new(HashMap::new())));
    }

    let shard_manager = client.shard_manager.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Could not register ctrl+c handler");
        shard_manager.lock().await.shutdown_all().await;
    });

    let _ = client
        .start()
        .await
        .map_err(|why| println!("Client ended: {:?}", why));

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
        let voice_channels = ctx
            .data
            .read()
            .await
            .get::<BotJoinningChannel>()
            .cloned()
            .unwrap();
        voice_channels.lock().await.insert(guild_id, connect_to);
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
        let lock = SOUND_DETAILS.read().await;
        let ret = search_impl(*query, lock.keys(), 30);
        check_msg(
            msg.channel_id
                .say(
                    &ctx.http,
                    ret.iter()
                        .map(|(name, _)| name.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .await,
        );
    }
    Ok(())
}

#[command]
async fn st(ctx: &Context, msg: &Message) -> CommandResult {
    if let Some(query) = msg.content.split_whitespace().collect::<Vec<_>>().get(1) {
        let lock = SOUND_DETAILS.read().await;
        let ret = search_impl(*query, lock.keys(), 10);
        let out_msg = prettify_sounds(ret.iter().map(|(name, _)| lock.get(name).unwrap()).cloned());
        check_msg(
            msg.channel_id
                .say(&ctx.http, format!("```\n{}\n```", out_msg))
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
    let lock = SOUND_DETAILS.read().await;
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
