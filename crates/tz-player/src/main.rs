//! tz-player binary — CLI entrypoints for the Rust rewrite.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

use tz_core::{
    about_info, app_paths_or_cwd, load_state, open_runtime, save_state, terminal_safe,
    terminal_safe_path, AppState,
};
use tz_db::{open_database, SCHEMA_VERSION};
use tz_playback::{probe_audio_output, AudioOutputInfo, BackendKind};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, ValueEnum)]
enum BackendCli {
    #[value(hide = true)]
    Vlc,
    #[value(name = "audio", alias = "rodio")]
    Audio,
    Fake,
}

impl From<BackendCli> for BackendKind {
    fn from(value: BackendCli) -> Self {
        match value {
            BackendCli::Vlc => BackendKind::Audio,
            BackendCli::Audio => BackendKind::Audio,
            BackendCli::Fake => BackendKind::Fake,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "tz-player",
    version,
    about = "TaggedZ's terminal music player (Rust rewrite)",
    long_about = "Local-first TUI music player.\n\
                  Playback and analysis: bundled Audio engine (native first, helper second).\n\
                  See `tz-player doctor` for package checks."
)]
struct Cli {
    /// Playback backend (default: audio; unavailable real backends fall back to fake)
    #[arg(long, value_enum, default_value_t = BackendCli::Audio, global = true)]
    backend: BackendCli,

    /// Enable verbose (debug) logging
    #[arg(long)]
    verbose: bool,

    /// Only show warnings and errors
    #[arg(long)]
    quiet: bool,

    /// Write logs to an explicit file path
    #[arg(long)]
    log_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Diagnose the selected Audio backend and bundled helper
    Doctor,
    /// Guided setup notes for the bundled Audio engine
    Setup,
    /// Print resolved paths and schema version
    Paths,
    /// Print product info: name, version, repository, license, schema
    About,
    /// Add files or folders to the default playlist
    Add {
        /// Media files or directories
        paths: Vec<PathBuf>,
    },
    /// List tracks in the default playlist
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Internal extracted-package playback and analysis verifier
    #[command(hide = true)]
    PackageSmoke {
        #[arg(long)]
        native: Vec<PathBuf>,
        #[arg(long)]
        helper: Vec<PathBuf>,
        #[arg(long)]
        database: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "Could not start async runtime: {}",
                terminal_safe(error.to_string())
            );
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> ExitCode {
    init_logging(&cli);
    tracing::info!(version = VERSION, "tz-player starting");

    if matches!(cli.backend, BackendCli::Vlc) {
        eprintln!("VLC was removed from this release; use --backend audio instead.");
        return ExitCode::from(2);
    }

    match cli.command {
        Some(Commands::Doctor) => cmd_doctor(cli.backend.into()),
        Some(Commands::Setup) => {
            cmd_setup();
            ExitCode::SUCCESS
        }
        Some(Commands::Paths) => {
            cmd_paths();
            ExitCode::SUCCESS
        }
        Some(Commands::About) => {
            println!("{}", about_info());
            ExitCode::SUCCESS
        }
        Some(Commands::Add { paths }) => match cmd_add(cli.backend.into(), paths).await {
            Ok(n) => {
                println!("Added {n} track(s).");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}", terminal_safe(&e));
                ExitCode::FAILURE
            }
        },
        Some(Commands::List { limit }) => match cmd_list(limit).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}", terminal_safe(&e));
                ExitCode::FAILURE
            }
        },
        Some(Commands::PackageSmoke {
            native,
            helper,
            database,
        }) => match cmd_package_smoke(&native, &helper, &database) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{}", terminal_safe(error));
                ExitCode::FAILURE
            }
        },
        None => match run_app(cli.backend.into()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}", terminal_safe(&e));
                ExitCode::FAILURE
            }
        },
    }
}

fn cmd_package_smoke(
    native: &[PathBuf],
    helper: &[PathBuf],
    database: &std::path::Path,
) -> Result<(), String> {
    if native.is_empty() && helper.is_empty() {
        return Err("package smoke requires at least one --native or --helper fixture".into());
    }
    let mut helper_underruns = 0_u64;
    for media in native {
        let report = tz_playback::package_playback_smoke(media).map_err(|error| {
            format!(
                "package playback smoke failed for {}: {error}",
                media.display()
            )
        })?;
        if report.route != "native" {
            return Err(format!(
                "native package fixture {} selected unexpected route: {}",
                media.display(),
                report.route
            ));
        }
    }
    for media in helper {
        let report = tz_playback::package_playback_smoke(media).map_err(|error| {
            format!(
                "package playback smoke failed for {}: {error}",
                media.display()
            )
        })?;
        if report.route != "bundled-helper" {
            return Err(format!(
                "helper package fixture {} selected unexpected route: {}",
                media.display(),
                report.route
            ));
        }
        helper_underruns = helper_underruns.saturating_add(report.underruns);
    }

    let levels = tz_core::LevelService::new(database);
    for media in native.iter().chain(helper) {
        levels.ensure_analysis(media)?;
        if levels.cache_flags(media) != (true, true, true, true) {
            return Err(format!(
                "package analysis did not create every cache product for {}",
                media.display()
            ));
        }
    }

    println!(
        "Package smoke PASS: native={} fixture(s), helper={} fixture(s), helper underruns={helper_underruns}",
        native.len(),
        helper.len(),
    );
    Ok(())
}

async fn run_app(backend: BackendKind) -> Result<(), String> {
    let paths = app_paths_or_cwd();
    let runtime = open_runtime(paths, Some(backend))
        .await
        .map_err(|e| e.to_string())?;
    tz_tui::run_tui(runtime).await.map_err(|e| e.to_string())
}

async fn cmd_add(backend: BackendKind, paths: Vec<PathBuf>) -> Result<usize, String> {
    if paths.is_empty() {
        return Err("usage: tz-player add <file-or-dir>...".into());
    }
    let app_paths = app_paths_or_cwd();
    let mut runtime = open_runtime(app_paths, Some(backend))
        .await
        .map_err(|e| e.to_string())?;
    let n = runtime.add_paths_cli(&paths).map_err(|e| e.to_string())?;
    runtime.persist().await;
    Ok(n)
}

async fn cmd_list(limit: usize) -> Result<(), String> {
    let paths = app_paths_or_cwd();
    let store = tz_db::PlaylistStore::new(&paths.db_file);
    store.initialize().map_err(|e| e.to_string())?;
    let pid = store
        .ensure_playlist("Default")
        .map_err(|e| e.to_string())?;
    let rows = store
        .fetch_window(pid, 0, limit)
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        println!("(empty playlist — try: tz-player add <files>)");
        return Ok(());
    }
    for (i, row) in rows.iter().enumerate() {
        let title = row
            .title
            .clone()
            .unwrap_or_else(|| row.path.display().to_string());
        let artist = row.artist.as_deref().unwrap_or("-");
        println!("{}", playlist_line(i + 1, artist, &title));
    }
    Ok(())
}

fn playlist_line(number: usize, artist: &str, title: &str) -> String {
    format!(
        "{number:>4}. {} — {}",
        terminal_safe(artist),
        terminal_safe(title)
    )
}

fn init_logging(cli: &Cli) {
    let level = if cli.quiet {
        "warn"
    } else if cli.verbose {
        "debug"
    } else {
        "info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let log_path = resolved_log_path(cli);

    if let Some(path) = log_path.as_deref() {
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "Could not create log directory '{}': {}",
                    terminal_safe_path(parent),
                    terminal_safe(error.to_string())
                );
            }
        }
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_target(false)
                    .with_writer(Mutex::new(file))
                    .try_init();
                return;
            }
            Err(error) => {
                eprintln!(
                    "Could not open log file '{}': {}",
                    terminal_safe_path(path),
                    terminal_safe(error.to_string())
                );
            }
        }
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn resolved_log_path(cli: &Cli) -> Option<PathBuf> {
    cli.log_file.clone().or_else(|| {
        // Never let tracing write into the alternate-screen TUI: even one
        // stderr line scrolls the physical terminal and desynchronizes it from
        // Ratatui's back buffer. CLI subcommands keep their normal stderr logs.
        cli.command
            .is_none()
            .then(|| app_paths_or_cwd().log_dir.join("tz-player.log"))
    })
}

fn cmd_doctor(backend: BackendKind) -> ExitCode {
    println!("tz-player doctor  v{VERSION}");
    println!("==========================");
    println!();
    println!("Media roles:");
    println!("  Playback (listen path): {}", playback_role(backend));
    println!("  Analysis / visualizers: native Symphonia + bundled helper");
    println!();

    let mut ok = true;
    let warns = 0u32;

    match backend {
        BackendKind::Audio => {
            println!("[INFO] Selected backend: audio");
            match probe_audio_output() {
                Ok(info) => println!("[OK]   Audio default output: {}", audio_output_line(&info)),
                Err(error) => {
                    println!("[FAIL] Audio default output: {}", terminal_safe(error));
                    ok = false;
                }
            }
            println!("[OK]   Native format families: {}", rodio_format_families());
            match doctor_bundled_helper() {
                Ok(()) => {}
                Err(error) => {
                    println!(
                        "[FAIL] Bundled helper: {}",
                        terminal_safe(error.to_string())
                    );
                    ok = false;
                }
            }
            match package_audio_metadata() {
                Ok(()) => println!("[OK]   Bundled helper build metadata and source offer"),
                Err(error) => {
                    println!("[FAIL] Bundled helper package: {}", terminal_safe(error));
                    ok = false;
                }
            }
        }
        BackendKind::Fake => {
            println!("[INFO] Selected backend: fake (no audio output is opened)");
            if running_from_distribution() {
                match doctor_bundled_helper() {
                    Ok(()) => {}
                    Err(error) => {
                        println!(
                            "[FAIL] Packaged helper: {}",
                            terminal_safe(error.to_string())
                        );
                        ok = false;
                    }
                }
                match package_audio_metadata() {
                    Ok(()) => println!("[OK]   Packaged helper build metadata and source offer"),
                    Err(error) => {
                        println!("[FAIL] Packaged helper package: {}", terminal_safe(error));
                        ok = false;
                    }
                }
            } else {
                println!("[INFO] Audio output and bundled helper are not required for this run");
            }
        }
    }

    let paths = app_paths_or_cwd();
    println!();
    println!("Paths:");
    println!("  data_dir:   {}", terminal_safe_path(&paths.data_dir));
    println!("  config_dir: {}", terminal_safe_path(&paths.config_dir));
    println!("  log_dir:    {}", terminal_safe_path(&paths.log_dir));
    println!("  state:      {}", terminal_safe_path(&paths.state_file));
    println!(
        "  theme:      {}",
        terminal_safe_path(&paths.config_dir.join("theme.json"))
    );
    println!("  database:   {}", terminal_safe_path(&paths.db_file));

    match open_database(&paths.db_file) {
        Ok(_) => println!("[OK]   Database writable (schema v{SCHEMA_VERSION})"),
        Err(e) => {
            println!("[FAIL] Database: {}", terminal_safe(e.to_string()));
            ok = false;
        }
    }

    match std::fs::create_dir_all(&paths.log_dir) {
        Ok(()) => println!("[OK]   Log directory writable"),
        Err(e) => {
            println!("[FAIL] Log directory: {}", terminal_safe(e.to_string()));
            ok = false;
        }
    }

    if paths.state_file.exists() {
        println!("[OK]   State file present");
    } else {
        println!("[INFO] State file will be created on first quit");
    }

    println!();
    println!("Build tip:");
    println!("  cargo build --release -p tz-player");
    println!("  # binary: target/release/tz-player{}", exe_suffix());
    println!();

    if ok {
        if warns > 0 {
            println!("Doctor result: PASS with {warns} warning(s)");
        } else {
            println!("Doctor result: PASS (required checks for selected backend)");
        }
        ExitCode::SUCCESS
    } else {
        println!("Doctor result: FAIL — run `tz-player setup` or use --backend fake");
        ExitCode::from(1)
    }
}

fn doctor_bundled_helper() -> Result<(), tz_audio::DecodeError> {
    let config = tz_audio::helper::HelperConfig::packaged()?;
    println!(
        "[OK]   Bundled helper path: {}",
        terminal_safe_path(&config.executable)
    );
    let caps = tz_audio::helper::capabilities(&config)?;
    let libraries = caps
        .library_majors
        .iter()
        .map(|(name, major)| format!("{name}={major}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "[OK]   Bundled helper protocol v{}.{} / FFmpeg {} ({})",
        caps.protocol_major, caps.protocol_minor, caps.ffmpeg_version, caps.ffmpeg_commit
    );
    println!("[OK]   FFmpeg library ABI majors: {libraries}");
    println!(
        "[OK]   FFmpeg configuration hash: {}",
        caps.configuration_hash
    );
    println!(
        "[OK]   Helper format families: {}",
        helper_format_families()
    );
    Ok(())
}

fn running_from_distribution() -> bool {
    std::env::current_exe().is_ok_and(|executable| {
        tz_audio::discovery::package_root_path(&executable)
            .join("audio")
            .is_dir()
    })
}

fn package_audio_metadata() -> Result<(), String> {
    let location = tz_audio::discovery::resolve_package_helper()?;
    let required = [
        location.package_root.join("FFMPEG_SOURCE.md"),
        location.package_root.join("NATIVE_DEPENDENCIES.md"),
        location.package_root.join("licenses/LGPL-2.1-or-later.txt"),
        location.package_root.join("audio/FFMPEG_BUILD.json"),
        location.package_root.join("audio/FFMPEG_COMPONENTS.json"),
        location.package_root.join("audio/FFMPEG_CONFIGURE.log"),
        location.package_root.join("audio/FFMPEG_CHANGES.diff"),
    ];
    for path in required {
        if !path.is_file() {
            return Err(format!(
                "required package file is missing: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn playback_role(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Audio => "Audio / Rodio / Symphonia / bundled helper",
        BackendKind::Fake => "Fake transport (no audio)",
    }
}

fn audio_output_line(info: &AudioOutputInfo) -> String {
    terminal_safe(info.to_string())
}

fn rodio_format_families() -> &'static str {
    "MP1/MP2/MP3, FLAC, WAV/ADPCM, Ogg Vorbis, AAC, ALAC, AIFF, CAF, Matroska/WebM"
}

fn helper_format_families() -> &'static str {
    "Ogg Opus, WMA/ASF, Monkey's Audio, WavPack, AC-3, E-AC-3, DTS, Musepack 7/8, TTA, Speex"
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

fn cmd_setup() {
    println!("tz-player setup  v{VERSION}");
    println!("=========================");
    println!();
    println!("1) Playback — use the bundled Audio engine");
    println!("   Native Symphonia decoding is attempted first; supported fallback formats use the package-relative helper.");
    if cfg!(target_os = "linux") {
        println!("     Linux source builds need ALSA development files (libasound2-dev on Ubuntu)");
    }
    println!("   Fake: no-audio testing; select --backend fake");
    println!();
    println!("2) Analysis — no system FFmpeg installation is required");
    println!();
    println!("3) Build a release binary");
    println!("   cargo build --release -p tz-player");
    println!("   # output: target/release/tz-player{}", exe_suffix());
    println!();
    println!("4) Verify:  tz-player doctor");
    println!("5) Add music: tz-player add path/to/song.mp3");
    println!("6) Run: tz-player   (Audio default)");
    println!("        tz-player --backend fake   # no audio");
    println!();
    println!("Data lives under a separate identity from the Python app:");
    let paths = app_paths_or_cwd();
    println!("  {}", terminal_safe_path(&paths.data_dir));
    println!();
    println!(
        "Note: the distributed helper carries its audited FFmpeg runtime; PATH is not consulted."
    );
    println!();
    println!("See also: docs/RELEASE.md");
}

fn cmd_paths() {
    let paths = app_paths_or_cwd();
    println!("tz-player paths  v{VERSION}");
    println!("data_dir:   {}", terminal_safe_path(&paths.data_dir));
    println!("config_dir: {}", terminal_safe_path(&paths.config_dir));
    println!("log_dir:    {}", terminal_safe_path(&paths.log_dir));
    println!("state:      {}", terminal_safe_path(&paths.state_file));
    println!(
        "theme:      {}",
        terminal_safe_path(&paths.config_dir.join("theme.json"))
    );
    println!("database:   {}", terminal_safe_path(&paths.db_file));
    println!("schema:     v{SCHEMA_VERSION}");
    // ensure state file can be created
    if !paths.state_file.exists() {
        let _ = save_state(&paths.state_file, &AppState::default());
    } else {
        let _ = load_state(&paths.state_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_explicit_rodio_backend() {
        let cli = Cli::try_parse_from(["tz-player", "--backend", "rodio", "about"]).unwrap();

        assert!(matches!(cli.backend, BackendCli::Audio));

        let cli = Cli::try_parse_from(["tz-player", "doctor", "--backend", "rodio"]).unwrap();
        assert!(matches!(cli.backend, BackendCli::Audio));
    }

    #[test]
    fn package_smoke_accepts_repeated_native_and_helper_fixtures() {
        let cli = Cli::try_parse_from([
            "tz-player",
            "package-smoke",
            "--database",
            "smoke.sqlite",
            "--native",
            "one.wav",
            "--native",
            "two.flac",
            "--helper",
            "one.opus",
            "--helper",
            "two.wma",
        ])
        .unwrap();

        let Some(Commands::PackageSmoke { native, helper, .. }) = cli.command else {
            panic!("package-smoke command was not parsed");
        };
        assert_eq!(
            native,
            [PathBuf::from("one.wav"), PathBuf::from("two.flac")]
        );
        assert_eq!(
            helper,
            [PathBuf::from("one.opus"), PathBuf::from("two.wma")]
        );
    }

    #[test]
    fn rodio_doctor_helpers_are_stable_and_hardware_independent() {
        let info = AudioOutputInfo {
            channels: 2,
            sample_rate: 48_000,
            sample_format: "F32".into(),
        };

        assert_eq!(audio_output_line(&info), "2 channel(s), 48000 Hz, F32");
        assert!(playback_role(BackendKind::Audio).contains("bundled helper"));
        assert!(rodio_format_families().contains("AAC"));
    }

    #[test]
    fn interactive_tui_logs_to_a_file_by_default() {
        let cli = Cli::try_parse_from(["tz-player"]).unwrap();
        let path = resolved_log_path(&cli).expect("interactive log path");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("tz-player.log")
        );
    }

    #[test]
    fn cli_subcommands_keep_terminal_logging_unless_file_is_requested() {
        let doctor = Cli::try_parse_from(["tz-player", "doctor"]).unwrap();
        assert!(resolved_log_path(&doctor).is_none());

        let explicit =
            Cli::try_parse_from(["tz-player", "--log-file", "diagnostics.log", "doctor"]).unwrap();
        assert_eq!(
            resolved_log_path(&explicit),
            Some(PathBuf::from("diagnostics.log"))
        );
    }

    #[test]
    fn playlist_lines_escape_terminal_and_bidi_controls() {
        let line = playlist_line(
            7,
            "artist\x1B]0;owned\x07\n",
            "title\x1B[31mred\x1B[0m\u{202E}",
        );
        assert_eq!(
            line,
            "   7. artist\\x1B]0;owned\\x07\\n — title\\x1B[31mred\\x1B[0m\\u{202E}"
        );
        assert!(!line.contains('\x1B'));
        assert!(!line.contains('\n'));
        assert!(!line.contains('\u{202E}'));
    }
}
