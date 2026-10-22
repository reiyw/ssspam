// TODO: refactor this file

use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

use chrono::{DateTime, Utc};
use google_cloud_storage::client::Storage;
use prost::Message;
use serde::Serialize;
use tempfile::tempdir;

use crate::{SoundStorage, sound::ToSoundsProto};

pub const GCS_BUCKET: &str = "projects/_/buckets/surfpvparena";

#[derive(Debug, Serialize)]
struct Data {
    data: Vec<(String, String, String, String, String)>,
}

pub fn gen_data_json_from_sound_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    sound_dir: P,
    out_file: Q,
) -> anyhow::Result<()> {
    let storage = SoundStorage::load(sound_dir);
    let mut data: Vec<(String, String, String, String, String)> = Vec::new();

    for file in storage.files() {
        let updated_at: DateTime<Utc> = file.updated_at().into();
        let src = format!("sound/{}.mp3", file.name);
        let row = (
            file.name.clone(),
            file.references().join(", "),
            format!("{:.1}", file.duration().as_secs_f64()),
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

pub fn gen_sounds_bin_from_sound_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    sound_dir: P,
    out_file: Q,
) -> anyhow::Result<()> {
    let storage = SoundStorage::load(sound_dir);
    let mut buf = Vec::new();
    storage.files().cloned().to_sounds().encode(&mut buf)?;
    fs::write(out_file, buf)?;
    Ok(())
}

#[allow(clippy::future_not_send)]
pub async fn update_data_json<P: AsRef<Path>>(sound_dir: P) -> anyhow::Result<()> {
    let temp_dir = tempdir()?;
    let out_file = temp_dir.path().join("data.json");

    gen_data_json_from_sound_dir(sound_dir, &out_file)?;

    let data = fs::read(&out_file)?;
    let client = Storage::builder().build().await?;
    client
        .write_object(GCS_BUCKET, "dist/data.json", bytes::Bytes::from(data))
        .set_content_type("application/json")
        .send_buffered()
        .await?;
    Ok(())
}

#[allow(clippy::future_not_send)]
pub async fn update_sounds_bin<P: AsRef<Path>>(sound_dir: P) -> anyhow::Result<()> {
    let temp_dir = tempdir()?;
    let out_file = temp_dir.path().join("sounds.bin");

    gen_sounds_bin_from_sound_dir(sound_dir, &out_file)?;

    let data = fs::read(&out_file)?;
    let client = Storage::builder().build().await?;
    client
        .write_object(GCS_BUCKET, "dist/sounds.bin", bytes::Bytes::from(data))
        .set_content_type("application/octet-stream")
        .send_buffered()
        .await?;
    Ok(())
}
