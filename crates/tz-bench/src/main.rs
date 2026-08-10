//! Opt-in performance and resource benchmark runner for tz-player.

mod metrics;
mod scenarios;

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand, ValueEnum};
use metrics::{
    compare_runs, print_table_header, process_memory, CountingAllocator, MetricSummary, PerfRun,
    ProcessMemory, ScenarioResult,
};
use scenarios::SuiteConfig;
use serde_json::{json, Value};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug, Parser)]
#[command(
    name = "tz-bench",
    about = "Opt-in tz-player performance and resource benchmarks"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run benchmark scenarios and write a JSON artifact.
    Run(RunArgs),
    /// Compare two JSON artifacts; positive changes are regressions.
    Compare(CompareArgs),
    /// List available scenarios.
    List,
}

#[derive(Debug, Clone, Args, Default)]
struct RunArgs {
    /// Run only selected scenarios (repeat the flag for more than one).
    #[arg(long, value_enum)]
    scenario: Vec<Scenario>,

    /// Workload size: standard, ancient (low-memory/slow CPU), or smoke.
    #[arg(long, value_enum, default_value_t = Preset::Standard)]
    preset: Preset,

    /// Use smaller fixtures and fewer samples for harness smoke tests.
    #[arg(long, conflicts_with = "preset")]
    quick: bool,

    /// Override the number of measured samples per case.
    #[arg(long)]
    samples: Option<usize>,

    /// Write the artifact to this file instead of .local/perf_results/.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Optional label included in the artifact run ID and filename.
    #[arg(long)]
    label: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CompareArgs {
    /// JSON artifact from the earlier run.
    baseline: PathBuf,

    /// JSON artifact from the candidate run.
    candidate: PathBuf,

    /// Median increase/decrease percentage classified as a change.
    #[arg(long, default_value_t = 5.0)]
    threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Scenario {
    Analysis,
    Database,
    Disk,
    Tui,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::Analysis, Self::Database, Self::Disk, Self::Tui];

    fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Database => "database",
            Self::Disk => "disk",
            Self::Tui => "tui",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum Preset {
    #[default]
    Standard,
    Ancient,
    Smoke,
}

impl Preset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Ancient => "ancient",
            Self::Smoke => "smoke",
        }
    }
}

fn main() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tz-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command.unwrap_or(Command::Run(RunArgs::default())) {
        Command::Run(args) => run(args),
        Command::Compare(args) => compare(args),
        Command::List => {
            println!("Available scenarios:");
            for scenario in Scenario::ALL {
                println!("  {}", scenario.as_str());
            }
            Ok(())
        }
    }
}

fn run(args: RunArgs) -> Result<(), Box<dyn Error>> {
    if args
        .samples
        .is_some_and(|samples| !(1..=500).contains(&samples))
    {
        return Err("--samples must be between 1 and 500".into());
    }
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: this is a debug build; use `cargo run --release -p tz-bench -- run` for comparable results"
        );
    }

    let selected: BTreeSet<_> = if args.scenario.is_empty() {
        Scenario::ALL.into_iter().collect()
    } else {
        args.scenario.iter().copied().collect()
    };
    let preset = if args.quick {
        Preset::Smoke
    } else {
        args.preset
    };
    let (
        default_samples,
        warm_up,
        target_ms,
        audio_seconds,
        playlist_tracks,
        window_rows,
        width,
        height,
    ) = match preset {
        Preset::Standard => (25, 3, 20, 30, 10_000, 100, 120, 40),
        Preset::Ancient => (9, 1, 10, 10, 2_000, 50, 80, 24),
        Preset::Smoke => (5, 1, 5, 3, 500, 25, 100, 30),
    };
    let samples = args.samples.unwrap_or(default_samples);
    let suite_config = SuiteConfig {
        preset: preset.as_str(),
        measure: metrics::MeasureConfig {
            samples,
            warm_up,
            target_sample_time: Duration::from_millis(target_ms),
        },
        audio_seconds,
        playlist_tracks,
        playlist_window_rows: window_rows,
        tui_width: width,
        tui_height: height,
    };

    println!("tz-player benchmark suite");
    println!(
        "profile={} mode={} samples={} target_sample_ms={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        preset.as_str(),
        samples,
        suite_config.measure.target_sample_time.as_millis()
    );
    println!(
        "Rust heap allocation counts exclude native allocations made inside SQLite, VLC, and audio drivers."
    );
    print_table_header();

    let memory_before = process_memory();
    let mut results = Vec::new();
    for scenario in &selected {
        let scenario_memory_before = process_memory();
        let mut result = match scenario {
            Scenario::Analysis => scenarios::analysis(suite_config),
            Scenario::Database => scenarios::database(suite_config),
            Scenario::Disk => scenarios::disk(suite_config),
            Scenario::Tui => scenarios::tui(suite_config),
        }
        .map_err(|error| format!("{} scenario failed: {error}", scenario.as_str()))?;
        let scenario_memory_after = process_memory();
        attach_process_memory(&mut result, scenario_memory_before, scenario_memory_after)?;
        results.push(result);
    }
    let memory_after = process_memory();

    let timestamp = utc_timestamp();
    let label = args.label.as_deref().map(sanitize_label);
    let run_id = match label.as_deref() {
        Some(label) if !label.is_empty() => format!("rust-{label}-{}", timestamp.file_stem),
        _ => format!("rust-{}", timestamp.file_stem),
    };
    let mut machine = BTreeMap::from([
        ("os".into(), Value::from(std::env::consts::OS)),
        ("arch".into(), Value::from(std::env::consts::ARCH)),
        (
            "logical_cpus".into(),
            Value::from(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1) as u64,
            ),
        ),
        (
            "rustc".into(),
            optional_string_value(command_output("rustc", &["--version"])),
        ),
    ]);
    machine.insert("memory_before".into(), serde_json::to_value(memory_before)?);
    machine.insert("memory_after".into(), serde_json::to_value(memory_after)?);

    let config = BTreeMap::from([
        (
            "profile".into(),
            Value::from(if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }),
        ),
        ("mode".into(), Value::from(preset.as_str())),
        ("samples".into(), Value::from(samples as u64)),
        (
            "target_sample_ms".into(),
            Value::from(suite_config.measure.target_sample_time.as_millis() as u64),
        ),
        (
            "selected_scenarios".into(),
            json!(selected
                .iter()
                .map(|scenario| scenario.as_str())
                .collect::<Vec<_>>()),
        ),
        (
            "audio_seconds".into(),
            Value::from(suite_config.audio_seconds),
        ),
        (
            "playlist_tracks".into(),
            Value::from(suite_config.playlist_tracks as u64),
        ),
        (
            "tui_size".into(),
            Value::from(format!(
                "{}x{}",
                suite_config.tui_width, suite_config.tui_height
            )),
        ),
    ]);
    let artifact = PerfRun {
        schema_version: 1,
        run_id,
        created_at: timestamp.iso8601,
        app_version: Some(env!("CARGO_PKG_VERSION").into()),
        git_sha: command_output("git", &["rev-parse", "HEAD"]),
        machine,
        config,
        scenarios: results,
    };

    let output = args.output.unwrap_or_else(|| {
        Path::new(".local")
            .join("perf_results")
            .join(format!("{}_{}.json", timestamp.file_stem, artifact.run_id))
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, serde_json::to_string_pretty(&artifact)? + "\n")?;

    println!("\nartifact={}", output.display());
    if let (Some(before), Some(after)) = (memory_before.resident_bytes, memory_after.resident_bytes)
    {
        println!(
            "process_rss_before={} process_rss_after={} delta={}",
            format_bytes(before),
            format_bytes(after),
            format_signed_bytes(after as i128 - before as i128)
        );
    }
    if let Some(peak) = memory_after.peak_resident_bytes {
        println!("process_peak_rss={}", format_bytes(peak));
    }
    Ok(())
}

fn compare(args: CompareArgs) -> Result<(), Box<dyn Error>> {
    if !args.threshold.is_finite() || args.threshold < 0.0 {
        return Err("--threshold must be a finite, non-negative percentage".into());
    }
    let baseline: PerfRun = serde_json::from_slice(&std::fs::read(&args.baseline)?)?;
    let candidate: PerfRun = serde_json::from_slice(&std::fs::read(&args.candidate)?)?;
    if baseline.config != candidate.config {
        eprintln!(
            "warning: artifact configs differ; only interpret metrics backed by equivalent workloads"
        );
    }
    let result = compare_runs(&baseline, &candidate, args.threshold);
    println!("Performance comparison");
    println!("baseline={}", baseline.run_id);
    println!("candidate={}", candidate.run_id);
    println!(
        "regressions={} improvements={} unchanged={} missing={} new={}",
        result.regressions.len(),
        result.improvements.len(),
        result.unchanged.len(),
        result.missing.len(),
        result.new.len()
    );
    print_deltas("Regressions", &result.regressions);
    print_deltas("Improvements", &result.improvements);
    if !result.missing.is_empty() {
        println!("Missing in candidate:");
        for key in &result.missing {
            println!("  {key}");
        }
    }
    if !result.new.is_empty() {
        println!("New in candidate:");
        for key in &result.new {
            println!("  {key}");
        }
    }
    Ok(())
}

fn attach_process_memory(
    result: &mut ScenarioResult,
    before: ProcessMemory,
    after: ProcessMemory,
) -> Result<(), Box<dyn Error>> {
    result.metadata.insert(
        "process_memory_before".into(),
        serde_json::to_value(before)?,
    );
    result
        .metadata
        .insert("process_memory_after".into(), serde_json::to_value(after)?);
    if let Some(bytes) = after.resident_bytes {
        result.metrics.insert(
            "process_resident_bytes_after".into(),
            MetricSummary::from_samples(&[bytes as f64], "bytes"),
        );
    }
    if let Some(bytes) = after.peak_resident_bytes {
        result.metrics.insert(
            "process_peak_resident_bytes".into(),
            MetricSummary::from_samples(&[bytes as f64], "bytes"),
        );
    }
    result.notes.push(
        "Process RSS is scenario-order dependent; compare runs with identical selected scenarios."
            .into(),
    );
    Ok(())
}

fn print_deltas(title: &str, deltas: &[metrics::MetricDelta]) {
    println!("{title}:");
    if deltas.is_empty() {
        println!("  none");
        return;
    }
    for delta in deltas {
        let percentage = delta
            .percent
            .map_or_else(|| "n/a".into(), |value| format!("{value:+.1}%"));
        println!(
            "  {} {:.3}->{:.3} {} ({})",
            delta.key, delta.baseline, delta.candidate, delta.unit, percentage
        );
    }
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = ProcessCommand::new(program).args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn optional_string_value(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

struct Timestamp {
    iso8601: String,
    file_stem: String,
}

fn utc_timestamp() -> Timestamp {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_seconds = duration.as_secs() as i64;
    let days = total_seconds.div_euclid(86_400);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let millis = duration.subsec_millis();
    Timestamp {
        iso8601: format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
        ),
        file_stem: format!(
            "{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}{millis:03}Z"
        ),
    }
}

// Howard Hinnant's civil-from-days algorithm, with day zero at Unix epoch.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.2} MiB", bytes as f64 / MIB)
}

fn format_signed_bytes(bytes: i128) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:+.2} MiB", bytes as f64 / MIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_date_conversion_is_correct() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_675), (2026, 8, 10));
    }

    #[test]
    fn labels_are_safe_for_filenames() {
        assert_eq!(sanitize_label("before/fast path"), "before_fast_path");
    }
}
