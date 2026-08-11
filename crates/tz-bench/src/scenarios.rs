use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tz_analysis::{
    analyze_beats_from_decoded, analyze_envelope_from_decoded, analyze_spectrum_from_decoded,
    analyze_waveform_proxy_from_decoded, DecodedAnalysisAudio,
};
use tz_db::{
    BeatParams, BeatStore, EnvelopeStore, PlaylistSort, PlaylistStore, SpectrumParams,
    SpectrumStore, WaveformParams, WaveformStore,
};
use tz_tui::bench_support::IdleFrameBenchmark;

use crate::metrics::{
    measure_case, print_case, scenario_from_cases, CaseMeasurement, MeasureConfig, MetricSummary,
    ScenarioResult,
};

#[derive(Debug, Clone, Copy)]
pub struct SuiteConfig {
    pub preset: &'static str,
    pub measure: MeasureConfig,
    pub audio_seconds: u64,
    pub playlist_tracks: usize,
    pub playlist_window_rows: usize,
    pub tui_width: u16,
    pub tui_height: u16,
}

pub fn analysis(config: SuiteConfig) -> Result<ScenarioResult, String> {
    println!(
        "\n[analysis] synthetic decoded audio, {} seconds",
        config.audio_seconds
    );
    let decoded = Arc::new(synthetic_audio(config.audio_seconds));
    let input_bytes =
        (decoded.mono_samples.len() + decoded.left_samples.len() + decoded.right_samples.len())
            * std::mem::size_of::<f32>();
    let audio_work = config.audio_seconds as f64;
    let mut cases = Vec::new();

    let input = Arc::clone(&decoded);
    cases.push(run_and_print(
        "envelope",
        audio_work,
        "audio-s/s",
        config.measure,
        move || {
            let result = analyze_envelope_from_decoded(&input, 50).map_err(|e| e.to_string())?;
            Ok(result.points.len() as u64 ^ result.duration_ms)
        },
    )?);

    let input = Arc::clone(&decoded);
    cases.push(run_and_print(
        "waveform_proxy",
        audio_work,
        "audio-s/s",
        config.measure,
        move || {
            let result =
                analyze_waveform_proxy_from_decoded(&input, 40).map_err(|e| e.to_string())?;
            Ok(result.frames.len() as u64 ^ result.duration_ms)
        },
    )?);

    let input = Arc::clone(&decoded);
    cases.push(run_and_print(
        "beat_detection",
        audio_work,
        "audio-s/s",
        config.measure,
        move || {
            let result = analyze_beats_from_decoded(&input, 40).map_err(|e| e.to_string())?;
            Ok(result.frames.len() as u64 ^ result.bpm.to_bits())
        },
    )?);

    let input = Arc::clone(&decoded);
    cases.push(run_and_print(
        "spectrum_48_band",
        audio_work,
        "audio-s/s",
        config.measure,
        move || {
            let result =
                analyze_spectrum_from_decoded(&input, 48, 40).map_err(|e| e.to_string())?;
            Ok(result.frames.len() as u64 ^ result.duration_ms)
        },
    )?);

    let counters = BTreeMap::from([
        ("audio_seconds".into(), Value::from(config.audio_seconds)),
        ("input_bytes".into(), Value::from(input_bytes as u64)),
    ]);
    let metadata = BTreeMap::from([
        (
            "fixture".into(),
            Value::from("deterministic layered sine waves"),
        ),
        ("mono_sample_rate_hz".into(), Value::from(11_025)),
        ("stereo_sample_rate_hz".into(), Value::from(44_100)),
        ("spectrum_bands".into(), Value::from(48)),
        ("hop_ms".into(), Value::from(40)),
    ]);
    Ok(scenario_from_cases(
        "long_track_analysis_on_demand",
        "analysis",
        cases,
        counters,
        metadata,
        vec![
            "Decode and file I/O are excluded; this isolates the bounded DSP hot paths.".into(),
            "Allocation metrics cover Rust's global allocator for each operation.".into(),
        ],
    ))
}

pub fn database(config: SuiteConfig) -> Result<ScenarioResult, String> {
    println!(
        "\n[database] SQLite playlist fixture, {} tracks",
        config.playlist_tracks
    );
    let fixture_started = Instant::now();
    let workspace = TempWorkspace::new("database")?;
    let database_path = workspace.path().join("benchmark.sqlite");
    let store = PlaylistStore::new(&database_path);
    store.initialize().map_err(|e| e.to_string())?;
    let playlist_id = store
        .ensure_playlist("Benchmark")
        .map_err(|e| e.to_string())?;
    let paths: Vec<PathBuf> = (0..config.playlist_tracks)
        .map(|index| {
            PathBuf::from(format!(
                "benchmark-library/artist_{:04}/album_{:03}/benchmark_track_{index:06}.flac",
                index % 500,
                index % 100
            ))
        })
        .collect();
    let inserted = store
        .add_tracks(playlist_id, &paths)
        .map_err(|e| e.to_string())?;
    if inserted != config.playlist_tracks {
        return Err(format!(
            "benchmark fixture inserted {inserted} of {} tracks",
            config.playlist_tracks
        ));
    }
    let fixture_setup_ms = fixture_started.elapsed().as_secs_f64() * 1000.0;
    let window_size = config.playlist_window_rows;
    let window_offset = config.playlist_tracks.saturating_sub(window_size) / 2;
    let search_token = format!("{:06}", config.playlist_tracks.saturating_sub(1));
    let search_check = store
        .search_item_ids(playlist_id, &search_token, 25)
        .map_err(|e| e.to_string())?;
    if search_check.is_empty() {
        return Err(format!(
            "FTS benchmark fixture could not find token {search_token}"
        ));
    }

    let mut cases = Vec::new();
    cases.push(run_and_print(
        "playlist_count",
        1.0,
        "queries/s",
        config.measure,
        || Ok(store.count(playlist_id).map_err(|e| e.to_string())? as u64),
    )?);
    cases.push(run_and_print(
        "fetch_window",
        1.0,
        "queries/s",
        config.measure,
        || {
            let rows = store
                .fetch_window(playlist_id, window_offset, window_size)
                .map_err(|e| e.to_string())?;
            Ok(rows.len() as u64 ^ rows.first().map_or(0, |row| row.item_id as u64))
        },
    )?);
    cases.push(run_and_print(
        "fetch_window_sorted",
        1.0,
        "queries/s",
        config.measure,
        || {
            let rows = store
                .fetch_window_sorted(playlist_id, window_offset, window_size, PlaylistSort::Track)
                .map_err(|e| e.to_string())?;
            Ok(rows.len() as u64 ^ rows.first().map_or(0, |row| row.item_id as u64))
        },
    )?);
    cases.push(run_and_print(
        "fts_search",
        1.0,
        "queries/s",
        config.measure,
        || {
            let ids = store
                .search_item_ids(playlist_id, &search_token, 25)
                .map_err(|e| e.to_string())?;
            Ok(ids.len() as u64 ^ ids.first().copied().unwrap_or_default() as u64)
        },
    )?);

    let database_bytes = sqlite_footprint(&database_path)?;
    let bytes_per_track = database_bytes as f64 / config.playlist_tracks as f64;
    println!(
        "{:<34} {:>12} bytes ({:.1} bytes/track)",
        "playlist_database", database_bytes, bytes_per_track
    );
    let counters = BTreeMap::from([
        (
            "playlist_tracks".into(),
            Value::from(config.playlist_tracks as u64),
        ),
        ("window_rows".into(), Value::from(window_size as u64)),
        ("database_bytes".into(), Value::from(database_bytes)),
    ]);
    let metadata = BTreeMap::from([
        ("fixture_setup_ms".into(), json!(fixture_setup_ms)),
        ("search_token".into(), Value::from(search_token)),
        (
            "storage".into(),
            Value::from("temporary local SQLite database"),
        ),
    ]);
    let mut result = scenario_from_cases(
        "large_playlist_db_query_matrix",
        "database",
        cases,
        counters,
        metadata,
        vec![
            "Fixture construction is reported separately and excluded from query timings.".into(),
            "Rust allocation metrics do not include SQLite's internal C allocations.".into(),
        ],
    );
    result.metrics.insert(
        "playlist_database_bytes".into(),
        MetricSummary::from_samples(&[database_bytes as f64], "bytes"),
    );
    result.metrics.insert(
        "playlist_database_bytes_per_track".into(),
        MetricSummary::from_samples(&[bytes_per_track], "bytes/track"),
    );
    Ok(result)
}

pub fn disk(config: SuiteConfig) -> Result<ScenarioResult, String> {
    const CACHE_TRACK_DURATION_MS: u64 = 4 * 60 * 1_000;
    let started = Instant::now();
    println!(
        "\n[disk] analysis-cache and executable footprint, {}-minute cache fixture",
        CACHE_TRACK_DURATION_MS / 60_000
    );
    let workspace = TempWorkspace::new("disk")?;
    let database_path = workspace.path().join("analysis-cache.sqlite");
    let track_path = PathBuf::from("benchmark-cache/synthetic-track.wav");

    let envelope_store = EnvelopeStore::new(&database_path, 50);
    let spectrum_store = SpectrumStore::new(
        &database_path,
        SpectrumParams {
            band_count: 48,
            hop_ms: 40,
        },
    );
    let beat_store = BeatStore::new(&database_path, BeatParams { hop_ms: 40 });
    let waveform_store = WaveformStore::new(&database_path, WaveformParams { hop_ms: 40 });
    envelope_store.initialize().map_err(|e| e.to_string())?;
    spectrum_store.initialize().map_err(|e| e.to_string())?;
    beat_store.initialize().map_err(|e| e.to_string())?;
    waveform_store.initialize().map_err(|e| e.to_string())?;
    let empty_database_bytes = sqlite_footprint(&database_path)?;

    let envelope: Vec<_> = (0..CACHE_TRACK_DURATION_MS / 50)
        .map(|index| {
            let left = ((index * 37) % 100) as f32 / 100.0;
            let right = ((index * 53 + 11) % 100) as f32 / 100.0;
            (index * 50, left, right)
        })
        .collect();
    let spectrum: Vec<_> = (0..CACHE_TRACK_DURATION_MS / 40)
        .map(|index| {
            let bands = (0..48)
                .map(|band| ((index * 13 + band * 29) % 256) as u8)
                .collect();
            (index * 40, bands)
        })
        .collect();
    let beats: Vec<_> = (0..CACHE_TRACK_DURATION_MS / 40)
        .map(|index| (index * 40, ((index * 41) % 256) as u8, index % 12 == 0))
        .collect();
    let waveform: Vec<_> = (0..CACHE_TRACK_DURATION_MS / 40)
        .map(|index| {
            let amplitude = ((index * 17) % 120) as i8;
            (
                index * 40,
                -amplitude,
                amplitude,
                -amplitude.saturating_sub(8),
                amplitude.saturating_sub(8),
            )
        })
        .collect();

    envelope_store
        .upsert_envelope(&track_path, CACHE_TRACK_DURATION_MS, &envelope)
        .map_err(|e| e.to_string())?;
    spectrum_store
        .upsert_spectrum(&track_path, CACHE_TRACK_DURATION_MS, &spectrum)
        .map_err(|e| e.to_string())?;
    beat_store
        .upsert_beats(&track_path, CACHE_TRACK_DURATION_MS, 120.0, &beats)
        .map_err(|e| e.to_string())?;
    waveform_store
        .upsert_waveform(&track_path, CACHE_TRACK_DURATION_MS, &waveform)
        .map_err(|e| e.to_string())?;

    let populated_database_bytes = sqlite_footprint(&database_path)?;
    let incremental_cache_bytes = populated_database_bytes.saturating_sub(empty_database_bytes);
    let cache_track_minutes = CACHE_TRACK_DURATION_MS as f64 / 60_000.0;
    let bytes_per_audio_minute = incremental_cache_bytes as f64 / cache_track_minutes;
    let projected_1000_tracks_4min =
        empty_database_bytes as f64 + incremental_cache_bytes as f64 * 1_000.0;
    let current_exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let runner_binary_bytes = std::fs::metadata(&current_exe)
        .map_err(|error| error.to_string())?
        .len();
    let player_path =
        current_exe
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(if cfg!(windows) {
                "tz-player.exe"
            } else {
                "tz-player"
            });
    let player_binary_bytes = std::fs::metadata(&player_path)
        .ok()
        .map(|metadata| metadata.len());

    println!(
        "{:<34} {:>12.0} bytes",
        "analysis_cache_incremental", incremental_cache_bytes
    );
    println!(
        "{:<34} {:>12.0} bytes/min",
        "analysis_cache_per_audio_minute", bytes_per_audio_minute
    );
    println!(
        "{:<34} {:>12.0} bytes",
        "projected_1000_tracks_x_4min", projected_1000_tracks_4min
    );
    if let Some(bytes) = player_binary_bytes {
        println!("{:<34} {:>12} bytes", "tz_player_binary", bytes);
    } else {
        println!("{:<34} {:>12}", "tz_player_binary", "not built");
    }

    let mut metrics = BTreeMap::from([
        (
            "analysis_cache_total_bytes".into(),
            MetricSummary::from_samples(&[populated_database_bytes as f64], "bytes"),
        ),
        (
            "analysis_cache_incremental_bytes".into(),
            MetricSummary::from_samples(&[incremental_cache_bytes as f64], "bytes"),
        ),
        (
            "analysis_cache_bytes_per_audio_minute".into(),
            MetricSummary::from_samples(&[bytes_per_audio_minute], "bytes/audio-minute"),
        ),
        (
            "benchmark_runner_binary_bytes".into(),
            MetricSummary::from_samples(&[runner_binary_bytes as f64], "bytes"),
        ),
    ]);
    if let Some(bytes) = player_binary_bytes {
        metrics.insert(
            "player_binary_bytes".into(),
            MetricSummary::from_samples(&[bytes as f64], "bytes"),
        );
    }
    Ok(ScenarioResult {
        scenario_id: "persistent_disk_footprint".into(),
        category: "disk".into(),
        status: "pass".into(),
        elapsed_s: started.elapsed().as_secs_f64(),
        metrics,
        counters: BTreeMap::from([
            (
                "cache_track_duration_ms".into(),
                Value::from(CACHE_TRACK_DURATION_MS),
            ),
            (
                "empty_database_bytes".into(),
                Value::from(empty_database_bytes),
            ),
            (
                "projected_1000_tracks_4min_bytes".into(),
                json!(projected_1000_tracks_4min),
            ),
        ]),
        metadata: BTreeMap::from([
            ("preset".into(), Value::from(config.preset)),
            (
                "player_binary_path".into(),
                Value::from(player_path.display().to_string()),
            ),
            (
                "projection".into(),
                Value::from("linear extrapolation from incremental cache bytes"),
            ),
        ]),
        notes: vec![
            "Cache footprint includes scalar, spectrum, beat, and waveform products in SQLite."
                .into(),
            "The library projection is directional; SQLite page packing and track durations vary."
                .into(),
            "Player binary size excludes the separately packaged audio helper/libraries, configuration, and music files."
                .into(),
        ],
    })
}

pub fn tui(config: SuiteConfig) -> Result<ScenarioResult, String> {
    println!(
        "\n[tui] headless {}x{} idle frames, {}-track playlist",
        config.tui_width, config.tui_height, config.playlist_tracks
    );
    let mut cases = Vec::new();

    let mut basic = IdleFrameBenchmark::new(
        config.tui_width,
        config.tui_height,
        config.playlist_tracks,
        "basic",
        false,
    )?;
    cases.push(run_and_print(
        "idle_frame_basic",
        1.0,
        "frames/s",
        config.measure,
        || basic.render(),
    )?);

    let mut spectrum = IdleFrameBenchmark::new(
        config.tui_width,
        config.tui_height,
        config.playlist_tracks,
        "spectrum.bars",
        false,
    )?;
    cases.push(run_and_print(
        "idle_frame_spectrum",
        1.0,
        "frames/s",
        config.measure,
        || spectrum.render(),
    )?);

    let mut hidden = IdleFrameBenchmark::new(
        config.tui_width,
        config.tui_height,
        config.playlist_tracks,
        "spectrum.bars",
        true,
    )?;
    cases.push(run_and_print(
        "idle_frame_visualizer_hidden",
        1.0,
        "frames/s",
        config.measure,
        || hidden.render(),
    )?);

    let counters = BTreeMap::from([
        ("terminal_width".into(), Value::from(config.tui_width)),
        ("terminal_height".into(), Value::from(config.tui_height)),
        (
            "playlist_tracks".into(),
            Value::from(config.playlist_tracks as u64),
        ),
    ]);
    let metadata = BTreeMap::from([
        ("backend".into(), Value::from("ratatui TestBackend")),
        ("theme".into(), Value::from("default")),
    ]);
    Ok(scenario_from_cases(
        "visualizer_matrix_render",
        "tui",
        cases,
        counters,
        metadata,
        vec![
            "The frame uses the production draw functions with a headless terminal backend.".into(),
            "Database fetching, event polling, and terminal transport writes are measured separately or excluded.".into(),
        ],
    ))
}

fn run_and_print<F>(
    name: &str,
    work_per_operation: f64,
    throughput_unit: &str,
    config: MeasureConfig,
    operation: F,
) -> Result<CaseMeasurement, String>
where
    F: FnMut() -> Result<u64, String>,
{
    let measurement = measure_case(name, work_per_operation, throughput_unit, config, operation)?;
    print_case(&measurement);
    Ok(measurement)
}

fn synthetic_audio(seconds: u64) -> DecodedAnalysisAudio {
    const MONO_RATE: u32 = 11_025;
    const STEREO_RATE: u32 = 44_100;
    let mono_samples = synth_channel(MONO_RATE, seconds, 0.0);
    let left_samples = synth_channel(STEREO_RATE, seconds, 0.0);
    let right_samples = synth_channel(STEREO_RATE, seconds, PI / 3.0);
    DecodedAnalysisAudio {
        duration_ms: seconds * 1000,
        mono_rate: MONO_RATE,
        mono_samples,
        stereo_rate: STEREO_RATE,
        left_samples,
        right_samples,
    }
}

fn synth_channel(sample_rate: u32, seconds: u64, phase_offset: f32) -> Vec<f32> {
    let sample_count = sample_rate as usize * seconds as usize;
    (0..sample_count)
        .map(|index| {
            let time = index as f32 / sample_rate as f32;
            let pulse = if ((time * 2.0) as usize).is_multiple_of(2) {
                1.0
            } else {
                0.35
            };
            let fundamental = (2.0 * PI * 220.0 * time + phase_offset).sin();
            let harmonic = (2.0 * PI * 880.0 * time + phase_offset * 0.5).sin();
            (pulse * (0.55 * fundamental + 0.2 * harmonic)).clamp(-1.0, 1.0)
        })
        .collect()
}

fn sqlite_footprint(database_path: &Path) -> Result<u64, String> {
    let parent = database_path
        .parent()
        .ok_or_else(|| "SQLite benchmark path has no parent".to_string())?;
    let database_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "SQLite benchmark filename is not UTF-8".to_string())?;
    let sidecar_prefix = format!("{database_name}-");
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == database_name || name.starts_with(&sidecar_prefix) {
            bytes =
                bytes.saturating_add(entry.metadata().map_err(|error| error.to_string())?.len());
        }
    }
    Ok(bytes)
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(label: &str) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tz-player-bench-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
