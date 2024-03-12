use std::{fs::File, path::PathBuf, str::FromStr};

use anyhow::Context as _;
use clap::Parser;
use counter::Counter;
use glob::glob;
use itertools::Itertools;
use serde::Deserialize;
use ssspam_bot::SayCommands;

/// Prints sounds usage stats
#[derive(Parser, Debug)]
#[command(about)]
struct Args {
    #[arg(long)]
    input: PathBuf,

    #[clap(long)]
    sound_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut sounds: Vec<String> = Vec::new();
    for path in (glob(&format!("{}/**/*.mp3", args.sound_dir.to_string_lossy())).unwrap()).flatten()
    {
        let name = path.file_stem().context("No file name")?.to_string_lossy();
        sounds.push(name.into());
    }

    let mut counts: Counter<String> = Counter::new();

    let mut reader = csv::Reader::from_reader(File::open(args.input)?);
    for result in reader.deserialize() {
        let record: Record = result?;
        if let Ok(cmds) = SayCommands::from_str(&record.content) {
            for cmd in cmds.iter().unique() {
                counts[&cmd.name] += 1;
            }
        }
    }

    for sound in sounds {
        println!("{}\t{}", sound, counts[&sound]);
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename(deserialize = "Content"))]
    content: String,
}
