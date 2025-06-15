pub mod command;
pub mod config;
pub mod core;
pub mod play;
pub mod scripting;
pub mod sound;
pub mod sslang;
pub mod web;

#[macro_use]
extern crate derive_builder;
#[macro_use]
extern crate prettytable;

pub use crate::{
    command::leave_voice_channel,
    config::Configs,
    core::{ChannelManager, GuildBroadcast, OpsMessage, process_message},
    play::{SaySoundCache, play_say_commands},
    scripting::interpret_rhai,
    sound::{SoundFile, SoundStorage},
    sslang::{SayCommand, SayCommandBuilder, SayCommands},
};
