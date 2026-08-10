use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

/// System allocator wrapper used only by the benchmark executable.
pub struct CountingAllocator;

// SAFETY: Every operation delegates to `System` with the original pointer and
// layout. The atomic accounting has no effect on allocator correctness.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies the layout required by `GlobalAlloc`.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies the layout required by `GlobalAlloc`.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: The caller guarantees that `pointer` and `layout` came from
        // this allocator; this wrapper delegates directly to `System`.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocated pointer, its layout,
        // and the requested replacement size as required by `GlobalAlloc`.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            if new_size >= layout.size() {
                record_live_growth((new_size - layout.size()) as u64);
            } else {
                LIVE_BYTES.fetch_sub((layout.size() - new_size) as u64, Ordering::Relaxed);
            }
        }
        replacement
    }
}

fn record_allocation(bytes: usize) {
    ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    record_live_growth(bytes as u64);
}

fn record_live_growth(bytes: u64) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
}

#[derive(Debug, Clone, Copy)]
struct AllocationSnapshot {
    count: u64,
    bytes: u64,
}

fn allocation_snapshot() -> AllocationSnapshot {
    AllocationSnapshot {
        count: ALLOCATION_COUNT.load(Ordering::Relaxed),
        bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricSummary {
    pub unit: String,
    pub count: usize,
    pub min_value: f64,
    pub median_value: f64,
    pub p95_value: f64,
    pub max_value: f64,
    pub mean_value: f64,
}

impl MetricSummary {
    pub fn from_samples(samples: &[f64], unit: &str) -> Self {
        assert!(!samples.is_empty(), "metric samples must not be empty");
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let mean_value = sorted.iter().sum::<f64>() / sorted.len() as f64;
        Self {
            unit: unit.into(),
            count: sorted.len(),
            min_value: sorted[0],
            median_value: percentile(&sorted, 0.5),
            p95_value: percentile(&sorted, 0.95),
            max_value: sorted[sorted.len() - 1],
            mean_value,
        }
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (sorted.len() - 1) as f64 * percentile;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    sorted[lower] + (sorted[upper] - sorted[lower]) * (rank - lower as f64)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario_id: String,
    pub category: String,
    pub status: String,
    pub elapsed_s: f64,
    #[serde(default)]
    pub metrics: BTreeMap<String, MetricSummary>,
    #[serde(default)]
    pub counters: BTreeMap<String, Value>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfRun {
    pub schema_version: u32,
    pub run_id: String,
    pub created_at: String,
    pub app_version: Option<String>,
    pub git_sha: Option<String>,
    #[serde(default)]
    pub machine: BTreeMap<String, Value>,
    #[serde(default)]
    pub config: BTreeMap<String, Value>,
    #[serde(default)]
    pub scenarios: Vec<ScenarioResult>,
}

#[derive(Debug, Clone, Copy)]
pub struct MeasureConfig {
    pub samples: usize,
    pub warm_up: usize,
    pub target_sample_time: Duration,
}

#[derive(Debug, Clone)]
pub struct CaseMeasurement {
    pub name: String,
    pub latency_ms: MetricSummary,
    pub allocations: MetricSummary,
    pub allocated_bytes: MetricSummary,
    pub peak_live_bytes: MetricSummary,
    pub iterations_per_sample: usize,
    pub throughput_value: f64,
    pub throughput_unit: String,
    pub elapsed: Duration,
}

pub fn measure_case<F>(
    name: &str,
    work_per_operation: f64,
    throughput_unit: &str,
    config: MeasureConfig,
    mut operation: F,
) -> Result<CaseMeasurement, String>
where
    F: FnMut() -> Result<u64, String>,
{
    let started = Instant::now();
    for _ in 0..config.warm_up {
        black_box(operation()?);
    }

    let iterations_per_sample = calibrate_iterations(config.target_sample_time, &mut operation)?;
    let mut latency_ms = Vec::with_capacity(config.samples);
    let mut allocations = Vec::with_capacity(config.samples);
    let mut allocated_bytes = Vec::with_capacity(config.samples);
    let mut peak_live_bytes = Vec::with_capacity(config.samples);

    for _ in 0..config.samples {
        let allocation_before = allocation_snapshot();
        let live_before = LIVE_BYTES.load(Ordering::Relaxed);
        PEAK_LIVE_BYTES.store(live_before, Ordering::Relaxed);
        let sample_started = Instant::now();
        let mut checksum = 0u64;
        for _ in 0..iterations_per_sample {
            checksum = checksum.rotate_left(7) ^ black_box(operation()?);
        }
        let sample_elapsed = sample_started.elapsed();
        let allocation_after = allocation_snapshot();
        black_box(checksum);

        let iterations = iterations_per_sample as f64;
        latency_ms.push(sample_elapsed.as_secs_f64() * 1000.0 / iterations);
        allocations.push(
            allocation_after
                .count
                .saturating_sub(allocation_before.count) as f64
                / iterations,
        );
        allocated_bytes.push(
            allocation_after
                .bytes
                .saturating_sub(allocation_before.bytes) as f64
                / iterations,
        );
        peak_live_bytes.push(
            PEAK_LIVE_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(live_before) as f64,
        );
    }

    let latency_ms = MetricSummary::from_samples(&latency_ms, "ms");
    let throughput_value = if latency_ms.median_value > 0.0 {
        work_per_operation / (latency_ms.median_value / 1000.0)
    } else {
        0.0
    };
    Ok(CaseMeasurement {
        name: name.into(),
        latency_ms,
        allocations: MetricSummary::from_samples(&allocations, "allocations/op"),
        allocated_bytes: MetricSummary::from_samples(&allocated_bytes, "bytes/op"),
        peak_live_bytes: MetricSummary::from_samples(&peak_live_bytes, "bytes/op"),
        iterations_per_sample,
        throughput_value,
        throughput_unit: throughput_unit.into(),
        elapsed: started.elapsed(),
    })
}

fn calibrate_iterations<F>(target: Duration, operation: &mut F) -> Result<usize, String>
where
    F: FnMut() -> Result<u64, String>,
{
    const MAX_ITERATIONS: usize = 1 << 20;
    let mut iterations = 1usize;
    loop {
        let started = Instant::now();
        let mut checksum = 0u64;
        for _ in 0..iterations {
            checksum ^= black_box(operation()?);
        }
        let elapsed = started.elapsed();
        black_box(checksum);
        if elapsed >= target || iterations >= MAX_ITERATIONS {
            return Ok(iterations);
        }
        let elapsed_nanos = elapsed.as_nanos().max(1);
        let multiplier = (target.as_nanos() / elapsed_nanos).clamp(2, 16) as usize;
        iterations = iterations.saturating_mul(multiplier).min(MAX_ITERATIONS);
    }
}

pub fn scenario_from_cases(
    scenario_id: &str,
    category: &str,
    cases: Vec<CaseMeasurement>,
    mut counters: BTreeMap<String, Value>,
    metadata: BTreeMap<String, Value>,
    notes: Vec<String>,
) -> ScenarioResult {
    let mut metrics = BTreeMap::new();
    let elapsed_s = cases.iter().map(|case| case.elapsed.as_secs_f64()).sum();
    for case in cases {
        metrics.insert(format!("{}_latency_ms", case.name), case.latency_ms);
        metrics.insert(format!("{}_allocations", case.name), case.allocations);
        metrics.insert(
            format!("{}_allocated_bytes", case.name),
            case.allocated_bytes,
        );
        metrics.insert(
            format!("{}_peak_live_bytes", case.name),
            case.peak_live_bytes,
        );
        counters.insert(
            format!("{}_iterations_per_sample", case.name),
            Value::from(case.iterations_per_sample as u64),
        );
        counters.insert(
            format!("{}_throughput", case.name),
            Value::from(case.throughput_value),
        );
        counters.insert(
            format!("{}_throughput_unit", case.name),
            Value::from(case.throughput_unit),
        );
    }
    ScenarioResult {
        scenario_id: scenario_id.into(),
        category: category.into(),
        status: "pass".into(),
        elapsed_s,
        metrics,
        counters,
        metadata,
        notes,
    }
}

pub fn print_case(measurement: &CaseMeasurement) {
    println!(
        "{:<34} {:>10.3} {:>10.3} {:>12.1} {:>12.1} {:>12.1}  {:>10.1} {}",
        measurement.name,
        measurement.latency_ms.median_value,
        measurement.latency_ms.p95_value,
        measurement.allocations.median_value,
        measurement.allocated_bytes.median_value,
        measurement.peak_live_bytes.median_value,
        measurement.throughput_value,
        measurement.throughput_unit,
    );
}

pub fn print_table_header() {
    println!(
        "{:<34} {:>10} {:>10} {:>12} {:>12} {:>12}  {:>10}",
        "case", "median ms", "p95 ms", "allocs/op", "bytes/op", "peak live", "throughput"
    );
    println!("{}", "-".repeat(118));
}

#[derive(Debug)]
pub struct MetricDelta {
    pub key: String,
    pub unit: String,
    pub baseline: f64,
    pub candidate: f64,
    pub percent: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub regressions: Vec<MetricDelta>,
    pub improvements: Vec<MetricDelta>,
    pub unchanged: Vec<MetricDelta>,
    pub missing: Vec<String>,
    pub new: Vec<String>,
}

pub fn compare_runs(baseline: &PerfRun, candidate: &PerfRun, threshold: f64) -> Comparison {
    let baseline_metrics = flatten_metrics(baseline);
    let candidate_metrics = flatten_metrics(candidate);
    let mut comparison = Comparison::default();

    for (key, baseline_metric) in &baseline_metrics {
        let Some(candidate_metric) = candidate_metrics.get(key) else {
            comparison.missing.push(key.clone());
            continue;
        };
        if baseline_metric.unit != candidate_metric.unit {
            comparison.missing.push(key.clone());
            continue;
        }
        let delta = candidate_metric.median_value - baseline_metric.median_value;
        let percent = if baseline_metric.median_value == 0.0 {
            None
        } else {
            Some(delta / baseline_metric.median_value * 100.0)
        };
        let row = MetricDelta {
            key: key.clone(),
            unit: baseline_metric.unit.clone(),
            baseline: baseline_metric.median_value,
            candidate: candidate_metric.median_value,
            percent,
        };
        match percent {
            Some(value) if value >= threshold => comparison.regressions.push(row),
            Some(value) if value <= -threshold => comparison.improvements.push(row),
            _ => comparison.unchanged.push(row),
        }
    }
    for key in candidate_metrics.keys() {
        if !baseline_metrics.contains_key(key) {
            comparison.new.push(key.clone());
        }
    }
    comparison.regressions.sort_by(|left, right| {
        right
            .percent
            .unwrap_or(0.0)
            .total_cmp(&left.percent.unwrap_or(0.0))
    });
    comparison.improvements.sort_by(|left, right| {
        left.percent
            .unwrap_or(0.0)
            .total_cmp(&right.percent.unwrap_or(0.0))
    });
    comparison
}

fn flatten_metrics(run: &PerfRun) -> BTreeMap<String, &MetricSummary> {
    let mut flattened = BTreeMap::new();
    for scenario in &run.scenarios {
        for (name, metric) in &scenario.metrics {
            flattened.insert(format!("{}.{}", scenario.scenario_id, name), metric);
        }
    }
    flattened
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProcessMemory {
    pub resident_bytes: Option<u64>,
    pub peak_resident_bytes: Option<u64>,
}

pub fn process_memory() -> ProcessMemory {
    platform_process_memory()
}

#[cfg(target_os = "linux")]
fn platform_process_memory() -> ProcessMemory {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return ProcessMemory {
            resident_bytes: None,
            peak_resident_bytes: None,
        };
    };
    let parse_kib = |label: &str| {
        status.lines().find_map(|line| {
            line.strip_prefix(label)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|value| value * 1024)
        })
    };
    ProcessMemory {
        resident_bytes: parse_kib("VmRSS:"),
        peak_resident_bytes: parse_kib("VmHWM:"),
    }
}

#[cfg(target_os = "macos")]
fn platform_process_memory() -> ProcessMemory {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output();
    let resident_bytes = output.ok().and_then(|output| {
        String::from_utf8(output.stdout)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .map(|value| value * 1024)
    });
    ProcessMemory {
        resident_bytes,
        peak_resident_bytes: None,
    }
}

#[cfg(windows)]
fn platform_process_memory() -> ProcessMemory {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }

    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // SAFETY: `GetCurrentProcess` returns a process pseudo-handle valid for the
    // current process. `counters` is initialized and its exact byte size is
    // passed to the Windows API.
    let success = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        )
    };
    if success == 0 {
        return ProcessMemory {
            resident_bytes: None,
            peak_resident_bytes: None,
        };
    }
    ProcessMemory {
        resident_bytes: Some(counters.working_set_size as u64),
        peak_resident_bytes: Some(counters.peak_working_set_size as u64),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_process_memory() -> ProcessMemory {
    ProcessMemory {
        resident_bytes: None,
        peak_resident_bytes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_matches_reference_percentile_method() {
        let summary = MetricSummary::from_samples(&[1.0, 2.0, 3.0, 4.0], "ms");
        assert_eq!(summary.median_value, 2.5);
        assert!((summary.p95_value - 3.85).abs() < 1e-12);
    }

    #[test]
    fn comparison_treats_larger_resource_values_as_regressions() {
        fn run(id: &str, value: f64) -> PerfRun {
            PerfRun {
                schema_version: 1,
                run_id: id.into(),
                created_at: "test".into(),
                app_version: None,
                git_sha: None,
                machine: BTreeMap::new(),
                config: BTreeMap::new(),
                scenarios: vec![ScenarioResult {
                    scenario_id: "scenario".into(),
                    category: "test".into(),
                    status: "pass".into(),
                    elapsed_s: 0.0,
                    metrics: BTreeMap::from([(
                        "latency_ms".into(),
                        MetricSummary::from_samples(&[value], "ms"),
                    )]),
                    counters: BTreeMap::new(),
                    metadata: BTreeMap::new(),
                    notes: Vec::new(),
                }],
            }
        }
        let comparison = compare_runs(&run("base", 10.0), &run("candidate", 12.0), 5.0);
        assert_eq!(comparison.regressions.len(), 1);
    }
}
