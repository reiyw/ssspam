use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use once_cell::sync::Lazy;

pub static CONFIG_DIR: Lazy<PathBuf> =
    Lazy::new(|| match ProjectDirs::from("com", "ssspam", "ssspambot") {
        Some(proj_dirs) => {
            let path = proj_dirs.config_dir().to_owned();
            if !path.exists() {
                fs::create_dir_all(&path).expect("Cannot create config dir");
            }
            path
        }
        None => PathBuf::from("."),
    });
