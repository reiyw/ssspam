use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use dotenvy::dotenv;
use parking_lot::{Mutex, RwLock};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    framework::StandardFramework,
    model::{
        channel::Message,
        gateway::Ready,
        id::{GuildId, UserId},
        voice::VoiceState,
    },
    prelude::GatewayIntents,
};
use songbird::SerenityInit;
use ssspambot::{
    leave_voice_channel, process_message, sound::watch_sound_storage, ChannelManager,
    GuildBroadcast, SaySoundCache, ShutdownChannel, SoundStorage, VolumeManager, GENERAL_GROUP,
    OWNER_GROUP,
};
use tracing::{info, warn};

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
    }

    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        let manager = songbird::get(&ctx).await.unwrap().clone();
        let channel_manager = ctx
            .data
            .read()
            .await
            .get::<ChannelManager>()
            .unwrap()
            .clone();
        for guild_id in guilds {
            let voice_channel_id = { channel_manager.read().get_voice_channel_id(&guild_id) };
            if let Some(voice_channel_id) = voice_channel_id {
                let (_, res) = manager.join(guild_id, voice_channel_id).await;
                res.ok();
            }
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if let Err(e) = process_message(&ctx, &msg).await {
            warn!("Error while processing a message: {e:?}");
        }
    }

    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        if let Some(guild_id) = new.guild_id {
            if let Err(e) = leave_voice_channel(ctx, guild_id).await {
                warn!("Error while deciding whether to leave: {e:?}");
            }
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

    tracing_subscriber::fmt::init();

    let opt = Opt::parse();

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

    let shutdown_receiver;
    {
        let mut data = client.data.write().await;

        let storage = Arc::new(RwLock::new(SoundStorage::load(&opt.sound_dir)));
        tokio::spawn(watch_sound_storage(Arc::clone(&storage)));
        data.insert::<SoundStorage>(storage);

        data.insert::<ChannelManager>(Arc::new(RwLock::new(ChannelManager::load_or_new())));

        data.insert::<SaySoundCache>(Arc::new(RwLock::new(SaySoundCache::new(
            50,
            Duration::from_secs(60 * 10),
        ))));

        data.insert::<GuildBroadcast>(Arc::new(Mutex::new(GuildBroadcast::new())));

        data.insert::<VolumeManager>(Arc::new(Mutex::new(VolumeManager::new())));

        let (rx, channel) = ShutdownChannel::new();
        data.insert::<ShutdownChannel>(channel);
        shutdown_receiver = rx;
    }

    let shard_manager = client.shard_manager.clone();

    #[allow(clippy::redundant_pub_crate)]
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = shutdown_receiver => {},
        }
        shard_manager.lock().await.shutdown_all().await;
    });

    let _ = client
        .start()
        .await
        .map_err(|why| println!("Client ended: {:?}", why));

    Ok(())
}
