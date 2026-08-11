//! Shared audio primitives used by playback, analysis, and the bundled helper.
//!
//! The default features provide native Symphonia decoding and the parent-side
//! helper client. The helper binary disables both and depends only on the
//! bounded wire types in this crate.

mod pcm;
pub mod protocol;

#[cfg(feature = "client")]
pub mod discovery;
#[cfg(feature = "client")]
pub mod helper;
#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "client")]
pub use helper::{HelperConfig, HelperPcmSource};
pub use pcm::{
    clamp_sample, duration_to_frames, frames_to_duration, DecodeError, PcmSource, PcmSpec,
};
pub use protocol::sanitize_diagnostic;
pub use protocol::{
    read_decode_header, write_decode_header, Capabilities, DecodeHeader, DecodeRequest, ExitCode,
    ProtocolError, SampleFormat, MAX_HEADER_BYTES, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};

#[cfg(feature = "native")]
pub use native::{decode_native, probe_helper_only_content, NativePcmSource};

/// The standardized analysis output requested by `tz-analysis`.
#[cfg(feature = "native")]
pub fn decode_analysis(path: &std::path::Path) -> Result<NativePcmSource, DecodeError> {
    decode_native(path, PcmSpec::new(44_100, 2)?)
}
