//! Deterministic fake playback backend for tests and VLC-unavailable fallback.

use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::backend::{EventHandler, PlaybackBackend, PlaybackError};
use crate::events::{BackendEvent, MediaChanged, PositionUpdated, StateChanged};
use crate::status::BackendStatus;

#[derive(Debug)]
struct FakeInner {
    status: BackendStatus,
    position_ms: u64,
    duration_ms: u64,
    volume: u8,
    speed: f64,
    item_id: Option<i64>,
    track_path: Option<String>,
    started: bool,
    play_started_at: Option<Instant>,
    paused_at_ms: u64,
}

impl Default for FakeInner {
    fn default() -> Self {
        Self {
            status: BackendStatus::Idle,
            position_ms: 0,
            duration_ms: 0,
            volume: 100,
            speed: 1.0,
            item_id: None,
            track_path: None,
            started: false,
            play_started_at: None,
            paused_at_ms: 0,
        }
    }
}

/// Fake engine that advances position wall-clock style when playing.
pub struct FakePlaybackBackend {
    inner: Mutex<FakeInner>,
    handler: Option<EventHandler>,
}

impl Default for FakePlaybackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePlaybackBackend {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FakeInner::default()),
            handler: None,
        }
    }

    async fn emit(&self, event: BackendEvent) {
        if let Some(handler) = &self.handler {
            handler(event).await;
        }
    }

    fn recompute_position(inner: &mut FakeInner) {
        if inner.status != BackendStatus::Playing {
            return;
        }
        if let Some(started) = inner.play_started_at {
            let elapsed = started.elapsed().as_millis() as f64 * inner.speed;
            let pos = inner.paused_at_ms.saturating_add(elapsed as u64);
            inner.position_ms = if inner.duration_ms > 0 {
                pos.min(inner.duration_ms)
            } else {
                pos
            };
            if inner.duration_ms > 0 && inner.position_ms >= inner.duration_ms {
                inner.status = BackendStatus::Stopped;
                inner.play_started_at = None;
                inner.position_ms = inner.duration_ms;
            }
        }
    }
}

#[async_trait]
impl PlaybackBackend for FakePlaybackBackend {
    fn set_event_handler(&mut self, handler: EventHandler) {
        self.handler = Some(handler);
    }

    async fn start(&mut self) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        inner.started = true;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        inner.started = false;
        inner.status = BackendStatus::Idle;
        inner.play_started_at = None;
        Ok(())
    }

    async fn play(
        &mut self,
        item_id: i64,
        track_path: &str,
        start_ms: u64,
        duration_ms: Option<u64>,
    ) -> Result<(), PlaybackError> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            inner.item_id = Some(item_id);
            inner.track_path = Some(track_path.to_string());
            inner.duration_ms = duration_ms.unwrap_or(180_000);
            inner.position_ms = start_ms.min(inner.duration_ms);
            inner.paused_at_ms = inner.position_ms;
            inner.play_started_at = Some(Instant::now());
            inner.status = BackendStatus::Playing;
        }

        self.emit(BackendEvent::MediaChanged(MediaChanged {
            duration_ms: self.inner.lock().await.duration_ms,
        }))
        .await;
        self.emit(BackendEvent::StateChanged(StateChanged {
            status: BackendStatus::Playing,
        }))
        .await;
        Ok(())
    }

    async fn toggle_pause(&mut self) -> Result<(), PlaybackError> {
        let status = {
            let mut inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            Self::recompute_position(&mut inner);
            match inner.status {
                BackendStatus::Playing => {
                    inner.paused_at_ms = inner.position_ms;
                    inner.play_started_at = None;
                    inner.status = BackendStatus::Paused;
                    BackendStatus::Paused
                }
                BackendStatus::Paused => {
                    inner.play_started_at = Some(Instant::now());
                    inner.status = BackendStatus::Playing;
                    BackendStatus::Playing
                }
                other => other,
            }
        };
        self.emit(BackendEvent::StateChanged(StateChanged { status }))
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PlaybackError> {
        {
            let mut inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            inner.status = BackendStatus::Stopped;
            inner.play_started_at = None;
            inner.position_ms = 0;
            inner.paused_at_ms = 0;
        }
        self.emit(BackendEvent::StateChanged(StateChanged {
            status: BackendStatus::Stopped,
        }))
        .await;
        Ok(())
    }

    async fn seek_ms(&mut self, position_ms: u64) -> Result<(), PlaybackError> {
        let (pos, dur) = {
            let mut inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            Self::recompute_position(&mut inner);
            let clamped = if inner.duration_ms > 0 {
                position_ms.min(inner.duration_ms)
            } else {
                position_ms
            };
            inner.position_ms = clamped;
            inner.paused_at_ms = clamped;
            if inner.status == BackendStatus::Playing {
                inner.play_started_at = Some(Instant::now());
            }
            (inner.position_ms, inner.duration_ms)
        };
        self.emit(BackendEvent::PositionUpdated(PositionUpdated {
            position_ms: pos,
            duration_ms: dur,
        }))
        .await;
        Ok(())
    }

    async fn set_volume(&mut self, volume: u8) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        inner.volume = volume.min(100);
        Ok(())
    }

    async fn set_speed(&mut self, speed: f64) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        Self::recompute_position(&mut inner);
        inner.paused_at_ms = inner.position_ms;
        if inner.status == BackendStatus::Playing {
            inner.play_started_at = Some(Instant::now());
        }
        inner.speed = speed.clamp(0.5, 4.0);
        Ok(())
    }

    async fn get_position_ms(&self) -> Result<u64, PlaybackError> {
        let mut inner = self.inner.lock().await;
        Self::recompute_position(&mut inner);
        Ok(inner.position_ms)
    }

    async fn get_duration_ms(&self) -> Result<u64, PlaybackError> {
        Ok(self.inner.lock().await.duration_ms)
    }

    async fn get_state(&self) -> Result<BackendStatus, PlaybackError> {
        let mut inner = self.inner.lock().await;
        Self::recompute_position(&mut inner);
        Ok(inner.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    #[tokio::test]
    async fn fake_play_pause_stop() {
        let mut backend = FakePlaybackBackend::new();
        backend.start().await.unwrap();
        backend
            .play(1, "/tmp/track.flac", 0, Some(60_000))
            .await
            .unwrap();
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Playing);

        backend.toggle_pause().await.unwrap();
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Paused);

        backend.stop().await.unwrap();
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Stopped);
        assert_eq!(backend.get_position_ms().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn fake_emits_events() {
        let events: Arc<TokioMutex<Vec<String>>> = Arc::new(TokioMutex::new(Vec::new()));
        let events_c = events.clone();
        let mut backend = FakePlaybackBackend::new();
        backend.set_event_handler(Arc::new(move |ev| {
            let events_c = events_c.clone();
            Box::pin(async move {
                let label = match ev {
                    BackendEvent::StateChanged(s) => format!("state:{}", s.status.as_str()),
                    BackendEvent::MediaChanged(_) => "media".into(),
                    BackendEvent::PositionUpdated(_) => "pos".into(),
                    BackendEvent::Error(_) => "err".into(),
                };
                events_c.lock().await.push(label);
            })
        }));
        backend.start().await.unwrap();
        backend.play(1, "a.mp3", 0, Some(10_000)).await.unwrap();
        let got = events.lock().await.clone();
        assert!(got.iter().any(|e| e == "media"));
        assert!(got.iter().any(|e| e.starts_with("state:")));
    }
}
