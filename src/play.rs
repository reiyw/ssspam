use std::cmp;
use std::sync::Arc;
use std::time::Duration;

use songbird::{create_player, input::Input, tracks::TrackHandle, Call};
use tokio::sync::Mutex;

use super::SayCommand;

const VOLUME: f64 = 0.05;

pub async fn play_source(
    source: Input,
    handler_lock: Arc<Mutex<Call>>,
    volume_multiplier: f64,
) -> TrackHandle {
    let (mut audio, audio_handle) = create_player(source);
    audio.set_volume((VOLUME * volume_multiplier) as f32);
    let mut handler = handler_lock.lock().await;
    handler.play(audio);
    audio_handle
}

/// Calculates the duration of the sound if the say command was played.
pub fn calc_sound_duration(cmd: &SayCommand, original_duration: &Duration) -> Duration {
    let mut dur = cmp::max((original_duration.as_millis() as i64) - cmd.start as i64, 0);
    if let Some(n) = cmd.duration {
        dur = cmp::min(dur, n as i64)
    }
    dur = ((dur as f64) * (100.0 / cmd.speed as f64)) as i64;
    if cmd.stop {
        dur = cmp::min(dur, cmd.wait as i64);
    }
    Duration::from_millis(dur as u64)
}
