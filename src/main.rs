use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use dotenv::dotenv;
use log::{info, warn};
use parking_lot::{Mutex, RwLock};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    framework::StandardFramework,
    model::{channel::Message, gateway::Ready, id::UserId, voice::VoiceState},
    prelude::GatewayIntents,
};
use songbird::SerenityInit;
use ssspambot::{
    leave_based_on_voice_state_update, process_message, sound::watch_sound_storage, ChannelManager,
    GuildBroadcast, SaySoundCache, SoundStorage, GENERAL_GROUP, OWNER_GROUP,
};

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if let Err(e) = process_message(&ctx, &msg).await {
            warn!("Error while processing a message: {e:?}");
        }
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, _new: VoiceState) {
        if let Err(e) = leave_based_on_voice_state_update(ctx, old).await {
            warn!("Error while deciding whether to leave: {e:?}");
        }
    }
}

#[derive(Parser)]
#[clap(version, about)]
struct Opt {
    #[clap(long, env, default_value_t = String::from("~"))]
    command_prefix: String,

    #[clap(long, env)]
    discord_token: String,

    #[clap(long, env, value_parser)]
    sound_dir: PathBuf,

    #[clap(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let opt = Opt::parse();

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(opt.verbose.log_level_filter())
        .level_for("tracing", log::LevelFilter::Warn)
        .level_for("serenity", log::LevelFilter::Warn)
        .level_for("songbird", log::LevelFilter::Warn)
        .level_for("rustls", log::LevelFilter::Warn)
        .chain(std::io::stderr())
        .apply()?;

    let framework = StandardFramework::new()
        .configure(|c| {
            c.prefix(opt.command_prefix).owners(HashSet::from([
                // TODO: Use Discord's team feature
                UserId(310620137608970240), // auzen
                UserId(342903795380125698), // nicotti
            ]))
        })
        .group(&GENERAL_GROUP)
        .group(&OWNER_GROUP);

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&opt.discord_token, intents)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Error while creating client");

    {
        let mut data = client.data.write().await;

        let storage = Arc::new(RwLock::new(SoundStorage::load(&opt.sound_dir)));
        tokio::spawn(watch_sound_storage(Arc::clone(&storage)));
        data.insert::<SoundStorage>(storage);

        data.insert::<ChannelManager>(Arc::new(RwLock::new(ChannelManager::default())));

        data.insert::<SaySoundCache>(Arc::new(RwLock::new(SaySoundCache::new(
            50,
            Duration::from_secs(60 * 10),
        ))));

        data.insert::<GuildBroadcast>(Arc::new(Mutex::new(GuildBroadcast::new())));
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
