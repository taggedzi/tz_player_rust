use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use tz_audio::{
    write_decode_header, Capabilities, DecodeHeader, SampleFormat, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};

const MANIFEST: &str = include_str!("../../../native/ffmpeg/manifest.toml");

fn main() {
    let mode = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "valid".into());
    let mut args = std::env::args_os();
    let _ = args.next();
    match args.next().as_deref() {
        Some(value) if value == "capabilities" => capabilities(&mode),
        Some(value) if value == "decode" => decode(&mode, args),
        _ => std::process::exit(2),
    }
}

fn capabilities(mode: &str) {
    if mode.contains("count-capabilities") {
        let counter = std::env::current_exe().unwrap().with_extension("count");
        let count = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        std::fs::write(counter, count.to_string()).unwrap();
    }
    if mode.contains("slow-capabilities") {
        std::thread::sleep(Duration::from_millis(250));
    }
    let mut capabilities = Capabilities {
        helper_version: env!("CARGO_PKG_VERSION").into(),
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        ffmpeg_version: manifest_value("version"),
        ffmpeg_commit: manifest_value("ffmpeg_release_commit"),
        configuration_hash: "abc123".into(),
        library_majors: manifest_library_majors(),
        demuxers: manifest_array("demuxers"),
        decoders: manifest_array("decoders"),
    };
    if mode.contains("incompatible") {
        capabilities.protocol_major += 1;
    }
    if mode.contains("wrong-build") {
        capabilities.ffmpeg_version = "0.0.0".into();
    }
    serde_json::to_writer(std::io::stdout(), &capabilities).unwrap();
}

fn decode(mode: &str, mut args: impl Iterator<Item = std::ffi::OsString>) {
    let mut input = None;
    let mut start_ms = 0_u64;
    let mut sample_rate = 48_000_u32;
    while let Some(flag) = args.next() {
        let Some(value) = args.next() else {
            std::process::exit(2);
        };
        match flag.to_string_lossy().as_ref() {
            "--input" => input = Some(PathBuf::from(value)),
            "--start-ms" => start_ms = value.to_string_lossy().parse().unwrap(),
            "--sample-rate" => sample_rate = value.to_string_lossy().parse().unwrap(),
            "--channels" | "--format" => {}
            _ => std::process::exit(2),
        }
    }
    let input = input.unwrap();
    if mode.contains("record-pid") {
        std::fs::write(input.with_extension("pid"), std::process::id().to_string()).unwrap();
    }
    if mode.contains("slow-header") {
        std::thread::sleep(Duration::from_millis(250));
    }
    if mode.contains("exit-before-header") {
        eprintln!("\x1b]8;;https://example.invalid\x07unsafe link\x1b]8;;\x07\nsecond line");
        std::process::exit(5);
    }
    if mode.contains("oversized-header") {
        std::io::stdout()
            .write_all(&((tz_audio::MAX_HEADER_BYTES as u32) + 1).to_le_bytes())
            .unwrap();
        return;
    }
    if mode.contains("truncated-header") {
        std::io::stdout().write_all(&100_u32.to_le_bytes()).unwrap();
        return;
    }
    let start_frame = start_ms.saturating_mul(u64::from(sample_rate)) / 1_000;
    let header = DecodeHeader {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        sample_format: SampleFormat::F32le,
        sample_rate: if mode.contains("mismatched-header") {
            sample_rate + 1
        } else {
            sample_rate
        },
        channels: 2,
        duration_frames: Some(u64::from(sample_rate)),
        start_frame,
    };
    write_decode_header(&mut std::io::stdout(), &header).unwrap();
    std::io::stdout().flush().unwrap();
    if mode.contains("exit-after-header") {
        eprintln!("\x1b[31mdecoder crash\x1b[0m\rterminal payload");
        std::process::exit(6);
    }
    if mode.contains("stall") {
        std::thread::sleep(Duration::from_secs(5));
    }
    if mode.contains("stderr-flood") {
        std::io::stderr()
            .write_all(&vec![b'x'; 128 * 1024])
            .unwrap();
        std::process::exit(6);
    }
    if mode.contains("invalid-sample") {
        std::io::stdout()
            .write_all(&f32::NAN.to_le_bytes())
            .unwrap();
        std::io::stdout().write_all(&0_f32.to_le_bytes()).unwrap();
        return;
    }
    if mode.contains("partial-frame") {
        std::io::stdout()
            .write_all(&0.25_f32.to_le_bytes())
            .unwrap();
        return;
    }
    let remaining = u64::from(sample_rate).saturating_sub(start_frame);
    let frame = [0.25_f32.to_le_bytes(), (-0.5_f32).to_le_bytes()].concat();
    let mut stdout = std::io::stdout().lock();
    for _ in 0..remaining {
        if stdout.write_all(&frame).is_err() {
            return;
        }
    }
}

fn manifest_value(name: &str) -> String {
    let prefix = format!("{name} = \"");
    MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'))
        .unwrap()
        .to_owned()
}

fn manifest_array(name: &str) -> Vec<String> {
    let prefix = format!("{name} = [");
    let mut values = MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix(']'))
        .unwrap()
        .split(',')
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn manifest_library_majors() -> BTreeMap<String, u32> {
    MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix("library_majors = {")?.strip_suffix('}'))
        .unwrap()
        .split(',')
        .map(|entry| {
            let (name, major) = entry.split_once('=').unwrap();
            (name.trim().into(), major.trim().parse().unwrap())
        })
        .collect()
}
