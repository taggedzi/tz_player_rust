//! Offline audio analysis for visualizers.
//!
//! **Not the listen path.** Playback uses VLC (`tz-playback`).

mod beat;
mod decode;
mod envelope;
mod spectrum;
mod types;
mod waveform;

pub use beat::{
    analyze_beats_from_decoded, analyze_beats_from_mono, analyze_track_beats, BeatAnalysisResult,
};
pub use decode::{
    decode_ffmpeg_raw_stereo, decode_track_for_analysis, decode_wave_raw, ffmpeg_available,
    AnalysisDecoder, FfmpegCliDecoder, WavNativeDecoder, MONO_TARGET_RATE, STEREO_TARGET_RATE,
};
pub use envelope::{analyze_track_envelope, analyze_track_envelope_default};
pub use spectrum::{
    analyze_spectrum_from_decoded, analyze_spectrum_from_mono, analyze_track_spectrum,
    SpectrumAnalysisResult,
};
pub use types::{AnalysisError, DecodedAnalysisAudio, EnvelopeAnalysisResult, LevelReading};
pub use waveform::{
    analyze_track_waveform_proxy, analyze_waveform_proxy_from_decoded,
    analyze_waveform_proxy_from_stereo, WaveformProxyAnalysisResult,
};

/// High-level analysis capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisKind {
    ScalarEnvelope,
    Spectrum,
    Beat,
    WaveformProxy,
}
