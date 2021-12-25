use std::{fs, path::PathBuf};

use dotenv::dotenv;
use structopt::StructOpt;

use ssspambot::load_sounds;

#[derive(Debug, StructOpt)]
#[structopt(name = "preload")]
struct Opt {
    #[structopt(long, parse(from_os_str), env)]
    sound_dir: PathBuf,
}

fn main() {
    dotenv().ok();
    let opt = Opt::from_args();

    let sounds = load_sounds(&opt.sound_dir);
    let encoded: Vec<u8> = bincode::serialize(&sounds).unwrap();
    fs::write(opt.sound_dir.join("sounds.bin"), encoded).unwrap();
}
