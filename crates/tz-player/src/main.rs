//! tz-player binary — CLI entrypoints for the Rust rewrite.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

use tz_analysis::ffmpeg_available;
use tz_core::{
    about_info, app_paths_or_cwd, load_state, open_runtime, save_state, terminal_safe,
    terminal_safe_path, AppState,
};
use tz_db::{open_database, SCHEMA_VERSION};
use tz_playback::{
    configure_vlc_environment, discover_vlc, probe_rodio_output, BackendKind, RodioOutputInfo,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, ValueEnum)]
enum BackendCli {
    Vlc,
    Rodio,
    Fake,
}

impl From<BackendCli> for BackendKind {
    fn from(value: BackendCli) -> Self {
        match value {
            BackendCli::Vlc => BackendKind::Vlc,
            BackendCli::Rodio => BackendKind::Rodio,
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
                  Playback: VLC/libVLC (default) or experimental Rodio.\n\
                  Analysis/visualizers: FFmpeg (optional).\n\
                  See `tz-player doctor` for environment checks."
)]
struct Cli {
    /// Playback backend (default: vlc; unavailable real backends fall back to fake)
    #[arg(long, value_enum, default_value_t = BackendCli::Vlc, global = true)]
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
    /// Diagnose the selected playback backend and optional FFmpeg analysis
    Doctor,
    /// Guided setup notes for VLC, Rodio, and FFmpeg
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if matches!(&cli.backend, BackendCli::Vlc) {
        // VLC_PLUGIN_PATH is process-global. Configure it while startup is
        // still single-threaded, before Tokio creates its worker threads.
        configure_vlc_environment();
    }

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
        None => match run_app(cli.backend.into()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}", terminal_safe(&e));
                ExitCode::FAILURE
            }
        },
    }
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
    println!("  Analysis / visualizers: FFmpeg (optional) + native WAV");
    println!();

    let mut ok = true;
    let mut warns = 0u32;

    match backend {
        BackendKind::Vlc => {
            println!("[INFO] Selected backend: vlc (default)");
            let discovery = discover_vlc();
            if let Some(exe) = &discovery.vlc_executable {
                println!("[OK]   VLC executable: {}", terminal_safe_path(exe));
            } else {
                println!("[WARN] VLC executable not found on PATH / common install paths");
                warns += 1;
            }
            if let Some(dir) = &discovery.libvlc_dir {
                println!("[OK]   libVLC directory: {}", terminal_safe_path(dir));
                println!("[OK]   libVLC dynamic load path ready (runtime FFI)");
            } else {
                println!("[FAIL] libVLC not found in common install paths");
                ok = false;
            }
            for note in &discovery.notes {
                println!("       note: {}", terminal_safe(note));
            }
        }
        BackendKind::Rodio => {
            println!("[INFO] Selected backend: rodio (experimental)");
            match probe_rodio_output() {
                Ok(info) => println!("[OK]   Rodio default output: {}", rodio_output_line(&info)),
                Err(error) => {
                    println!("[FAIL] Rodio default output: {}", terminal_safe(error));
                    ok = false;
                }
            }
            println!("[OK]   Rodio format families: {}", rodio_format_families());
            println!("[INFO] VLC is not required for selected Rodio playback");
        }
        BackendKind::Fake => {
            println!("[INFO] Selected backend: fake (no audio output is opened)");
            println!("[INFO] VLC and Rodio output are not required for this run");
        }
    }

    if ffmpeg_available() {
        println!("[OK]   FFmpeg available (analysis / visualizers)");
    } else {
        println!("[WARN] FFmpeg not found — analysis-backed visualizers degrade");
        match backend {
            BackendKind::Vlc => {
                println!("       Selected VLC playback still works; native WAV analysis remains")
            }
            BackendKind::Rodio => {
                println!("       Selected Rodio playback still works; native WAV analysis remains")
            }
            BackendKind::Fake => {
                println!("       Fake transport still works; native WAV analysis remains")
            }
        }
        warns += 1;
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

fn playback_role(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Vlc => "VLC / libVLC (default)",
        BackendKind::Rodio => "Rodio / Symphonia / system audio (experimental)",
        BackendKind::Fake => "Fake transport (no audio)",
    }
}

fn rodio_output_line(info: &RodioOutputInfo) -> String {
    terminal_safe(info.to_string())
}

fn rodio_format_families() -> &'static str {
    "MP1/MP2/MP3, FLAC, WAV/ADPCM, Ogg Vorbis, AAC, ALAC, AIFF, CAF, Matroska/WebM"
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
    println!("1) Playback — choose a backend");
    println!("   VLC (default): broad compatibility; install VLC from");
    println!("     https://www.videolan.org/vlc/");
    if cfg!(windows) {
        println!("     Windows: winget install VideoLAN.VLC");
    } else if cfg!(target_os = "macos") {
        println!("     macOS:   brew install --cask vlc");
    } else {
        println!("     Linux:   install VLC via your package manager (libvlc + plugins)");
    }
    println!("   Rodio (experimental): no VLC codec runtime; select --backend rodio");
    if cfg!(target_os = "linux") {
        println!("     Linux source builds need ALSA development files (libasound2-dev on Ubuntu)");
    }
    println!("   Fake: no-audio testing; select --backend fake");
    println!();
    println!("2) Analysis — install FFmpeg (optional, for visualizers)");
    if cfg!(windows) {
        println!("   Windows: winget install Gyan.FFmpeg");
    } else if cfg!(target_os = "macos") {
        println!("   macOS:   brew install ffmpeg");
    } else {
        println!("   Linux:   install ffmpeg via your package manager");
    }
    println!();
    println!("3) Build a release binary");
    println!("   cargo build --release -p tz-player");
    println!("   # output: target/release/tz-player{}", exe_suffix());
    println!();
    println!("4) Verify:  tz-player --backend vlc doctor");
    println!("             tz-player --backend rodio doctor");
    println!("5) Add music: tz-player add path/to/song.mp3");
    println!("6) Run: tz-player   (VLC default)");
    println!("        tz-player --backend rodio");
    println!("        tz-player --backend fake   # no audio");
    println!();
    println!("Data lives under a separate identity from the Python app:");
    let paths = app_paths_or_cwd();
    println!("  {}", terminal_safe_path(&paths.data_dir));
    println!();
    println!("Note: FFmpeg is not used by either real playback backend.");
    println!("      It feeds optional offline analysis and visualizers only.");
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

        assert!(matches!(cli.backend, BackendCli::Rodio));

        let cli = Cli::try_parse_from(["tz-player", "doctor", "--backend", "rodio"]).unwrap();
        assert!(matches!(cli.backend, BackendCli::Rodio));
    }

    #[test]
    fn rodio_doctor_helpers_are_stable_and_hardware_independent() {
        let info = RodioOutputInfo {
            channels: 2,
            sample_rate: 48_000,
            sample_format: "F32".into(),
        };

        assert_eq!(rodio_output_line(&info), "2 channel(s), 48000 Hz, F32");
        assert!(playback_role(BackendKind::Rodio).contains("experimental"));
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
