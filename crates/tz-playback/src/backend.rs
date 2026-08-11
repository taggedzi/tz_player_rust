//! Playback engine trait consumed by core services.

use async_trait::async_trait;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use crate::events::{BackendEvent, LevelSample};
use crate::status::BackendStatus;

/// Async event handler registered by the player service.
pub type EventHandler =
    Arc<dyn Fn(BackendEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Optional capability for live audio level sampling from the playback engine.
#[async_trait]
pub trait PlaybackLevelProvider: Send + Sync {
    async fn get_level_sample(&self) -> Option<LevelSample>;
}

/// Playback engine protocol (Audio or Fake).
///
/// Implementations must not perform analysis decode; that is `tz-analysis`.
#[async_trait]
pub trait PlaybackBackend: Send + Sync {
    fn set_event_handler(&mut self, handler: EventHandler);

    async fn start(&mut self) -> Result<(), PlaybackError>;
    async fn shutdown(&mut self) -> Result<(), PlaybackError>;

    async fn play(
        &mut self,
        item_id: i64,
        track_path: &Path,
        start_ms: u64,
        duration_ms: Option<u64>,
    ) -> Result<(), PlaybackError>;

    async fn toggle_pause(&mut self) -> Result<(), PlaybackError>;
    async fn stop(&mut self) -> Result<(), PlaybackError>;
    async fn seek_ms(&mut self, position_ms: u64) -> Result<(), PlaybackError>;
    async fn set_volume(&mut self, volume: u8) -> Result<(), PlaybackError>;
    async fn set_speed(&mut self, speed: f64) -> Result<(), PlaybackError>;

    async fn get_position_ms(&self) -> Result<u64, PlaybackError>;
    async fn get_duration_ms(&self) -> Result<u64, PlaybackError>;
    async fn get_state(&self) -> Result<BackendStatus, PlaybackError>;

    async fn get_transport_snapshot(&self) -> Result<(u64, u64, BackendStatus), PlaybackError> {
        let position = self.get_position_ms().await?;
        let duration = self.get_duration_ms().await?;
        let status = self.get_state().await?;
        Ok((position, duration, status))
    }
}

/// Errors from the listen-path playback engine.
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("{0}")]
    Message(String),

    #[error("backend not started")]
    NotStarted,

    #[error("Audio output unavailable: {0}")]
    AudioUnavailable(String),
}

impl PlaybackError {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
