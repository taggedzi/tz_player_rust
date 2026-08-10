//! Hardware-independent state and decode primitives for the Rodio backend.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use rodio::{Decoder, Source};

use crate::{BackendStatus, PlaybackError};

pub(crate) type FileDecoder = Decoder<BufReader<File>>;

/// A lazily decoded local source plus its original-timeline duration.
pub(crate) struct DecodedFile {
    pub(crate) decoder: FileDecoder,
    pub(crate) duration_ms: Option<u64>,
}

/// Open a local file for streaming decode without requiring an output device.
pub(crate) fn decode_file(path: &Path) -> Result<DecodedFile, PlaybackError> {
    let file = File::open(path).map_err(|error| {
        PlaybackError::message(format!("Rodio could not open {}: {error}", path.display()))
    })?;
    let decoder = Decoder::try_from(file).map_err(|error| {
        PlaybackError::message(format!(
            "Rodio could not decode {} (unsupported or corrupt media): {error}",
            path.display()
        ))
    })?;
    let duration_ms = decoder
        .total_duration()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64);
    Ok(DecodedFile {
        decoder,
        duration_ms,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RodioSnapshot {
    pub(crate) item_id: Option<i64>,
    pub(crate) status: BackendStatus,
    pub(crate) position_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) volume: u8,
    pub(crate) speed: f64,
    pub(crate) error: Option<String>,
}

/// Backend-neutral transport state driven by the Rodio worker.
///
/// The worker supplies positions in the original source timeline. Keeping this
/// state independent of CPAL lets CI prove transitions without an audio device.
#[derive(Debug)]
pub(crate) struct RodioTransport {
    snapshot: RodioSnapshot,
    natural_end_latched: bool,
}

impl Default for RodioTransport {
    fn default() -> Self {
        Self {
            snapshot: RodioSnapshot {
                item_id: None,
                status: BackendStatus::Idle,
                position_ms: 0,
                duration_ms: 0,
                volume: 100,
                speed: 1.0,
                error: None,
            },
            natural_end_latched: false,
        }
    }
}

impl RodioTransport {
    pub(crate) fn snapshot(&self) -> RodioSnapshot {
        self.snapshot.clone()
    }

    pub(crate) fn begin_load(
        &mut self,
        item_id: i64,
        start_ms: u64,
        fallback_duration_ms: Option<u64>,
    ) {
        self.snapshot.item_id = Some(item_id);
        self.snapshot.status = BackendStatus::Loading;
        self.snapshot.position_ms = start_ms;
        self.snapshot.duration_ms = fallback_duration_ms.unwrap_or(0);
        self.snapshot.error = None;
        self.natural_end_latched = false;
    }

    pub(crate) fn loaded(&mut self, decoded_duration_ms: Option<u64>) {
        if let Some(duration_ms) = decoded_duration_ms.filter(|duration| *duration > 0) {
            self.snapshot.duration_ms = duration_ms;
        }
        self.snapshot.status = BackendStatus::Playing;
    }

    pub(crate) fn toggle_pause(&mut self) -> Result<BackendStatus, PlaybackError> {
        self.snapshot.status = match self.snapshot.status {
            BackendStatus::Playing => BackendStatus::Paused,
            BackendStatus::Paused => BackendStatus::Playing,
            _ => {
                return Err(PlaybackError::message(
                    "Rodio cannot pause or resume without an active track",
                ))
            }
        };
        Ok(self.snapshot.status)
    }

    pub(crate) fn stop(&mut self) {
        self.snapshot.status = BackendStatus::Stopped;
        self.snapshot.position_ms = 0;
        self.natural_end_latched = true;
    }

    pub(crate) fn seek_accepted(&mut self, requested_ms: u64) -> u64 {
        let position_ms = if self.snapshot.duration_ms > 0 {
            requested_ms.min(self.snapshot.duration_ms)
        } else {
            requested_ms
        };
        self.snapshot.position_ms = position_ms;
        position_ms
    }

    pub(crate) fn set_volume(&mut self, volume: u8) {
        self.snapshot.volume = volume.min(100);
    }

    pub(crate) fn set_speed(&mut self, speed: f64) {
        self.snapshot.speed = speed;
    }

    pub(crate) fn observe_position(&mut self, source_position_ms: u64) {
        if matches!(
            self.snapshot.status,
            BackendStatus::Playing | BackendStatus::Paused
        ) {
            self.snapshot.position_ms = if self.snapshot.duration_ms > 0 {
                source_position_ms.min(self.snapshot.duration_ms)
            } else {
                source_position_ms
            };
        }
    }

    /// Latch one natural completion. Returns true exactly once per loaded item.
    pub(crate) fn observe_empty(&mut self) -> bool {
        if !self.natural_end_latched
            && self.snapshot.item_id.is_some()
            && matches!(self.snapshot.status, BackendStatus::Playing)
        {
            self.natural_end_latched = true;
            self.snapshot.status = BackendStatus::Stopped;
            if self.snapshot.duration_ms > 0 {
                self.snapshot.position_ms = self.snapshot.duration_ms;
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.snapshot.status = BackendStatus::Error;
        self.snapshot.error = Some(message);
        self.natural_end_latched = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tz_player_rodio_{name}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_silent_wav(path: &Path, duration_ms: u32) {
        const SAMPLE_RATE: u32 = 8_000;
        const CHANNELS: u16 = 1;
        const BITS_PER_SAMPLE: u16 = 16;
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
        let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.resize(44 + data_size as usize, 0);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn decodes_and_seeks_generated_wav_without_device() {
        let path = temp_path("decode.wav");
        write_silent_wav(&path, 100);

        let mut decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.duration_ms, Some(100));
        assert!(decoded.decoder.next().is_some());
        decoded.decoder.try_seek(Duration::from_millis(50)).unwrap();
        assert!(decoded.decoder.next().is_some());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_file_is_a_decode_error() {
        let path = temp_path("corrupt.wav");
        fs::write(&path, b"not a wave").unwrap();

        let error = match decode_file(&path) {
            Ok(_) => panic!("corrupt media unexpectedly decoded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unsupported or corrupt"));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn transport_models_pause_seek_stop_and_controls() {
        let mut transport = RodioTransport::default();
        transport.begin_load(7, 250, Some(2_000));
        transport.loaded(Some(1_500));
        transport.observe_position(400);
        assert_eq!(transport.snapshot().position_ms, 400);
        assert_eq!(transport.toggle_pause().unwrap(), BackendStatus::Paused);
        transport.observe_position(450);
        assert_eq!(transport.snapshot().position_ms, 450);
        assert_eq!(transport.seek_accepted(9_000), 1_500);
        transport.set_volume(250);
        transport.set_speed(2.0);

        let snapshot = transport.snapshot();
        assert_eq!(snapshot.volume, 100);
        assert_eq!(snapshot.speed, 2.0);
        assert_eq!(snapshot.duration_ms, 1_500);
        transport.stop();
        assert_eq!(transport.snapshot().status, BackendStatus::Stopped);
        assert_eq!(transport.snapshot().position_ms, 0);
        assert!(!transport.observe_empty());
    }

    #[test]
    fn natural_end_latches_once_at_duration() {
        let mut transport = RodioTransport::default();
        transport.begin_load(9, 0, Some(1_000));
        transport.loaded(None);
        transport.observe_position(990);

        assert!(transport.observe_empty());
        assert_eq!(transport.snapshot().position_ms, 1_000);
        assert_eq!(transport.snapshot().status, BackendStatus::Stopped);
        assert!(!transport.observe_empty());
    }

    #[test]
    fn new_load_clears_error_and_completion_latch() {
        let mut transport = RodioTransport::default();
        transport.fail("device lost");
        transport.begin_load(1, 0, Some(10));
        transport.loaded(None);

        assert_eq!(transport.snapshot().error, None);
        assert!(transport.observe_empty());
    }
}
