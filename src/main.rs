use std::{
    cmp,
    collections::{BTreeMap, HashMap},
    convert::TryInto,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use async_zip::read::mem::ZipFileReader;
use dotenv::dotenv;
use moka::future::Cache;
use once_cell::sync::{Lazy, OnceCell};
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
        id::{ChannelId, GuildId, UserId},
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
    SerenityInit,
};
use structopt::StructOpt;
use systemstat::{Platform, System};

use ssspambot::{
    load_sounds, load_sounds_try_from_cache,
    parser::{Action, Command, Commands, SayCommand},
    play_source, prettify_sounds, search_impl, SoundDetail,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::broadcast,
};

static SOUND_DETAILS: Lazy<RwLock<BTreeMap<String, SoundDetail>>> =
    Lazy::new(|| RwLock::new(BTreeMap::new()));

// TODO: manage for each guild
static MESSAGE_BROADCAST_CONNECT: Lazy<RwLock<broadcast::Sender<bool>>> =
    Lazy::new(|| RwLock::new(broadcast::channel(16).0));
static MESSAGE_BROADCAST_RECEIVER: Lazy<Mutex<broadcast::Receiver<bool>>> =
    Lazy::new(|| Mutex::new(broadcast::channel(16).1));

static SOUND_DIR: OnceCell<PathBuf> = OnceCell::new();

static ADMIN_USER_IDS: &[UserId] = &[
    UserId(310620137608970240), // auzen
    UserId(342903795380125698), // nicotti
];

async fn get_or_make_source(
    cmd: &SayCommand,
    sources_lock: Arc<tokio::sync::Mutex<Cache<SayCommand, Arc<CachedSound>>>>,
) -> Option<Arc<CachedSound>> {
    let cmd = {
        let mut cmd = cmd.clone();
        cmd.name = cmd.name.to_lowercase();
        cmd
    };

    {
        let sources = sources_lock.lock().await;
        let source = sources.get(&cmd);
        if source.is_some() {
            return source;
        }
    }

    let detail = {
        let details = SOUND_DETAILS.read().await;
        let detail_opt = details.get(&cmd.name.to_lowercase())?;
        detail_opt.clone()
    };

    let audio_filters = {
        let speed_multiplier = cmd.speed as f64 / 100.0;
        let pitch_multiplier = cmd.pitch as f64 / 100.0;
        let asetrate = detail.sample_rate_hz as f64 * speed_multiplier * pitch_multiplier;
        let atempo = 1.0 / pitch_multiplier;
        [
            format!("asetrate={}", asetrate),
            format!("atempo={}", atempo),
            format!("aresample={}", detail.sample_rate_hz),
        ]
    };

    let t_opt_value = match cmd.duration {
        Some(dur) => format!("{}ms", dur),
        None => "0".to_string(),
    };

    let mem = Memory::new(
        input::ffmpeg_optioned(
            detail.path,
            &["-ss", &format!("{}ms", cmd.start), "-t", &t_opt_value],
            &[
                "-f",
                "s16le",
                "-ac",
                &detail.channel_count.to_string(),
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
    let sources = sources_lock.lock().await;
    sources.insert(cmd.clone(), Arc::new(source)).await;
    sources.get(&cmd)
}

async fn process_message(ctx: &Context, msg: &Message) {
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

    let cmds = {
        let cmds = Commands::from_str(&msg.content);
        if cmds.is_err() {
            return;
        }
        let mut cmds = cmds.unwrap();
        if cmds.is_empty() {
            return;
        }
        cmds.sanitize();
        cmds
    };

    let manager = songbird::get(ctx)
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
        let mut sources: Vec<Option<Arc<CachedSound>>> = Vec::new();
        let mut playable_sound_count = 0;
        for cmd in cmds.iter() {
            if let Command::Say(saycmd) = cmd {
                let source = get_or_make_source(saycmd, sources_lock.clone()).await;
                let playable = source.is_some();
                sources.push(source);

                if playable {
                    playable_sound_count += 1;
                    if playable_sound_count == 10 {
                        break;
                    }
                }
            } else {
                sources.push(None);
            };
        }

        for (source, cmd) in sources.into_iter().zip(cmds.into_iter()) {
            match (source, cmd) {
                (Some(source), Command::Say(cmd)) => {
                    let track_handle = play_source((&*source).into(), handler_lock.clone()).await;

                    match cmd.action {
                        Action::Synthesize => {
                            tokio::time::sleep(Duration::from_millis(cmd.wait as u64)).await;
                        }
                        Action::Concat => {
                            let details = SOUND_DETAILS.read().await;
                            let detail = details.get(&cmd.name.to_lowercase()).unwrap();
                            let mut dur = cmp::max(
                                (detail.duration.as_millis() as i64) - cmd.start as i64,
                                0,
                            );
                            if let Some(n) = cmd.duration {
                                dur = cmp::min(dur, n as i64)
                            }
                            let dur = ((dur as f64) * (100.0 / cmd.speed as f64)) as u64;
                            tokio::time::sleep(Duration::from_millis(dur)).await;
                        }
                    }
                    if cmd.stop {
                        track_handle.stop().ok();
                    }
                }
                (None, Command::Wait(n)) => {
                    tokio::time::sleep(Duration::from_millis(n as u64)).await;
                }
                _ => (),
            }
        }
    }
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let lock = MESSAGE_BROADCAST_CONNECT.read().await;
        let mut rx = lock.subscribe();

        tokio::select! {
            _ = process_message(&ctx, &msg) => (),
            _ = rx.recv() => (),
            _ = async {tokio::time::sleep(Duration::from_secs(60)).await;} => stop_impl(&ctx, &msg).await,
        }
    }

    async fn voice_state_update(
        &self,
        ctx: Context,
        guild_id: Option<GuildId>,
        old_state: Option<VoiceState>,
        _: VoiceState,
    ) {
        if let Some(old_state) = old_state {
            let guild_id = guild_id.unwrap();
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

struct SoundStore;

impl TypeMapKey for SoundStore {
    type Value = Arc<Mutex<Cache<SayCommand, Arc<CachedSound>>>>;
}

struct BotJoinningChannel;

impl TypeMapKey for BotJoinningChannel {
    type Value = Arc<Mutex<HashMap<GuildId, ChannelId>>>;
}

#[group]
#[commands(
    join, leave, mute, unmute, s, st, r, stop, uptime, cpu, parse, reload, upload
)]
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
    openssl_probe::init_ssl_cert_env_vars();

    tracing_subscriber::fmt::init();

    dotenv().ok();

    let opt = Opt::from_args();

    SOUND_DIR.set(opt.sound_dir.clone()).ok();

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
        data.insert::<SoundStore>(Arc::new(Mutex::new(
            Cache::builder()
                .max_capacity(50)
                .time_to_idle(Duration::from_secs(10 * 60))
                .build(),
        )));
    }

    {
        let mut data = client.data.write().await;
        data.insert::<BotJoinningChannel>(Arc::new(Mutex::new(HashMap::new())));
    }

    {
        let (tx, rx) = broadcast::channel::<bool>(16);
        *MESSAGE_BROADCAST_CONNECT.write().await = tx;
        *MESSAGE_BROADCAST_RECEIVER.lock().await = rx;
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
        let ret = search_impl(*query, lock.values().map(|v| &v.name), 30);
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
    let lock = SOUND_DETAILS.read().await;
    let names: Vec<_> = lock.keys().collect();
    let mut rng: StdRng = SeedableRng::from_entropy();
    if let Some(mut cmd) = names.choose(&mut rng).map(|r| r.to_string()) {
        if msg.content.len() > 1 {
            cmd += &msg.content[2..];
        }
        if let Ok(cmds) = Commands::from_str(&cmd) {
            check_msg(msg.channel_id.say(&ctx.http, cmds.to_string()).await);
        }
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn stop(ctx: &Context, msg: &Message) -> CommandResult {
    stop_impl(ctx, msg).await;
    Ok(())
}

async fn stop_impl(ctx: &Context, msg: &Message) {
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
            return;
        }
    };
    let mut handler = handler_lock.lock().await;
    handler.stop();

    let lock = MESSAGE_BROADCAST_CONNECT.read().await;
    lock.send(true).ok();
}

#[command]
#[only_in(guilds)]
async fn uptime(ctx: &Context, msg: &Message) -> CommandResult {
    let sys = System::new();
    check_msg(
        msg.channel_id
            .say(
                &ctx.http,
                humantime::format_duration(sys.uptime().unwrap()).to_string(),
            )
            .await,
    );
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn cpu(ctx: &Context, msg: &Message) -> CommandResult {
    let sys = System::new();
    let cpu = sys.cpu_load_aggregate().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let cpu = cpu.done().unwrap();
    check_msg(
        msg.channel_id
            .say(
                &ctx.http,
                format!(
                    "CPU load: {:.1}% user, {:.1}% nice, {:.1}% system, {:.1}% intr, {:.1}% idle ",
                    cpu.user * 100.0,
                    cpu.nice * 100.0,
                    cpu.system * 100.0,
                    cpu.interrupt * 100.0,
                    cpu.idle * 100.0
                ),
            )
            .await,
    );
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn parse(ctx: &Context, msg: &Message) -> CommandResult {
    match Commands::from_str(&msg.content[6..]) {
        Ok(cmds) => check_msg(msg.reply(ctx, cmds.to_string()).await),
        Err(e) => check_msg(msg.reply(ctx, e.to_string()).await),
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
async fn reload(ctx: &Context, msg: &Message) -> CommandResult {
    if !ADMIN_USER_IDS.contains(&msg.author.id) {
        check_msg(
            msg.reply(ctx, "You are not authorized to run `~reload`")
                .await,
        );
        return Ok(());
    }

    let (old, new) = reload_impl().await;
    check_msg(
        msg.reply(ctx, format!("Updated sounds: {} -> {}", old, new))
            .await,
    );
    Ok(())
}

async fn reload_impl() -> (usize, usize) {
    let new_sound_details = load_sounds(SOUND_DIR.get().unwrap());
    let mut sound_details = SOUND_DETAILS.write().await;

    let old_sounds_len = sound_details.len();
    let new_sounds_len = new_sound_details.len();

    *sound_details = new_sound_details;

    (old_sounds_len, new_sounds_len)
}

#[command]
#[only_in(guilds)]
async fn upload(ctx: &Context, msg: &Message) -> CommandResult {
    if !ADMIN_USER_IDS.contains(&msg.author.id) {
        check_msg(
            msg.reply(ctx, "You are not authorized to run `~upload`")
                .await,
        );
        return Ok(());
    }

    match upload_impl(ctx, msg).await {
        Ok(count) => {
            if count > 0 {
                reload_impl().await;
                check_msg(
                    msg.reply(ctx, format!("Successfully uploaded {} sounds", count))
                        .await,
                );
            }
        }
        Err(e) => {
            check_msg(
                msg.reply(ctx, "Could not upload the sound files for some reason")
                    .await,
            );
            println!("{:?}", e)
        }
    }

    Ok(())
}

async fn upload_impl(_ctx: &Context, msg: &Message) -> anyhow::Result<u32> {
    let sound_dir = SOUND_DIR.get().with_context(|| "Failed to get sound_dir")?;
    let mut mp3_count = 0;

    let client = cloud_storage::Client::default();

    for attachment in &msg.attachments {
        let content = attachment.download().await?;

        if attachment.filename.ends_with(".zip") {
            let mut zip = ZipFileReader::new(&content[..]).await?;
            for i in 0..zip.entries().len() {
                let reader = zip.entry_reader(i).await?;
                let entry = reader.entry();

                if entry.dir() || !entry.name().ends_with(".mp3") {
                    continue;
                }

                let out_path = sound_dir.join(PathBuf::from(entry.name()).file_name().unwrap());
                let mut output = File::create(&out_path).await?;
                reader.copy_to_end_crc(&mut output, 65536).await?;
                mp3_count += 1;

                let mut file = File::open(&out_path).await?;
                let mut content = vec![];
                file.read_to_end(&mut content).await?;
                client
                    .object()
                    .create(
                        "surfpvparena",
                        content,
                        &format!(
                            "dist/sound/{}",
                            out_path.file_name().unwrap().to_str().unwrap()
                        ),
                        "audio/mpeg",
                    )
                    .await?;
            }
        } else if attachment.filename.ends_with(".mp3") {
            let out_path = sound_dir.join(&attachment.filename);
            let mut file = File::create(&out_path).await?;
            file.write_all(&content).await?;
            mp3_count += 1;

            client
                .object()
                .create(
                    "surfpvparena",
                    content,
                    &format!(
                        "dist/sound/{}",
                        out_path.file_name().unwrap().to_str().unwrap()
                    ),
                    "audio/mpeg",
                )
                .await?;
        }
    }

    Ok(mp3_count)
}
