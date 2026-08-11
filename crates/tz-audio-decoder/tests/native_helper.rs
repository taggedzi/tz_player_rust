#![cfg(feature = "ffmpeg-native")]

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use tz_audio::{read_decode_header, Capabilities, DecodeHeader, DecodeRequest, SampleFormat};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BYTES_PER_FRAME: usize = 2 * size_of::<f32>();
const FORMATS: [&str; 20] = [
    "tone-aac.m4a",
    "tone-alac.m4a",
    "tone-opus.ogg",
    "tone.flac",
    "tone.mp3",
    "tone.ogg",
    "tone.wav",
    "tone.aiff",
    "tone.caf",
    "tone.mka",
    "tone-wma.wma",
    "tone-ape.ape",
    "tone-wavpack.wv",
    "tone-ac3.ac3",
    "tone-eac3.eac3",
    "tone-dts.dts",
    "tone-musepack7.mpc",
    "tone-musepack8.mpc",
    "tone-tta.tta",
    "tone-speex.ogg",
];

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tz-audio-decoder"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tz-playback/tests/fixtures")
        .join(name)
}

fn decode(name: &str, start_ms: u64) -> (DecodeHeader, Vec<u8>) {
    decode_path(&fixture(name), name, start_ms)
}

fn decode_path(path: &Path, label: &str, start_ms: u64) -> (DecodeHeader, Vec<u8>) {
    let output = Command::new(helper())
        .args(["decode", "--input"])
        .arg(path)
        .args([
            "--start-ms",
            &start_ms.to_string(),
            "--sample-rate",
            &SAMPLE_RATE.to_string(),
            "--channels",
            &CHANNELS.to_string(),
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert!(
        output.status.success(),
        "{label} failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let request = DecodeRequest {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        format: SampleFormat::F32le,
        start_frame: start_ms * u64::from(SAMPLE_RATE) / 1_000,
    };
    let mut bytes = Cursor::new(output.stdout);
    let header = read_decode_header(&mut bytes, &request).expect("valid decode header");
    let offset = usize::try_from(bytes.position()).expect("header offset fits usize");
    let output = bytes.into_inner();
    let pcm = output[offset..].to_vec();
    assert_eq!(pcm.len() % BYTES_PER_FRAME, 0, "complete PCM frames");
    assert!(!pcm.is_empty(), "helper returned PCM");
    assert!(pcm.chunks_exact(4).all(|sample| {
        f32::from_le_bytes(sample.try_into().expect("one f32 sample")).is_finite()
    }));
    (header, pcm)
}

#[test]
fn decodes_local_paths_with_spaces_and_unicode() {
    let directory = std::env::temp_dir().join(format!(
        "tz audio AVIO Ω {} {}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("töne with spaces.wav");
    std::fs::copy(fixture("tone.wav"), &path).unwrap();

    let (header, pcm) = decode_path(&path, "Unicode path", 100);
    assert_eq!(header.start_frame, 4_410);
    assert!(!pcm.is_empty());

    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn decodes_committed_format_matrix() {
    let mut duration_mismatches = Vec::new();
    for name in FORMATS {
        let (header, pcm) = decode(name, 0);
        assert_eq!(header.start_frame, 0, "{name}");
        let decoded_frames = pcm.len() / BYTES_PER_FRAME;
        assert!(decoded_frames > 40_000, "{name}");
        if let Some(duration_frames) = header.duration_frames {
            let tolerance = u64::from(SAMPLE_RATE) / 40;
            if duration_frames.abs_diff(decoded_frames as u64) > tolerance {
                duration_mismatches.push(format!(
                    "{name}: advertised {duration_frames} frames but decoded {decoded_frames}"
                ));
            }
        }
    }
    assert!(
        duration_mismatches.is_empty(),
        "duration mismatches:\n{}",
        duration_mismatches.join("\n")
    );
}

#[test]
fn seek_discards_leading_pcm_at_the_requested_output_frame() {
    let requested_start = 4_410;
    for name in FORMATS {
        let (_, full) = decode(name, 0);
        let (header, tail) = decode(name, 100);
        assert_eq!(header.start_frame, requested_start, "{name}");
        let discarded = full.len() / BYTES_PER_FRAME - tail.len() / BYTES_PER_FRAME;
        assert!(
            discarded.abs_diff(requested_start as usize) <= 1,
            "{name}: discarded {discarded} frames instead of {requested_start}"
        );
    }
}

#[cfg(unix)]
#[test]
fn decodes_a_non_utf8_local_path() {
    let directory = std::env::temp_dir().join(format!(
        "tz-audio-non-utf8-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(OsString::from_vec(b"tone-\xff.wav".to_vec()));
    std::fs::copy(fixture("tone.wav"), &path).unwrap();

    let output = Command::new(helper())
        .arg("decode")
        .arg("--input")
        .arg(&path)
        .args([
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = DecodeRequest {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        format: SampleFormat::F32le,
        start_frame: 0,
    };
    let mut bytes = Cursor::new(output.stdout);
    read_decode_header(&mut bytes, &request).expect("valid non-UTF-8 decode header");
    assert!(bytes.into_inner().len() > 40_000 * BYTES_PER_FRAME);

    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn capabilities_report_the_audited_native_build() {
    let output = Command::new(helper())
        .args(["capabilities", "--json"])
        .output()
        .expect("helper starts");
    assert!(output.status.success());
    let capabilities: Capabilities =
        serde_json::from_slice(&output.stdout).expect("capabilities JSON");
    assert_eq!(capabilities.ffmpeg_version, "7.1.5");
    assert_eq!(capabilities.ffmpeg_commit, "7.1.5");
    assert!(!capabilities.configuration_hash.is_empty());
    assert!(capabilities.demuxers.iter().any(|name| name == "ape"));
    assert!(capabilities.decoders.iter().any(|name| name == "speex"));
    assert!(!capabilities.demuxers.iter().any(|name| name == "http"));
}

#[test]
fn missing_corrupt_and_url_inputs_fail_without_unbounded_diagnostics() {
    let missing = Command::new(helper())
        .args([
            "decode",
            "--input",
            "this-file-does-not-exist.wav",
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert_eq!(missing.status.code(), Some(3));
    assert!(missing.stderr.len() <= 64 * 1024);

    let corrupt = Command::new(helper())
        .args([
            "decode",
            "--input",
            fixture("corrupt.bin")
                .to_str()
                .expect("fixture path is UTF-8"),
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert_eq!(corrupt.status.code(), Some(4));
    assert!(corrupt.stderr.len() <= 64 * 1024);

    let url = Command::new(helper())
        .args([
            "decode",
            "--input",
            "https://example.invalid/audio.mp3",
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert_eq!(url.status.code(), Some(3));

    let directory = std::env::temp_dir().join(format!("tz-audio-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let empty_path = directory.join("empty.wav");
    std::fs::write(&empty_path, []).unwrap();
    let truncated_path = directory.join("truncated.wav");
    let wav = std::fs::read(fixture("tone.wav")).unwrap();
    std::fs::write(&truncated_path, &wav[..32]).unwrap();
    for path in [&empty_path, &truncated_path] {
        let output = Command::new(helper())
            .args(["decode", "--input"])
            .arg(path)
            .args([
                "--start-ms",
                "0",
                "--sample-rate",
                "44100",
                "--channels",
                "2",
                "--format",
                "f32le",
            ])
            .output()
            .expect("helper starts");
        assert_eq!(output.status.code(), Some(4), "{}", path.display());
        assert!(output.stderr.len() <= 64 * 1024);
    }
    std::fs::remove_file(empty_path).unwrap();
    std::fs::remove_file(truncated_path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn no_audio_and_large_metadata_inputs_are_bounded() {
    let no_audio = Command::new(helper())
        .args(["decode", "--input"])
        .arg(fixture("tone-video-only.mp4"))
        .args([
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    assert_eq!(no_audio.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&no_audio.stderr).contains("no audio stream"));
    assert!(no_audio.stderr.len() <= 64 * 1024);

    let directory =
        std::env::temp_dir().join(format!("tz-audio-large-metadata-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("large-id3.mp3");
    let metadata_size = 1024 * 1024_u32;
    let mut media = Vec::with_capacity(metadata_size as usize + 10);
    media.extend_from_slice(b"ID3\x04\x00\x00");
    media.extend_from_slice(&[
        ((metadata_size >> 21) & 0x7f) as u8,
        ((metadata_size >> 14) & 0x7f) as u8,
        ((metadata_size >> 7) & 0x7f) as u8,
        (metadata_size & 0x7f) as u8,
    ]);
    media.resize(10 + metadata_size as usize, 0);
    media.extend_from_slice(&std::fs::read(fixture("tone.mp3")).unwrap());
    std::fs::write(&path, media).unwrap();
    let (_, pcm) = decode_path(&path, "large ID3 metadata", 0);
    assert!(!pcm.is_empty());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn unreadable_local_input_fails_as_an_input_error() {
    use std::os::unix::fs::PermissionsExt;

    let directory =
        std::env::temp_dir().join(format!("tz-audio-unreadable-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("unreadable.wav");
    std::fs::copy(fixture("tone.wav"), &path).unwrap();
    let original = std::fs::metadata(&path).unwrap().permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
    let output = Command::new(helper())
        .args(["decode", "--input"])
        .arg(&path)
        .args([
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .output()
        .expect("helper starts");
    std::fs::set_permissions(&path, original).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.len() <= 64 * 1024);
}

#[test]
fn closing_stdout_after_the_header_is_clean_cancellation() {
    let mut child = Command::new(helper())
        .args([
            "decode",
            "--input",
            fixture("tone.wav").to_str().expect("fixture path is UTF-8"),
            "--start-ms",
            "0",
            "--sample-rate",
            "44100",
            "--channels",
            "2",
            "--format",
            "f32le",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("helper starts");
    let request = DecodeRequest {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        format: SampleFormat::F32le,
        start_frame: 0,
    };
    let mut stdout = child.stdout.take().expect("helper stdout");
    read_decode_header(&mut stdout, &request).expect("valid decode header");
    drop(stdout);
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().expect("helper status") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("helper did not stop after its parent closed stdout");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(status.success(), "cancellation exit was {status}");
}
