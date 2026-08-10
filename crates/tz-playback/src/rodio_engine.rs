//! Hardware-independent state and decode primitives for the Rodio backend.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{Decoder, Source};

use crate::{BackendStatus, LevelSample, PlaybackError};

pub(crate) type FileDecoder = Decoder<BufReader<File>>;

/// A lazily decoded local source plus its original-timeline duration.
pub(crate) struct DecodedFile {
    pub(crate) decoder: FileDecoder,
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineHandle {
    sample_position: Arc<AtomicU64>,
    level_peaks: Arc<AtomicU64>,
    channels: u16,
    sample_rate: u32,
}

impl TimelineHandle {
    fn new<S: Source>(source: &S, start: Duration) -> Self {
        let channels = source.channels().get();
        let sample_rate = source.sample_rate().get();
        let sample_position = duration_to_samples(start, channels, sample_rate);
        Self {
            sample_position: Arc::new(AtomicU64::new(sample_position)),
            level_peaks: Arc::new(AtomicU64::new(0)),
            channels,
            sample_rate,
        }
    }

    pub(crate) fn position_ms(&self) -> u64 {
        let frames = self.sample_position.load(Ordering::Relaxed) / u64::from(self.channels);
        frames.saturating_mul(1_000) / u64::from(self.sample_rate)
    }

    fn store(&self, samples: u64) {
        self.sample_position.store(samples, Ordering::Relaxed);
    }

    pub(crate) fn level_sample(&self) -> LevelSample {
        let packed = self.level_peaks.load(Ordering::Relaxed);
        LevelSample {
            left: f32::from_bits(packed as u32),
            right: f32::from_bits((packed >> 32) as u32),
        }
    }

    fn store_level_sample(&self, left: f32, right: f32) {
        let packed = u64::from(left.to_bits()) | (u64::from(right.to_bits()) << 32);
        self.level_peaks.store(packed, Ordering::Relaxed);
    }
}

/// Track original-source samples before Rodio applies its variable-rate filter.
///
/// Rodio's public player position is expressed in its speed-adjusted playback
/// clock. Counting decoder samples instead keeps tz-player's public position in
/// the original media timeline, including after multiple rate changes.
pub(crate) struct TimelineSource<S> {
    inner: S,
    handle: TimelineHandle,
    sample_position: u64,
    samples_since_publish: u16,
    peak_left: f32,
    peak_right: f32,
    level_frames: u32,
    level_window_frames: u32,
}

impl<S: Source> TimelineSource<S> {
    pub(crate) fn new(inner: S, start: Duration) -> (Self, TimelineHandle) {
        let handle = TimelineHandle::new(&inner, start);
        let sample_position = handle.sample_position.load(Ordering::Relaxed);
        (
            Self {
                inner,
                handle: handle.clone(),
                sample_position,
                samples_since_publish: 0,
                peak_left: 0.0,
                peak_right: 0.0,
                level_frames: 0,
                // A 50 ms peak window follows the output callback closely
                // enough for the 10-60 FPS TUI without synchronizing on every
                // decoded sample.
                level_window_frames: (handle.sample_rate / 20).max(1),
            },
            handle,
        )
    }

    fn publish(&mut self) {
        self.handle.store(self.sample_position);
        self.samples_since_publish = 0;
    }

    fn observe_level(&mut self, sample: f32) {
        let channel = (self.sample_position % u64::from(self.handle.channels)) as u16;
        let magnitude = if sample.is_finite() {
            sample.abs().min(1.0)
        } else {
            0.0
        };
        if self.handle.channels == 1 {
            self.peak_left = self.peak_left.max(magnitude);
            self.peak_right = self.peak_right.max(magnitude);
        } else if channel == 0 {
            self.peak_left = self.peak_left.max(magnitude);
        } else if channel == 1 {
            self.peak_right = self.peak_right.max(magnitude);
        }

        if channel + 1 == self.handle.channels {
            self.level_frames = self.level_frames.saturating_add(1);
            if self.level_frames >= self.level_window_frames {
                self.publish_levels();
            }
        }
    }

    fn publish_levels(&mut self) {
        self.handle
            .store_level_sample(self.peak_left, self.peak_right);
        self.peak_left = 0.0;
        self.peak_right = 0.0;
        self.level_frames = 0;
    }
}

impl<S: Source> Iterator for TimelineSource<S> {
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next();
        if let Some(value) = sample {
            self.observe_level(value);
            self.sample_position = self.sample_position.saturating_add(1);
            self.samples_since_publish = self.samples_since_publish.saturating_add(1);
            if self.samples_since_publish >= 256 {
                self.publish();
            }
        } else {
            self.publish();
            self.publish_levels();
        }
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for TimelineSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(position)?;
        self.sample_position =
            duration_to_samples(position, self.handle.channels, self.handle.sample_rate);
        self.publish();
        self.peak_left = 0.0;
        self.peak_right = 0.0;
        self.level_frames = 0;
        self.handle.store_level_sample(0.0, 0.0);
        Ok(())
    }
}

impl<S> Drop for TimelineSource<S> {
    fn drop(&mut self) {
        self.handle.store(self.sample_position);
        self.handle.store_level_sample(0.0, 0.0);
    }
}

fn duration_to_samples(duration: Duration, channels: u16, sample_rate: u32) -> u64 {
    let frames = duration.as_nanos().saturating_mul(u128::from(sample_rate)) / 1_000_000_000;
    frames
        .saturating_mul(u128::from(channels))
        .min(u128::from(u64::MAX)) as u64
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
    pub(crate) level_sample: Option<LevelSample>,
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
                level_sample: None,
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
        self.snapshot.level_sample = None;
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
        if self.snapshot.status == BackendStatus::Paused {
            self.snapshot.level_sample = None;
        }
        Ok(self.snapshot.status)
    }

    pub(crate) fn stop(&mut self) {
        self.snapshot.status = BackendStatus::Stopped;
        self.snapshot.position_ms = 0;
        self.snapshot.level_sample = None;
        self.natural_end_latched = true;
    }

    pub(crate) fn seek_accepted(&mut self, requested_ms: u64) -> u64 {
        let position_ms = if self.snapshot.duration_ms > 0 {
            requested_ms.min(self.snapshot.duration_ms)
        } else {
            requested_ms
        };
        self.snapshot.position_ms = position_ms;
        self.snapshot.level_sample = None;
        position_ms
    }

    pub(crate) fn set_volume(&mut self, volume: u8) {
        self.snapshot.volume = volume.min(100);
    }

    pub(crate) fn set_speed(&mut self, speed: f64) {
        self.snapshot.speed = speed;
    }

    pub(crate) fn observe_level_sample(&mut self, sample: LevelSample) {
        self.snapshot.level_sample =
            (self.snapshot.status == BackendStatus::Playing).then_some(sample);
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
            self.snapshot.level_sample = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.snapshot.status = BackendStatus::Error;
        self.snapshot.level_sample = None;
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
        transport.observe_level_sample(LevelSample {
            left: 0.25,
            right: 0.75,
        });
        assert_eq!(transport.snapshot().position_ms, 400);
        assert_eq!(
            transport.snapshot().level_sample,
            Some(LevelSample {
                left: 0.25,
                right: 0.75
            })
        );
        assert_eq!(transport.toggle_pause().unwrap(), BackendStatus::Paused);
        assert_eq!(transport.snapshot().level_sample, None);
        transport.observe_position(450);
        assert_eq!(transport.snapshot().position_ms, 450);
        assert_eq!(transport.toggle_pause().unwrap(), BackendStatus::Playing);
        transport.observe_level_sample(LevelSample {
            left: 0.5,
            right: 0.5,
        });
        assert_eq!(transport.seek_accepted(9_000), 1_500);
        assert_eq!(transport.snapshot().level_sample, None);
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

    #[test]
    fn timeline_tracks_source_samples_and_seek() {
        use rodio::buffer::SamplesBuffer;
        use rodio::nz;

        let samples = vec![0.25; 2_000];
        let source = SamplesBuffer::new(nz!(2), nz!(1_000), samples);
        let (mut source, handle) = TimelineSource::new(source, Duration::ZERO);

        for _ in 0..1_000 {
            assert!(source.next().is_some());
        }
        // The source publishes in bounded batches to avoid an atomic write per
        // sample. At this deliberately tiny sample rate the maximum lag is
        // visible; normal music rates keep it to a few milliseconds.
        assert!((380..=500).contains(&handle.position_ms()));

        source.try_seek(Duration::from_millis(750)).unwrap();
        assert_eq!(handle.position_ms(), 750);
        assert_eq!(
            handle.level_sample(),
            LevelSample {
                left: 0.0,
                right: 0.0
            }
        );
    }

    #[test]
    fn timeline_publishes_stereo_peaks_without_audio_hardware() {
        use rodio::buffer::SamplesBuffer;
        use rodio::nz;

        let samples: Vec<f32> = (0..50).flat_map(|_| [0.25, -0.75]).collect();
        let source = SamplesBuffer::new(nz!(2), nz!(1_000), samples);
        let (mut source, handle) = TimelineSource::new(source, Duration::ZERO);

        for _ in 0..100 {
            assert!(source.next().is_some());
        }

        assert_eq!(
            handle.level_sample(),
            LevelSample {
                left: 0.25,
                right: 0.75
            }
        );
    }

    #[test]
    fn timeline_stays_in_source_time_at_all_supported_rates() {
        use rodio::buffer::SamplesBuffer;
        use rodio::nz;
        use rodio::Player;

        const SAMPLE_RATE: u32 = 48_000;
        const OUTPUT_WINDOW_MS: u64 = 250;
        let output_samples = SAMPLE_RATE as usize * OUTPUT_WINDOW_MS as usize / 1_000;

        for rate in [0.5_f32, 1.0, 2.0, 4.0] {
            let samples = vec![0.25; SAMPLE_RATE as usize * 2];
            let source = SamplesBuffer::new(nz!(1), nz!(48_000), samples);
            let (source, handle) = TimelineSource::new(source, Duration::ZERO);
            let (mixer, mut output) = rodio::mixer::mixer(nz!(1), nz!(48_000));
            let player = Player::connect_new(&mixer);
            player.set_speed(rate);
            player.append(source);

            for _ in 0..output_samples {
                assert!(output.next().is_some());
            }

            let expected_ms = (OUTPUT_WINDOW_MS as f32 * rate) as u64;
            let actual_ms = handle.position_ms();
            assert!(
                actual_ms.abs_diff(expected_ms) <= 50,
                "{rate}x reported {actual_ms} ms of source time; expected about {expected_ms} ms"
            );
        }
    }

    #[test]
    fn timeline_remains_anchored_across_multiple_rate_changes() {
        use rodio::buffer::SamplesBuffer;
        use rodio::nz;
        use rodio::Player;

        const SAMPLE_RATE: usize = 48_000;
        const WARMUP_MS: usize = 2_000;
        const MEASURE_MS: usize = 250;
        let samples = vec![0.25; SAMPLE_RATE * 30];
        let source = SamplesBuffer::new(nz!(1), nz!(48_000), samples);
        let (source, handle) = TimelineSource::new(source, Duration::ZERO);
        let (mixer, mut output) = rodio::mixer::mixer(nz!(1), nz!(48_000));
        let player = Player::connect_new(&mixer);
        player.append(source);

        let mut prior_position_ms = 0_u64;
        for rate in [0.5_f32, 1.0, 2.0, 4.0] {
            player.set_speed(rate);
            // Rodio's output-rate converter re-samples in bounded spans. Drive
            // past the longest span before measuring a newly selected rate.
            for _ in 0..(SAMPLE_RATE * WARMUP_MS / 1_000) {
                assert!(output.next().is_some());
            }
            let before_ms = handle.position_ms();
            assert!(before_ms > prior_position_ms, "source time must not reset");
            for _ in 0..(SAMPLE_RATE * MEASURE_MS / 1_000) {
                assert!(output.next().is_some());
            }
            let after_ms = handle.position_ms();
            let actual_delta_ms = after_ms.saturating_sub(before_ms);
            let expected_delta_ms = (MEASURE_MS as f32 * rate) as u64;
            assert!(
                actual_delta_ms.abs_diff(expected_delta_ms) <= 50,
                "after switching to {rate}x, source time advanced {actual_delta_ms} ms; expected about {expected_delta_ms} ms"
            );
            prior_position_ms = after_ms;
        }
    }
}
