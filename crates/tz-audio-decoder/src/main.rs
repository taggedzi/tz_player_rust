//! Narrow process boundary for the packaged audio decoder.
//!
//! The release build links this binary to the pinned FFmpeg SDK. Keeping the
//! command parser here independent from the parent client makes malformed
//! requests fail before any media bytes are touched.

use std::path::PathBuf;
use std::process::ExitCode;

use tz_audio::{Capabilities, ExitCode as HelperExitCode, PROTOCOL_MAJOR, PROTOCOL_MINOR};

#[cfg(feature = "ffmpeg-native")]
mod avio;
#[cfg(feature = "ffmpeg-native")]
mod ffi;
#[cfg(feature = "ffmpeg-native")]
mod native_decode;

fn main() -> ExitCode {
    match std::panic::catch_unwind(run) {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err((HelperExitCode::Ok, _))) => ExitCode::SUCCESS,
        Ok(Err((code, message))) => {
            eprintln!(
                "{}",
                tz_audio::protocol::sanitize_diagnostic(&message, 64 * 1024)
            );
            ExitCode::from(code as u8)
        }
        Err(_) => ExitCode::from(HelperExitCode::Internal as u8),
    }
}

fn run() -> Result<(), (HelperExitCode, String)> {
    verify_library_compatibility()?;
    let mut args = std::env::args_os();
    let _program = args.next();
    match args.next().as_deref() {
        Some(value) if value == "capabilities" => {
            if args.next().as_deref() != Some(std::ffi::OsStr::new("--json")) || args.next().is_some() {
                return Err((HelperExitCode::InvalidArguments, "usage: capabilities --json".into()));
            }
            let capabilities = Capabilities {
                helper_version: env!("CARGO_PKG_VERSION").into(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: PROTOCOL_MINOR,
                ffmpeg_version: ffmpeg_version(),
                ffmpeg_commit: env!("TZ_FFMPEG_COMMIT").into(),
                configuration_hash: configuration_hash(),
                library_majors: library_majors(),
                demuxers: manifest_components(env!("TZ_FFMPEG_DEMUXERS")),
                decoders: manifest_components(env!("TZ_FFMPEG_DECODERS")),
            };
            serde_json::to_writer(std::io::stdout(), &capabilities).map_err(|e| (HelperExitCode::ProtocolIo, e.to_string()))?;
            Ok(())
        }
        Some(value) if value == "decode" => decode(args),
        _ => Err((HelperExitCode::InvalidArguments, "usage: capabilities --json | decode --input <PATH> --start-ms <U64> --sample-rate <U32> --channels 2 --format f32le".into())),
    }
}

fn verify_library_compatibility() -> Result<(), (HelperExitCode, String)> {
    #[cfg(feature = "ffmpeg-native")]
    {
        let expected = env!("TZ_FFMPEG_EXPECTED_VERSION");
        let actual = ffi::version();
        if actual != expected {
            return Err((
                HelperExitCode::LibraryCompatibility,
                format!("packaged FFmpeg version mismatch: expected {expected}, loaded {actual}"),
            ));
        }
        let configuration = ffi::configuration();
        for forbidden in [
            "--enable-gpl",
            "--enable-nonfree",
            "--enable-network",
            "--enable-protocols",
        ] {
            if configuration.contains(forbidden) {
                return Err((
                    HelperExitCode::LibraryCompatibility,
                    format!("packaged FFmpeg has forbidden configuration: {forbidden}"),
                ));
            }
        }
    }
    Ok(())
}

fn manifest_components(value: &str) -> Vec<String> {
    value.split(',').map(str::to_owned).collect()
}

fn ffmpeg_version() -> String {
    #[cfg(feature = "ffmpeg-native")]
    {
        ffi::version()
    }
    #[cfg(not(feature = "ffmpeg-native"))]
    {
        option_env!("TZ_FFMPEG_VERSION")
            .unwrap_or("unconfigured")
            .into()
    }
}

fn configuration_hash() -> String {
    #[cfg(feature = "ffmpeg-native")]
    {
        let configuration = ffi::configuration();
        format!("{:x}", fnv1a(configuration.as_bytes()))
    }
    #[cfg(not(feature = "ffmpeg-native"))]
    {
        option_env!("TZ_FFMPEG_CONFIGURATION_HASH")
            .unwrap_or("unconfigured")
            .into()
    }
}

fn library_majors() -> std::collections::BTreeMap<String, u32> {
    #[cfg(feature = "ffmpeg-native")]
    {
        ffi::library_majors()
    }
    #[cfg(not(feature = "ffmpeg-native"))]
    {
        std::collections::BTreeMap::new()
    }
}

#[cfg(feature = "ffmpeg-native")]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn decode(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), (HelperExitCode, String)> {
    let mut input: Option<PathBuf> = None;
    let mut start_ms = None;
    let mut sample_rate = None;
    let mut channels = None;
    let mut format = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or((
            HelperExitCode::InvalidArguments,
            "missing decode argument".into(),
        ))?;
        match flag.to_string_lossy().as_ref() {
            "--input" => input = Some(PathBuf::from(value)),
            "--start-ms" => start_ms = value.to_string_lossy().parse::<u64>().ok(),
            "--sample-rate" => sample_rate = value.to_string_lossy().parse::<u32>().ok(),
            "--channels" => channels = value.to_string_lossy().parse::<u16>().ok(),
            "--format" => format = Some(value),
            _ => {
                return Err((
                    HelperExitCode::InvalidArguments,
                    "unknown decode argument".into(),
                ))
            }
        }
    }
    let input = input.ok_or((
        HelperExitCode::InvalidArguments,
        "--input is required".into(),
    ))?;
    let sample_rate = sample_rate.ok_or((
        HelperExitCode::InvalidArguments,
        "--sample-rate is invalid".into(),
    ))?;
    if channels != Some(2)
        || format.as_deref() != Some(std::ffi::OsStr::new("f32le"))
        || start_ms.is_none()
        || sample_rate == 0
    {
        return Err((
            HelperExitCode::InvalidArguments,
            "decode requires stereo f32le and a valid start/rate".into(),
        ));
    }
    #[cfg(feature = "ffmpeg-native")]
    {
        native_decode::decode(&input, start_ms.unwrap(), sample_rate, channels.unwrap())
    }
    #[cfg(not(feature = "ffmpeg-native"))]
    {
        let _ = input;
        Err((
            HelperExitCode::UnsupportedMedia,
            "FFmpeg decoder libraries are not configured in this development build".into(),
        ))
    }
}
