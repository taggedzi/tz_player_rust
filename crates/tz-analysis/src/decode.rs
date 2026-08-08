//! Analysis decode: native WAV + FFmpeg CLI → PCM (not used for listening).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::types::{AnalysisError, DecodedAnalysisAudio};

pub const MONO_TARGET_RATE: u32 = 11_025;
pub const STEREO_TARGET_RATE: u32 = 44_100;

/// Trait for offline analysis decoders.
pub trait AnalysisDecoder: Send + Sync {
    fn decode(&self, path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError>;
}

/// Returns true if an `ffmpeg` executable is discoverable on PATH.
pub fn ffmpeg_available() -> bool {
    which_ffmpeg().is_some()
}

fn which_ffmpeg() -> Option<String> {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| "ffmpeg".into())
}

/// Decode any supported track into analysis PCM streams.
pub fn decode_track_for_analysis(path: &Path) -> Result<DecodedAnalysisAudio, AnalysisError> {
    if !path.is_file() {
        return Err(AnalysisError::NotFound(path.display().to_string()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let (source_rate, left, right) = if ext == "wav" || ext == "wave" {
        decode_wave_raw(path)?
    } else {
        decode_ffmpeg_raw_stereo(path)?
    };

    if source_rate == 0 || left.is_empty() || left.len() != right.len() {
        return Err(AnalysisError::Decode("empty or mismatched channels".into()));
    }

    let (stereo_rate, stereo_left, stereo_right) =
        resample_stereo(&left, &right, source_rate, STEREO_TARGET_RATE);
    let mono_source: Vec<f32> = stereo_left
        .iter()
        .zip(stereo_right.iter())
        .map(|(l, r)| (l + r) * 0.5)
        .collect();
    let (mono_rate, mono_samples) = resample_mono(&mono_source, stereo_rate, MONO_TARGET_RATE);
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

/// Raw WAV decode → (rate, left, right) at source rate.
pub fn decode_wave_raw(path: &Path) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| AnalysisError::Decode(format!("wav open: {e}")))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let rate = spec.sample_rate;
    if channels == 0 || rate == 0 {
        return Err(AnalysisError::Decode("invalid wav header".into()));
    }

    let mut left = Vec::new();
    let mut right = Vec::new();
    match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = match spec.bits_per_sample {
                8 => 128.0,
                16 => 32768.0,
                24 => 8_388_608.0,
                32 => 2_147_483_648.0,
                b => return Err(AnalysisError::Decode(format!("unsupported bits {b}"))),
            };
            let samples = reader
                .samples::<i32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AnalysisError::Decode(e.to_string()))?;
            for chunk in samples.chunks(channels) {
                let l = clamp_sample(chunk[0] as f32 / max);
                let r = if channels > 1 {
                    clamp_sample(chunk[1] as f32 / max)
                } else {
                    l
                };
                left.push(l);
                right.push(r);
            }
        }
        hound::SampleFormat::Float => {
            let samples = reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AnalysisError::Decode(e.to_string()))?;
            for chunk in samples.chunks(channels) {
                let l = clamp_sample(chunk[0]);
                let r = if channels > 1 {
                    clamp_sample(chunk[1])
                } else {
                    l
                };
                left.push(l);
                right.push(r);
            }
        }
    }
    if left.is_empty() {
        return Err(AnalysisError::Decode("empty wav".into()));
    }
    Ok((rate, left, right))
}

/// FFmpeg → stereo s16le at 44.1 kHz.
pub fn decode_ffmpeg_raw_stereo(path: &Path) -> Result<(u32, Vec<f32>, Vec<f32>), AnalysisError> {
    let ffmpeg = which_ffmpeg().ok_or(AnalysisError::FfmpegUnavailable)?;
    let mut child = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-vn",
            "-sn",
            "-dn",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "2",
            "-ar",
            &STEREO_TARGET_RATE.to_string(),
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AnalysisError::Decode(format!("ffmpeg spawn: {e}")))?;

    let mut raw = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut raw)
            .map_err(|e| AnalysisError::Decode(format!("ffmpeg read: {e}")))?;
    }
    let status = child
        .wait()
        .map_err(|e| AnalysisError::Decode(format!("ffmpeg wait: {e}")))?;
    if !status.success() || raw.is_empty() {
        return Err(AnalysisError::Decode(
            "ffmpeg failed or empty output".into(),
        ));
    }

    let mut left = Vec::with_capacity(raw.len() / 4);
    let mut right = Vec::with_capacity(raw.len() / 4);
    for frame in raw.chunks_exact(4) {
        let l = i16::from_le_bytes([frame[0], frame[1]]);
        let r = i16::from_le_bytes([frame[2], frame[3]]);
        left.push(clamp_sample(f32::from(l) / 32768.0));
        right.push(clamp_sample(f32::from(r) / 32768.0));
    }
    if left.is_empty() {
        return Err(AnalysisError::Decode("no pcm frames".into()));
    }
    Ok((STEREO_TARGET_RATE, left, right))
}

fn resample_mono(samples: &[f32], source_rate: u32, target_rate: u32) -> (u32, Vec<f32>) {
    if source_rate == 0 || target_rate == 0 || source_rate == target_rate {
        return (source_rate, samples.to_vec());
    }
    let step = f64::from(source_rate) / f64::from(target_rate);
    if step <= 1.0 {
        return (source_rate, samples.to_vec());
    }
    let mut out = Vec::new();
    let mut idx = 0.0;
    let size = samples.len();
    while (idx as usize) < size {
        out.push(samples[idx as usize]);
        idx += step;
    }
    (target_rate, out)
}

fn resample_stereo(
    left: &[f32],
    right: &[f32],
    source_rate: u32,
    target_rate: u32,
) -> (u32, Vec<f32>, Vec<f32>) {
    if source_rate == 0 || target_rate == 0 || source_rate == target_rate {
        return (source_rate, left.to_vec(), right.to_vec());
    }
    let step = f64::from(source_rate) / f64::from(target_rate);
    if step <= 1.0 {
        return (source_rate, left.to_vec(), right.to_vec());
    }
    let mut out_l = Vec::new();
    let mut out_r = Vec::new();
    let mut idx = 0.0;
    let size = left.len().min(right.len());
    while (idx as usize) < size {
        let i = idx as usize;
        out_l.push(left[i]);
        out_r.push(right[i]);
        idx += step;
    }
    (target_rate, out_l, out_r)
}

fn clamp_sample(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sine_wav(path: &Path, seconds: f32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let n = (44_100.0 * seconds) as usize;
        for i in 0..n {
            let t = i as f32 / 44_100.0;
            let s = (0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 32767.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn decode_wav_sine() {
        let dir = std::env::temp_dir().join(format!(
            "tz_dec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        write_sine_wav(&path, 0.2);
        let dec = decode_track_for_analysis(&path).unwrap();
        assert!(dec.duration_ms >= 100);
        assert!(!dec.mono_samples.is_empty());
        assert_eq!(dec.left_samples.len(), dec.right_samples.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file() {
        let err = decode_track_for_analysis(Path::new("nope.wav")).unwrap_err();
        assert!(matches!(err, AnalysisError::NotFound(_)));
    }

    #[test]
    fn ffmpeg_probe() {
        let _ = ffmpeg_available();
    }
}
