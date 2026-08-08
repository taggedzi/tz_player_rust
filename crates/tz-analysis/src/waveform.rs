//! Waveform-proxy analysis: stereo min/max envelopes per hop (Python parity).

use std::path::Path;

use crate::decode::decode_track_for_analysis;
use crate::types::{AnalysisError, DecodedAnalysisAudio};

/// Quantized waveform-proxy frames for cache storage.
#[derive(Debug, Clone)]
pub struct WaveformProxyAnalysisResult {
    pub duration_ms: u64,
    /// (position_ms, min_l_i8, max_l_i8, min_r_i8, max_r_i8)
    pub frames: Vec<(u64, i8, i8, i8, i8)>,
}

pub fn analyze_track_waveform_proxy(
    path: &Path,
    hop_ms: u64,
) -> Result<WaveformProxyAnalysisResult, AnalysisError> {
    let decoded = decode_track_for_analysis(path)?;
    analyze_waveform_proxy_from_decoded(&decoded, hop_ms)
}

pub fn analyze_waveform_proxy_from_decoded(
    decoded: &DecodedAnalysisAudio,
    hop_ms: u64,
) -> Result<WaveformProxyAnalysisResult, AnalysisError> {
    analyze_waveform_proxy_from_stereo(
        decoded.stereo_rate,
        &decoded.left_samples,
        &decoded.right_samples,
        hop_ms,
    )
}

pub fn analyze_waveform_proxy_from_stereo(
    sample_rate: u32,
    left_samples: &[f32],
    right_samples: &[f32],
    hop_ms: u64,
) -> Result<WaveformProxyAnalysisResult, AnalysisError> {
    const MAX_FRAMES: usize = 30_000;
    if sample_rate == 0 || left_samples.is_empty() || left_samples.len() != right_samples.len() {
        return Err(AnalysisError::Decode("invalid stereo samples".into()));
    }
    let hop_ms = hop_ms.max(10);
    let hop_frames = ((u64::from(sample_rate) * hop_ms) / 1000).max(1) as usize;

    let mut frames = Vec::new();
    let total = left_samples.len();
    let mut start = 0usize;
    while start < total && frames.len() < MAX_FRAMES {
        let end = (start + hop_frames).min(total);
        let left_bucket = &left_samples[start..end];
        let right_bucket = &right_samples[start..end];
        if left_bucket.is_empty() {
            break;
        }
        let (min_l, max_l) = min_max(left_bucket);
        let (min_r, max_r) = min_max(right_bucket);
        let position_ms = ((start as u64) * 1000) / u64::from(sample_rate);
        frames.push((
            position_ms,
            to_i8(min_l),
            to_i8(max_l),
            to_i8(min_r),
            to_i8(max_r),
        ));
        start = end;
    }
    if frames.is_empty() {
        return Err(AnalysisError::Decode("no waveform frames".into()));
    }
    let duration_ms = ((total as u64) * 1000) / u64::from(sample_rate);
    Ok(WaveformProxyAnalysisResult {
        duration_ms: duration_ms.max(1),
        frames,
    })
}

fn min_max(samples: &[f32]) -> (f32, f32) {
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for &s in samples {
        min_v = min_v.min(s);
        max_v = max_v.max(s);
    }
    (min_v, max_v)
}

fn to_i8(value: f32) -> i8 {
    let c = value.clamp(-1.0, 1.0);
    (c * 127.0).round().clamp(-127.0, 127.0) as i8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_from_wav() {
        let dir = std::env::temp_dir().join(format!(
            "tz_wf_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..8_820 {
            let t = i as f32 / 44_100.0;
            let s = (0.4 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 20000.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(-s / 2).unwrap();
        }
        w.finalize().unwrap();
        let r = analyze_track_waveform_proxy(&path, 20).unwrap();
        assert!(!r.frames.is_empty());
        assert!(r.frames.iter().any(|(_, a, b, _, _)| *a < 0 && *b > 0));
        let _ = std::fs::remove_dir_all(dir);
    }
}
