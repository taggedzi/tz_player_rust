//! Rodio/Symphonia/CPAL playback backend.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::backend::{EventHandler, PlaybackBackend, PlaybackError, PlaybackLevelProvider};
use crate::events::{BackendEvent, MediaChanged, PositionUpdated, StateChanged};
use crate::rodio_engine::{RodioSnapshot, RodioTransport};
use crate::rodio_worker::{AudioOutputInfo, RodioCmd, RodioWorker, RodioWorkerEventKind};
use crate::{BackendStatus, LevelSample};

struct RodioInner {
    started: bool,
    snapshot: RodioSnapshot,
    worker: Option<RodioWorker>,
    output_info: Option<AudioOutputInfo>,
}

/// Local-file playback through Rodio, Symphonia, and the system output device.
pub struct AudioPlaybackBackend {
    inner: Mutex<RodioInner>,
    handler: Option<EventHandler>,
}

impl AudioPlaybackBackend {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RodioInner {
                started: false,
                snapshot: RodioTransport::default().snapshot(),
                worker: None,
                output_info: None,
            }),
            handler: None,
        }
    }

    pub async fn output_info(&self) -> Option<AudioOutputInfo> {
        self.inner.lock().await.output_info.clone()
    }

    async fn emit(&self, event: BackendEvent) {
        if let Some(handler) = &self.handler {
            handler(event).await;
        }
    }

    fn drain_events(inner: &mut RodioInner) -> Vec<BackendEvent> {
        let mut output = Vec::new();
        let Some(worker) = inner.worker.as_ref() else {
            return output;
        };
        while let Some(event) = worker.try_recv_event() {
            match event.kind {
                RodioWorkerEventKind::State(status) => {
                    inner.snapshot.status = status;
                    if status != BackendStatus::Playing {
                        inner.snapshot.level_sample = None;
                    }
                    output.push(BackendEvent::StateChanged(StateChanged { status }));
                }
                RodioWorkerEventKind::Media { duration_ms } => {
                    if duration_ms > 0 {
                        inner.snapshot.duration_ms = duration_ms;
                    }
                    output.push(BackendEvent::MediaChanged(MediaChanged { duration_ms }));
                }
                RodioWorkerEventKind::Position {
                    position_ms,
                    duration_ms,
                } => {
                    inner.snapshot.position_ms = position_ms;
                    if duration_ms > 0 {
                        inner.snapshot.duration_ms = duration_ms;
                    }
                    output.push(BackendEvent::PositionUpdated(PositionUpdated {
                        position_ms,
                        duration_ms: inner.snapshot.duration_ms,
                    }));
                }
                RodioWorkerEventKind::Error(message) => {
                    inner.snapshot.status = BackendStatus::Error;
                    inner.snapshot.error = Some(message.clone());
                    output.push(BackendEvent::Error(crate::events::BackendError { message }));
                }
            }
        }
        output
    }

    async fn submit_with_timeout<T, F>(
        &self,
        timeout: Duration,
        build: F,
    ) -> Result<T, PlaybackError>
    where
        T: Send + 'static,
        F: FnOnce(mpsc::Sender<Result<T, String>>) -> RodioCmd + Send + 'static,
    {
        let (reply, reply_rx) = mpsc::channel();
        {
            let inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            let worker = inner.worker.as_ref().ok_or_else(|| {
                PlaybackError::AudioUnavailable("Audio worker not running".into())
            })?;
            worker
                .cmd_tx()
                .send(build(reply))
                .map_err(|_| PlaybackError::AudioUnavailable("Audio worker disconnected".into()))?;
        }

        let result = tokio::task::spawn_blocking(move || {
            reply_rx
                .recv_timeout(timeout)
                .map_err(|_| "Rodio command timeout".to_string())
                .and_then(|result| result)
        })
        .await
        .map_err(|error| PlaybackError::message(error.to_string()))
        .and_then(|result| result.map_err(PlaybackError::message));

        let events = {
            let mut inner = self.inner.lock().await;
            Self::drain_events(&mut inner)
        };
        for event in events {
            self.emit(event).await;
        }
        result
    }

    async fn submit<T, F>(&self, build: F) -> Result<T, PlaybackError>
    where
        T: Send + 'static,
        F: FnOnce(mpsc::Sender<Result<T, String>>) -> RodioCmd + Send + 'static,
    {
        self.submit_with_timeout(Duration::from_secs(30), build)
            .await
    }

    async fn transport_snapshot(&self) -> Result<RodioSnapshot, PlaybackError> {
        let snapshot = self
            .submit(|reply| RodioCmd::GetTransport { reply })
            .await?;
        self.inner.lock().await.snapshot = snapshot.clone();
        Ok(snapshot)
    }
}

impl Default for AudioPlaybackBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlaybackLevelProvider for AudioPlaybackBackend {
    async fn get_level_sample(&self) -> Option<LevelSample> {
        let inner = self.inner.lock().await;
        (inner.snapshot.status == BackendStatus::Playing)
            .then_some(inner.snapshot.level_sample)
            .flatten()
    }
}

/// Silently open and close the default output device for doctor/smoke tools.
pub fn probe_audio_output() -> Result<AudioOutputInfo, String> {
    let worker = RodioWorker::spawn()?;
    let info = worker.output_info();
    worker.shutdown();
    Ok(info)
}

#[async_trait]
impl PlaybackBackend for AudioPlaybackBackend {
    fn set_event_handler(&mut self, handler: EventHandler) {
        self.handler = Some(handler);
    }

    async fn start(&mut self) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        if inner.started {
            return Ok(());
        }
        let worker = RodioWorker::spawn().map_err(|error| {
            PlaybackError::AudioUnavailable(format!(
                "Rodio could not initialize the default audio output.\n\
                 Likely cause: no enabled output device or the device is busy.\n\
                 Next step: check operating-system sound settings and re-run `tz-player doctor --backend audio`.\n\
                 Details: {error}"
            ))
        })?;
        inner.output_info = Some(worker.output_info());
        inner.worker = Some(worker);
        inner.started = true;
        inner.snapshot = RodioTransport::default().snapshot();
        tracing::info!("Rodio playback backend started");
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), PlaybackError> {
        let worker = {
            let mut inner = self.inner.lock().await;
            inner.started = false;
            inner.output_info = None;
            inner.snapshot = RodioTransport::default().snapshot();
            inner.worker.take()
        };
        if let Some(worker) = worker {
            tokio::task::spawn_blocking(move || worker.shutdown())
                .await
                .map_err(|error| PlaybackError::message(error.to_string()))?;
        }
        Ok(())
    }

    async fn play(
        &mut self,
        item_id: i64,
        track_path: &Path,
        start_ms: u64,
        duration_ms: Option<u64>,
    ) -> Result<(), PlaybackError> {
        let path = track_path.to_path_buf();
        self.submit(move |reply| RodioCmd::Play {
            item_id,
            path,
            start_ms,
            duration_ms,
            reply,
        })
        .await
    }

    async fn toggle_pause(&mut self) -> Result<(), PlaybackError> {
        self.submit(|reply| RodioCmd::TogglePause { reply }).await
    }

    async fn stop(&mut self) -> Result<(), PlaybackError> {
        self.submit(|reply| RodioCmd::Stop { reply }).await
    }

    async fn seek_ms(&mut self, position_ms: u64) -> Result<(), PlaybackError> {
        self.submit_with_timeout(Duration::from_secs(5), move |reply| RodioCmd::SeekMs {
            position_ms,
            reply,
        })
        .await?;
        self.inner.lock().await.snapshot.level_sample = None;
        Ok(())
    }

    async fn set_volume(&mut self, volume: u8) -> Result<(), PlaybackError> {
        let volume = volume.min(100);
        self.submit(move |reply| RodioCmd::SetVolume { volume, reply })
            .await
    }

    async fn set_speed(&mut self, speed: f64) -> Result<(), PlaybackError> {
        let speed = speed.clamp(0.5, 4.0);
        self.submit(move |reply| RodioCmd::SetSpeed { speed, reply })
            .await
    }

    async fn get_position_ms(&self) -> Result<u64, PlaybackError> {
        Ok(self.transport_snapshot().await?.position_ms)
    }

    async fn get_duration_ms(&self) -> Result<u64, PlaybackError> {
        Ok(self.transport_snapshot().await?.duration_ms)
    }

    async fn get_state(&self) -> Result<BackendStatus, PlaybackError> {
        Ok(self.transport_snapshot().await?.status)
    }

    async fn get_transport_snapshot(&self) -> Result<(u64, u64, BackendStatus), PlaybackError> {
        let snapshot = self.transport_snapshot().await?;
        Ok((snapshot.position_ms, snapshot.duration_ms, snapshot.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn output_device_tests_enabled() -> bool {
        std::env::var("TZ_PLAYER_RODIO_OUTPUT_TESTS").as_deref() == Ok("1")
    }

    fn silent_wav(duration_ms: u32) -> std::path::PathBuf {
        const SAMPLE_RATE: u32 = 8_000;
        const CHANNELS: u16 = 1;
        const BITS_PER_SAMPLE: u16 = 16;
        let path = std::env::temp_dir().join(format!(
            "tz_player_rodio_live_{}_{}.wav",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sample_count = SAMPLE_RATE * duration_ms / 1_000;
        let data_size = sample_count * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE / 8);
        let mut bytes = Vec::with_capacity(44 + data_size as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&CHANNELS.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE / 8);
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&(CHANNELS * (BITS_PER_SAMPLE / 8)).to_le_bytes());
        bytes.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.resize(44 + data_size as usize, 0);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[tokio::test]
    async fn live_level_provider_only_reports_playing_samples() {
        let backend = AudioPlaybackBackend::new();
        {
            let mut inner = backend.inner.lock().await;
            inner.snapshot.status = BackendStatus::Playing;
            inner.snapshot.level_sample = Some(LevelSample {
                left: 0.2,
                right: 0.6,
            });
        }
        assert_eq!(
            backend.get_level_sample().await,
            Some(LevelSample {
                left: 0.2,
                right: 0.6
            })
        );

        backend.inner.lock().await.snapshot.status = BackendStatus::Paused;
        assert_eq!(backend.get_level_sample().await, None);
    }

    #[tokio::test]
    async fn startup_is_clean_or_reports_device_unavailable() {
        if !output_device_tests_enabled() {
            return;
        }
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {
                assert!(backend.output_info().await.is_some());
                backend.shutdown().await.unwrap();
            }
            Err(PlaybackError::AudioUnavailable(message)) => {
                assert!(message.contains("default audio output"));
            }
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
    }

    #[tokio::test]
    async fn silent_wav_reaches_one_natural_end_when_device_exists() {
        if !output_device_tests_enabled() {
            return;
        }
        let path = silent_wav(200);
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => {
                fs::remove_file(path).unwrap();
                return;
            }
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
        backend
            .play(42, &path, 0, Some(200))
            .await
            .expect("generated WAV should play");

        let deadline = Instant::now() + Duration::from_secs(3);
        let snapshot = loop {
            let snapshot = backend.get_transport_snapshot().await.unwrap();
            if snapshot.2 == BackendStatus::Stopped || Instant::now() >= deadline {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(snapshot, (200, 200, BackendStatus::Stopped));

        backend.shutdown().await.unwrap();
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn tone_reaches_live_level_provider_through_output_when_device_exists() {
        if !output_device_tests_enabled() {
            return;
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("tone.wav");
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => return,
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
        // Metering is intentionally pre-volume, so verification stays silent.
        backend.set_volume(0).await.unwrap();
        backend.play(43, &path, 0, None).await.unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let sample = loop {
            let _ = backend.get_transport_snapshot().await.unwrap();
            if let Some(sample) = backend
                .get_level_sample()
                .await
                .filter(|sample| sample.left > 0.05 || sample.right > 0.05)
            {
                break sample;
            }
            assert!(
                Instant::now() < deadline,
                "Rodio output produced no live visualization level"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(sample.left > 0.05);
        assert!(sample.right > 0.05);

        backend.stop().await.unwrap();
        let _ = backend.get_transport_snapshot().await.unwrap();
        assert_eq!(backend.get_level_sample().await, None);
        backend.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn decode_failure_does_not_poison_the_next_track_when_device_exists() {
        if !output_device_tests_enabled() {
            return;
        }
        let valid_path = silent_wav(200);
        let unsupported_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("tone-opus.ogg");
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => {
                fs::remove_file(valid_path).unwrap();
                return;
            }
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }

        let error = backend
            .play(1, &unsupported_path, 0, Some(1_000))
            .await
            .expect_err("Ogg Opus should be rejected by the selected decoder set");
        assert!(error.to_string().contains("unsupported or corrupt"));
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Error);

        backend
            .play(2, &valid_path, 0, Some(200))
            .await
            .expect("a supported track must remain playable after a decode error");
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Playing);

        backend.stop().await.unwrap();
        backend.shutdown().await.unwrap();
        fs::remove_file(valid_path).unwrap();
    }

    #[tokio::test]
    async fn muted_format_matrix_reaches_natural_end_when_device_exists() {
        if !output_device_tests_enabled() {
            return;
        }
        const FIXTURES: &[&str] = &[
            "tone.wav",
            "tone.mp3",
            "tone.flac",
            "tone.ogg",
            "tone-aac.m4a",
            "tone-alac.m4a",
            "tone.aiff",
            "tone.caf",
            "tone.mka",
        ];

        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => return,
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
        backend.set_volume(0).await.unwrap();
        backend.set_speed(4.0).await.unwrap();

        for (index, name) in FIXTURES.iter().enumerate() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join(name);
            backend
                .play(index as i64 + 1, &path, 0, None)
                .await
                .unwrap_or_else(|error| panic!("{name} did not start: {error}"));

            let deadline = Instant::now() + Duration::from_secs(3);
            let snapshot = loop {
                let snapshot = backend.get_transport_snapshot().await.unwrap();
                if snapshot.2 == BackendStatus::Stopped || Instant::now() >= deadline {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            assert_eq!(
                snapshot.2,
                BackendStatus::Stopped,
                "{name} did not reach natural end: {snapshot:?}"
            );
            assert!(snapshot.1 > 0, "{name} did not report a duration");
            assert_eq!(snapshot.0, snapshot.1, "{name} did not latch final time");
        }

        backend.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn muted_helper_fixture_reaches_natural_end_when_configured() {
        if !output_device_tests_enabled() || std::env::var_os("TZ_PLAYER_AUDIO_HELPER").is_none() {
            return;
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("tone-opus.ogg");
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => return,
            Err(error) => panic!("unexpected Audio startup result: {error}"),
        }
        backend.set_volume(0).await.unwrap();
        backend.set_speed(4.0).await.unwrap();
        backend
            .play(81, &path, 0, None)
            .await
            .expect("configured helper-only fixture should start");

        let deadline = Instant::now() + Duration::from_secs(3);
        let snapshot = loop {
            let snapshot = backend.get_transport_snapshot().await.unwrap();
            if snapshot.2 == BackendStatus::Stopped || Instant::now() >= deadline {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(snapshot.2, BackendStatus::Stopped, "{snapshot:?}");
        assert!(snapshot.1 > 0);
        assert_eq!(snapshot.0, snapshot.1);
        backend.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn silent_transport_controls_work_through_output_when_device_exists() {
        if !output_device_tests_enabled() {
            return;
        }
        let path = silent_wav(3_000);
        let mut backend = AudioPlaybackBackend::new();
        match backend.start().await {
            Ok(()) => {}
            Err(PlaybackError::AudioUnavailable(_)) => {
                fs::remove_file(path).unwrap();
                return;
            }
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
        backend.set_volume(0).await.unwrap();
        backend.play(7, &path, 0, Some(3_000)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        backend.toggle_pause().await.unwrap();
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Paused);
        backend.seek_ms(1_500).await.unwrap();
        assert_eq!(backend.get_position_ms().await.unwrap(), 1_500);
        backend.seek_ms(300).await.unwrap();
        assert_eq!(backend.get_position_ms().await.unwrap(), 300);

        for speed in [0.5, 1.0, 2.0, 4.0] {
            backend.set_speed(speed).await.unwrap();
        }
        backend.toggle_pause().await.unwrap();
        assert_eq!(backend.get_state().await.unwrap(), BackendStatus::Playing);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(backend.get_position_ms().await.unwrap() > 300);

        backend.stop().await.unwrap();
        assert_eq!(
            backend.get_transport_snapshot().await.unwrap(),
            (0, 3_000, BackendStatus::Stopped)
        );
        backend.shutdown().await.unwrap();
        backend.shutdown().await.unwrap();
        fs::remove_file(path).unwrap();
    }
}
