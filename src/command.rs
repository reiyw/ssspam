use anyhow::Context as _;
use log::{warn, info};
use serenity::{
    client::Context,
    framework::standard::{macros::command, CommandResult},
    model::{channel::Message, prelude::VoiceState},
    prelude::Mentionable,
};

use crate::{ChannelManager, GuildBroadcast, OpsMessage};

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
    let voice_channel_id = guild
        .voice_states
        .get(&msg.author.id)
        .and_then(|voice_state| voice_state.channel_id);
    let voice_channel_id = match voice_channel_id {
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

    let (_handler_lock, success_reader) = manager.join(guild.id, voice_channel_id).await;
    if success_reader.is_ok() {
        msg.channel_id
            .say(&ctx.http, &format!("Joined {}", voice_channel_id.mention()))
            .await?;
        let channel_manager = ctx
            .data
            .read()
            .await
            .get::<ChannelManager>()
            .context("Could not get ChannelManager")?
            .clone();
        channel_manager
            .write()
            .join(guild.id, voice_channel_id, msg.channel_id);
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
    if let Err(e) = leave_impl(ctx, msg).await {
        warn!("Error leaving the channel: {e:?}");
    }
    Ok(())
}

async fn leave_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    manager.remove(guild.id).await?;

    let channel_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();
    channel_manager.write().leave(&guild.id);

    Ok(())
}

pub async fn leave_based_on_voice_state_update(
    ctx: Context,
    old_state: Option<VoiceState>,
) -> anyhow::Result<()> {
    if let Some(old_state) = old_state {
        if let Some(guild_id) = old_state.guild_id {
            let channel_manager = ctx
                .data
                .read()
                .await
                .get::<ChannelManager>()
                .context("Could not get ChannelManager")?
                .clone();
            let bots_voice_channel_id = channel_manager.read().get_voice_channel_id(&guild_id);
            let authors_old_state_voice_channel_id = old_state.channel_id;
            if bots_voice_channel_id != authors_old_state_voice_channel_id {
                return Ok(());
            }

            if let Some(bots_voice_channel_id) = bots_voice_channel_id {
                let channel = ctx
                    .cache
                    .guild_channel(bots_voice_channel_id)
                    .context("Failed to get GuildChannel")?;
                let members = channel
                    .members(&ctx.cache)
                    .await
                    .context("Should get members")?;
                if members.len() == 1 && members[0].user.bot {
                    let manager = songbird::get(&ctx)
                        .await
                        .context("Songbird Voice client placed in at initialization.")?
                        .clone();
                    manager.remove(guild_id).await?;
                    channel_manager.write().leave(&guild_id);
                }
            }
        }
    }
    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn mute(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = mute_impl(ctx, msg).await {
        warn!("Failed to mute: {e:?}");
    }
    Ok(())
}

async fn mute_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild.id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    let mut handler = handler_lock.lock().await;

    handler.mute(true).await?;

    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn unmute(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = unmute_impl(ctx, msg).await {
        warn!("Failed to unmute: {e:?}");
    }
    Ok(())
}

async fn unmute_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild.id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    let mut handler = handler_lock.lock().await;

    handler.mute(false).await?;

    Ok(())
}

#[command]
#[only_in(guilds)]
pub async fn stop(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = stop_impl(ctx, msg).await {
        warn!("Failed to stop: {e:?}");
    }
    Ok(())
}

async fn stop_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg.guild(&ctx.cache).context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild.id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };
    let mut handler = handler_lock.lock().await;
    handler.stop();

    let guild_broadcast = ctx
        .data
        .read()
        .await
        .get::<GuildBroadcast>()
        .context("Could not get GuildBroadcast")?
        .clone();
    let tx = guild_broadcast.lock().get_sender(guild.id);
    tx.send(OpsMessage::Stop)?;

    Ok(())
}
