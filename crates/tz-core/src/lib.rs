//! Core domain for tz-player: state, paths, player orchestration, runtime.
//!
//! The TUI and future headless clients depend on this crate, not the reverse.

mod levels;
mod metadata;
mod paths;
mod player;
mod runtime;
mod state;

pub use levels::{AnalysisSample, LevelSample, LevelService, LevelSource};
pub use metadata::{read_track_meta, refresh_playlist_metadata};
pub use paths::{app_paths, app_paths_or_cwd, AppPaths};
pub use player::{PlayerError, PlayerService, PlayerState, RepeatMode};
pub use runtime::{open_runtime, AppRuntime, RuntimeError};
pub use state::{load_state, load_state_with_notice, save_state, AppState};

/// Speed limits (ADR-0003 / Python parity).
pub const SPEED_MIN: f64 = 0.5;
pub const SPEED_MAX: f64 = 4.0;
pub const SPEED_STEP: f64 = 0.25;

/// Clamp playback speed to the supported range.
pub fn clamp_speed(speed: f64) -> f64 {
    if !speed.is_finite() {
        return 1.0;
    }
    speed.clamp(SPEED_MIN, SPEED_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_clamp() {
        assert_eq!(clamp_speed(0.1), SPEED_MIN);
        assert_eq!(clamp_speed(10.0), SPEED_MAX);
        assert_eq!(clamp_speed(1.25), 1.25);
        assert_eq!(clamp_speed(f64::NAN), 1.0);
    }
}
