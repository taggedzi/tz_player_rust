//! Events emitted by playback backends toward `PlayerService` / UI.

use crate::status::BackendStatus;

/// Marker for backend-originated events.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    PositionUpdated(PositionUpdated),
    StateChanged(StateChanged),
    MediaChanged(MediaChanged),
    Error(BackendError),
}

/// Periodic transport position update in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionUpdated {
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Backend playback state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateChanged {
    pub status: BackendStatus,
}

/// Loaded media metadata update (currently duration only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaChanged {
    pub duration_ms: u64,
}

/// Backend-reported non-recoverable runtime error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub message: String,
}

/// Stereo audio-level sample normalized to `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelSample {
    pub left: f32,
    pub right: f32,
}
