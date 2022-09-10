use std::{collections::HashMap, str::FromStr, sync::Arc};

use anyhow::Context as _;
use log::{info, warn};
use parking_lot::RwLock;
use serenity::{
    client::Context,
    framework::standard::{macros::command, CommandResult},
    model::{
        channel::Message,
        id::{ChannelId, GuildId, UserId},
    },
    prelude::{GatewayIntents, Mentionable, TypeMapKey},
};

use super::SayCommands;

/// Keeps track of channels where the bot joining.
#[derive(Debug, Clone, Default)]
pub struct ChannelManager {
    /// The value is the tuple of the following:
    /// - ID of the voice channel where the bot joining and
    /// - ID of the text channel where the join command invoked
    channels: HashMap<GuildId, (ChannelId, ChannelId)>,
}

impl ChannelManager {
    pub fn join(
        &mut self,
        guild_id: GuildId,
        voice_channel_id: ChannelId,
        text_channel_id: ChannelId,
    ) -> Option<(ChannelId, ChannelId)> {
        self.channels
            .insert(guild_id, (voice_channel_id, text_channel_id))
    }

    pub fn leave(&mut self, guild_id: &GuildId) -> Option<(ChannelId, ChannelId)> {
        self.channels.remove(guild_id)
    }

    pub fn get_voice_channel_id(&self, guild_id: &GuildId) -> Option<ChannelId> {
        self.channels.get(guild_id).map(|p| p.0)
    }

    pub fn get_text_channel_id(&self, guild_id: &GuildId) -> Option<ChannelId> {
        self.channels.get(guild_id).map(|p| p.1)
    }
}

impl TypeMapKey for ChannelManager {
    type Value = Arc<RwLock<Self>>;
}

pub async fn process_message(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;

    let channel_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();

    if channel_manager.read().get_text_channel_id(&guild.id) != Some(msg.channel_id) {
        return Ok(());
    }

    let authors_voice_channel_id = guild
        .voice_states
        .get(&msg.author.id)
        .and_then(|voice_state| voice_state.channel_id);

    if channel_manager.read().get_voice_channel_id(&guild.id) != authors_voice_channel_id {
        return Ok(());
    }

    let saycmds = {
        let mut saycmds = SayCommands::from_str(&msg.content)?;
        if saycmds.is_empty() {
            return Ok(());
        }
        saycmds.sanitize();
        saycmds
    };

    let manager = songbird::get(&ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    Ok(())
}
