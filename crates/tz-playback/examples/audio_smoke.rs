use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use tz_playback::{probe_audio_output, AudioPlaybackBackend, BackendStatus, PlaybackBackend};

const USAGE: &str = "Usage:\n  audio_smoke --startup-only\n  audio_smoke <local-media-file>";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Help,
    StartupOnly,
    Play(PathBuf),
}

fn parse_args(args: Vec<OsString>) -> Result<Mode, String> {
    match args.as_slice() {
        [argument] if argument == OsStr::new("--help") || argument == OsStr::new("-h") => {
            Ok(Mode::Help)
        }
        [argument] if argument == OsStr::new("--startup-only") => Ok(Mode::StartupOnly),
        [path] if !path.to_string_lossy().starts_with('-') => Ok(Mode::Play(PathBuf::from(path))),
        [] => Err(format!("a smoke mode is required\n{USAGE}")),
        _ => Err(format!("expected one mode or one media file\n{USAGE}")),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mode = match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{}", terminal_safe(error));
            return ExitCode::FAILURE;
        }
    };

    let result = match mode {
        Mode::Help => {
            println!("Rodio output and local-file smoke test\n\n{USAGE}");
            println!("\n--startup-only opens and closes the default output without playing audio.");
            println!("A media-file argument plays that file to completion; press Ctrl+C to stop.");
            Ok(())
        }
        Mode::StartupOnly => startup_only(),
        Mode::Play(path) => play_file(&path).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Rodio smoke failed: {}", terminal_safe(error));
            ExitCode::FAILURE
        }
    }
}

fn startup_only() -> Result<(), String> {
    let info = probe_audio_output()?;
    println!(
        "Rodio default output ready: {}",
        terminal_safe(info.to_string())
    );
    println!("No audio was played.");
    Ok(())
}

async fn play_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("media file does not exist: {}", safe_path(path)));
    }

    let mut backend = AudioPlaybackBackend::new();
    backend.start().await.map_err(|error| error.to_string())?;
    if let Some(info) = backend.output_info().await {
        println!("Rodio default output: {}", terminal_safe(info.to_string()));
    }
    println!("Playing: {}", safe_path(path));

    let playback_result = async {
        backend
            .play(1, path, 0, None)
            .await
            .map_err(|error| error.to_string())?;
        loop {
            let (position_ms, duration_ms, status) = backend
                .get_transport_snapshot()
                .await
                .map_err(|error| error.to_string())?;
            match status {
                BackendStatus::Stopped => {
                    println!(
                        "Completed at {position_ms} ms{}.",
                        if duration_ms > 0 {
                            format!(" / {duration_ms} ms")
                        } else {
                            String::new()
                        }
                    );
                    return Ok(());
                }
                BackendStatus::Error => return Err("backend entered the error state".into()),
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }
    .await;

    let shutdown_result = backend.shutdown().await.map_err(|error| error.to_string());
    playback_result.and(shutdown_result)
}

fn safe_path(path: &Path) -> String {
    terminal_safe(path.as_os_str().to_string_lossy())
}

fn terminal_safe(value: impl AsRef<str>) -> String {
    let mut output = String::with_capacity(value.as_ref().len());
    for character in value.as_ref().chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if matches!(character as u32, 0x00..=0x1F | 0x7F..=0x9F) => {
                output.push_str(&format!("\\x{:02X}", character as u32));
            }
            character
                if matches!(
                    character as u32,
                    0x061C | 0x200E | 0x200F | 0x2028..=0x202E | 0x2066..=0x206F
                ) =>
            {
                output.push_str(&format!("\\u{{{:04X}}}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_silent_and_explicit_file_modes() {
        assert_eq!(
            parse_args(vec![OsString::from("--startup-only")]).unwrap(),
            Mode::StartupOnly
        );
        assert_eq!(
            parse_args(vec![OsString::from("music/Björk - Jóga.flac")]).unwrap(),
            Mode::Play(PathBuf::from("music/Björk - Jóga.flac"))
        );
        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(vec![OsString::from("one.mp3"), OsString::from("two.mp3")]).is_err());
    }

    #[test]
    fn printed_values_escape_terminal_controls() {
        assert_eq!(
            terminal_safe("track\x1B]0;owned\x07\n.mp3\u{202E}"),
            "track\\x1B]0;owned\\x07\\n.mp3\\u{202E}"
        );
    }
}
