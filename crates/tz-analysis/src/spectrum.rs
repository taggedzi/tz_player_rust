//! Log-spaced Goertzel spectrum analysis for visualizers (Python parity).

use std::f64::consts::PI;
use std::path::Path;

use crate::decode::decode_track_for_analysis;
use crate::types::{AnalysisError, DecodedAnalysisAudio};

const MIN_FREQ_HZ: f64 = 40.0;
const MAX_FREQ_HZ: f64 = 5_000.0;

/// Quantized spectrum frames ready for persistent cache storage.
#[derive(Debug, Clone)]
pub struct SpectrumAnalysisResult {
    pub duration_ms: u64,
    /// (position_ms, bands as u8[band_count] 0..=255)
    pub frames: Vec<(u64, Vec<u8>)>,
}

/// Decode track and compute quantized log-spaced spectrum frames.
pub fn analyze_track_spectrum(
    path: &Path,
    band_count: usize,
    hop_ms: u64,
) -> Result<SpectrumAnalysisResult, AnalysisError> {
    let decoded = decode_track_for_analysis(path)?;
    analyze_spectrum_from_decoded(&decoded, band_count, hop_ms)
}

pub fn analyze_spectrum_from_decoded(
    decoded: &DecodedAnalysisAudio,
    band_count: usize,
    hop_ms: u64,
) -> Result<SpectrumAnalysisResult, AnalysisError> {
    analyze_spectrum_from_mono(decoded.mono_rate, &decoded.mono_samples, band_count, hop_ms)
}

pub fn analyze_spectrum_from_mono(
    sample_rate: u32,
    mono_samples: &[f32],
    band_count: usize,
    hop_ms: u64,
) -> Result<SpectrumAnalysisResult, AnalysisError> {
    const MAX_FRAMES: usize = 12_000;
    if sample_rate == 0 || mono_samples.is_empty() {
        return Err(AnalysisError::Decode("empty mono samples".into()));
    }
    let band_count = band_count.max(8);
    let hop_ms = hop_ms.max(10);
    let hop_samples = ((u64::from(sample_rate) * hop_ms) / 1000).max(1) as usize;
    let window_size = window_size(hop_samples);
    let freqs = log_frequencies(band_count, sample_rate);
    let hann = hann_weights(window_size);
    let coeffs = goertzel_coeffs(sample_rate, &freqs, window_size);

    let mut magnitudes: Vec<Vec<f64>> = Vec::new();
    let mut frame_positions: Vec<u64> = Vec::new();
    let mut window_buf = vec![0.0f32; window_size];
    let mut windowed = vec![0.0f32; window_size];
    let total = mono_samples.len();

    for (frame_count, start) in (0..total).step_by(hop_samples).enumerate() {
        if frame_count >= MAX_FRAMES {
            break;
        }
        frame_positions.push(((start as u64) * 1000) / u64::from(sample_rate));
        fill_window(mono_samples, start, window_size, &mut window_buf);
        for i in 0..window_size {
            windowed[i] = window_buf[i] * hann[i];
        }
        magnitudes.push(frame_magnitudes(&windowed, &coeffs));
    }
    if magnitudes.is_empty() {
        return Err(AnalysisError::Decode("no spectrum frames".into()));
    }

    let mut max_mag = 0.0f64;
    for row in &magnitudes {
        for v in row {
            if *v > max_mag {
                max_mag = *v;
            }
        }
    }
    if max_mag <= 0.0 {
        max_mag = 1.0;
    }

    let mut frames = Vec::with_capacity(magnitudes.len());
    for (idx, row) in magnitudes.iter().enumerate() {
        let quantized: Vec<u8> = row
            .iter()
            .map(|v| quantize_level((*v / max_mag) as f32))
            .collect();
        frames.push((frame_positions[idx], quantized));
    }

    let duration_ms = ((mono_samples.len() as u64) * 1000) / u64::from(sample_rate);
    Ok(SpectrumAnalysisResult {
        duration_ms: duration_ms.max(1),
        frames,
    })
}

fn window_size(hop_samples: usize) -> usize {
    let target = (hop_samples * 2).max(256);
    let mut size = 1usize;
    while size < target {
        size <<= 1;
    }
    size.min(2048)
}

fn log_frequencies(band_count: usize, sample_rate: u32) -> Vec<f64> {
    let nyquist = ((sample_rate as f64 / 2.0) - 1.0).max(100.0);
    let min_freq = MIN_FREQ_HZ;
    let max_freq = MAX_FREQ_HZ.min(nyquist);
    if band_count <= 1 {
        return vec![min_freq];
    }
    let ratio = (max_freq / min_freq).powf(1.0 / (band_count as f64 - 1.0));
    (0..band_count)
        .map(|i| min_freq * ratio.powi(i as i32))
        .collect()
}

fn hann_weights(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size.max(1)];
    }
    (0..size)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * (i as f64) / ((size - 1) as f64)).cos()) as f32)
        .collect()
}

fn fill_window(source: &[f32], start: usize, window_size: usize, out: &mut [f32]) {
    let end = (start + window_size).min(source.len());
    let mut copied = 0;
    for &sample in source.iter().take(end).skip(start) {
        out[copied] = sample;
        copied += 1;
    }
    while copied < window_size {
        out[copied] = 0.0;
        copied += 1;
    }
}

fn goertzel_coeffs(sample_rate: u32, freqs: &[f64], sample_count: usize) -> Vec<f64> {
    if sample_count == 0 || sample_rate == 0 {
        return vec![0.0; freqs.len()];
    }
    freqs
        .iter()
        .map(|freq_hz| {
            let k = (0.5 + ((sample_count as f64 * freq_hz) / f64::from(sample_rate))) as i32;
            let omega = (2.0 * PI * f64::from(k)) / sample_count as f64;
            2.0 * omega.cos()
        })
        .collect()
}

fn frame_magnitudes(window: &[f32], coeffs: &[f64]) -> Vec<f64> {
    coeffs
        .iter()
        .map(|coeff| goertzel_power_with_coeff(window, *coeff))
        .collect()
}

fn goertzel_power_with_coeff(samples: &[f32], coeff: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut s_prev = 0.0f64;
    let mut s_prev2 = 0.0f64;
    for &sample in samples {
        let s = f64::from(sample) + (coeff * s_prev) - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = (s_prev2 * s_prev2) + (s_prev * s_prev) - (coeff * s_prev * s_prev2);
    if power <= 0.0 {
        0.0
    } else {
        (1.0 + power).ln()
    }
}

fn quantize_level(normalized: f32) -> u8 {
    let clamped = normalized.clamp(0.0, 1.0);
    let curved = (clamped as f64).sqrt();
    (curved * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_from_sine_wav() {
        let dir = std::env::temp_dir().join(format!(
            "tz_spec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..22_050 {
            let t = i as f32 / 22_050.0;
            let s = (0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 20000.0) as i16;
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();

        let result = analyze_track_spectrum(&path, 24, 40).unwrap();
        assert!(!result.frames.is_empty());
        assert_eq!(result.frames[0].1.len(), 24);
        // some energy present
        assert!(result.frames.iter().any(|(_, b)| b.iter().any(|v| *v > 10)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
