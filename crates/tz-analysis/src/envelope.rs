//! Scalar envelope analysis for reactive visualizers.

use std::path::Path;

use crate::decode::decode_track_for_analysis;
use crate::types::{AnalysisError, DecodedAnalysisAudio, EnvelopeAnalysisResult};

const DEFAULT_BUCKET_MS: u64 = 50;
const MAX_POINTS: usize = 12_000;

/// Analyze a track into timestamped stereo level buckets.
pub fn analyze_track_envelope(
    path: &Path,
    bucket_ms: u64,
) -> Result<EnvelopeAnalysisResult, AnalysisError> {
    let decoded = decode_track_for_analysis(path)?;
    analyze_envelope_from_decoded(&decoded, bucket_ms)
}

/// Build an envelope from an existing bounded decode so callers generating
/// several analysis products do not decode the same track twice.
pub fn analyze_envelope_from_decoded(
    decoded: &DecodedAnalysisAudio,
    bucket_ms: u64,
) -> Result<EnvelopeAnalysisResult, AnalysisError> {
    let result = analyze_from_pcm(
        &decoded.left_samples,
        &decoded.right_samples,
        decoded.stereo_rate,
        bucket_ms.max(10),
    )?;
    Ok(limit_points(result, MAX_POINTS))
}

pub fn analyze_track_envelope_default(
    path: &Path,
) -> Result<EnvelopeAnalysisResult, AnalysisError> {
    analyze_track_envelope(path, DEFAULT_BUCKET_MS)
}

fn analyze_from_pcm(
    left: &[f32],
    right: &[f32],
    rate: u32,
    bucket_ms: u64,
) -> Result<EnvelopeAnalysisResult, AnalysisError> {
    if rate == 0 || left.is_empty() {
        return Err(AnalysisError::Decode("empty pcm".into()));
    }
    let n = left.len().min(right.len());
    let bucket_frames = ((u64::from(rate) * bucket_ms) / 1000).max(1) as usize;
    let mut points = Vec::new();
    let mut i = 0usize;
    while i < n {
        let end = (i + bucket_frames).min(n);
        let mut lsum = 0.0f32;
        let mut rsum = 0.0f32;
        let count = (end - i) as f32;
        for j in i..end {
            lsum += left[j].abs();
            rsum += right[j].abs();
        }
        let position_ms = ((i as u64) * 1000) / u64::from(rate);
        points.push((
            position_ms,
            (lsum / count).clamp(0.0, 1.0),
            (rsum / count).clamp(0.0, 1.0),
        ));
        i = end;
    }
    if points.is_empty() {
        return Err(AnalysisError::Decode("no envelope points".into()));
    }
    let duration_ms = ((n as u64) * 1000) / u64::from(rate);
    Ok(EnvelopeAnalysisResult {
        duration_ms: duration_ms.max(1),
        points,
    })
}

fn limit_points(mut result: EnvelopeAnalysisResult, max_points: usize) -> EnvelopeAnalysisResult {
    if result.points.len() <= max_points {
        return result;
    }
    let step = result.points.len() as f64 / max_points as f64;
    let mut out = Vec::with_capacity(max_points);
    let mut idx = 0.0;
    while out.len() < max_points && (idx as usize) < result.points.len() {
        out.push(result.points[idx as usize]);
        idx += step;
    }
    result.points = out;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_track_for_analysis;

    #[test]
    fn envelope_from_wav() {
        let dir = std::env::temp_dir().join(format!(
            "tz_env_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        // Reuse decode test helper path: write via hound
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..11_025 {
            let t = i as f32 / 22_050.0;
            let s = (0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 20000.0) as i16;
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();

        let env = analyze_track_envelope_default(&path).unwrap();
        assert!(!env.points.is_empty());
        assert!(env.duration_ms >= 400);
        // peak somewhere above silence
        assert!(env.points.iter().any(|(_, l, r)| *l > 0.05 || *r > 0.05));
        let _ = decode_track_for_analysis(&path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
