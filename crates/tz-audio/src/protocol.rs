use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 1;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    F32le,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecodeRequest {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    pub start_frame: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecodeHeader {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub sample_format: SampleFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_frames: Option<u64>,
    pub start_frame: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    pub helper_version: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub ffmpeg_version: String,
    pub ffmpeg_commit: String,
    pub configuration_hash: String,
    pub library_majors: BTreeMap<String, u32>,
    pub demuxers: Vec<String>,
    pub decoders: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    Ok = 0,
    InvalidArguments = 2,
    Input = 3,
    UnsupportedMedia = 4,
    LibraryCompatibility = 5,
    Decode = 6,
    ProtocolIo = 7,
    Internal = 70,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("protocol header is truncated")]
    Truncated,
    #[error("protocol header exceeds {MAX_HEADER_BYTES} bytes")]
    Oversized,
    #[error("invalid protocol JSON: {0}")]
    Json(String),
    #[error("protocol major {0} is not supported")]
    MajorMismatch(u16),
    #[error("protocol header does not match the decode request: {0}")]
    RequestMismatch(String),
    #[error("invalid protocol header: {0}")]
    Invalid(String),
    #[error("protocol I/O: {0}")]
    Io(String),
}

pub fn write_decode_header<W: Write>(
    writer: &mut W,
    header: &DecodeHeader,
) -> Result<(), ProtocolError> {
    validate_header(header)?;
    let bytes = serde_json::to_vec(header).map_err(|e| ProtocolError::Json(e.to_string()))?;
    if bytes.len() > MAX_HEADER_BYTES {
        return Err(ProtocolError::Oversized);
    }
    let length = u32::try_from(bytes.len()).map_err(|_| ProtocolError::Oversized)?;
    writer.write_all(&length.to_le_bytes()).map_err(io_error)?;
    writer.write_all(&bytes).map_err(io_error)
}

pub fn read_decode_header<R: Read>(
    reader: &mut R,
    request: &DecodeRequest,
) -> Result<DecodeHeader, ProtocolError> {
    let mut length_bytes = [0; 4];
    reader
        .read_exact(&mut length_bytes)
        .map_err(|_| ProtocolError::Truncated)?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_HEADER_BYTES {
        return Err(ProtocolError::Oversized);
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| ProtocolError::Truncated)?;
    let header: DecodeHeader =
        serde_json::from_slice(&bytes).map_err(|e| ProtocolError::Json(e.to_string()))?;
    validate_header(&header)?;
    validate_request(&header, request)?;
    Ok(header)
}

fn validate_header(header: &DecodeHeader) -> Result<(), ProtocolError> {
    if header.protocol_major != PROTOCOL_MAJOR {
        return Err(ProtocolError::MajorMismatch(header.protocol_major));
    }
    if header.sample_rate == 0 || header.channels == 0 || header.channels > 32 {
        return Err(ProtocolError::Invalid(
            "invalid rate or channel count".into(),
        ));
    }
    if header
        .duration_frames
        .is_some_and(|duration| header.start_frame > duration)
    {
        return Err(ProtocolError::Invalid(
            "start frame exceeds duration".into(),
        ));
    }
    Ok(())
}

fn validate_request(header: &DecodeHeader, request: &DecodeRequest) -> Result<(), ProtocolError> {
    if header.sample_format != request.format {
        return Err(ProtocolError::RequestMismatch("sample format".into()));
    }
    if header.sample_rate != request.sample_rate {
        return Err(ProtocolError::RequestMismatch("sample rate".into()));
    }
    if header.channels != request.channels {
        return Err(ProtocolError::RequestMismatch("channel count".into()));
    }
    if header.start_frame > request.start_frame {
        return Err(ProtocolError::RequestMismatch("start frame".into()));
    }
    Ok(())
}

fn io_error(error: io::Error) -> ProtocolError {
    ProtocolError::Io(error.to_string())
}

pub fn sanitize_diagnostic(input: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    let mut escape = 0u8;
    for ch in input.chars() {
        if escape != 0 {
            if escape == 1 && ch == ']' {
                escape = 2;
            } else if ch == '\u{7}' || (escape == 1 && ch.is_ascii_alphabetic()) {
                escape = 0;
            }
            continue;
        }
        if ch == '\u{1b}' {
            escape = 1;
            continue;
        }
        let safe = if ch.is_control() { ' ' } else { ch };
        let next = output.len() + safe.len_utf8();
        if next > max_bytes {
            break;
        }
        output.push(safe);
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(4 + bytes.len());
        output.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
        output.extend_from_slice(bytes);
        output
    }

    fn framed_header(header: &DecodeHeader) -> Vec<u8> {
        framed(&serde_json::to_vec(header).unwrap())
    }

    fn request() -> DecodeRequest {
        DecodeRequest {
            sample_rate: 48_000,
            channels: 2,
            format: SampleFormat::F32le,
            start_frame: 0,
        }
    }
    fn header() -> DecodeHeader {
        DecodeHeader {
            protocol_major: 1,
            protocol_minor: 99,
            sample_format: SampleFormat::F32le,
            sample_rate: 48_000,
            channels: 2,
            duration_frames: None,
            start_frame: 0,
        }
    }

    #[test]
    fn round_trip_accepts_unknown_minor_fields() {
        let mut value = serde_json::to_value(header()).unwrap();
        value["future_minor_field"] = serde_json::json!({ "ignored": true });
        let bytes = framed(&serde_json::to_vec(&value).unwrap());
        let parsed = read_decode_header(&mut bytes.as_slice(), &request()).unwrap();
        assert_eq!(parsed.protocol_minor, 99);
    }

    #[test]
    fn rejects_major_mismatch() {
        let mut value = header();
        value.protocol_major = PROTOCOL_MAJOR + 1;
        assert!(matches!(
            read_decode_header(&mut framed_header(&value).as_slice(), &request()),
            Err(ProtocolError::MajorMismatch(2))
        ));
        assert!(matches!(
            write_decode_header(&mut Vec::new(), &value),
            Err(ProtocolError::MajorMismatch(2))
        ));
    }

    #[test]
    fn malformed_lengths_are_bounded() {
        let bytes = (MAX_HEADER_BYTES as u32 + 1).to_le_bytes();
        assert!(matches!(
            read_decode_header(&mut bytes.as_slice(), &request()),
            Err(ProtocolError::Oversized)
        ));
    }

    #[test]
    fn truncated_and_invalid_json_are_rejected() {
        assert!(matches!(
            read_decode_header(&mut [1, 2, 3].as_slice(), &request()),
            Err(ProtocolError::Truncated)
        ));

        let payload = br#"{"protocol_major":1}"#;
        let mut truncated = u32::try_from(payload.len() + 1)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        truncated.extend_from_slice(payload);
        assert!(matches!(
            read_decode_header(&mut truncated.as_slice(), &request()),
            Err(ProtocolError::Truncated)
        ));

        let invalid = framed(br#"{"protocol_major": definitely-not-json}"#);
        assert!(matches!(
            read_decode_header(&mut invalid.as_slice(), &request()),
            Err(ProtocolError::Json(_))
        ));
    }

    #[test]
    fn invalid_rate_channels_format_and_duration_are_rejected() {
        for (sample_rate, channels) in [(0, 2), (48_000, 0), (48_000, 33)] {
            let mut value = header();
            value.sample_rate = sample_rate;
            value.channels = channels;
            assert!(matches!(
                read_decode_header(&mut framed_header(&value).as_slice(), &request()),
                Err(ProtocolError::Invalid(_))
            ));
        }

        let invalid_format = serde_json::json!({
            "protocol_major": 1,
            "protocol_minor": 0,
            "sample_format": "s16le",
            "sample_rate": 48_000,
            "channels": 2,
            "duration_frames": null,
            "start_frame": 0
        });
        let invalid_format = framed(&serde_json::to_vec(&invalid_format).unwrap());
        assert!(matches!(
            read_decode_header(&mut invalid_format.as_slice(), &request()),
            Err(ProtocolError::Json(_))
        ));

        let mut invalid_duration = header();
        invalid_duration.duration_frames = Some(10);
        invalid_duration.start_frame = 11;
        assert!(matches!(
            read_decode_header(&mut framed_header(&invalid_duration).as_slice(), &request()),
            Err(ProtocolError::Invalid(_))
        ));
    }

    #[test]
    fn request_and_header_mismatches_are_rejected() {
        let mut value = header();
        value.sample_rate = 44_100;
        assert!(matches!(
            read_decode_header(&mut framed_header(&value).as_slice(), &request()),
            Err(ProtocolError::RequestMismatch(_))
        ));

        let mut value = header();
        value.channels = 1;
        assert!(matches!(
            read_decode_header(&mut framed_header(&value).as_slice(), &request()),
            Err(ProtocolError::RequestMismatch(_))
        ));

        let mut value = header();
        value.start_frame = 1;
        assert!(matches!(
            read_decode_header(&mut framed_header(&value).as_slice(), &request()),
            Err(ProtocolError::RequestMismatch(_))
        ));
    }

    #[test]
    fn capabilities_include_version_and_configuration_identity() {
        let capabilities = Capabilities {
            helper_version: "1.2.3".into(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            ffmpeg_version: "7.1.5".into(),
            ffmpeg_commit: "7.1.5".into(),
            configuration_hash: "abc123".into(),
            library_majors: BTreeMap::from([
                ("avcodec".into(), 61),
                ("avformat".into(), 61),
                ("avutil".into(), 59),
                ("swresample".into(), 5),
            ]),
            demuxers: vec!["wav".into()],
            decoders: vec!["pcm_s16le".into()],
        };
        let value = serde_json::to_value(capabilities).unwrap();
        assert_eq!(value["helper_version"], "1.2.3");
        assert_eq!(value["ffmpeg_version"], "7.1.5");
        assert_eq!(value["ffmpeg_commit"], "7.1.5");
        assert_eq!(value["configuration_hash"], "abc123");
        assert_eq!(value["library_majors"]["avcodec"], 61);
    }

    #[test]
    fn diagnostics_are_unicode_safe() {
        let output = sanitize_diagnostic("ok\u{1b}[31m\u{1f600}", 6);
        assert!(!output.contains('\u{1b}'));
        assert!(output.starts_with("ok"));
    }
}
