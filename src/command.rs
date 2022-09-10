use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use anyhow::Context as _;
use log::warn;
use parking_lot::RwLock;
use serenity::{
    client::Context,
    framework::standard::{macros::command, CommandResult},
    model::{
        channel::Message,
        id::{ChannelId, GuildId, UserId},
        prelude::VoiceState,
    },
    prelude::{Mentionable, TypeMapKey},
};

/// Keeps track of channels where the bot joining.
#[derive(Debug, Clone, Default)]
struct ChannelManager {
    channels: BTreeMap<GuildId, ChannelId>,
}

impl ChannelManager {
    fn join(&mut self, guild_id: GuildId, channel_id: ChannelId) -> Option<ChannelId> {
        self.channels.insert(guild_id, channel_id)
    }

    fn leave(&mut self, guild_id: &GuildId) -> Option<ChannelId> {
        self.channels.remove(guild_id)
    }
}

impl TypeMapKey for ChannelManager {
    type Value = Arc<RwLock<Self>>;
    // type Value = Arc<Mutex<Self>>;
}

#[command]
#[only_in(guilds)]
pub async fn join(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = join_impl(ctx, msg).await {
        warn!("Error joining the channel: {e:?}");
    }
    Ok(())
}

async fn join_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;
    let channel_id = guild
        .voice_states
        .get(&msg.author.id)
        .and_then(|voice_state| voice_state.channel_id);
    let channel_id = match channel_id {
        Some(c) => c,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let (_handler_lock, success_reader) = manager.join(guild.id, channel_id).await;
    if success_reader.is_ok() {
        msg.channel_id
            .say(&ctx.http, &format!("Joined {}", channel_id.mention()))
            .await?;
        let channel_manager = ctx
            .data
            .read()
            .await
            .get::<ChannelManager>()
            .context("Could not get ChannelManager")?
            .clone();
        channel_manager.write().join(guild.id, channel_id);
    } else {
        msg.channel_id
            .say(&ctx.http, "Error joining the channel")
            .await?;
    }

    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn leave(ctx: &Context, msg: &Message) -> CommandResult {
    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn mute(ctx: &Context, msg: &Message) -> CommandResult {
    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn unmute(ctx: &Context, msg: &Message) -> CommandResult {
    Ok(())
}
