//! Playback backends for tz-player.
//!
//! **Listen path only.** Real audio output goes through VLC/libVLC or Rodio.
//! Offline analysis/decode for visualizers lives in `tz-analysis` (FFmpeg), not here.

mod backend;
mod events;
mod fake;
mod rodio;
mod rodio_engine;
mod rodio_worker;
mod status;
mod vlc;
mod vlc_engine;
mod vlc_ffi;

pub use backend::{EventHandler, PlaybackBackend, PlaybackError, PlaybackLevelProvider};
pub use events::{
    BackendError, BackendEvent, LevelSample, MediaChanged, PositionUpdated, StateChanged,
};
pub use fake::FakePlaybackBackend;
pub use rodio::{probe_rodio_output, RodioPlaybackBackend};
pub use rodio_worker::RodioOutputInfo;
pub use status::BackendStatus;
pub use vlc::{configure_vlc_environment, discover_vlc, VlcDiscovery, VlcPlaybackBackend};

/// Supported playback backend identifiers (CLI / state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Vlc,
    Rodio,
    Fake,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vlc => "vlc",
            Self::Rodio => "rodio",
            Self::Fake => "fake",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vlc" => Some(Self::Vlc),
            "rodio" => Some(Self::Rodio),
            "fake" => Some(Self::Fake),
            _ => None,
        }
    }
}

#[cfg(test)]
mod backend_kind_tests {
    use super::BackendKind;

    #[test]
    fn backend_identifiers_round_trip_and_vlc_remains_default() {
        assert_eq!(BackendKind::default(), BackendKind::Vlc);
        for kind in [BackendKind::Vlc, BackendKind::Rodio, BackendKind::Fake] {
            assert_eq!(BackendKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                BackendKind::parse(&kind.as_str().to_ascii_uppercase()),
                Some(kind)
            );
        }
        assert_eq!(BackendKind::parse("unknown"), None);
    }
}
