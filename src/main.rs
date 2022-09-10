use std::{
    collections::HashMap,
    convert::TryInto,
    env,
    path::PathBuf,
    sync::{Arc},
};

use clap::Parser;
use dotenv::dotenv;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::RwLock;
use serenity::{
    async_trait,
    client::{Client, ClientBuilder, Context, EventHandler},
    framework::{
        standard::{
            macros::{command, group},
            Args, CommandResult,
        },
        StandardFramework,
    },
    model::{channel::Message, gateway::Ready},
    prelude::{GatewayIntents, Mentionable, TypeMapKey},
    Result as SerenityResult,
};
use songbird::{
    driver::Bitrate,
    input::{
        cached::{Compressed, Memory},
        Input, {self},
    },
    Call, Event, EventContext, EventHandler as VoiceEventHandler, SerenityInit, TrackEvent,
};
use ssspambot::{
    sound::watch_sound_storage, SoundStorage, JOIN_COMMAND, LEAVE_COMMAND, MUTE_COMMAND,
    UNMUTE_COMMAND,
};

// static SOUND_STORAGE: Lazy<Arc<RwLock<SoundStorage>>> = Lazy::default();
// static SOUND_STORAGE: OnceCell<Arc<RwLock<SoundStorage>>> = OnceCell::new();

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }
}

#[group]
#[commands(join, leave, mute, unmute)]
struct General;

#[derive(Parser)]
#[clap(version, about)]
struct Opt {
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

    // fern::Dispatch::new().

    let framework = StandardFramework::new()
        .configure(|c| c.prefix("-"))
        .group(&GENERAL_GROUP);

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&opt.discord_token, intents)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Error while creating client");

    // let storage = Arc::new(RwLock::new(SoundStorage::load(&opt.sound_dir)));
    // tokio::spawn(watch_sound_storage(Arc::clone(&storage)));
    // SOUND_STORAGE.set(storage);

    {
        let mut data = client.data.write().await;

        let storage = Arc::new(RwLock::new(SoundStorage::load(&opt.sound_dir)));
        tokio::spawn(watch_sound_storage(Arc::clone(&storage)));
        data.insert::<SoundStorage>(storage);
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
