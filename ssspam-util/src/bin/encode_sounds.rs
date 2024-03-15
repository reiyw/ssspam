use std::{fs, path::PathBuf};

use clap::Parser;
use prost::Message;
use ssspam_bot::sound::{SoundStorage, ToSoundsProto};

#[derive(Parser)]
struct Args {
    #[arg(long, env)]
    sound_dir: PathBuf,

    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let storage = SoundStorage::load(args.sound_dir);
    let mut buf = Vec::new();
    storage.files().cloned().to_sounds().encode(&mut buf)?;
    fs::write(args.output, buf)?;
    Ok(())
}
