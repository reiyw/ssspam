pub mod command;
pub mod parser;
pub mod play;
pub mod sound;
pub mod sslang;
pub mod web;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use counter::Counter;
#[macro_use]
extern crate derive_builder;
use glob::glob;
#[macro_use]
extern crate prettytable;
use prettytable::{format, Table};
use serde::{Deserialize, Serialize};

pub use crate::{
    command::{
        leave_based_on_voice_state_update, JOIN_COMMAND, LEAVE_COMMAND, MUTE_COMMAND,
        UNMUTE_COMMAND, ChannelManager,
    },
    play::{calc_sound_duration, play_source},
    sound::{Sound, SoundStorage},
    sslang::{SayCommand, SayCommands},
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SoundDetail {
    pub name: String,
    pub path: PathBuf,
    pub sample_rate_hz: u32,
    pub channel_count: u8,

    pub duration: Duration,
    pub updated_at: SystemTime,
}

impl SoundDetail {
    const fn new(
        name: String,
        path: PathBuf,
        sample_rate_hz: u32,
        channel_count: u8,
        duration: Duration,
        updated_at: SystemTime,
    ) -> Self {
        Self {
            name,
            path,
            sample_rate_hz,
            channel_count,
            duration,
            updated_at,
        }
    }

    fn from_mp3(path: &Path) -> Result<Self> {
        let ss_name = path
            .file_stem()
            .context("file name must exist")?
            .to_str()
            .context("failed to convert OsStr to str")?
            .to_string();

        let data = mp3_metadata::read_from_file(path)
            .with_context(|| format!("failed to read metadata: {:?}", path))?;

        let freqs: Counter<_> = data.frames.iter().map(|f| f.sampling_freq).collect();
        let sample_rate_hz = freqs.most_common()[0].0 as u32;

        let channel_counts: Counter<_> = data
            .frames
            .iter()
            .map(|f| match f.chan_type {
                mp3_metadata::ChannelType::SingleChannel => 1,
                // FIXME: I'm not sure this logic is correct.
                _ => 2,
            })
            .collect();
        let channel_count = channel_counts.most_common()[0].0;

        let updated_at = fs::metadata(path)?.modified()?;

        Ok(Self::new(
            ss_name,
            path.to_path_buf(),
            sample_rate_hz,
            channel_count,
            data.duration,
            updated_at,
        ))
    }

    fn from_m4a(path: &Path) -> Result<Self> {
        let ss_name = path
            .file_stem()
            .context("file name must exist")?
            .to_str()
            .context("failed to convert OsStr to str")?
            .to_string();

        let data = mp4ameta::Tag::read_from_path(path)?;

        let sample_rate_hz = data
            .sample_rate()
            .context("failed to load a samping rate")?
            .hz();

        let channel_count = data
            .channel_config()
            .context("failed to load a channel count")?
            .channel_count();

        let duration = data.duration().context("failed to load a duration")?;

        let updated_at = fs::metadata(path)?.modified()?;

        Ok(Self::new(
            ss_name,
            path.to_path_buf(),
            sample_rate_hz,
            channel_count,
            duration,
            updated_at,
        ))
    }
}

pub fn load_sounds<P: AsRef<Path>>(sound_dir: P) -> BTreeMap<String, SoundDetail> {
    let mut path_map = BTreeMap::new();

    for path in (glob(&format!("{}/*.mp3", sound_dir.as_ref().to_string_lossy()))
        .expect("Failed to read glob pattern"))
    .flatten()
    {
        match SoundDetail::from_mp3(&path) {
            Ok(sound) => {
                path_map.insert(sound.name.clone().to_lowercase(), sound);
            }
            Err(e) => eprintln!("Failed to read metadata from {:?}. Reason: {}", path, e),
        }
    }

    for path in (glob(&format!("{}/*.m4a", sound_dir.as_ref().to_string_lossy()))
        .expect("Failed to read glob pattern"))
    .flatten()
    {
        match SoundDetail::from_m4a(&path) {
            Ok(sound) => {
                path_map.insert(sound.name.clone().to_lowercase(), sound);
            }
            Err(e) => eprintln!("Failed to read metadata from {:?}. Reason: {}", path, e),
        }
    }

    path_map
}

pub fn load_sounds_try_from_cache<P: AsRef<Path>>(sound_dir: P) -> BTreeMap<String, SoundDetail> {
    match fs::read(sound_dir.as_ref().join("sounds.bin")) {
        Ok(encoded) => bincode::deserialize(&encoded).unwrap(),
        Err(_) => load_sounds(sound_dir),
    }
}

pub fn search_impl<S: AsRef<str>, T: AsRef<str>>(
    query: S,
    target: impl Iterator<Item = T>,
    max_results: usize,
) -> Vec<(String, f64)> {
    let mut sims: Vec<_> = target
        .map(|t| {
            (
                t.as_ref().to_string(),
                strsim::jaro_winkler(query.as_ref(), &t.as_ref().to_lowercase()),
            )
        })
        .collect();
    sims.sort_by(|(_, d1), (_, d2)| d2.partial_cmp(d1).unwrap());

    let filtered: Vec<&(String, f64)> = sims
        .iter()
        .filter(|(_, d)| d >= &0.85)
        .take(max_results)
        .collect();
    if filtered.len() < 10 {
        sims[..10].to_vec()
    } else {
        filtered.into_iter().cloned().collect()
    }
}

pub fn prettify_sounds(sounds: impl Iterator<Item = SoundDetail>) -> String {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_CLEAN);

    table.set_titles(row!["Name", "Dur", "Updated"]);
    for sound in sounds {
        let updated_at: DateTime<Utc> = sound.updated_at.into();
        // let updated_at = updated_at.with_timezone(&FixedOffset::east(9 * 3600));

        table.add_row(row![
            sound.name,
            format!("{:.1}", sound.duration.as_secs_f64()),
            updated_at.format("%Y-%m-%d") // updated_at.format("%Y-%m-%d %T")
        ]);
    }

    table.to_string()
}
