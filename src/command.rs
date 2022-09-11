use std::{fs, path::PathBuf, str::FromStr};

use anyhow::Context as _;
use async_zip::read::mem::ZipFileReader;
use chrono::{DateTime, Utc};
use log::{info, warn};
use prettytable::{format, Table};
use serenity::{
    client::Context,
    framework::standard::{
        macros::{command, group},
        Args, CommandResult,
    },
    model::{channel::Message, prelude::VoiceState},
    prelude::Mentionable,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{ChannelManager, GuildBroadcast, OpsMessage, SayCommands, SaySoundCache, SoundStorage};

#[group]
#[only_in(guilds)]
#[commands(join, leave, mute, unmute, stop, clean_cache, r, s, st)]
struct General;

#[command]
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

#[command]
pub async fn clean_cache(ctx: &Context, _msg: &Message) -> CommandResult {
    if let Err(e) = clean_cache_impl(ctx).await {
        warn!("Failed to clean cache: {e:?}");
    }
    Ok(())
}

async fn clean_cache_impl(ctx: &Context) -> anyhow::Result<()> {
    ctx.data
        .read()
        .await
        .get::<SaySoundCache>()
        .context("Could not get SaySoundCache")?
        .clone()
        .write()
        .clean();
    Ok(())
}

#[command]
pub async fn r(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    match r_impl(ctx, args).await {
        Ok(say_commands) => {
            msg.channel_id
                .say(&ctx.http, say_commands.to_string())
                .await
                .ok();
        }
        Err(e) => warn!("Failed r: {e:?}"),
    }
    Ok(())
}

async fn r_impl(ctx: &Context, args: Args) -> anyhow::Result<SayCommands> {
    let storage = ctx
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
    let storage = storage.read();
    let file = storage.get_random().context("Has no sound file")?;
    let cmds = SayCommands::from_str(&format!("{} {}", file.name, args.rest()))?;
    Ok(cmds)
}

#[command]
pub async fn s(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    if let Some(arg) = args.current() {
        let storage = ctx.data.read().await.get::<SoundStorage>().unwrap().clone();
        let storage = storage.read();
        let sims = storage.calc_similarities(arg);
        let names: Vec<_> = sims
            .iter()
            .take(20)
            .filter(|(s, _)| s > &0.85)
            .map(|(_, f)| f.name.clone())
            .collect();
        let names: Vec<_> = if names.len() < 10 {
            sims.iter().take(10).map(|(_, f)| f.name.clone()).collect()
        } else {
            names
        };
        msg.channel_id.say(&ctx.http, names.join(", ")).await.ok();
    }
    Ok(())
}

#[command]
pub async fn st(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    if let Some(arg) = args.current() {
        let storage = ctx.data.read().await.get::<SoundStorage>().unwrap().clone();
        let storage = storage.read();
        let sims = storage.calc_similarities(arg);

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_CLEAN);
        table.set_titles(row!["Name", "Dur", "Updated"]);

        for (_, file) in sims.iter().take(10) {
            let updated_at: DateTime<Utc> = file.updated_at().into();
            table.add_row(row![
                file.name,
                format!("{:.1}", file.duration().as_secs_f64()),
                updated_at.format("%Y-%m-%d") // updated_at.format("%Y-%m-%d %T")
            ]);
        }

        msg.channel_id
            .say(&ctx.http, format!("```\n{}\n```", table.to_string()))
            .await
            .ok();
    }
    Ok(())
}

#[group]
#[owners_only]
#[only_in(guilds)]
#[commands(upload, delete)]
struct Owner;

#[command]
pub async fn upload(ctx: &Context, msg: &Message) -> CommandResult {
    match upload_impl(ctx, msg).await {
        Ok(n) => {
            msg.reply(ctx, format!("Successfully uploaded {n} sounds"))
                .await
                .ok();
        }
        Err(e) => {
            msg.reply(ctx, format!("Failed to upload: {e:?}"))
                .await
                .ok();
        }
    }
    Ok(())
}

async fn upload_impl(ctx: &Context, msg: &Message) -> anyhow::Result<u32> {
    let mut count = 0;
    let storage = ctx
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
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

                let out_path = storage
                    .read()
                    .dir
                    .join(PathBuf::from(entry.name()).file_name().unwrap());
                let mut output = tokio::fs::File::create(&out_path).await?;
                reader.copy_to_end_crc(&mut output, 65536).await?;
                count += 1;

                let mut file = tokio::fs::File::open(&out_path).await?;
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
            let out_path = storage.read().dir.join(&attachment.filename);
            let mut file = tokio::fs::File::create(&out_path).await?;
            file.write_all(&content).await?;
            count += 1;

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

    // TODO: update data.json

    clean_cache_impl(ctx).await?;

    Ok(count)
}

#[command]
pub async fn delete(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    if args.is_empty() {
        msg.reply(
            ctx,
            "Specify one or more saysound names, separated by spaces.",
        )
        .await
        .ok();
        return Ok(());
    }

    match delete_impl(ctx, msg, args).await {
        Ok(deleted) if !deleted.is_empty() => {
            msg.reply(ctx, format!("Deleted: {}", deleted.join(", ")))
                .await
                .ok();
        }
        Ok(deleted) if deleted.is_empty() => {
            msg.reply(ctx, "The given saysounds were not found")
                .await
                .ok();
        }
        Ok(_) => unreachable!(),
        Err(e) => {
            msg.reply(ctx, format!("Failed to delete: {e:?}"))
                .await
                .ok();
        }
    }
    Ok(())
}

async fn delete_impl(ctx: &Context, msg: &Message, mut args: Args) -> anyhow::Result<Vec<String>> {
    let storage = ctx
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
    let client = cloud_storage::Client::default();
    let mut deleted = Vec::new();

    for name in args.iter::<String>().flatten() {
        if let Some(file) = storage.read().get(name) {
            if fs::remove_file(&file.path).is_ok() {
                deleted.push(file.name.clone());
            }
        }
    }

    // TODO: delete from gcs and update data.json

    clean_cache_impl(ctx).await?;

    Ok(deleted)
}
