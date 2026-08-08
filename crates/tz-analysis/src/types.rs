//! Shared analysis data types.

/// Decoded PCM suitable for spectrum / beat / waveform analysis.
#[derive(Debug, Clone)]
pub struct DecodedAnalysisAudio {
    pub duration_ms: u64,
    pub mono_rate: u32,
    pub mono_samples: Vec<f32>,
    pub stereo_rate: u32,
    pub left_samples: Vec<f32>,
    pub right_samples: Vec<f32>,
}

/// Single stereo level sample in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelReading {
    pub left: f32,
    pub right: f32,
    pub position_ms: u64,
}

/// Bucketed envelope points: (position_ms, left, right).
#[derive(Debug, Clone)]
pub struct EnvelopeAnalysisResult {
    pub duration_ms: u64,
    pub points: Vec<(u64, f32, f32)>,
}

/// Analysis-path errors (never fatal to VLC playback).
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("file not found: {0}")]
    NotFound(String),

    #[error("decode failed: {0}")]
    Decode(String),

    #[error("FFmpeg unavailable")]
    FfmpegUnavailable,

    #[error("unsupported format for analysis: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Message(String),
}
