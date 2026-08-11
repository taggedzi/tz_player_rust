//! Playback backends for tz-player.
//!
//! **Listen path only.** Real audio output goes through the bundled Audio engine.

mod backend;
mod events;
mod fake;
mod rodio;
mod rodio_engine;
mod rodio_worker;
mod status;

pub use backend::{EventHandler, PlaybackBackend, PlaybackError, PlaybackLevelProvider};
pub use events::{
    BackendError, BackendEvent, LevelSample, MediaChanged, PositionUpdated, StateChanged,
};
pub use fake::FakePlaybackBackend;
pub use rodio::{probe_audio_output, AudioPlaybackBackend};
pub use rodio_engine::{package_playback_smoke, PackagePlaybackSmokeReport};
pub use rodio_worker::AudioOutputInfo;
pub use status::BackendStatus;

/// Supported playback backend identifiers (CLI / state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Audio,
    Fake,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Fake => "fake",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "audio" | "rodio" => Some(Self::Audio),
            "fake" => Some(Self::Fake),
            _ => None,
        }
    }
}

#[cfg(test)]
mod backend_kind_tests {
    use super::BackendKind;

    #[test]
    fn backend_identifiers_round_trip_and_audio_is_default() {
        assert_eq!(BackendKind::default(), BackendKind::Audio);
        for kind in [BackendKind::Audio, BackendKind::Fake] {
            assert_eq!(BackendKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                BackendKind::parse(&kind.as_str().to_ascii_uppercase()),
                Some(kind)
            );
        }
        assert_eq!(BackendKind::parse("unknown"), None);
    }
}
