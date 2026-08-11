use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PcmSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

impl PcmSpec {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, DecodeError> {
        if sample_rate == 0 || !(1..=32).contains(&channels) {
            return Err(DecodeError::InvalidPcmSpec);
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }

    pub fn frame_samples(self) -> usize {
        usize::from(self.channels)
    }
}

pub trait PcmSource: Send {
    fn spec(&self) -> PcmSpec;
    fn duration_frames(&self) -> Option<u64>;
    /// Reads whole or partial interleaved frames. `Ok(0)` is EOF.
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, DecodeError>;
    fn seek_to_frame(&mut self, frame: u64) -> Result<(), DecodeError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("invalid PCM specification")]
    InvalidPcmSpec,
    #[error("invalid PCM sample")]
    InvalidSample,
    #[error("PCM buffer is not frame-aligned")]
    UnalignedBuffer,
    #[error("decode failed: {0}")]
    Message(String),
}

pub fn clamp_sample(value: f32) -> Result<f32, DecodeError> {
    if !value.is_finite() {
        return Err(DecodeError::InvalidSample);
    }
    Ok(value.clamp(-1.0, 1.0))
}

pub fn duration_to_frames(duration: Duration, sample_rate: u32) -> u64 {
    duration
        .as_nanos()
        .saturating_mul(u128::from(sample_rate))
        .checked_div(1_000_000_000)
        .unwrap_or(u128::MAX)
        .min(u128::from(u64::MAX)) as u64
}

pub fn frames_to_duration(frames: u64, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        return Duration::ZERO;
    }
    let nanos = u128::from(frames)
        .saturating_mul(1_000_000_000)
        .checked_div(u128::from(sample_rate))
        .unwrap_or(u128::MAX)
        .min(u128::from(u64::MAX));
    Duration::from_nanos(nanos as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_finite_and_clamped() {
        assert_eq!(clamp_sample(2.0).unwrap(), 1.0);
        assert_eq!(clamp_sample(-2.0).unwrap(), -1.0);
        assert!(clamp_sample(f32::NAN).is_err());
    }

    #[test]
    fn frame_time_conversion_is_bounded() {
        let frames = duration_to_frames(Duration::from_secs(2), 48_000);
        assert_eq!(frames, 96_000);
        assert_eq!(frames_to_duration(frames, 48_000), Duration::from_secs(2));
    }
}
