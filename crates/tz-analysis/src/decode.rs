//! Native-first bounded PCM decode for offline analysis.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::types::{AnalysisError, DecodedAnalysisAudio};
use tz_audio::{DecodeError, PcmSource, PcmSpec};

pub const MONO_TARGET_RATE: u32 = 11_025;
pub const STEREO_TARGET_RATE: u32 = 44_100;
const PCM_FRAME_BYTES: usize = std::mem::size_of::<f32>() * 2;
const DEFAULT_MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
const HARD_MAX_DECODED_BYTES: usize = 1024 * 1024 * 1024;
const DEFAULT_MAX_DURATION_SECS: u64 = 60 * 60;
const HARD_MAX_DURATION_SECS: u64 = 6 * 60 * 60;
const DEFAULT_TIMEOUT_SECS: u64 = 2 * 60;
const HARD_MAX_TIMEOUT_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisLimits {
    pub max_decoded_bytes: usize,
    pub max_duration: Duration,
    pub execution_timeout: Duration,
}
impl AnalysisLimits {
    pub fn from_env() -> Self {
        Self {
            max_decoded_bytes: env_u64(
                "TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES",
                DEFAULT_MAX_DECODED_BYTES as u64,
                PCM_FRAME_BYTES as u64,
                HARD_MAX_DECODED_BYTES as u64,
            ) as usize,
            max_duration: Duration::from_secs(env_u64(
                "TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS",
                DEFAULT_MAX_DURATION_SECS,
                1,
                HARD_MAX_DURATION_SECS,
            )),
            execution_timeout: Duration::from_secs(env_u64(
                "TZ_PLAYER_ANALYSIS_TIMEOUT_SECS",
                DEFAULT_TIMEOUT_SECS,
                1,
                HARD_MAX_TIMEOUT_SECS,
            )),
        }
    }
    fn max_frames(self, sample_rate: u32) -> usize {
        let by_bytes = self.max_decoded_bytes / PCM_FRAME_BYTES;
        let by_duration = self
            .max_duration
            .as_nanos()
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
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

pub trait AnalysisDecoder: Send + Sync {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError>;
}

pub fn decode_track_for_analysis(path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
    decode_track_for_analysis_with_limits(path, AnalysisLimits::from_env())
}

pub fn decode_track_for_analysis_with_limits(
    path: &Path,
    limits: AnalysisLimits,
) -> Result<DecodedAnalysisAudio, AnalysisError> {
    if !path.is_file() {
        return Err(AnalysisError::NotFound(path.display().to_string()));
    }
    std::fs::File::open(path).map_err(|error| {
        AnalysisError::Decode(format!(
            "cannot read {}: {error}; check the file permissions",
            path.display()
        ))
    })?;
    validate_limits(limits)?;
    let spec = PcmSpec::new(STEREO_TARGET_RATE, 2).map_err(map_decode_error)?;
    let native = tz_audio::decode_native(path, spec);
    let mut source: Box<dyn PcmSource> = match native {
        Ok(source) => Box::new(source),
        Err(native_error) => {
            let config = tz_audio::helper::HelperConfig::packaged().map_err(|helper_error| AnalysisError::Decode(format!("native decode rejected: {native_error}; bundled helper unavailable: {helper_error}")))?;
            Box::new(tz_audio::helper::decode(&config, path, 0, spec).map_err(|helper_error| AnalysisError::Decode(format!("native decode rejected: {native_error}; bundled helper rejected media: {helper_error}")))?)
        }
    };
    let max_frames = limits.max_frames(STEREO_TARGET_RATE);
    let started = Instant::now();
    let mut interleaved = Vec::new();
    let mut buffer = vec![0.0_f32; 16_384];
    loop {
        if started.elapsed() >= limits.execution_timeout {
            return Err(AnalysisError::Timeout(
                "audio decode exceeded its execution-time limit".into(),
            ));
        }
        let read = source
            .read_interleaved(&mut buffer)
            .map_err(map_decode_error)?;
        if read == 0 {
            break;
        }
        let frames = read / 2;
        if interleaved.len() / 2 + frames > max_frames {
            return Err(decoded_limit_error(max_frames));
        }
        interleaved.extend_from_slice(&buffer[..read]);
    }
    if interleaved.is_empty() {
        return Err(AnalysisError::Decode("empty audio stream".into()));
    }
    let mut left = Vec::with_capacity(interleaved.len() / 2);
    let mut right = Vec::with_capacity(interleaved.len() / 2);
    for frame in interleaved.chunks_exact(2) {
        left.push(frame[0]);
        right.push(frame[1]);
    }
    let mono_source: Vec<f32> = left
        .iter()
        .zip(&right)
        .map(|(left, right)| (left + right) * 0.5)
        .collect();
    let (mono_rate, mono_samples) =
        resample_mono_owned(mono_source, STEREO_TARGET_RATE, MONO_TARGET_RATE);
    let duration_ms = (left.len() as u64).saturating_mul(1_000) / u64::from(STEREO_TARGET_RATE);
    Ok(DecodedAnalysisAudio {
        duration_ms: duration_ms.max(1),
        mono_rate,
        mono_samples,
        stereo_rate: STEREO_TARGET_RATE,
        left_samples: left,
        right_samples: right,
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
            "analysis limits exceed compiled safety ceilings".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct NativeAnalysisDecoder;
impl AnalysisDecoder for NativeAnalysisDecoder {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
        decode_track_for_analysis(path)
    }
}

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
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err(AnalysisError::Decode("invalid wav header".into()));
    }
    let max_frames = limits.max_frames(spec.sample_rate);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut pending = [0.0_f32, 0.0_f32];
    let mut sample_index = 0usize;
    let started = Instant::now();
    for sample in reader.samples::<i32>() {
        if started.elapsed() >= limits.execution_timeout {
            return Err(AnalysisError::Timeout(
                "native WAV decode exceeded its execution-time limit".into(),
            ));
        }
        let sample = sample.map_err(|error| AnalysisError::Decode(error.to_string()))?;
        let channel = sample_index % usize::from(spec.channels);
        let normalized = match spec.bits_per_sample {
            8 => sample as f32 / 128.0,
            16 => sample as f32 / 32_768.0,
            24 => sample as f32 / 8_388_608.0,
            32 => sample as f32 / 2_147_483_648.0,
            bits => return Err(AnalysisError::Decode(format!("unsupported bits {bits}"))),
        }
        .clamp(-1.0, 1.0);
        if channel == 0 {
            if left.len() >= max_frames {
                return Err(decoded_limit_error(max_frames));
            }
            pending[0] = normalized;
            pending[1] = normalized;
        } else if channel == 1 {
            pending[1] = normalized;
        }
        if channel + 1 == usize::from(spec.channels) {
            left.push(pending[0]);
            right.push(pending[1]);
        }
        sample_index += 1;
    }
    if !sample_index.is_multiple_of(usize::from(spec.channels)) {
        return Err(AnalysisError::Decode("truncated WAV frame".into()));
    }
    if left.is_empty() {
        return Err(AnalysisError::Decode("empty wav".into()));
    }
    Ok((spec.sample_rate, left, right))
}

fn map_decode_error(error: DecodeError) -> AnalysisError {
    AnalysisError::Decode(error.to_string())
}
fn decoded_limit_error(max_frames: usize) -> AnalysisError {
    AnalysisError::ResourceLimit(format!(
        "decoded PCM exceeds configured limit ({max_frames} stereo frames)"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn one_second_wav(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tz-analysis-{name}-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: STEREO_TARGET_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..STEREO_TARGET_RATE {
            let time = index as f32 / STEREO_TARGET_RATE as f32;
            let sample =
                (0.4 * (2.0 * std::f32::consts::PI * 440.0 * time).sin() * 32_767.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    #[test]
    fn missing_file_is_reported_without_starting_a_helper() {
        assert!(matches!(
            decode_track_for_analysis(Path::new("nope.wav")),
            Err(AnalysisError::NotFound(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn permission_error_is_reported_without_starting_a_helper() {
        use std::os::unix::fs::PermissionsExt;

        let path = one_second_wav("permission-denied");
        let original = std::fs::metadata(&path).unwrap().permissions();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
        let error = decode_track_for_analysis(&path).expect_err("unreadable media must fail");
        std::fs::set_permissions(&path, original).unwrap();
        std::fs::remove_file(path).unwrap();

        let message = error.to_string();
        assert!(message.contains("cannot read"), "{message}");
        assert!(!message.contains("bundled helper"), "{message}");
    }

    #[test]
    fn analysis_enforces_byte_and_duration_limits_as_pcm_arrives() {
        let path = one_second_wav("limits");
        let byte_limited = AnalysisLimits {
            max_decoded_bytes: PCM_FRAME_BYTES * 16,
            max_duration: Duration::from_secs(HARD_MAX_DURATION_SECS),
            execution_timeout: Duration::from_secs(HARD_MAX_TIMEOUT_SECS),
        };
        assert!(matches!(
            decode_track_for_analysis_with_limits(&path, byte_limited),
            Err(AnalysisError::ResourceLimit(_))
        ));

        let duration_limited = AnalysisLimits {
            max_decoded_bytes: HARD_MAX_DECODED_BYTES,
            max_duration: Duration::from_millis(1),
            execution_timeout: Duration::from_secs(HARD_MAX_TIMEOUT_SECS),
        };
        assert!(matches!(
            decode_track_for_analysis_with_limits(&path, duration_limited),
            Err(AnalysisError::ResourceLimit(_))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn analysis_enforces_wall_clock_timeout() {
        let path = one_second_wav("timeout");
        let limits = AnalysisLimits {
            max_decoded_bytes: HARD_MAX_DECODED_BYTES,
            max_duration: Duration::from_secs(HARD_MAX_DURATION_SECS),
            execution_timeout: Duration::from_nanos(1),
        };
        assert!(matches!(
            decode_track_for_analysis_with_limits(&path, limits),
            Err(AnalysisError::Timeout(_))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn native_route_preserves_pre_migration_wav_dsp_with_tight_tolerances() {
        use crate::{
            analyze_beats_from_decoded, analyze_envelope_from_decoded,
            analyze_spectrum_from_decoded, analyze_waveform_proxy_from_decoded,
        };

        let path = one_second_wav("decoder-parity");
        let (rate, left, right) = decode_wave_raw(&path).unwrap();
        let mono_source = left
            .iter()
            .zip(&right)
            .map(|(left, right)| (left + right) * 0.5)
            .collect();
        let (mono_rate, mono_samples) = resample_mono_owned(mono_source, rate, MONO_TARGET_RATE);
        let baseline = DecodedAnalysisAudio {
            duration_ms: left.len() as u64 * 1_000 / u64::from(rate),
            mono_rate,
            mono_samples,
            stereo_rate: rate,
            left_samples: left,
            right_samples: right,
        };
        let migrated = decode_track_for_analysis(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert!(baseline.duration_ms.abs_diff(migrated.duration_ms) <= 1);
        assert_eq!(baseline.stereo_rate, migrated.stereo_rate);
        assert_eq!(baseline.mono_rate, migrated.mono_rate);
        assert!(
            baseline
                .left_samples
                .len()
                .abs_diff(migrated.left_samples.len())
                <= 1
        );
        let max_sample_delta = baseline
            .left_samples
            .iter()
            .zip(&migrated.left_samples)
            .map(|(old, new)| (old - new).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_sample_delta <= 1.0 / 32_768.0, "{max_sample_delta}");

        let old_envelope = analyze_envelope_from_decoded(&baseline, 50).unwrap();
        let new_envelope = analyze_envelope_from_decoded(&migrated, 50).unwrap();
        assert_eq!(old_envelope.points.len(), new_envelope.points.len());
        for (old, new) in old_envelope.points.iter().zip(&new_envelope.points) {
            assert_eq!(old.0, new.0);
            assert!((old.1 - new.1).abs() <= 1.0 / 32_768.0);
            assert!((old.2 - new.2).abs() <= 1.0 / 32_768.0);
        }

        let old_spectrum = analyze_spectrum_from_decoded(&baseline, 24, 40).unwrap();
        let new_spectrum = analyze_spectrum_from_decoded(&migrated, 24, 40).unwrap();
        assert_eq!(old_spectrum.frames, new_spectrum.frames);
        let old_beats = analyze_beats_from_decoded(&baseline, 40).unwrap();
        let new_beats = analyze_beats_from_decoded(&migrated, 40).unwrap();
        assert!((old_beats.bpm - new_beats.bpm).abs() <= f64::EPSILON);
        assert_eq!(old_beats.frames, new_beats.frames);
        let old_waveform = analyze_waveform_proxy_from_decoded(&baseline, 20).unwrap();
        let new_waveform = analyze_waveform_proxy_from_decoded(&migrated, 20).unwrap();
        assert_eq!(old_waveform.frames, new_waveform.frames);
    }
}
