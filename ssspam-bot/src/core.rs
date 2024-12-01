use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use dashmap::{DashMap, DashSet};
use serde::{Deserialize, Serialize};
use serenity::{
    client::Context,
    model::{
        channel::Message,
        id::{ChannelId, GuildId},
        prelude::UserId,
    },
    prelude::TypeMapKey,
};
use tokio::sync::{
    broadcast,
    broadcast::{Receiver, Sender},
};
use tracing::Instrument;

use crate::{play_say_commands, SayCommands};

/// Keeps track of channels where the bot joining.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelManager {
    /// The value is the tuple of the following:
    /// - ID of the voice channel where the bot joining and
    /// - ID of the text channel where the join command invoked
    channels: DashMap<GuildId, (ChannelId, ChannelId)>,

    config_file: PathBuf,
}

impl ChannelManager {
    pub fn load_or_new(config_file: PathBuf) -> Self {
        fs::read_to_string(&config_file).map_or_else(
            |_| Self {
                config_file,
                ..Default::default()
            },
            |j| serde_json::from_str(&j).expect("Should parse JSON file"),
        )
    }

    fn save(&self) -> anyhow::Result<()> {
        let j = serde_json::to_string(self)?;
        fs::write(&self.config_file, j)?;
        Ok(())
    }

    pub fn join(
        &self,
        guild_id: GuildId,
        voice_channel_id: ChannelId,
        text_channel_id: ChannelId,
    ) -> Option<(ChannelId, ChannelId)> {
        let ret = self
            .channels
            .insert(guild_id, (voice_channel_id, text_channel_id));
        self.save().ok();
        ret
    }

    pub fn leave(&self, guild_id: &GuildId) -> Option<(GuildId, (ChannelId, ChannelId))> {
        let ret = self.channels.remove(guild_id);
        self.save().ok();
        ret
    }

    pub fn get_voice_channel_id(&self, guild_id: &GuildId) -> Option<ChannelId> {
        self.channels.get(guild_id).map(|p| p.0)
    }

    pub fn get_text_channel_id(&self, guild_id: &GuildId) -> Option<ChannelId> {
        self.channels.get(guild_id).map(|p| p.1)
    }
}

impl TypeMapKey for ChannelManager {
    type Value = Arc<Self>;
}

/// Keeps track of users who are in the voice channel where the bot joining.
///
/// This is required because Serenity does not hold the information of users in the
/// voice channel.
#[derive(Debug, Clone, Default)]
pub struct ChannelUserManager {
    users: DashMap<GuildId, DashSet<UserId>>,
}

impl ChannelUserManager {
    pub fn get(&self, guild_id: &GuildId) -> HashSet<UserId> {
        self.users.get(guild_id).map_or_else(HashSet::new, |users| {
            users.iter().map(|user| *user).collect()
        })
    }

    pub fn add(&self, guild_id: GuildId, user_id: UserId) -> bool {
        let users = self.users.entry(guild_id).or_default();
        users.insert(user_id)
    }

    pub fn remove(&self, guild_id: &GuildId, user_id: &UserId) -> Option<UserId> {
        self.users
            .get_mut(guild_id)
            .map_or_else(|| None, |users| users.remove(user_id))
    }
}

impl TypeMapKey for ChannelUserManager {
    type Value = Arc<Self>;
}

#[derive(Debug)]
pub struct GuildBroadcast {
    // Holds Receiver to keep the channel open.
    channels: HashMap<GuildId, (Sender<OpsMessage>, Receiver<OpsMessage>)>,
}

impl GuildBroadcast {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, guild_id: GuildId) -> Receiver<OpsMessage> {
        match self.channels.get(&guild_id) {
            Some((tx, _)) => tx.subscribe(),
            None => {
                let (tx, rx1) = broadcast::channel(16);
                self.channels.insert(guild_id, (tx.clone(), rx1));
                tx.subscribe()
            }
        }
    }

    pub fn get_sender(&mut self, guild_id: GuildId) -> Sender<OpsMessage> {
        match self.channels.get(&guild_id) {
            Some((tx, _)) => tx.clone(),
            None => {
                let (tx, rx1) = broadcast::channel(16);
                self.channels.insert(guild_id, (tx.clone(), rx1));
                tx
            }
        }
    }
}

impl Default for GuildBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpsMessage {
    Stop,
}

impl TypeMapKey for GuildBroadcast {
    type Value = Arc<Mutex<Self>>;
}

#[tracing::instrument(skip_all)]
pub async fn process_message(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let get_guild_span = tracing::info_span!("get_guild");
    let guild = get_guild_span.in_scope(|| {
        msg.guild(&ctx.cache)
            .expect("Guild's ID was not found")
            .clone()
    });
    drop(get_guild_span);

    let get_channel_manager_span = tracing::info_span!("get_channel_manager");
    let channel_manager = ctx
        .data
        .read()
        .instrument(get_channel_manager_span)
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();

    let get_text_channel_id_span = tracing::info_span!("get_text_channel_id");
    let Some(text_channel_id) =
        get_text_channel_id_span.in_scope(|| channel_manager.get_text_channel_id(&guild.id))
    else {
        return Ok(());
    };
    drop(get_text_channel_id_span);
    if text_channel_id != msg.channel_id {
        return Ok(());
    }

    let get_voice_channel_id_span = tracing::info_span!("get_voice_channel_id");
    let authors_voice_channel_id = get_voice_channel_id_span.in_scope(|| {
        guild
            .voice_states
            .get(&msg.author.id)
            .and_then(|voice_state| voice_state.channel_id)
    });
    drop(get_voice_channel_id_span);
    if channel_manager.get_voice_channel_id(&guild.id) != authors_voice_channel_id {
        return Ok(());
    }

    let saycmds = {
        let mut saycmds = match SayCommands::from_str(&msg.content) {
            Ok(saycmds) => saycmds,
            // A parse failure does not imply an error because normal messages also exist.
            Err(_) => return Ok(()),
        };
        if saycmds.is_empty() {
            return Ok(());
        }
        saycmds.sanitize();
        saycmds
    };

    let guild_broadcast = ctx
        .data
        .read()
        .await
        .get::<GuildBroadcast>()
        .context("Could not get GuildBroadcast")?
        .clone();
    let mut rx = guild_broadcast.lock().unwrap().subscribe(guild.id);

    tokio::select! {
        res = play_say_commands(saycmds, ctx, guild.id) => res,
        _ = async move {
            while let Ok(msg) = rx.recv().await {
                if msg == OpsMessage::Stop {
                    break;
                }
            }
        } => Ok(())
    }
}

#[tracing::instrument]
pub async fn process_from_string(
    ctx: &Context,
    guild_id: GuildId,
    sound: &str,
) -> anyhow::Result<()> {
    let saycmds = {
        let mut saycmds = match SayCommands::from_str(sound) {
            Ok(saycmds) => saycmds,
            // A parse failure does not imply an error because normal messages also exist.
            Err(_) => return Ok(()),
        };
        if saycmds.is_empty() {
            return Ok(());
        }
        saycmds.sanitize();
        saycmds
    };

    let guild_broadcast = ctx
        .data
        .read()
        .await
        .get::<GuildBroadcast>()
        .context("Could not get GuildBroadcast")?
        .clone();
    let mut rx = guild_broadcast.lock().unwrap().subscribe(guild_id);

    tokio::select! {
        res = play_say_commands(saycmds, ctx, guild_id) => res,
        _ = async move {
            while let Ok(msg) = rx.recv().await {
                if msg == OpsMessage::Stop {
                    break;
                }
            }
        } => Ok(())
    }
}
