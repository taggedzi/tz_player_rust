//! Analysis decode: native WAV + FFmpeg CLI -> bounded PCM (not used for listening).

use std::fmt::Display;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::types::{AnalysisError, DecodedAnalysisAudio};

pub const MONO_TARGET_RATE: u32 = 11_025;
pub const STEREO_TARGET_RATE: u32 = 44_100;

const PCM_FRAME_BYTES: usize = std::mem::size_of::<f32>() * 2;
const FFMPEG_IO_CHUNK_BYTES: usize = 32 * 1024;
const DEFAULT_MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
const HARD_MAX_DECODED_BYTES: usize = 1024 * 1024 * 1024;
const DEFAULT_MAX_DURATION_SECS: u64 = 60 * 60;
const HARD_MAX_DURATION_SECS: u64 = 6 * 60 * 60;
const DEFAULT_TIMEOUT_SECS: u64 = 2 * 60;
const HARD_MAX_TIMEOUT_SECS: u64 = 15 * 60;

/// Resource limits applied to every offline media-analysis decode.
///
/// The default entry point reads these optional environment overrides:
/// `TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES`,
/// `TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS`, and
/// `TZ_PLAYER_ANALYSIS_TIMEOUT_SECS`. Values are clamped to hard ceilings so a
/// hostile environment cannot turn the guardrails into unbounded allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisLimits {
    /// Maximum bytes occupied by decoded stereo `f32` PCM.
    pub max_decoded_bytes: usize,
    /// Maximum decoded audio duration.
    pub max_duration: Duration,
    /// Maximum wall-clock time spent decoding one track.
    pub execution_timeout: Duration,
}

impl AnalysisLimits {
    pub fn from_env() -> Self {
        let max_decoded_bytes = env_u64(
            "TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES",
            DEFAULT_MAX_DECODED_BYTES as u64,
            PCM_FRAME_BYTES as u64,
            HARD_MAX_DECODED_BYTES as u64,
        ) as usize;
        let max_duration_secs = env_u64(
            "TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS",
            DEFAULT_MAX_DURATION_SECS,
            1,
            HARD_MAX_DURATION_SECS,
        );
        let timeout_secs = env_u64(
            "TZ_PLAYER_ANALYSIS_TIMEOUT_SECS",
            DEFAULT_TIMEOUT_SECS,
            1,
            HARD_MAX_TIMEOUT_SECS,
        );
        Self {
            max_decoded_bytes,
            max_duration: Duration::from_secs(max_duration_secs),
            execution_timeout: Duration::from_secs(timeout_secs),
        }
    }

    fn max_frames(self, sample_rate: u32) -> usize {
        let by_bytes = self.max_decoded_bytes / PCM_FRAME_BYTES;
        let duration_nanos = self.max_duration.as_nanos();
        let by_duration = duration_nanos
            .saturating_mul(u128::from(sample_rate))
            .checked_div(1_000_000_000)
            .unwrap_or(u128::MAX)
            .min(usize::MAX as u128) as usize;
        by_bytes.min(by_duration)
    }
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
            max_duration: Duration::from_secs(DEFAULT_MAX_DURATION_SECS),
            execution_timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }
}

fn env_u64(name: &str, default: u64, min: u64, max: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

/// Trait for offline analysis decoders.
pub trait AnalysisDecoder: Send + Sync {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError>;
}

/// Returns true if an `ffmpeg` executable on PATH starts and exits promptly.
pub fn ffmpeg_available() -> bool {
    let Ok(mut child) = Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    wait_for_exit(&mut child, Duration::from_secs(3)).is_ok_and(|status| status.success())
}

/// Decode a track with environment-configured resource limits.
pub fn decode_track_for_analysis(path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
    decode_track_for_analysis_with_limits(path, AnalysisLimits::from_env())
}

/// Decode a track with explicit resource limits.
pub fn decode_track_for_analysis_with_limits(
    path: &Path,
    limits: AnalysisLimits,
) -> Result<DecodedAnalysisAudio, AnalysisError> {
    if !path.is_file() {
        return Err(AnalysisError::NotFound(path.display().to_string()));
    }
    validate_limits(limits)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let (source_rate, left, right) = if ext == "wav" || ext == "wave" {
        decode_wave_raw_with_limits(path, limits)?
    } else {
        decode_ffmpeg_raw_stereo_with_limits(path, limits)?
    };

    if source_rate == 0 || left.is_empty() || left.len() != right.len() {
        return Err(AnalysisError::Decode("empty or mismatched channels".into()));
    }

    let (stereo_rate, stereo_left, stereo_right) =
        resample_stereo_owned(left, right, source_rate, STEREO_TARGET_RATE);
    let mono_source: Vec<f32> = stereo_left
        .iter()
        .zip(stereo_right.iter())
        .map(|(left, right)| (left + right) * 0.5)
        .collect();
    let (mono_rate, mono_samples) = resample_mono_owned(mono_source, stereo_rate, MONO_TARGET_RATE);
    if mono_rate == 0 || mono_samples.is_empty() {
        return Err(AnalysisError::Decode("resample failed".into()));
    }
    let duration_ms = ((mono_samples.len() as u64) * 1000) / u64::from(mono_rate);
    Ok(DecodedAnalysisAudio {
        duration_ms: duration_ms.max(1),
        mono_rate,
        mono_samples,
        stereo_rate,
        left_samples: stereo_left,
        right_samples: stereo_right,
    })
}

fn validate_limits(limits: AnalysisLimits) -> Result<(), AnalysisError> {
    if limits.max_decoded_bytes < PCM_FRAME_BYTES
        || limits.max_duration.is_zero()
        || limits.execution_timeout.is_zero()
    {
        return Err(AnalysisError::ResourceLimit(
            "analysis limits must be greater than zero".into(),
        ));
    }
    if limits.max_decoded_bytes > HARD_MAX_DECODED_BYTES
        || limits.max_duration > Duration::from_secs(HARD_MAX_DURATION_SECS)
        || limits.execution_timeout > Duration::from_secs(HARD_MAX_TIMEOUT_SECS)
    {
        return Err(AnalysisError::ResourceLimit(
            "analysis limits exceed the compiled safety ceilings".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct WavNativeDecoder;

impl AnalysisDecoder for WavNativeDecoder {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
        decode_track_for_analysis(path)
    }
}

#[derive(Debug, Default)]
pub struct FfmpegCliDecoder;

impl AnalysisDecoder for FfmpegCliDecoder {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
        decode_track_for_analysis(path)
    }
}

/// Raw WAV decode with default limits -> (rate, left, right) at source rate.
pub fn decode_wave_raw(path: &Path) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    decode_wave_raw_with_limits(path, AnalysisLimits::from_env())
}

pub fn decode_wave_raw_with_limits(
    path: &Path,
    limits: AnalysisLimits,
) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    validate_limits(limits)?;
    let mut reader = hound::WavReader::open(path)
        .map_err(|error| AnalysisError::Decode(format!("wav open: {error}")))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let rate = spec.sample_rate;
    if channels == 0 || rate == 0 {
        return Err(AnalysisError::Decode("invalid wav header".into()));
    }
    let max_frames = limits.max_frames(rate);
    if max_frames == 0 {
        return Err(AnalysisError::ResourceLimit(
            "analysis limits allow no PCM frames".into(),
        ));
    }
    let started = Instant::now();

    match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = match spec.bits_per_sample {
                8 => 128.0,
                16 => 32_768.0,
                24 => 8_388_608.0,
                32 => 2_147_483_648.0,
                bits => return Err(AnalysisError::Decode(format!("unsupported bits {bits}"))),
            };
            collect_wave_frames(
                reader.samples::<i32>(),
                channels,
                max_frames,
                started,
                limits.execution_timeout,
                |sample| clamp_sample(sample as f32 / max),
            )
            .map(|(left, right)| (rate, left, right))
        }
        hound::SampleFormat::Float => collect_wave_frames(
            reader.samples::<f32>(),
            channels,
            max_frames,
            started,
            limits.execution_timeout,
            clamp_sample,
        )
        .map(|(left, right)| (rate, left, right)),
    }
}

fn collect_wave_frames<T, E, I, F>(
    samples: I,
    channels: usize,
    max_frames: usize,
    started: Instant,
    timeout: Duration,
    normalize: F,
) -> Result<(Vec<f32>, Vec<f32>), AnalysisError>
where
    I: Iterator<Item = Result<T, E>>,
    E: Display,
    F: Fn(T) -> f32,
{
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut pending_left = 0.0;
    let mut pending_right = 0.0;
    let mut sample_count = 0usize;

    for sample in samples {
        if sample_count % 4096 == 0 && started.elapsed() >= timeout {
            return Err(AnalysisError::Timeout(
                "native WAV decode exceeded its execution-time limit".into(),
            ));
        }
        let channel = sample_count % channels;
        if channel == 0 {
            if left.len() >= max_frames {
                return Err(decoded_limit_error(max_frames));
            }
            pending_left =
                normalize(sample.map_err(|error| AnalysisError::Decode(error.to_string()))?);
            pending_right = pending_left;
        } else if channel == 1 {
            pending_right =
                normalize(sample.map_err(|error| AnalysisError::Decode(error.to_string()))?);
        } else {
            sample.map_err(|error| AnalysisError::Decode(error.to_string()))?;
        }

        if channel + 1 == channels {
            left.push(pending_left);
            right.push(pending_right);
        }
        sample_count += 1;
    }

    if sample_count % channels != 0 {
        return Err(AnalysisError::Decode("truncated WAV frame".into()));
    }
    if left.is_empty() {
        return Err(AnalysisError::Decode("empty wav".into()));
    }
    Ok((left, right))
}

/// FFmpeg -> stereo s16le at 44.1 kHz with default limits.
pub fn decode_ffmpeg_raw_stereo(path: &Path) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    decode_ffmpeg_raw_stereo_with_limits(path, AnalysisLimits::from_env())
}

pub fn decode_ffmpeg_raw_stereo_with_limits(
    path: &Path,
    limits: AnalysisLimits,
) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    validate_limits(limits)?;
    let max_frames = limits.max_frames(STEREO_TARGET_RATE);
    if max_frames == 0 {
        return Err(AnalysisError::ResourceLimit(
            "analysis limits allow no PCM frames".into(),
        ));
    }
    // Ask FFmpeg to stop just beyond our limit as a second line of defense;
    // the streaming reader rejects and kills it on the first excess frame.
    let output_seconds = (max_frames.saturating_add(1)) as f64 / f64::from(STEREO_TARGET_RATE);
    let mut child = Command::new("ffmpeg")
        .arg("-nostdin")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-sn", "-dn", "-t"])
        .arg(format!("{output_seconds:.6}"))
        .args([
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "2",
            "-ar",
            "44100",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AnalysisError::FfmpegUnavailable
            } else {
                AnalysisError::Decode(format!("ffmpeg spawn: {error}"))
            }
        })?;

    let Some(stdout) = child.stdout.take() else {
        terminate_and_reap(&mut child);
        return Err(AnalysisError::Decode("FFmpeg stdout unavailable".into()));
    };
    let (rx, reader) = spawn_stdout_reader(stdout);
    let started = Instant::now();
    let mut status: Option<ExitStatus> = None;
    let mut reader_done = false;
    let mut pending = Vec::with_capacity(FFMPEG_IO_CHUNK_BYTES + 3);
    let mut left = Vec::new();
    let mut right = Vec::new();

    while !reader_done || status.is_none() {
        if started.elapsed() >= limits.execution_timeout {
            terminate_and_reap(&mut child);
            drop(rx);
            let _ = reader.join();
            return Err(AnalysisError::Timeout(format!(
                "FFmpeg exceeded the {}s execution-time limit",
                limits.execution_timeout.as_secs_f64()
            )));
        }

        if !reader_done {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(chunk)) => {
                    pending.extend_from_slice(&chunk);
                    let complete_bytes = pending.len() / 4 * 4;
                    for frame in pending[..complete_bytes].chunks_exact(4) {
                        if left.len() >= max_frames {
                            terminate_and_reap(&mut child);
                            drop(rx);
                            let _ = reader.join();
                            return Err(decoded_limit_error(max_frames));
                        }
                        let left_sample = i16::from_le_bytes([frame[0], frame[1]]);
                        let right_sample = i16::from_le_bytes([frame[2], frame[3]]);
                        left.push(clamp_sample(f32::from(left_sample) / 32_768.0));
                        right.push(clamp_sample(f32::from(right_sample) / 32_768.0));
                    }
                    pending.drain(..complete_bytes);
                }
                Ok(Err(error)) => {
                    terminate_and_reap(&mut child);
                    drop(rx);
                    let _ = reader.join();
                    return Err(AnalysisError::Decode(format!(
                        "FFmpeg output read failed: {error}"
                    )));
                }
                Err(RecvTimeoutError::Disconnected) => reader_done = true,
                Err(RecvTimeoutError::Timeout) => {}
            }
        } else {
            thread::sleep(Duration::from_millis(5));
        }

        if status.is_none() {
            match child.try_wait() {
                Ok(next_status) => status = next_status,
                Err(error) => {
                    terminate_and_reap(&mut child);
                    drop(rx);
                    let _ = reader.join();
                    return Err(AnalysisError::Decode(format!("ffmpeg wait: {error}")));
                }
            }
        }
    }

    let reader_result = reader
        .join()
        .map_err(|_| AnalysisError::Decode("FFmpeg output reader panicked".into()))?;
    reader_result.map_err(|error| AnalysisError::Decode(format!("FFmpeg read: {error}")))?;
    if !pending.is_empty() {
        return Err(AnalysisError::Decode(
            "FFmpeg returned a truncated PCM frame".into(),
        ));
    }
    if !status.is_some_and(|status| status.success()) || left.is_empty() {
        return Err(AnalysisError::Decode(
            "FFmpeg failed or returned empty output".into(),
        ));
    }
    Ok((STEREO_TARGET_RATE, left, right))
}

type ReaderMessage = Result<Vec<u8>, String>;

fn spawn_stdout_reader(
    mut stdout: impl Read + Send + 'static,
) -> (
    Receiver<ReaderMessage>,
    thread::JoinHandle<Result<(), String>>,
) {
    // A synchronous channel keeps at most two chunks queued, so a producer
    // cannot outrun validation and grow memory without bound.
    let (tx, rx) = mpsc::sync_channel::<ReaderMessage>(2);
    let join = thread::spawn(move || {
        let mut buffer = vec![0u8; FFMPEG_IO_CHUNK_BYTES];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    if tx.send(Ok(buffer[..count].to_vec())).is_err() {
                        return Ok(());
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = tx.send(Err(message.clone()));
                    return Err(message);
                }
            }
        }
    });
    (rx, join)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<ExitStatus, ()> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(10));
            }
            _ => {
                terminate_and_reap(child);
                return Err(());
            }
        }
    }
}

fn terminate_and_reap(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn decoded_limit_error(max_frames: usize) -> AnalysisError {
    AnalysisError::ResourceLimit(format!(
        "decoded PCM exceeds configured byte or duration limit ({max_frames} stereo frames)"
    ))
}

fn resample_mono_owned(samples: Vec<f32>, source_rate: u32, target_rate: u32) -> (u32, Vec<f32>) {
    if source_rate == 0 || target_rate == 0 || source_rate <= target_rate {
        return (source_rate, samples);
    }
    let step = f64::from(source_rate) / f64::from(target_rate);
    let mut output =
        Vec::with_capacity(((samples.len() as f64 / step).ceil() as usize).min(samples.len()));
    let mut index = 0.0;
    while (index as usize) < samples.len() {
        output.push(samples[index as usize]);
        index += step;
    }
    (target_rate, output)
}

fn resample_stereo_owned(
    left: Vec<f32>,
    right: Vec<f32>,
    source_rate: u32,
    target_rate: u32,
) -> (u32, Vec<f32>, Vec<f32>) {
    if source_rate == 0 || target_rate == 0 || source_rate <= target_rate {
        return (source_rate, left, right);
    }
    let step = f64::from(source_rate) / f64::from(target_rate);
    let size = left.len().min(right.len());
    let capacity = ((size as f64 / step).ceil() as usize).min(size);
    let mut output_left = Vec::with_capacity(capacity);
    let mut output_right = Vec::with_capacity(capacity);
    let mut index = 0.0;
    while (index as usize) < size {
        let sample_index = index as usize;
        output_left.push(left[sample_index]);
        output_right.push(right[sample_index]);
        index += step;
    }
    (target_rate, output_left, output_right)
}

fn clamp_sample(value: f32) -> f32 {
    value.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sine_wav(path: &Path, seconds: f32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let sample_count = (44_100.0 * seconds) as usize;
        for index in 0..sample_count {
            let time = index as f32 / 44_100.0;
            let sample =
                (0.3 * (2.0 * std::f32::consts::PI * 440.0 * time).sin() * 32_767.0) as i16;
            writer.write_sample(sample).unwrap();
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn decode_wav_sine() {
        let dir = temp_dir("tz_dec");
        let path = dir.join("track.wav");
        write_sine_wav(&path, 0.2);
        let decoded = decode_track_for_analysis(&path).unwrap();
        assert!(decoded.duration_ms >= 100);
        assert!(!decoded.mono_samples.is_empty());
        assert_eq!(decoded.left_samples.len(), decoded.right_samples.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn streaming_wav_decode_rejects_oversized_pcm() {
        let dir = temp_dir("tz_dec_limit");
        let path = dir.join("track.wav");
        write_sine_wav(&path, 0.2);
        let limits = AnalysisLimits {
            max_decoded_bytes: PCM_FRAME_BYTES * 100,
            max_duration: Duration::from_secs(60),
            execution_timeout: Duration::from_secs(2),
        };
        let error = decode_track_for_analysis_with_limits(&path, limits).unwrap_err();
        assert!(matches!(error, AnalysisError::ResourceLimit(_)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ffmpeg_stream_respects_decoded_limit_when_available() {
        if !ffmpeg_available() {
            return;
        }
        let dir = temp_dir("tz_ffmpeg_limit");
        let wav_path = dir.join("source.wav");
        let media_path = dir.join("source.media");
        write_sine_wav(&wav_path, 0.2);
        std::fs::rename(&wav_path, &media_path).unwrap();
        let limits = AnalysisLimits {
            max_decoded_bytes: PCM_FRAME_BYTES * 100,
            max_duration: Duration::from_secs(60),
            execution_timeout: Duration::from_secs(5),
        };
        let error = decode_ffmpeg_raw_stereo_with_limits(&media_path, limits).unwrap_err();
        assert!(matches!(error, AnalysisError::ResourceLimit(_)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file() {
        let error = decode_track_for_analysis(Path::new("nope.wav")).unwrap_err();
        assert!(matches!(error, AnalysisError::NotFound(_)));
    }

    #[test]
    fn ffmpeg_probe() {
        let _ = ffmpeg_available();
    }
}
