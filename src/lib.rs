pub mod parser;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use counter::Counter;
use glob::glob;
#[macro_use]
extern crate pest_derive;
#[macro_use]
extern crate prettytable;
use prettytable::{format, Table};
use regex::Regex;
use serde::{Deserialize, Serialize};
use songbird::{create_player, input::Input, Call};
use tokio::sync::Mutex;

const VOLUME: f32 = 0.05;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SoundDetail {
    name: String,
    pub path: PathBuf,
    pub sample_rate_hz: u32,
    pub is_stereo: bool,
}

impl SoundDetail {
    fn new(name: String, path: PathBuf, sample_rate_hz: u32, is_stereo: bool) -> Self {
        Self {
            name,
            path,
            sample_rate_hz,
            is_stereo,
        }
    }
}

pub fn load_sounds<P: AsRef<Path>>(sound_dir: P) -> BTreeMap<String, SoundDetail> {
    let fixed_ss_name_reg = Regex::new(r"^([^_]+)_\d+$").unwrap();

    let mut path_map = BTreeMap::new();

    for path in
        (glob(&format!("{}/*.mp3", sound_dir.as_ref().to_str().unwrap())).unwrap()).flatten()
    {
        let mut ss_name = path.file_stem().unwrap().to_str().unwrap().to_string();
        if let Some(caps) = fixed_ss_name_reg.captures(&ss_name) {
            ss_name = caps.get(1).unwrap().as_str().to_string();
        };

        let data = mp3_metadata::read_from_file(path.clone());
        if data.is_err() {
            println!("invalid: {:?}", path);
        }

        let freqs: Counter<_> = data
            .as_ref()
            .unwrap()
            .frames
            .iter()
            .map(|f| f.sampling_freq)
            .collect();
        let sample_rate_hz = freqs.most_common()[0].0 as u32;

        let chan_types: Counter<_> = data
            .unwrap()
            .frames
            .iter()
            .map(|f| f.chan_type == mp3_metadata::ChannelType::SingleChannel)
            .collect();
        let is_stereo = !chan_types.most_common()[0].0;

        path_map.insert(
            ss_name.clone(),
            SoundDetail::new(ss_name, path, sample_rate_hz, is_stereo),
        );
    }

    path_map
}

pub fn load_sounds_try_from_cache<P: AsRef<Path>>(sound_dir: P) -> BTreeMap<String, SoundDetail> {
    match fs::read(sound_dir.as_ref().join("sounds.bin")) {
        Ok(encoded) => bincode::deserialize(&encoded).unwrap(),
        Err(_) => load_sounds(sound_dir),
    }
}

pub async fn play_source(source: Input, handler_lock: Arc<Mutex<Call>>) {
    let (mut audio, _audio_handle) = create_player(source);
    audio.set_volume(VOLUME);
    let mut handler = handler_lock.lock().await;
    handler.play(audio);
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

    let filtered: Vec<&(String, f64)> = sims.iter().filter(|(_, d)| d >= &0.85).take(max_results).collect();
    if filtered.len() < 10 {
        sims[..10].to_vec()
    } else {
        filtered.into_iter().cloned().collect()
    }
}

pub fn prettify_sounds(sounds: impl Iterator<Item = SoundDetail>) -> String {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_CLEAN);

    table.set_titles(row!["Name", "Sampling Rate"]);
    for sound in sounds {
        table.add_row(row![sound.name, sound.sample_rate_hz]);
    }

    table.to_string()
}
