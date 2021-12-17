// use serde::{Deserialize, Serialize};
use songbird::input::cached::{Compressed, Memory};

// #[derive(Serialize, Deserialize, Debug)]
pub enum CachedSound {
    Compressed(Compressed),
    Uncompressed(Memory),
}
