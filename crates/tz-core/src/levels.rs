//! Audio analysis resolution for visualizers: envelope + spectrum + beat + waveform caches.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tz_analysis::{
    analyze_beats_from_decoded, analyze_spectrum_from_decoded, analyze_track_envelope_default,
    analyze_waveform_proxy_from_decoded, decode_track_for_analysis,
};
use tz_db::{
    BeatParams, BeatStore, EnvelopeStore, SpectrumParams, SpectrumStore, WaveformParams,
    WaveformStore,
};

/// Source of the current level sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelSource {
    Envelope,
    Fallback,
    Missing,
}

impl LevelSource {
    pub fn as_str(self) -> &'static str {
        match self {
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
    active_path: Mutex<Option<PathBuf>>,
    analyzing: Mutex<Option<PathBuf>>,
}

impl LevelService {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        let envelope = EnvelopeStore::new(db_path.clone(), 50);
        let spectrum = SpectrumStore::new(db_path.clone(), SpectrumParams::default());
        let beat = BeatStore::new(db_path.clone(), BeatParams::default());
        let waveform = WaveformStore::new(db_path, WaveformParams::default());
        let _ = envelope.initialize();
        let _ = spectrum.initialize();
        let _ = beat.initialize();
        let _ = waveform.initialize();
        Self {
            envelope,
            spectrum,
            beat,
            waveform,
            active_path: Mutex::new(None),
            analyzing: Mutex::new(None),
        }
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

        if need_env {
            let env = analyze_track_envelope_default(path).map_err(|e| e.to_string())?;
            self.envelope
                .upsert_envelope(path, env.duration_ms, &env.points)
                .map_err(|e| e.to_string())?;
        }

        if need_spec || need_beat || need_wave {
            let decoded = decode_track_for_analysis(path).map_err(|e| e.to_string())?;
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
