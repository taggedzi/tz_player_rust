//! Audio analysis resolution for visualizers: envelope + spectrum + beat + waveform caches.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tz_analysis::{
    analyze_beats_from_decoded, analyze_envelope_from_decoded, analyze_spectrum_from_decoded,
    analyze_waveform_proxy_from_decoded, decode_track_for_analysis,
};
use tz_db::{
    AnalysisCachePruner, BeatParams, BeatStore, EnvelopeStore, SpectrumParams, SpectrumStore,
    WaveformParams, WaveformStore,
};

/// Retention limits for the shared analysis cache. Defaults match the Python
/// reference (`ANALYSIS_CACHE_*` constants in app.py).
#[derive(Debug, Clone, Copy)]
pub struct CacheLimits {
    pub max_bytes: i64,
    pub max_age_days: i64,
    pub min_recent_tracks_protected: i64,
    pub prune_trigger_threshold: f64,
    /// Minimum time between threshold checks. `ensure_analysis` runs once per
    /// track on a folder-add, so this keeps a bulk add from doing one
    /// `SUM(byte_size)` scan (and possibly a full prune scan) per track.
    pub check_interval: Duration,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024 * 1024,
            max_age_days: 180,
            min_recent_tracks_protected: 200,
            prune_trigger_threshold: 0.90,
            check_interval: Duration::from_secs(30),
        }
    }
}

/// Source of the current level sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelSource {
    Live,
    Envelope,
    Fallback,
    Missing,
}

impl LevelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Envelope => "envelope",
            Self::Fallback => "fallback",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LevelSample {
    pub left: f32,
    pub right: f32,
    pub source: LevelSource,
}

#[derive(Debug, Clone)]
pub struct AnalysisSample {
    pub levels: LevelSample,
    pub spectrum_bands: Option<Vec<u8>>,
    pub spectrum_source: Option<&'static str>,
    pub beat_strength: Option<f32>,
    pub beat_is_onset: Option<bool>,
    pub beat_bpm: Option<f32>,
    pub beat_source: Option<&'static str>,
    pub waveform_min_left: Option<f32>,
    pub waveform_max_left: Option<f32>,
    pub waveform_min_right: Option<f32>,
    pub waveform_max_right: Option<f32>,
    pub waveform_source: Option<&'static str>,
    /// Recent (min_left, max_left, min_right, max_right) buckets, oldest first.
    pub waveform_history: Option<Vec<(f32, f32, f32, f32)>>,
}

/// Buckets of lookback kept for the scrolling waveform trace visualizer.
const WAVEFORM_HISTORY_LEN: usize = 200;

/// Manages envelope + spectrum + beat + waveform analysis and cache lookups.
pub struct LevelService {
    envelope: EnvelopeStore,
    spectrum: SpectrumStore,
    beat: BeatStore,
    waveform: WaveformStore,
    pruner: AnalysisCachePruner,
    cache_limits: CacheLimits,
    last_prune_check: Mutex<Option<Instant>>,
    active_path: Mutex<Option<PathBuf>>,
    analyzing: Mutex<Option<PathBuf>>,
}

impl LevelService {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self::with_cache_limits(db_path, CacheLimits::default())
    }

    pub fn with_cache_limits(db_path: impl Into<PathBuf>, cache_limits: CacheLimits) -> Self {
        let db_path = db_path.into();
        let envelope = EnvelopeStore::new(db_path.clone(), 50);
        let spectrum = SpectrumStore::new(db_path.clone(), SpectrumParams::default());
        let beat = BeatStore::new(db_path.clone(), BeatParams::default());
        let waveform = WaveformStore::new(db_path.clone(), WaveformParams::default());
        let pruner = AnalysisCachePruner::new(db_path);
        let _ = envelope.initialize();
        let _ = spectrum.initialize();
        let _ = beat.initialize();
        let _ = waveform.initialize();
        Self {
            envelope,
            spectrum,
            beat,
            waveform,
            pruner,
            cache_limits,
            last_prune_check: Mutex::new(None),
            active_path: Mutex::new(None),
            analyzing: Mutex::new(None),
        }
    }

    /// Evicts old/oversized analysis cache entries once the cache is at or
    /// above its trigger threshold. Cheap no-op when under threshold, and
    /// throttled to at most once per `cache_limits.check_interval` so a
    /// bulk folder-add (one `ensure_analysis` call per track) doesn't do a
    /// full cache scan per track.
    fn maybe_prune_cache(&self) {
        {
            let mut last = self.last_prune_check.lock().unwrap();
            let now = Instant::now();
            if let Some(prev) = *last {
                if now.duration_since(prev) < self.cache_limits.check_interval {
                    return;
                }
            }
            *last = Some(now);
        }
        let over_threshold = self
            .pruner
            .exceeds_threshold(
                self.cache_limits.max_bytes,
                self.cache_limits.prune_trigger_threshold,
            )
            .unwrap_or(false);
        if !over_threshold {
            return;
        }
        let _ = self.pruner.prune(
            self.cache_limits.max_bytes,
            self.cache_limits.max_age_days,
            self.cache_limits.min_recent_tracks_protected,
        );
    }

    pub fn ensure_analysis(&self, path: &Path) -> Result<(), String> {
        {
            let mut a = self.analyzing.lock().unwrap();
            if a.as_ref().is_some_and(|p| p == path) {
                return Ok(());
            }
            *a = Some(path.to_path_buf());
        }
        let result = self.ensure_analysis_inner(path);
        *self.analyzing.lock().unwrap() = None;
        result
    }

    pub fn ensure_envelope(&self, path: &Path) -> Result<(), String> {
        self.ensure_analysis(path)
    }

    fn ensure_analysis_inner(&self, path: &Path) -> Result<(), String> {
        let need_env = !self
            .envelope
            .has_envelope(path)
            .map_err(|e| e.to_string())?;
        let need_spec = !self
            .spectrum
            .has_spectrum(path)
            .map_err(|e| e.to_string())?;
        let need_beat = !self.beat.has_beats(path).map_err(|e| e.to_string())?;
        let need_wave = !self
            .waveform
            .has_waveform(path)
            .map_err(|e| e.to_string())?;
        if !need_env && !need_spec && !need_beat && !need_wave {
            return Ok(());
        }

        // Decode once under tz-analysis's byte/duration/time limits, then
        // derive every missing cache product from the same PCM allocation.
        let decoded = decode_track_for_analysis(path).map_err(|e| e.to_string())?;

        if need_env {
            let env = analyze_envelope_from_decoded(&decoded, self.envelope.bucket_ms())
                .map_err(|e| e.to_string())?;
            self.envelope
                .upsert_envelope(path, env.duration_ms, &env.points)
                .map_err(|e| e.to_string())?;
        }

        if need_spec || need_beat || need_wave {
            if need_spec {
                let params = self.spectrum.params();
                let spec =
                    analyze_spectrum_from_decoded(&decoded, params.band_count, params.hop_ms)
                        .map_err(|e| e.to_string())?;
                self.spectrum
                    .upsert_spectrum(path, spec.duration_ms, &spec.frames)
                    .map_err(|e| e.to_string())?;
            }
            if need_beat {
                let hop = self.beat.params().hop_ms;
                let beats = analyze_beats_from_decoded(&decoded, hop).map_err(|e| e.to_string())?;
                self.beat
                    .upsert_beats(path, beats.duration_ms, beats.bpm, &beats.frames)
                    .map_err(|e| e.to_string())?;
            }
            if need_wave {
                let hop = self.waveform.params().hop_ms;
                let wave = analyze_waveform_proxy_from_decoded(&decoded, hop)
                    .map_err(|e| e.to_string())?;
                self.waveform
                    .upsert_waveform(path, wave.duration_ms, &wave.frames)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.maybe_prune_cache();
        Ok(())
    }

    pub fn set_active_track(&self, path: Option<PathBuf>) {
        *self.active_path.lock().unwrap() = path;
    }

    /// True while a background `ensure_analysis` call is in progress.
    pub fn is_analyzing(&self) -> bool {
        self.analyzing.lock().unwrap().is_some()
    }

    /// Compact readiness flags for status UI: envelope / spectrum / beat / waveform.
    pub fn cache_flags(&self, path: &Path) -> (bool, bool, bool, bool) {
        (
            self.envelope.has_envelope(path).unwrap_or(false),
            self.spectrum.has_spectrum(path).unwrap_or(false),
            self.beat.has_beats(path).unwrap_or(false),
            self.waveform.has_waveform(path).unwrap_or(false),
        )
    }

    /// Short label for transport/status: analyzing, ready, partial, missing.
    pub fn readiness_label(&self, path: &Path) -> &'static str {
        if self.is_analyzing() {
            return "analyzing";
        }
        let (e, s, b, w) = self.cache_flags(path);
        match (e, s, b, w) {
            (true, true, true, true) => "ready",
            (false, false, false, false) => "missing",
            _ => "partial",
        }
    }

    pub fn sample_at(&self, path: &Path, position_ms: u64, status: &str) -> LevelSample {
        self.sample_all(path, position_ms, status).levels
    }

    pub fn sample_all(&self, path: &Path, position_ms: u64, status: &str) -> AnalysisSample {
        let levels = self.sample_levels(path, position_ms, status);
        let playing = matches!(status, "playing" | "paused");
        let spectrum_lookup = position_ms.saturating_add(self.spectrum.params().hop_ms / 2);
        let (spectrum_bands, spectrum_source) = if playing {
            match self.spectrum.get_bands_at(path, spectrum_lookup) {
                Ok(Some(b)) => (Some(b), Some("cache")),
                _ => (None, Some("missing")),
            }
        } else {
            (None, None)
        };
        let beat_lookup = position_ms.saturating_add(self.beat.params().hop_ms / 2);
        let (beat_strength, beat_is_onset, beat_bpm, beat_source) = if playing {
            match self.beat.get_beat_at(path, beat_lookup) {
                Ok(Some(b)) => (
                    Some(b.strength),
                    Some(b.is_onset),
                    Some(b.bpm),
                    Some("cache"),
                ),
                _ => (None, None, None, Some("missing")),
            }
        } else {
            (None, None, None, None)
        };
        let waveform_lookup = position_ms.saturating_add(self.waveform.params().hop_ms / 2);
        let (
            waveform_min_left,
            waveform_max_left,
            waveform_min_right,
            waveform_max_right,
            waveform_source,
        ) = if playing {
            match self.waveform.get_waveform_at(path, waveform_lookup) {
                Ok(Some(w)) => (
                    Some(w.min_left),
                    Some(w.max_left),
                    Some(w.min_right),
                    Some(w.max_right),
                    Some("cache"),
                ),
                _ => (None, None, None, None, Some("missing")),
            }
        } else {
            (None, None, None, None, None)
        };
        let waveform_history = if playing {
            match self
                .waveform
                .get_waveform_range(path, waveform_lookup, WAVEFORM_HISTORY_LEN)
            {
                Ok(rows) if rows.len() >= 2 => Some(
                    rows.into_iter()
                        .map(|w| (w.min_left, w.max_left, w.min_right, w.max_right))
                        .collect(),
                ),
                _ => None,
            }
        } else {
            None
        };
        AnalysisSample {
            levels,
            spectrum_bands,
            spectrum_source,
            beat_strength,
            beat_is_onset,
            beat_bpm,
            beat_source,
            waveform_min_left,
            waveform_max_left,
            waveform_min_right,
            waveform_max_right,
            waveform_source,
            waveform_history,
        }
    }

    fn sample_levels(&self, path: &Path, position_ms: u64, status: &str) -> LevelSample {
        if !matches!(status, "playing" | "paused") {
            return LevelSample {
                left: 0.0,
                right: 0.0,
                source: LevelSource::Missing,
            };
        }
        if status == "paused" {
            return LevelSample {
                left: 0.0,
                right: 0.0,
                source: LevelSource::Envelope,
            };
        }
        let lookup_pos = position_ms.saturating_add(self.envelope.bucket_ms() / 2);
        match self.envelope.get_level_at(path, lookup_pos) {
            Ok(Some((l, r))) => LevelSample {
                left: l,
                right: r,
                source: LevelSource::Envelope,
            },
            _ => {
                let t = position_ms as f32 / 1000.0;
                let left = (0.15 + 0.35 * (0.5 + 0.5 * (t * 5.4).sin())).clamp(0.0, 1.0);
                let right = (0.15 + 0.35 * (0.5 + 0.5 * (t * 6.1 + 1.2).sin())).clamp(0.0, 1.0);
                LevelSample {
                    left,
                    right,
                    source: LevelSource::Fallback,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tz_player3_levels_{name}_{nanos}.db"))
    }

    // byte_size for an envelope entry is points.len() * 24 (see EnvelopeStore::upsert_envelope).
    fn seed_envelope_entry(db_path: &Path) {
        let seed = EnvelopeStore::new(db_path, 50);
        seed.initialize().unwrap();
        let points: Vec<(u64, f32, f32)> = (0..10).map(|i| (i as u64 * 50, 0.1, 0.1)).collect();
        seed.upsert_envelope(Path::new("/tmp/seed-track.mp3"), 500, &points)
            .unwrap();
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tz-playback/tests/fixtures")
            .join(name)
    }

    fn assert_all_cache_products(name: &str) {
        let database = temp_db_path(name);
        let service = LevelService::new(&database);
        let media = fixture(name);
        service.ensure_analysis(&media).unwrap();
        assert_eq!(service.cache_flags(&media), (true, true, true, true));
        drop(service);
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn native_fixture_creates_all_analysis_products() {
        assert_all_cache_products("tone.wav");
    }

    #[test]
    fn helper_only_fixture_creates_all_analysis_products_when_injected() {
        if std::env::var_os("TZ_PLAYER_AUDIO_HELPER").is_none() {
            return;
        }
        assert_all_cache_products("tone-opus.ogg");
    }

    #[test]
    fn version_one_cache_rows_are_preserved_and_rebuilt_lazily_as_version_two() {
        let database = temp_db_path("lazy_v2");
        let media = fixture("tone.wav");
        let service = LevelService::new(&database);
        service.ensure_analysis(&media).unwrap();
        drop(service);

        let connection = tz_db::open_connection(&database).unwrap();
        assert_eq!(
            connection
                .execute(
                    "UPDATE analysis_cache_entries SET analysis_version = 1 WHERE analysis_version = 2",
                    [],
                )
                .unwrap(),
            4
        );
        drop(connection);

        let service = LevelService::new(&database);
        assert_eq!(service.cache_flags(&media), (false, false, false, false));
        service.ensure_analysis(&media).unwrap();
        assert_eq!(service.cache_flags(&media), (true, true, true, true));
        drop(service);

        let connection = tz_db::open_connection(&database).unwrap();
        let old_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM analysis_cache_entries WHERE analysis_version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let current_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM analysis_cache_entries WHERE analysis_version = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((old_count, current_count), (4, 4));
        drop(connection);
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn maybe_prune_cache_evicts_when_over_threshold() {
        let path = temp_db_path("prune_wiring");
        let service = LevelService::with_cache_limits(
            &path,
            CacheLimits {
                max_bytes: 100,
                max_age_days: 180,
                min_recent_tracks_protected: 0,
                prune_trigger_threshold: 0.0,
                check_interval: Duration::ZERO,
            },
        );
        seed_envelope_entry(&path);

        service.maybe_prune_cache();

        assert_eq!(service.pruner.total_cache_bytes().unwrap(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn maybe_prune_cache_is_noop_under_threshold() {
        let path = temp_db_path("prune_wiring_noop");
        let service = LevelService::with_cache_limits(
            &path,
            CacheLimits {
                max_bytes: 10_000,
                max_age_days: 180,
                min_recent_tracks_protected: 200,
                prune_trigger_threshold: 0.90,
                check_interval: Duration::ZERO,
            },
        );
        seed_envelope_entry(&path);

        service.maybe_prune_cache();

        assert_eq!(service.pruner.total_cache_bytes().unwrap(), 240);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn maybe_prune_cache_throttles_repeated_checks() {
        let path = temp_db_path("prune_wiring_throttle");
        let service = LevelService::with_cache_limits(
            &path,
            CacheLimits {
                max_bytes: 100,
                max_age_days: 180,
                min_recent_tracks_protected: 0,
                prune_trigger_threshold: 0.0,
                check_interval: Duration::from_secs(60),
            },
        );

        // First call always checks (no prior check recorded) and evicts.
        seed_envelope_entry(&path);
        service.maybe_prune_cache();
        assert_eq!(service.pruner.total_cache_bytes().unwrap(), 0);

        // Second call happens well within check_interval, so it must be
        // skipped even though the cache is over budget again.
        seed_envelope_entry(&path);
        service.maybe_prune_cache();
        assert_eq!(service.pruner.total_cache_bytes().unwrap(), 240);

        let _ = std::fs::remove_file(&path);
    }
}
