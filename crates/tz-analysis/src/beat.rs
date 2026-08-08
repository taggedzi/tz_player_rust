//! Beat / onset analysis for visualizers (Python parity).

use std::path::Path;

use crate::decode::decode_track_for_analysis;
use crate::types::{AnalysisError, DecodedAnalysisAudio};

/// Quantized onset/beat frames ready for cache storage.
#[derive(Debug, Clone)]
pub struct BeatAnalysisResult {
    pub duration_ms: u64,
    pub bpm: f64,
    /// (position_ms, strength_u8, is_beat)
    pub frames: Vec<(u64, u8, bool)>,
}

pub fn analyze_track_beats(path: &Path, hop_ms: u64) -> Result<BeatAnalysisResult, AnalysisError> {
    let decoded = decode_track_for_analysis(path)?;
    analyze_beats_from_decoded(&decoded, hop_ms)
}

pub fn analyze_beats_from_decoded(
    decoded: &DecodedAnalysisAudio,
    hop_ms: u64,
) -> Result<BeatAnalysisResult, AnalysisError> {
    analyze_beats_from_mono(decoded.mono_rate, &decoded.mono_samples, hop_ms)
}

pub fn analyze_beats_from_mono(
    sample_rate: u32,
    mono_samples: &[f32],
    hop_ms: u64,
) -> Result<BeatAnalysisResult, AnalysisError> {
    const MAX_FRAMES: usize = 12_000;
    if sample_rate == 0 || mono_samples.is_empty() {
        return Err(AnalysisError::Decode("empty mono".into()));
    }
    let hop_ms = hop_ms.max(10);
    let hop_samples = ((u64::from(sample_rate) * hop_ms) / 1000).max(1) as usize;
    let window_samples = hop_samples.max(hop_samples * 2);

    let mut energies = Vec::new();
    for start in (0..mono_samples.len()).step_by(hop_samples) {
        let end = (start + window_samples).min(mono_samples.len());
        if start >= end {
            break;
        }
        energies.push(rms_energy(&mono_samples[start..end]));
        if energies.len() >= MAX_FRAMES {
            break;
        }
    }
    if energies.is_empty() {
        return Err(AnalysisError::Decode("no beat frames".into()));
    }

    let onsets = onset_envelope(&energies);
    let max_onset = onsets.iter().copied().fold(0.0f64, f64::max);
    let strengths: Vec<f64> = if max_onset <= 0.0 {
        vec![0.0; onsets.len()]
    } else {
        onsets.iter().map(|v| (*v / max_onset).min(1.0)).collect()
    };

    let fps = 1000.0 / hop_ms as f64;
    let (bpm, beat_lag) = estimate_bpm(&onsets, fps);
    let beat_flags = mark_beats(&strengths, beat_lag);

    let mut frames = Vec::with_capacity(strengths.len());
    for (idx, strength) in strengths.iter().enumerate() {
        let position_ms = idx as u64 * hop_ms;
        let strength_u8 = (*strength * 255.0).round().clamp(0.0, 255.0) as u8;
        let is_beat = beat_flags.get(idx).copied().unwrap_or(false);
        frames.push((position_ms, strength_u8, is_beat));
    }

    let duration_ms = ((mono_samples.len() as u64) * 1000) / u64::from(sample_rate);
    Ok(BeatAnalysisResult {
        duration_ms: duration_ms.max(1),
        bpm,
        frames,
    })
}

fn rms_energy(values: &[f32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let total: f64 = values
        .iter()
        .map(|v| {
            let x = f64::from(*v);
            x * x
        })
        .sum();
    (total / values.len() as f64).sqrt()
}

fn onset_envelope(energies: &[f64]) -> Vec<f64> {
    if energies.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0.0];
    for idx in 1..energies.len() {
        let diff = energies[idx] - energies[idx - 1];
        out.push(if diff > 0.0 { diff } else { 0.0 });
    }
    out
}

fn estimate_bpm(onsets: &[f64], fps: f64) -> (f64, usize) {
    if onsets.len() < 8 || fps <= 0.0 {
        return (0.0, 0);
    }
    let min_bpm = 60.0;
    let max_bpm = 180.0;
    let lag_min = ((60.0 * fps) / max_bpm).round().max(1.0) as usize;
    let mut lag_max = ((60.0 * fps) / min_bpm).round() as usize;
    lag_max = lag_max.max(lag_min + 1).min(onsets.len() - 1);
    if lag_max <= lag_min {
        return (0.0, 0);
    }

    let mut best_lag = 0usize;
    let mut best_score = 0.0f64;
    for lag in lag_min..=lag_max {
        let mut score = 0.0;
        for idx in lag..onsets.len() {
            score += onsets[idx] * onsets[idx - lag];
        }
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }
    if best_lag == 0 || best_score <= 0.0 {
        return (0.0, 0);
    }
    let bpm = (60.0 * fps) / best_lag as f64;
    (bpm.max(0.0), best_lag)
}

fn mark_beats(strengths: &[f64], lag: usize) -> Vec<bool> {
    if strengths.is_empty() || lag == 0 {
        return vec![false; strengths.len()];
    }
    let mut phase_scores = vec![0.0f64; lag];
    for (idx, strength) in strengths.iter().enumerate() {
        phase_scores[idx % lag] += strength;
    }
    let phase = phase_scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mean_strength = strengths.iter().sum::<f64>() / strengths.len() as f64;
    let threshold = (mean_strength * 1.35).max(0.12);
    strengths
        .iter()
        .enumerate()
        .map(|(idx, strength)| (idx % lag == phase) && (*strength >= threshold))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_from_sine_wav() {
        let dir = std::env::temp_dir().join(format!(
            "tz_beat_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        // Amplitude-modulated tone to create pseudo onsets
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..44_100 {
            let t = i as f32 / 22_050.0;
            let env = if (i / 4000) % 2 == 0 { 0.6 } else { 0.05 };
            let s = (env * (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 20000.0) as i16;
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
        let result = analyze_track_beats(&path, 40).unwrap();
        assert!(!result.frames.is_empty());
        assert!(result.frames.iter().any(|(_, s, _)| *s > 0));
        let _ = std::fs::remove_dir_all(dir);
    }
}
