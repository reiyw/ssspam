pub mod command;
pub mod config;
pub mod core;
pub mod play;
pub mod sound;
pub mod sslang;
pub mod web;

#[macro_use]
extern crate derive_builder;
#[macro_use]
extern crate prettytable;

pub use crate::{
    command::{leave_voice_channel, ShutdownChannel, GENERAL_GROUP, OWNER_GROUP},
    config::CONFIG_DIR,
    core::{process_message, ChannelManager, GuildBroadcast, OpsMessage},
    play::{play_say_commands, SaySoundCache, VolumeManager},
    sound::{SoundFile, SoundStorage},
    sslang::{SayCommand, SayCommands},
};
