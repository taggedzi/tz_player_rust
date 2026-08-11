#![cfg(feature = "client")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tz_audio::{helper, PcmSource, PcmSpec};

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "tz-audio-helper-{name}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn fake_helper(directory: &Path, mode: &str) -> PathBuf {
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let target = directory.join(format!("{mode}{extension}"));
    std::fs::copy(env!("CARGO_BIN_EXE_tz-audio-fake-helper"), &target).unwrap();
    target
}

fn input(directory: &Path) -> PathBuf {
    let path = directory.join("input with spaces Ω.bin");
    std::fs::write(&path, b"fake media").unwrap();
    path
}

#[test]
fn valid_helper_handshakes_streams_seeks_and_reaps() {
    let directory = temp_dir("valid");
    let executable = fake_helper(&directory, "record-pid");
    let input = input(&directory);
    let config = helper::HelperConfig::injected(executable).unwrap();
    let capabilities = helper::capabilities(&config).unwrap();
    assert_eq!(capabilities.protocol_major, tz_audio::PROTOCOL_MAJOR);
    assert_eq!(capabilities.library_majors["avcodec"], 61);

    let spec = PcmSpec::new(48_000, 2).unwrap();
    let mut source = helper::decode(&config, &input, 0, spec).unwrap();
    let mut samples = [0.0; 512];
    let count = source.read_interleaved(&mut samples).unwrap();
    assert!(count > 0 && count <= samples.len() && count.is_multiple_of(2));
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let first_pid = std::fs::read_to_string(input.with_extension("pid")).unwrap();
    source.seek_to_frame(4_800).unwrap();
    assert_process_id_is_gone(first_pid.trim());
    let second_pid = std::fs::read_to_string(input.with_extension("pid")).unwrap();
    assert_ne!(first_pid.trim(), second_pid.trim());
    let count = source.read_interleaved(&mut samples).unwrap();
    assert!(count > 0 && count <= samples.len() && count.is_multiple_of(2));
    drop(source);
    assert!(input.with_extension("pid").is_file());
    assert_process_is_gone(&input.with_extension("pid"));
    std::fs::remove_dir_all(directory).unwrap();
}

fn assert_process_is_gone(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    assert_process_id_is_gone(pid.trim());
}

fn assert_process_id_is_gone(pid: &str) {
    #[cfg(unix)]
    assert!(!Path::new("/proc").join(pid).exists());
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(!listing.contains(&format!(",\"{pid}\"")));
    }
}

#[test]
fn successful_capability_handshake_is_cached_for_decode_and_seek() {
    let directory = temp_dir("capability-cache");
    let executable = fake_helper(&directory, "count-capabilities");
    let counter = executable.with_extension("count");
    let input = input(&directory);
    let config = helper::HelperConfig::injected(executable).unwrap();
    let spec = PcmSpec::new(48_000, 2).unwrap();
    let mut first = helper::decode(&config, &input, 0, spec).unwrap();
    first.seek_to_frame(4_800).unwrap();
    drop(first);
    drop(helper::decode(&config, &input, 0, spec).unwrap());
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "1");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn incompatible_and_wrong_build_capabilities_fail_closed() {
    for mode in ["incompatible", "wrong-build"] {
        let directory = temp_dir(mode);
        let config = helper::HelperConfig::injected(fake_helper(&directory, mode)).unwrap();
        assert!(helper::capabilities(&config).is_err(), "{mode}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn malformed_headers_and_invalid_pcm_are_rejected() {
    for mode in [
        "oversized-header",
        "truncated-header",
        "mismatched-header",
        "invalid-sample",
        "partial-frame",
        "stderr-flood",
    ] {
        let directory = temp_dir(mode);
        let executable = fake_helper(&directory, mode);
        let input = input(&directory);
        let config = helper::HelperConfig::injected(executable).unwrap();
        let spec = PcmSpec::new(48_000, 2).unwrap();
        match helper::decode(&config, &input, 0, spec) {
            Err(_) => {}
            Ok(mut source) => {
                let mut samples = [0.0; 8];
                assert!(source.read_interleaved(&mut samples).is_err(), "{mode}");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn failures_before_and_after_header_are_bounded_and_terminal_safe() {
    for mode in ["exit-before-header", "exit-after-header"] {
        let directory = temp_dir(mode);
        let executable = fake_helper(&directory, mode);
        let input = input(&directory);
        let config = helper::HelperConfig::injected(executable).unwrap();
        let spec = PcmSpec::new(48_000, 2).unwrap();
        let error = match helper::decode(&config, &input, 0, spec) {
            Err(error) => error,
            Ok(mut source) => {
                let mut samples = [0.0; 8];
                loop {
                    match source.read_interleaved(&mut samples) {
                        Err(error) => break error,
                        Ok(0) => panic!("{mode} was reported as a clean EOF"),
                        Ok(_) => {}
                    }
                }
            }
        };
        let message = error.to_string();
        assert!(message.len() <= 64 * 1024, "{mode}");
        assert!(!message.contains('\x1b'), "{mode}: {message:?}");
        assert!(!message.contains('\r'), "{mode}: {message:?}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn startup_timeout_and_relative_injection_are_rejected() {
    assert!(helper::HelperConfig::injected(PathBuf::from("relative-helper")).is_err());
    let directory = temp_dir("slow-header");
    let executable = fake_helper(&directory, "slow-header");
    let input = input(&directory);
    let mut config = helper::HelperConfig::injected(executable).unwrap();
    config.startup_timeout = Duration::from_millis(50);
    let error = match helper::decode(&config, &input, 0, PcmSpec::new(48_000, 2).unwrap()) {
        Ok(_) => panic!("slow helper unexpectedly started"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("timed out"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn capability_timeout_and_pcm_stall_fail_promptly() {
    let directory = temp_dir("timeouts");
    let mut config =
        helper::HelperConfig::injected(fake_helper(&directory, "slow-capabilities")).unwrap();
    config.startup_timeout = Duration::from_millis(50);
    assert!(helper::capabilities(&config)
        .unwrap_err()
        .to_string()
        .contains("timed out"));

    let config_path = fake_helper(&directory, "stall");
    let input = input(&directory);
    let mut config = helper::HelperConfig::injected(config_path).unwrap();
    config.pcm_stall_timeout = Duration::from_millis(50);
    config.stop_grace = Duration::from_millis(50);
    let mut source = helper::decode(&config, &input, 0, PcmSpec::new(48_000, 2).unwrap()).unwrap();
    let mut frame = [0.0; 2];
    let started = std::time::Instant::now();
    loop {
        match source.try_read_for_playback(&mut frame) {
            Err(error) => {
                assert!(error.to_string().contains("stalled"));
                break;
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(started.elapsed() < Duration::from_secs(1));
    std::fs::remove_dir_all(directory).unwrap();
}
