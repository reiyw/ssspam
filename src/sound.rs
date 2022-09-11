#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::Context as _;
use counter::Counter;
use glob::glob;
use log::{info, warn};
use notify::{
    event::{CreateKind, ModifyKind, RenameMode},
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use rand::{rngs::StdRng, seq::IteratorRandom, SeedableRng};
use serenity::prelude::TypeMapKey;
use tokio::{runtime::Handle, sync::mpsc};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Metadata {
    sample_rate_hz: u32,
    channel_count: u8,
    duration: Duration,
    updated_at: SystemTime,
}

impl Metadata {
    fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let data = mp3_metadata::read_from_file(path.as_ref())?;
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
        let duration = data.duration;
        let updated_at = fs::metadata(path.as_ref())?.modified()?;
        Ok(Self {
            sample_rate_hz,
            channel_count,
            duration,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFile {
    pub name: String,
    pub path: PathBuf,

    // Retrieving metadata requires file parsing and is time consuming. For most
    // files, metadata is not needed immediately, so wrap in OnceCell to delay
    // metadata retrieval.
    metadata: OnceCell<Metadata>,
}

impl SoundFile {
    /// Initializes [`Sound`] without loading metadata.
    fn new_unchecked<P: AsRef<Path>>(path: P) -> Self {
        Self {
            name: path.as_ref().file_stem().unwrap().to_string_lossy().into(),
            path: path.as_ref().into(),
            metadata: OnceCell::new(),
        }
    }

    /// Initializes [`Sound`] with the metadata loaded.
    ///
    /// Use this method if the input file is unreliable as sound data.
    fn new_checked<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        Ok(Self {
            name: path
                .as_ref()
                .file_stem()
                .context("No file name")?
                .to_string_lossy()
                .into(),
            path: path.as_ref().into(),
            metadata: Metadata::load(path.as_ref())?.into(),
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.metadata
            .get_or_init(|| self.load_unchecked())
            .sample_rate_hz
    }

    pub fn channel_count(&self) -> u8 {
        self.metadata
            .get_or_init(|| self.load_unchecked())
            .channel_count
    }

    pub fn duration(&self) -> Duration {
        self.metadata.get_or_init(|| self.load_unchecked()).duration
    }

    pub fn updated_at(&self) -> SystemTime {
        self.metadata
            .get_or_init(|| self.load_unchecked())
            .updated_at
    }

    fn load_unchecked(&self) -> Metadata {
        Metadata::load(&self.path)
            .expect(&format!("Failed to load the metadata of {:?}", self.path))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundStorage {
    /// Lowercased name to [`Sound`].
    sounds: BTreeMap<String, SoundFile>,

    pub dir: PathBuf,
}

impl SoundStorage {
    pub fn load<P: AsRef<Path>>(dir: P) -> Self {
        let mut sounds = BTreeMap::new();
        for path in
            (glob(&format!("{}/**/*.mp3", dir.as_ref().to_string_lossy())).unwrap()).flatten()
        {
            let sound = SoundFile::new_unchecked(&path);
            sounds.insert(sound.name.to_lowercase(), sound);
        }
        Self {
            sounds,
            dir: dir.as_ref().into(),
        }
    }

    pub fn get(&self, name: impl AsRef<str>) -> Option<&SoundFile> {
        self.sounds.get(&name.as_ref().to_lowercase())
    }

    fn remove(&mut self, name: impl AsRef<str>) -> Option<SoundFile> {
        self.sounds.remove(&name.as_ref().to_lowercase())
    }

    fn add(&mut self, sound: SoundFile) -> Option<SoundFile> {
        self.sounds.insert(sound.name.to_lowercase(), sound)
    }

    pub fn get_random(&self) -> Option<&SoundFile> {
        let mut rng: StdRng = SeedableRng::from_entropy();
        self.sounds.values().choose(&mut rng)
    }

    pub fn calc_similarities(&self, query: impl AsRef<str>) -> Vec<(f64, &SoundFile)> {
        let mut sims: Vec<_> = self
            .sounds
            .iter()
            .map(|(name, sound)| {
                (
                    strsim::jaro_winkler(&query.as_ref().to_lowercase(), name),
                    sound,
                )
            })
            .collect();
        sims.sort_by(|(d1, _), (d2, _)| d2.partial_cmp(d1).unwrap());
        sims
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }
}

pub async fn watch_sound_storage(storage: Arc<RwLock<SoundStorage>>) {
    let (tx, mut rx) = mpsc::channel(1);
    let handle = Handle::current();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            handle.block_on(tx.send(res)).ok();
        },
        Config::default(),
    )
    .unwrap();

    {
        let storage = storage.read();
        watcher
            .watch(&storage.dir, RecursiveMode::Recursive)
            .unwrap();
    }

    while let Some(Ok(event)) = rx.recv().await {
        if event.paths[0].extension() != Some(OsStr::new("mp3")) {
            continue;
        }
        info!("Event in the sound directory: {event:?}");
        match event {
            Event {
                kind: EventKind::Create(CreateKind::File),
                paths,
                ..
            }
            | Event {
                kind: EventKind::Modify(ModifyKind::Data(_)),
                paths,
                ..
            } => match SoundFile::new_checked(&paths[0]) {
                Ok(sound) => {
                    let mut storage = storage.write();
                    storage.add(sound);
                }
                Err(e) => {
                    warn!("Error loading a sound file {:?}: {e:?}", paths[0]);
                }
            },
            Event {
                kind: EventKind::Remove(_),
                paths,
                ..
            } => {
                let mut storage = storage.write();
                if let Some(file_stem) = paths[0].file_stem() {
                    storage.remove(file_stem.to_string_lossy());
                }
            }
            Event {
                kind: EventKind::Modify(ModifyKind::Name(rename_mode)),
                paths,
                ..
            } => match rename_mode {
                RenameMode::Any | RenameMode::Other => {
                    if let Ok(sound) = SoundFile::new_checked(&paths[0]) {
                        let mut storage = storage.write();
                        storage.add(sound);
                    } else {
                        let mut storage = storage.write();
                        if let Some(file_stem) = paths[0].file_stem() {
                            storage.remove(file_stem.to_string_lossy());
                        }
                    }
                }
                RenameMode::From => {
                    let mut storage = storage.write();
                    if let Some(file_stem) = paths[0].file_stem() {
                        storage.remove(file_stem.to_string_lossy());
                    }
                }
                RenameMode::To => {
                    if let Ok(sound) = SoundFile::new_checked(&paths[0]) {
                        let mut storage = storage.write();
                        storage.add(sound);
                    }
                }
                RenameMode::Both => {
                    let mut storage = storage.write();
                    if let Some(file_stem) = paths[0].file_stem() {
                        storage.remove(file_stem.to_string_lossy());
                    }
                    if let Ok(sound) = SoundFile::new_checked(&paths[1]) {
                        storage.add(sound);
                    }
                }
            },
            _ => {}
        }
    }
}

impl TypeMapKey for SoundStorage {
    type Value = Arc<RwLock<Self>>;
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_sound() {
        let sound_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sound");
        let sound = SoundFile::new_unchecked(sound_dir.join("sainou.mp3"));
        assert_eq!(sound.name, "sainou".to_string());
        assert_eq!(sound.path, sound_dir.join("sainou.mp3"));
        assert_eq!(sound.sample_rate_hz(), 44100);
        assert_eq!(sound.channel_count(), 2);
    }

    #[test]
    fn test_sound_storage() {
        let sound_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sound");
        let mut storage = SoundStorage::load(&sound_dir);
        assert_eq!(storage.len(), 3);
        assert_eq!(
            storage.get("sainou").unwrap().path,
            sound_dir.join("sainou.mp3")
        );
        assert_eq!(storage.get("d").unwrap().path, sound_dir.join("d.mp3"));
        assert_eq!(
            storage.get("dadeisan").unwrap().path,
            sound_dir.join("dadeisan.mp3")
        );

        storage.remove("d");
        storage.remove("dadeisan");
        assert_eq!(storage.len(), 1);
        assert_eq!(storage.get("d"), None);
        assert_eq!(storage.get("dadeisan"), None);
        assert_eq!(
            storage.get_random().unwrap(),
            storage.get("sainou").unwrap()
        );

        storage.remove("sainou");
        assert_eq!(storage.get_random(), None);
    }

    #[test]
    fn test_calc_similarities() {
        let sound_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sound");
        let storage = SoundStorage::load(&sound_dir);
        let sims = storage.calc_similarities("dadei");
        assert_eq!(sims[0].1, storage.get("dadeisan").unwrap());
        assert_eq!(sims[1].1, storage.get("d").unwrap());
        assert_eq!(sims[2].1, storage.get("sainou").unwrap());
    }

    #[tokio::test]
    async fn test_watch_sound_storage() {
        const DELAY: Duration = Duration::from_millis(100);
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_dir_path = temp_dir.path();
        let storage = Arc::new(RwLock::new(SoundStorage::load(&temp_dir)));
        tokio::spawn(watch_sound_storage(Arc::clone(&storage)));
        tokio::time::sleep(DELAY).await;

        let sound_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sound");

        fs::copy(
            sound_dir.join("sainou.mp3"),
            temp_dir_path.join("sainou.mp3"),
        )
        .unwrap();
        tokio::time::sleep(DELAY).await;
        {
            let storage = storage.read();
            assert!(storage.get("sainou").is_some());
        }

        fs::copy(
            sound_dir.join("dadeisan.mp3"),
            temp_dir_path.join("dadeisan.mp3"),
        )
        .unwrap();
        tokio::time::sleep(DELAY).await;
        {
            let storage = storage.read();
            assert!(storage.get("dadeisan").is_some());
            assert_eq!(storage.len(), 2);
        }

        fs::remove_file(temp_dir_path.join("dadeisan.mp3")).unwrap();
        tokio::time::sleep(DELAY).await;
        {
            let storage = storage.read();
            assert!(storage.get("dadeisan").is_none());
            assert_eq!(storage.len(), 1);
        }

        fs::rename(
            temp_dir_path.join("sainou.mp3"),
            temp_dir_path.join("sainou2.mp3"),
        )
        .unwrap();
        tokio::time::sleep(DELAY).await;
        {
            let storage = storage.read();
            assert!(storage.get("sainou").is_none());
            assert!(storage.get("sainou2").is_some());
            assert_eq!(storage.len(), 1);
        }
    }
}
