//! JSON persistence for user-facing app runtime state.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;
use tz_db::PlaylistSort;

use crate::clamp_speed;

/// Persisted application state (Python `AppState` parity).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppState {
    pub playlist_id: Option<i64>,
    pub current_item_id: Option<i64>,
    pub volume: f64,
    pub speed: f64,
    pub repeat_mode: String,
    pub shuffle: bool,
    /// Non-destructive playlist view order: playlist, track, artist, or album.
    #[serde(default = "default_playlist_sort")]
    pub playlist_sort: String,
    /// Playback backend: `"audio"` (default) or `"fake"`.
    #[serde(default = "default_backend")]
    pub playback_backend: String,
    pub visualizer_id: Option<String>,
    pub visualizer_fps: u32,
    pub visualizer_responsiveness_profile: String,
    pub visualizer_plugin_paths: Vec<String>,
    pub visualizer_plugin_security_mode: String,
    pub visualizer_plugin_runtime_mode: String,
    pub native_helper_enabled: bool,
    pub native_helper_timeout_s: f64,
    pub ansi_enabled: bool,
    pub log_level: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            playlist_id: None,
            current_item_id: None,
            volume: 1.0,
            speed: 1.0,
            repeat_mode: "off".into(),
            shuffle: false,
            playlist_sort: default_playlist_sort(),
            playback_backend: "audio".into(),
            visualizer_id: None,
            visualizer_fps: 10,
            visualizer_responsiveness_profile: "balanced".into(),
            visualizer_plugin_paths: Vec::new(),
            visualizer_plugin_security_mode: "warn".into(),
            visualizer_plugin_runtime_mode: "in-process".into(),
            native_helper_enabled: true,
            native_helper_timeout_s: 30.0,
            ansi_enabled: true,
            log_level: "INFO".into(),
        }
    }
}

impl AppState {
    /// Normalize out-of-range or invalid fields after load.
    pub fn sanitize(mut self) -> Self {
        if !self.volume.is_finite() || self.volume < 0.0 {
            self.volume = 0.0;
        } else if self.volume > 1.0 {
            // Accept 0..100 style from mistakes by clamping ratio-ish values.
            if self.volume <= 100.0 {
                self.volume = (self.volume / 100.0).clamp(0.0, 1.0);
            } else {
                self.volume = 1.0;
            }
        }
        self.speed = clamp_speed(self.speed);
        if self.visualizer_fps == 0 || self.visualizer_fps > 60 {
            self.visualizer_fps = 10;
        }
        if self.native_helper_timeout_s < 0.1 {
            self.native_helper_timeout_s = 30.0;
        }
        let backend = self.playback_backend.to_ascii_lowercase();
        self.playback_backend = match backend.as_str() {
            "fake" => "fake".into(),
            _ => "audio".into(),
        };
        self.playlist_sort = PlaylistSort::parse(&self.playlist_sort)
            .unwrap_or_default()
            .as_str()
            .into();
        self
    }
}

fn default_playlist_sort() -> String {
    PlaylistSort::Playlist.as_str().into()
}

fn default_backend() -> String {
    "audio".into()
}

/// Load state from disk, or defaults if missing/invalid. Returns optional user notice.
pub fn load_state_with_notice(path: &Path) -> (AppState, Option<String>) {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<AppState>(&raw) {
            Ok(state) => (state.sanitize(), None),
            Err(_) => (
                AppState::default(),
                Some(format!(
                    "State settings were reset to defaults.\n\
                     Likely cause: state file is corrupt or partially written.\n\
                     Next step: remove or repair '{}' and restart.",
                    path.display()
                )),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (AppState::default(), None),
        Err(_) => (
            AppState::default(),
            Some(format!(
                "State settings were reset to defaults.\n\
                 Likely cause: state file is unreadable due to permissions or IO issues.\n\
                 Next step: verify access to '{}' and restart.",
                path.display()
            )),
        ),
    }
}

/// Load state or defaults (ignore notice).
pub fn load_state(path: &Path) -> AppState {
    load_state_with_notice(path).0
}

/// Atomically write state JSON.
pub fn save_state(path: &Path, state: &AppState) -> Result<(), StateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| StateError::Io(e.to_string()))?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| StateError::Serde(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| StateError::Io(e.to_string()))?;
        f.write_all(json.as_bytes())
            .map_err(|e| StateError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| StateError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, path).map_err(|e| StateError::Io(e.to_string()))?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io: {0}")]
    Io(String),
    #[error("serde: {0}")]
    Serde(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state() -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tz_player3_state_{n}.json"))
    }

    #[test]
    fn round_trip_state() {
        let path = temp_state();
        let state = AppState {
            volume: 0.8,
            shuffle: true,
            playback_backend: "audio".into(),
            ..Default::default()
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path);
        assert_eq!(loaded.volume, 0.8);
        assert!(loaded.shuffle);
        assert_eq!(loaded.playback_backend, "audio");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_defaults() {
        let path = temp_state();
        let _ = fs::remove_file(&path);
        let (state, notice) = load_state_with_notice(&path);
        assert!(notice.is_none());
        assert_eq!(state.playback_backend, "audio");
    }

    #[test]
    fn invalid_json_resets() {
        let path = temp_state();
        fs::write(&path, "{not json").unwrap();
        let (state, notice) = load_state_with_notice(&path);
        assert!(notice.is_some());
        assert_eq!(state, AppState::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn all_backend_values_sanitize_and_round_trip() {
        for backend in ["vlc", "rodio", "audio", "fake"] {
            let path = temp_state();
            let state = AppState {
                playback_backend: backend.to_ascii_uppercase(),
                ..Default::default()
            };
            save_state(&path, &state).unwrap();
            let expected = if backend == "fake" { "fake" } else { "audio" };
            assert_eq!(load_state(&path).playback_backend, expected);
            let _ = fs::remove_file(path);
        }

        let state = AppState {
            playback_backend: "removed-backend".into(),
            ..Default::default()
        };
        assert_eq!(state.sanitize().playback_backend, "audio");
    }

    #[test]
    fn playlist_sort_is_backward_compatible_and_sanitized() {
        let mut old_state = serde_json::to_value(AppState::default()).unwrap();
        old_state.as_object_mut().unwrap().remove("playlist_sort");
        let loaded: AppState = serde_json::from_value(old_state).unwrap();
        assert_eq!(loaded.playlist_sort, "playlist");

        let invalid = AppState {
            playlist_sort: "unexpected".into(),
            ..Default::default()
        };
        assert_eq!(invalid.sanitize().playlist_sort, "playlist");
    }
}
