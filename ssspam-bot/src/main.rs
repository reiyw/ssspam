use std::{
    collections::HashSet,
    io::IsTerminal,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use clap::Parser;
use dotenvy::dotenv;
use opentelemetry::{global, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{SdkTracer, SdkTracerProvider},
};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    model::{channel::Message, gateway::Ready, id::GuildId, voice::VoiceState},
    prelude::GatewayIntents,
};
use songbird::{self, SerenityInit};
use ssspam_bot::{
    ChannelManager, Configs, GuildBroadcast, SaySoundCache, SoundStorage, command,
    command::play_join_or_leave_sound,
    core::ChannelUserManager,
    leave_voice_channel,
    play::keep_warm,
    process_message,
    sound::{preload_metadata, watch_sound_storage},
};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

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
            let voice_channel_id = { channel_manager.get_voice_channel_id(&guild_id) };
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

fn init_tracer(otlp_endpoint: String) -> SdkTracer {
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()
        .expect("Failed to build OTLP span exporter");
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(Resource::builder().with_service_name("ssspambot").build())
        .build();
    let tracer = provider.tracer("ssspambot");
    global::set_tracer_provider(provider);
    tracer
}

/// Logs to stdout what `RUST_LOG` lets through, and also exports it to `otlp_endpoint` if one is
/// given.
fn init_tracing_subscriber(otlp_endpoint: Option<String>) {
    tracing_subscriber::registry()
        // Colored only on a terminal, not in the container's log.
        .with(tracing_subscriber::fmt::layer().with_ansi(std::io::stdout().is_terminal()))
        .with(EnvFilter::from_default_env())
        .with(otlp_endpoint.map(|endpoint| OpenTelemetryLayer::new(init_tracer(endpoint))))
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

    // Both ring (serenity/songbird) and aws-lc-rs (google-cloud-storage) are
    // compiled into rustls, so it can't pick a process-wide default by itself.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Failed to install the rustls crypto provider"))?;

    let opt = Opt::parse();

    init_tracing_subscriber(opt.otlp_endpoint);

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
        tokio::task::spawn_blocking({
            let storage = Arc::clone(&storage);
            move || preload_metadata(&storage)
        });
        tokio::spawn(keep_warm(Arc::clone(&storage)));
        data.insert::<SoundStorage>(storage);

        data.insert::<ChannelManager>(Arc::new(ChannelManager::load_or_new(
            opt.config_dir.join("channel_state.json"),
        )));

        data.insert::<ChannelUserManager>(Arc::new(ChannelUserManager::default()));

        // Decoded 48 kHz f32 audio takes 384 KB a second in stereo: 64 MB holds about 55 sounds of
        // the average 2.9 s, while one slowed down to 180 s (69 MB) is not kept.
        data.insert::<SaySoundCache>(Arc::new(SaySoundCache::new(64 * 1024 * 1024)));

        data.insert::<GuildBroadcast>(Arc::new(Mutex::new(GuildBroadcast::new())));

        data.insert::<Configs>(Arc::new(RwLock::new(configs)));
    }

    let shard_manager = client.shard_manager.clone();

    // SIGTERM is how a container is stopped. The bot is its PID 1, and the kernel gives PID 1 only
    // the signals it handles: without this, stopping it waits until it is killed.
    let mut terminate = signal(SignalKind::terminate())?;
    #[allow(clippy::redundant_pub_crate)]
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        shard_manager.shutdown_all().await;
    });

    let _ = client
        .start()
        .await
        .map_err(|why| println!("Client ended: {why:?}"));

    Ok(())
}
