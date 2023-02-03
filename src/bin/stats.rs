use std::{fs::File, path::PathBuf, str::FromStr};

use anyhow::Context as _;
use clap::Parser;
use counter::Counter;
use glob::glob;
use itertools::Itertools;
use serde::Deserialize;
use ssspambot::SayCommands;

/// Prints sounds usage stats
#[derive(Parser, Debug)]
#[command(about)]
struct Args {
    /// Name of the person to greet
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

    let data: Data = serde_json::from_reader(File::open(args.input)?)?;
    for msg in data.messages {
        if let Ok(cmds) = SayCommands::from_str(&msg.content) {
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
struct Data {
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: String,
}
