//! Playback backends for tz-player.
//!
//! **Listen path only.** Real audio output goes through VLC/libVLC.
//! Offline analysis/decode for visualizers lives in `tz-analysis` (FFmpeg), not here.

mod backend;
mod events;
mod fake;
mod status;
mod vlc;
mod vlc_engine;
mod vlc_ffi;

pub use backend::{EventHandler, PlaybackBackend, PlaybackError, PlaybackLevelProvider};
pub use events::{
    BackendError, BackendEvent, LevelSample, MediaChanged, PositionUpdated, StateChanged,
};
pub use fake::FakePlaybackBackend;
pub use status::BackendStatus;
pub use vlc::{discover_vlc, VlcDiscovery, VlcPlaybackBackend};

/// Supported playback backend identifiers (CLI / state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Vlc,
    Fake,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vlc => "vlc",
            Self::Fake => "fake",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vlc" => Some(Self::Vlc),
            "fake" => Some(Self::Fake),
            _ => None,
        }
    }
}
