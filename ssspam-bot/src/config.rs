// TODO: need to completely rewrite the configuration logic.
use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use anyhow::{Context as _, bail};
use pickledb::{PickleDb, PickleDbDumpPolicy, SerializationMethod};
use serenity::{
    model::prelude::{GuildId, UserId},
    prelude::TypeMapKey,
};

pub struct Configs {
    db: PickleDb,
}

impl Configs {
    pub fn load_or_create<P: AsRef<Path>>(config_file: P) -> anyhow::Result<Self> {
        let db = if config_file.as_ref().exists() {
            PickleDb::load(
                config_file,
                PickleDbDumpPolicy::AutoDump,
                SerializationMethod::Json,
            )?
        } else {
            PickleDb::new(
                config_file,
                PickleDbDumpPolicy::AutoDump,
                SerializationMethod::Json,
            )
        };
        Ok(Self { db })
    }

    pub fn get_joinsound(&self, user_id: &UserId) -> Option<String> {
        self.db
            .get::<String>(&format!("users.u{user_id}.joinsound"))
    }

    pub fn set_joinsound(&mut self, user_id: &UserId, sound: &str) -> anyhow::Result<()> {
        self.db
            .set::<String>(&format!("users.u{user_id}.joinsound"), &sound.to_owned())
            .context("Failed to set joinsound")
    }

    pub fn remove_joinsound(&mut self, user_id: &UserId) -> anyhow::Result<bool> {
        self.db
            .rem(&format!("users.u{user_id}.joinsound"))
            .context("Faield to remove joinsound")
    }

    pub fn get_leavesound(&self, user_id: &UserId) -> Option<String> {
        self.db
            .get::<String>(&format!("users.u{user_id}.leavesound"))
    }

    pub fn set_leavesound(&mut self, user_id: &UserId, sound: &str) -> anyhow::Result<()> {
        self.db
            .set::<String>(&format!("users.u{user_id}.leavesound"), &sound.to_owned())
            .context("Failed to set joinsound")
    }

    pub fn remove_leavesound(&mut self, user_id: &UserId) -> anyhow::Result<bool> {
        self.db
            .rem(&format!("users.u{user_id}.leavesound"))
            .context("Faield to remove joinsound")
    }

    pub fn get(&self, _guild_id: &GuildId, key: &str, user_id: &UserId) -> Option<String> {
        match key {
            "joinsound" => self.get_joinsound(user_id),
            "leavesound" => self.get_leavesound(user_id),
            _ => None,
        }
    }

    pub fn set(
        &mut self,
        _guild_id: &GuildId,
        key: &str,
        value: &str,
        user_id: &UserId,
    ) -> anyhow::Result<()> {
        match key {
            "joinsound" => self.set_joinsound(user_id, value),
            "leavesound" => self.set_leavesound(user_id, value),
            _ => bail!("Unrecognized key"),
        }
    }

    pub fn remove(
        &mut self,
        _guild_id: &GuildId,
        key: &str,
        user_id: &UserId,
    ) -> anyhow::Result<bool> {
        match key {
            "joinsound" => self.remove_joinsound(user_id),
            "leavesound" => self.remove_leavesound(user_id),
            _ => bail!("Unrecognized key"),
        }
    }
}

impl TypeMapKey for Configs {
    type Value = Arc<RwLock<Self>>;
}
