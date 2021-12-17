use std::{collections::HashMap, env};

use bincode::serialize;
use dotenv::dotenv;
use glob::glob;
use songbird::{
    driver::Bitrate,
    input::{
        self,
        cached::{Compressed, Memory},
    },
};

use surfpvparena::CachedSound;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv()?;

    let mut audio_map = HashMap::new();

    for entry in glob(&format!("{}/*.mp3", env::var("SOUND_DIR")?))? {
        if let Ok(path) = entry {
            println!("Processing {:?}", path);
            let song_src = Compressed::new(
                input::ffmpeg(path.clone()).await?,
                Bitrate::BitsPerSecond(128_000),
            )
            .expect("These parameters are well-defined.");
            let _ = song_src.raw.spawn_loader();
            audio_map.insert(
                path.file_stem().unwrap().to_str().unwrap().to_string(),
                CachedSound::Compressed(song_src),
            );
            if audio_map.len() == 100 {
                break;
            }
        }
    }

    // let encoded = serialize(&audio_map)?;

    Ok(())
}
