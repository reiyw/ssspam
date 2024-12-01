use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use clap::Parser;
use dotenvy::dotenv;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{trace::Tracer, Resource};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    model::{channel::Message, gateway::Ready, id::GuildId, voice::VoiceState},
    prelude::GatewayIntents,
};
use songbird::{self, SerenityInit};
use ssspam_bot::{
    command, command::play_join_or_leave_sound, core::ChannelUserManager, leave_voice_channel,
    process_message, sound::watch_sound_storage, ChannelManager, Configs, GuildBroadcast,
    SaySoundCache, SoundStorage,
};
use tracing::{info, warn};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

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
            let voice_channel_id = {
                channel_manager
                    .read()
                    .unwrap()
                    .get_voice_channel_id(&guild_id)
            };
            if let Some(voice_channel_id) = voice_channel_id {
                let res = manager.join(guild_id, voice_channel_id).await;
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
            if let Err(e) = play_join_or_leave_sound(&ctx, guild_id, new.user_id).await {
                warn!("Error while playing join or leave sound: {e:?}");
            }

            if let Err(e) = leave_voice_channel(&ctx, guild_id).await {
                warn!("Error while deciding whether to leave: {e:?}");
            }
        }
    }
}

fn init_tracer(otlp_endpoint: String) -> Tracer {
    let otlp_exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(otlp_endpoint);
    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        .with_trace_config(
            opentelemetry_sdk::trace::config().with_resource(Resource::new(vec![KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                "ssspambot",
            )])),
        )
        .install_simple()
        .expect("Failed to install opentelemetry pipeline")
}

fn init_tracing_subscriber(otlp_endpoint: String) {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::from_default_env())
        .with(OpenTelemetryLayer::new(init_tracer(otlp_endpoint)))
        .init();
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

    #[clap(long, env, value_parser)]
    config_dir: PathBuf,

    #[clap(long, env)]
    otlp_endpoint: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let opt = Opt::parse();

    if let Some(endpoint) = opt.otlp_endpoint {
        init_tracing_subscriber(endpoint);
    }

    let framework = poise::Framework::builder()
        .setup(|_, _, _: &poise::Framework<(), anyhow::Error>| Box::pin(async move { Ok(()) }))
        .options(poise::FrameworkOptions {
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some(opt.command_prefix.clone()),
                ..Default::default()
            },
            commands: vec![
                command::clean_cache(),
                command::config(),
                command::delete(),
                command::help(),
                command::join(),
                command::leave(),
                command::mute(),
                command::r(),
                command::restart(),
                command::rhai(),
                command::s(),
                command::st(),
                command::stop(),
                command::unmute(),
                command::upload(),
                command::uptime(),
            ],
            owners: HashSet::from([
                // TODO: Make this configurable
                310620137608970240.into(), // auzen
                342903795380125698.into(), // nicotti
            ]),
            ..Default::default()
        })
        .build();

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;

    let configs = Configs::load_or_create(opt.config_dir.join("config.json"))?;

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

        data.insert::<ChannelManager>(Arc::new(RwLock::new(ChannelManager::load_or_new(
            opt.config_dir.join("channel_state.json"),
        ))));

        data.insert::<ChannelUserManager>(Arc::new(Mutex::new(ChannelUserManager::default())));

        data.insert::<SaySoundCache>(Arc::new(SaySoundCache::new(50)));

        data.insert::<GuildBroadcast>(Arc::new(Mutex::new(GuildBroadcast::new())));

        data.insert::<Configs>(Arc::new(RwLock::new(configs)));
    }

    let shard_manager = client.shard_manager.clone();

    #[allow(clippy::redundant_pub_crate)]
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
        }
        shard_manager.shutdown_all().await;
    });

    let _ = client
        .start()
        .await
        .map_err(|why| println!("Client ended: {why:?}"));

    Ok(())
}
