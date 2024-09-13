use std::{collections::HashSet, fs, path::PathBuf, str::FromStr, time::Duration};

use anyhow::Context as _;
use async_zip::read::mem::ZipFileReader;
use chrono::{DateTime, Utc};
use itertools::Itertools;
use prettytable::{format, Table};
use serenity::{
    all::{Attachment, Context as SerenityContext},
    model::{id::GuildId, prelude::UserId},
    prelude::Mentionable,
};
use systemstat::{Platform, System};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use crate::{
    core::{process_from_string, ChannelUserManager},
    interpret_rhai,
    web::update_sounds_bin,
    ChannelManager, Configs, GuildBroadcast, OpsMessage, SayCommands, SaySoundCache, SoundStorage,
};

type Context<'a> = poise::Context<'a, (), anyhow::Error>;

#[poise::command(prefix_command)]
pub async fn help(
    ctx: Context<'_>,
    #[description = "Specific command to show help about"] command: Option<String>,
) -> anyhow::Result<()> {
    poise::builtins::help(ctx, command.as_deref(), Default::default()).await?;
    Ok(())
}

#[poise::command(prefix_command, guild_only)]
pub async fn join(ctx: Context<'_>) -> anyhow::Result<()> {
    let voice_channel_id = ctx
        .guild()
        .context("Guild was not found")?
        .voice_states
        .get(&ctx.author().id)
        .and_then(|voice_state| voice_state.channel_id);
    let voice_channel_id = match voice_channel_id {
        Some(c) => c,
        None => {
            ctx.reply("Not in a voice channel").await?;
            return Ok(());
        }
    };

    let manager = songbird::get(ctx.as_ref())
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = manager
        .join(
            ctx.guild_id().context("Guild was not found")?,
            voice_channel_id,
        )
        .await;
    if handler_lock.is_ok() {
        ctx.channel_id()
            .say(&ctx, &format!("Joined {}", voice_channel_id.mention()))
            .await?;
        let channel_manager = ctx
            .serenity_context()
            .data
            .read()
            .await
            .get::<ChannelManager>()
            .context("Could not get ChannelManager")?
            .clone();
        channel_manager.write().join(
            ctx.guild_id().context("Guild was not found")?,
            voice_channel_id,
            ctx.channel_id(),
        );
    } else {
        ctx.channel_id()
            .say(&ctx, "Error joining the channel")
            .await?;
    }

    Ok(())
}

#[poise::command(prefix_command, guild_only)]
pub async fn leave(ctx: Context<'_>) -> anyhow::Result<()> {
    let manager = songbird::get(ctx.as_ref())
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    manager
        .remove(ctx.guild_id().context("Guild was not found")?)
        .await?;

    let channel_manager = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();
    channel_manager
        .write()
        .leave(&ctx.guild_id().context("Guild was not found")?);

    Ok(())
}

pub async fn leave_voice_channel(ctx: &SerenityContext, guild_id: GuildId) -> anyhow::Result<()> {
    let channel_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();
    let bots_voice_channel_id = { channel_manager.read().get_voice_channel_id(&guild_id) };
    if let Some(bots_voice_channel_id) = bots_voice_channel_id {
        let guild = ctx.cache.guild(guild_id).unwrap().clone();
        let channel = guild
            .channels
            .get(&bots_voice_channel_id)
            .context("Failed to get GuildChannel")?
            .clone();
        let members = channel.members(&ctx.cache)?;
        if members.iter().all(|m| m.user.bot) {
            let manager = songbird::get(ctx)
                .await
                .context("Songbird Voice client placed in at initialization.")?
                .clone();
            manager.remove(guild_id).await?;
            channel_manager.write().leave(&guild_id);
        }
    }

    Ok(())
}

#[tracing::instrument]
pub async fn play_join_or_leave_sound(
    ctx: &SerenityContext,
    guild_id: GuildId,
    actioned_user: UserId,
) -> anyhow::Result<()> {
    let current_users: HashSet<UserId> = {
        let channel_manager = ctx
            .data
            .read()
            .await
            .get::<ChannelManager>()
            .context("Could not get ChannelManager")?
            .clone();
        let bots_voice_channel_id = { channel_manager.read().get_voice_channel_id(&guild_id) };
        if let Some(bots_voice_channel_id) = bots_voice_channel_id {
            let guild = ctx.cache.guild(guild_id).unwrap().clone();
            let channel = guild
                .channels
                .get(&bots_voice_channel_id)
                .context("Failed to get GuildChannel")?
                .clone();
            let members = channel.members(&ctx.cache)?;
            HashSet::from_iter(members.into_iter().map(|m| m.user.id))
        } else {
            return Ok(());
        }
    };

    let channel_user_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelUserManager>()
        .context("Could not get ChannelUserManager")?
        .clone();
    let old_users = channel_user_manager.read().get(&guild_id);

    let configs = ctx
        .data
        .read()
        .await
        .get::<Configs>()
        .context("Could not get Configs")?
        .clone();

    // アクションを起こしたユーザーが以前いなくて今いるなら join したことになる
    let mut diff = current_users.difference(&old_users);
    if diff.contains(&actioned_user) {
        {
            let mut lock = channel_user_manager.write();
            lock.add(guild_id, actioned_user);
        }
        let sound = { configs.read().get_joinsound(&actioned_user) };
        if let Some(sound) = sound {
            info!(sound, "playing joinsound");
            process_from_string(ctx, guild_id, sound.as_str()).await?
        }
        return Ok(());
    }

    // アクションを起こしたユーザーが今いなくて以前いたなら leave したことになる
    let mut diff = old_users.difference(&current_users);
    if diff.contains(&actioned_user) {
        {
            let mut lock = channel_user_manager.write();
            lock.remove(&guild_id, &actioned_user);
        }
        let sound = { configs.read().get_leavesound(&actioned_user) };
        if let Some(sound) = sound {
            info!(sound, "playing leavesound");
            process_from_string(ctx, guild_id, sound.as_str()).await?
        }
    }

    Ok(())
}

#[poise::command(prefix_command, guild_only)]
pub async fn mute(ctx: Context<'_>) -> anyhow::Result<()> {
    let manager = songbird::get(ctx.as_ref())
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(ctx.guild_id().context("Guild was not found")?) {
        Some(handler) => handler,
        None => {
            ctx.reply("Not in a voice channel").await?;
            return Ok(());
        }
    };

    handler_lock.lock().await.mute(true).await?;

    Ok(())
}

#[poise::command(prefix_command, guild_only)]
pub async fn unmute(ctx: Context<'_>) -> anyhow::Result<()> {
    let manager = songbird::get(ctx.as_ref())
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(ctx.guild_id().context("Guild was not found")?) {
        Some(handler) => handler,
        None => {
            ctx.reply("Not in a voice channel").await?;
            return Ok(());
        }
    };

    handler_lock.lock().await.mute(false).await?;

    Ok(())
}

#[poise::command(prefix_command, guild_only)]
pub async fn stop(ctx: Context<'_>) -> anyhow::Result<()> {
    let manager = songbird::get(ctx.as_ref())
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(ctx.guild_id().context("Guild was not found")?) {
        Some(handler) => handler,
        None => {
            ctx.reply("Not in a voice channel").await?;
            return Ok(());
        }
    };
    handler_lock.lock().await.stop();

    let guild_broadcast = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<GuildBroadcast>()
        .context("Could not get GuildBroadcast")?
        .clone();
    let tx = guild_broadcast
        .lock()
        .get_sender(ctx.guild_id().context("Guild was not found")?);
    tx.send(OpsMessage::Stop)?;

    Ok(())
}

#[poise::command(prefix_command)]
pub async fn clean_cache(ctx: Context<'_>) -> anyhow::Result<()> {
    clean_cache_inner(ctx.serenity_context()).await?;
    Ok(())
}

async fn clean_cache_inner(ctx: &SerenityContext) -> anyhow::Result<()> {
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

#[poise::command(prefix_command)]
pub async fn r(ctx: Context<'_>, #[rest] rest: Option<String>) -> anyhow::Result<()> {
    let storage = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
    let file = storage.read().get_random().context("Has no sound file")?;
    match SayCommands::from_str(&format!("{} {}", file.name, rest.unwrap_or_default())) {
        Ok(say_commands) => {
            ctx.say(say_commands.to_string()).await.ok();
        }
        Err(e) => warn!("Failed r: {e:?}"),
    }

    Ok(())
}

#[poise::command(prefix_command)]
pub async fn s(ctx: Context<'_>, query: String) -> anyhow::Result<()> {
    let names: Vec<_> = {
        let storage = ctx
            .serenity_context()
            .data
            .read()
            .await
            .get::<SoundStorage>()
            .unwrap()
            .clone();
        let sims = storage.read().calc_similarities(query);
        let names: Vec<_> = sims
            .iter()
            .take(20)
            .filter(|(s, _)| s > &0.85)
            .map(|(_, f)| f.name.clone())
            .collect();
        if names.len() < 10 {
            sims.iter().take(10).map(|(_, f)| f.name.clone()).collect()
        } else {
            names
        }
    };
    ctx.say(names.join(", ")).await.ok();
    Ok(())
}

#[poise::command(prefix_command)]
pub async fn st(ctx: Context<'_>, query: String) -> anyhow::Result<()> {
    let storage = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .unwrap()
        .clone();
    let sims = storage.read().calc_similarities(query);

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

    ctx.say(format!("```\n{table}\n```")).await.ok();
    Ok(())
}

#[poise::command(prefix_command)]
pub async fn uptime(ctx: Context<'_>) -> anyhow::Result<()> {
    let sys = System::new();
    ctx.say(humantime::format_duration(sys.uptime().unwrap()).to_string())
        .await
        .ok();
    Ok(())
}

#[poise::command(prefix_command)]
pub async fn rhai(ctx: Context<'_>, #[rest] rest: String) -> anyhow::Result<()> {
    let source = rest.trim().trim_matches('`').to_owned();
    dbg!(&source);
    let task = tokio::task::spawn_blocking(move || interpret_rhai(&source));
    match tokio::time::timeout(Duration::from_secs(1), task).await {
        Ok(Ok(Ok(say_commands))) => {
            ctx.say(say_commands).await.ok();
        }
        Err(e) => {
            ctx.say(e.to_string()).await.ok();
        }
        Ok(Err(e)) => {
            ctx.say(e.to_string()).await.ok();
        }
        Ok(Ok(Err(e))) => {
            ctx.say(e.to_string()).await.ok();
        }
    }
    Ok(())
}

#[tracing::instrument]
#[poise::command(prefix_command, owners_only)]
pub async fn upload(ctx: Context<'_>, files: Vec<Attachment>) -> anyhow::Result<()> {
    let mut count = 0;
    let storage = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
    let client = cloud_storage::Client::default();

    for attachment in files {
        let content = attachment.download().await?;

        if attachment.filename.ends_with(".zip") {
            let reader = ZipFileReader::new(content).await?;
            for i in 0..reader.file().entries().len() {
                let entry = reader.file().entries().get(i).unwrap().entry();
                let mut entry_reader = reader.entry(i).await?;

                if entry.dir() || !entry.filename().ends_with(".mp3") {
                    continue;
                }

                let out_path = storage
                    .read()
                    .dir
                    .join(PathBuf::from(entry.filename()).file_name().unwrap());
                let mut writer = tokio::fs::File::create(&out_path).await?;
                tokio::io::copy(&mut entry_reader, &mut writer).await?;
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

    tokio::spawn(update_sounds_bin(storage.read().dir.clone()));

    storage.write().reload();

    clean_cache_inner(ctx.serenity_context()).await?;

    ctx.reply(format!("Successfully uploaded {count} sounds"))
        .await
        .ok();
    Ok(())
}

#[poise::command(prefix_command, owners_only)]
pub async fn delete(ctx: Context<'_>, #[rest] rest: String) -> anyhow::Result<()> {
    let storage = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<SoundStorage>()
        .context("Could not get SoundStorage")?
        .clone();
    let client = cloud_storage::Client::default();
    let mut deleted = Vec::new();

    for name in rest.split_whitespace() {
        let sound_file = { storage.read().get(name) };
        if let Some(file) = sound_file {
            if fs::remove_file(&file.path).is_ok()
                && client
                    .object()
                    .delete("surfpvparena", &format!("dist/sound/{}.mp3", file.name))
                    .await
                    .is_ok()
            {
                deleted.push(file.name.clone());
            }
        }
    }

    tokio::spawn(update_sounds_bin(storage.read().dir.clone()));

    clean_cache_inner(ctx.serenity_context()).await?;

    if deleted.is_empty() {
        ctx.reply("The given saysounds were not found").await.ok();
    } else {
        ctx.reply(format!("Deleted: {}", deleted.join(", ")))
            .await
            .ok();
    }
    Ok(())
}

#[allow(clippy::single_match)]
#[poise::command(prefix_command, guild_only)]
pub async fn config(
    ctx: Context<'_>,
    action: String,
    key: Option<String>,
    value: Option<String>,
) -> anyhow::Result<()> {
    let configs = ctx
        .serenity_context()
        .data
        .read()
        .await
        .get::<Configs>()
        .context("Could not get Configs")?
        .clone();
    match action.as_str() {
        "set" => match key {
            Some(key) => match value {
                Some(value) => {
                    let old_value = {
                        let mut configs = configs.write();
                        let old_value = configs.get(
                            &ctx.guild_id().context("Guild was not found")?,
                            &key,
                            &ctx.author().id,
                        );
                        configs.set(
                            &ctx.guild_id().context("Guild was not found")?,
                            &key,
                            &value,
                            &ctx.author().id,
                        )?;
                        old_value
                    };
                    if let Some(old_value) = old_value {
                        ctx.reply(format!("Set {key}: {old_value} -> {value}"))
                            .await?;
                    } else {
                        ctx.reply(format!("Set {key}: {value}")).await?;
                    }
                }
                None => {}
            },
            None => {}
        },
        "remove" => match key {
            Some(key) => {
                let old_value = {
                    let mut configs = configs.write();
                    let old_value = configs.get(
                        &ctx.guild_id().context("Guild was not found")?,
                        &key,
                        &ctx.author().id,
                    );
                    configs.remove(
                        &ctx.guild_id().context("Guild was not found")?,
                        &key,
                        &ctx.author().id,
                    )?;
                    old_value
                };
                if let Some(old_value) = old_value {
                    ctx.reply(format!("Removed {key}: {old_value}")).await?;
                }
            }
            None => {}
        },
        "list" => {}
        _ => {}
    }
    Ok(())
}
