// TODO: need to completely rewrite the configuration logic.
use std::{path::Path, sync::Arc};

use anyhow::{bail, Context as _};
use parking_lot::RwLock;
use pickledb::{PickleDb, PickleDbDumpPolicy, SerializationMethod};
use serenity::{model::prelude::GuildId, prelude::TypeMapKey};

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

    pub fn get_clip_threshold(&self) -> f32 {
        self.db.get::<f32>("global.clip_threshold").unwrap_or(0.01)
    }

    // TODO: Generic values
    pub fn set_clip_threshold(&mut self, value: &str) -> anyhow::Result<()> {
        self.db
            .set("global.clip_threshold", &value.parse::<f32>()?)
            .context("Failed to set clip_threshold")
    }

    pub fn get_sharpness(&self) -> f32 {
        self.db.get::<f32>("global.sharpness").unwrap_or(250.0)
    }

    pub fn set_sharpness(&mut self, value: &str) -> anyhow::Result<()> {
        self.db
            .set("global.sharpness", &value.parse::<f32>()?)
            .context("Failed to set sharpness")
    }

    pub fn set(&mut self, _guild_id: &GuildId, key: &str, value: &str) -> anyhow::Result<()> {
        match key {
            "clip_threshold" => self.set_clip_threshold(value),
            "sharpness" => self.set_sharpness(value),
            _ => bail!("Unrecognized key"),
        }
    }
}

impl TypeMapKey for Configs {
    type Value = Arc<RwLock<Self>>;
}
