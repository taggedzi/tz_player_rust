//! Playback engine status values shared by backends and core.

/// Backend playback state machine status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
    Error,
}

impl BackendStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}
