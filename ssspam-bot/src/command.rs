use std::{collections::HashSet, fs, path::PathBuf, str::FromStr, time::Duration};

use anyhow::Context as _;
use async_zip::read::mem::ZipFileReader;
use chrono::{DateTime, Utc};
use itertools::Itertools;
use prettytable::{format, Table};
use serenity::{
    client::Context,
    framework::standard::{
        macros::{command, group},
        Args, CommandResult,
    },
    model::{channel::Message, id::GuildId, prelude::UserId},
    prelude::{Mentionable, TypeMapKey},
};
use systemstat::{Platform, System};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot::{self, Receiver, Sender},
};
use tracing::{info, warn};

use crate::{
    core::{process_from_string, ChannelUserManager},
    interpret_rhai,
    web::update_data_json,
    ChannelManager, Configs, GuildBroadcast, OpsMessage, SayCommands, SaySoundCache, SoundStorage,
};

#[group]
#[only_in(guilds)]
#[commands(
    join,
    leave,
    mute,
    unmute,
    stop,
    clean_cache,
    r,
    s,
    st,
    uptime,
    rhai,
    config
)]
struct General;

#[command]
pub async fn join(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = join_impl(ctx, msg).await {
        warn!("Error joining the channel: {e:?}");
    }
    Ok(())
}

async fn join_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild = msg
        .guild(&ctx.cache)
        .context("Guild's ID was not found")?
        .clone();
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

    let handler_lock = manager.join(guild.id, voice_channel_id).await;
    if handler_lock.is_ok() {
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
    let guild_id = msg.guild_id.context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    manager.remove(guild_id).await?;

    let channel_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();
    channel_manager.write().leave(&guild_id);

    Ok(())
}

pub async fn leave_voice_channel(ctx: &Context, guild_id: GuildId) -> anyhow::Result<()> {
    let channel_manager = ctx
        .data
        .read()
        .await
        .get::<ChannelManager>()
        .context("Could not get ChannelManager")?
        .clone();
    let bots_voice_channel_id = { channel_manager.read().get_voice_channel_id(&guild_id) };
    if let Some(bots_voice_channel_id) = bots_voice_channel_id {
        let channel = ctx
            .cache
            .channel(bots_voice_channel_id)
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
    ctx: &Context,
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
            let channel = ctx
                .cache
                .channel(bots_voice_channel_id)
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

#[command]
pub async fn mute(ctx: &Context, msg: &Message) -> CommandResult {
    if let Err(e) = mute_impl(ctx, msg).await {
        warn!("Failed to mute: {e:?}");
    }
    Ok(())
}

async fn mute_impl(ctx: &Context, msg: &Message) -> anyhow::Result<()> {
    let guild_id = msg.guild_id.context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    handler_lock.lock().await.mute(true).await?;

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
    let guild_id = msg.guild_id.context("Guild's ID was not found")?;

    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    handler_lock.lock().await.mute(false).await?;

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
    let guild_id = msg.guild_id.context("Guild's ID was not found")?;
    let manager = songbird::get(ctx)
        .await
        .context("Songbird Voice client placed in at initialization.")?
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };
    handler_lock.lock().await.stop();

    let guild_broadcast = ctx
        .data
        .read()
        .await
        .get::<GuildBroadcast>()
        .context("Could not get GuildBroadcast")?
        .clone();
    let tx = guild_broadcast.lock().get_sender(guild_id);
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
    let file = storage.read().get_random().context("Has no sound file")?;
    let cmds = SayCommands::from_str(&format!("{} {}", file.name, args.rest()))?;
    Ok(cmds)
}

#[command]
pub async fn s(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    if let Some(arg) = args.current() {
        let names: Vec<_> = {
            let storage = ctx.data.read().await.get::<SoundStorage>().unwrap().clone();
            let sims = storage.read().calc_similarities(arg);
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
        msg.channel_id.say(&ctx.http, names.join(", ")).await.ok();
    }
    Ok(())
}

#[command]
pub async fn st(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    if let Some(arg) = args.current() {
        let mut table = Table::new();

        {
            let storage = ctx.data.read().await.get::<SoundStorage>().unwrap().clone();
            let sims = storage.read().calc_similarities(arg);

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
        }

        msg.channel_id
            .say(&ctx.http, format!("```\n{table}\n```"))
            .await
            .ok();
    }
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn uptime(ctx: &Context, msg: &Message) -> CommandResult {
    let sys = System::new();
    msg.channel_id
        .say(
            &ctx.http,
            humantime::format_duration(sys.uptime().unwrap()).to_string(),
        )
        .await
        .ok();
    Ok(())
}

#[command]
pub async fn rhai(ctx: &Context, msg: &Message) -> CommandResult {
    let source = msg.content[6..].trim().trim_matches('`').to_owned();
    dbg!(&source);
    let task = tokio::task::spawn_blocking(move || interpret_rhai(&source));
    match tokio::time::timeout(Duration::from_secs(1), task).await {
        Ok(Ok(Ok(say_commands))) => {
            msg.channel_id.say(&ctx.http, say_commands).await.ok();
        }
        Err(e) => {
            msg.channel_id.say(&ctx.http, e.to_string()).await.ok();
        }
        Ok(Err(e)) => {
            msg.channel_id.say(&ctx.http, e.to_string()).await.ok();
        }
        Ok(Ok(Err(e))) => {
            msg.channel_id.say(&ctx.http, e.to_string()).await.ok();
        }
    }
    Ok(())
}

#[group]
#[owners_only]
#[only_in(guilds)]
#[commands(upload, delete, shutdown)]
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

#[tracing::instrument]
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

    tokio::spawn(update_data_json(storage.read().dir.clone()));

    storage.write().reload();

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

    match delete_impl(ctx, args).await {
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

async fn delete_impl(ctx: &Context, mut args: Args) -> anyhow::Result<Vec<String>> {
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

    tokio::spawn(update_data_json(storage.read().dir.clone()));

    clean_cache_impl(ctx).await?;

    Ok(deleted)
}

pub struct ShutdownChannel {
    tx: Sender<()>,
}

impl ShutdownChannel {
    pub fn new() -> (Receiver<()>, Self) {
        let (tx, rx) = oneshot::channel();
        (rx, Self { tx })
    }

    fn send_shutdown(self) {
        self.tx.send(()).unwrap();
    }
}

impl TypeMapKey for ShutdownChannel {
    type Value = Self;
}

#[command]
pub async fn shutdown(ctx: &Context, _msg: &Message) -> CommandResult {
    let channel = ctx.data.write().await.remove::<ShutdownChannel>().unwrap();
    channel.send_shutdown();
    Ok(())
}

#[command]
pub async fn config(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    match config_impl(ctx, msg, args).await {
        Ok(()) => {}
        Err(e) => {
            msg.channel_id
                .say(&ctx.http, format!("Error: {e}"))
                .await
                .ok();
        }
    }
    Ok(())
}

#[allow(clippy::single_match)]
async fn config_impl(ctx: &Context, msg: &Message, args: Args) -> anyhow::Result<()> {
    let configs = ctx
        .data
        .read()
        .await
        .get::<Configs>()
        .context("Could not get Configs")?
        .clone();
    let guild_id = msg.guild_id.context("Guild's ID was not found")?;
    match args.clone().current() {
        Some("set") => match args.clone().advance().current() {
            Some(key) => match args.clone().advance().advance().current() {
                Some(value) => {
                    let old_value = {
                        let mut configs = configs.write();
                        let old_value = configs.get(&guild_id, key, &msg.author.id);
                        configs.set(&guild_id, key, value, &msg.author.id)?;
                        old_value
                    };
                    if let Some(old_value) = old_value {
                        msg.reply(ctx, format!("Set {key}: {old_value} -> {value}"))
                            .await?;
                    } else {
                        msg.reply(ctx, format!("Set {key}: {value}")).await?;
                    }
                }
                None => {}
            },
            None => {}
        },
        Some("remove") => match args.clone().advance().current() {
            Some(key) => {
                let old_value = {
                    let mut configs = configs.write();
                    let old_value = configs.get(&guild_id, key, &msg.author.id);
                    configs.remove(&guild_id, key, &msg.author.id)?;
                    old_value
                };
                if let Some(old_value) = old_value {
                    msg.reply(ctx, format!("Removed {key}: {old_value}"))
                        .await?;
                }
            }
            None => {}
        },
        Some("list") => {}
        Some(_sub_cmd) => {}
        None => {}
    }

    Ok(())
}
