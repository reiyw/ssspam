use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{load_sounds, SoundDetail};

#[derive(Debug, Serialize)]
struct Data {
    data: Vec<(String, String, String, String)>,
}

pub fn gen_data_json_from_sound_dir(
    sound_dir: impl AsRef<Path>,
    out_file: impl AsRef<Path>,
) -> anyhow::Result<()> {
    gen_data_json_from_sounds(load_sounds(sound_dir).values(), out_file)
}

pub fn gen_data_json_from_sounds<'a>(
    sounds: impl Iterator<Item = &'a SoundDetail>,
    out_file: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let mut data: Vec<(String, String, String, String)> = Vec::new();
    for sound in sounds {
        let updated_at: DateTime<Utc> = sound.updated_at.into();
        let src = format!(
            "sound/{}",
            sound.path.file_name().unwrap().to_str().unwrap()
        );
        let row = (
            sound.name.clone(),
            format!("{:.1}", sound.duration.as_secs_f64()),
            updated_at.format("%Y-%m-%d").to_string(),
            src,
        );
        data.push(row);
    }
    let data = Data { data };
    let j = serde_json::to_string(&data)?;
    let mut f = File::create(out_file)?;
    f.write_all(j.as_bytes())?;
    Ok(())
}

#[allow(clippy::future_not_send)]
pub async fn update_data_json(sound_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    gen_data_json_from_sound_dir(sound_dir, "data.json")?;
    let data = fs::read("data.json")?;
    let client = cloud_storage::Client::default();
    client
        .object()
        .create("surfpvparena", data, "dist/data.json", "application/json")
        .await?;
    Ok(())
}
