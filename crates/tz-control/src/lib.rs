//! Structured command API for frontends (TUI, future headless/remote).
//!
//! All UIs should talk to the player through commands + snapshots, not by
//! reaching into VLC or the database directly.

use serde::{Deserialize, Serialize};

/// Commands that any frontend may issue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek {
        position_ms: u64,
    },
    SeekRelative {
        delta_ms: i64,
    },
    SetVolume {
        volume: u8,
    },
    SetSpeed {
        speed: f64,
    },
    VolumeDelta {
        delta: i16,
    },
    SpeedDelta {
        delta: f64,
    },
    CycleRepeat,
    ToggleShuffle,
    PlayItem {
        item_id: i64,
    },
    PlayCursor,
    CursorUp,
    CursorDown,
    PageUp,
    PageDown,
    /// Move the cursor to the currently-playing (or last-played) track.
    LocatePlaying,
    AddPaths {
        paths: Vec<String>,
    },
    /// Prompt/path for interactive add (TUI fills path string).
    RequestAddPath,
    RemoveSelected,
    ClearPlaylist,
    /// Confirm pending clear when TUI shows a confirm dialog.
    ConfirmClear {
        yes: bool,
    },
    CycleVisualizer,
    RefreshMetadata,
    /// Enter / exit find mode or apply query.
    SetFindQuery {
        query: String,
    },
    ClearFind,
    Quit,
}

/// Errors from the control layer.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("{0}")]
    Message(String),
}

/// Minimal transport snapshot for UI binding (expanded in later phases).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TransportSnapshot {
    pub status: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: u8,
    pub speed: f64,
    pub repeat_mode: String,
    pub shuffle: bool,
    pub item_id: Option<i64>,
    pub track_path: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub error: Option<String>,
    pub backend: String,
    pub playlist_id: Option<i64>,
    pub playlist_count: usize,
    pub cursor_index: usize,
    pub level_left: Option<f32>,
    pub level_right: Option<f32>,
    pub level_source: Option<String>,
    /// Quantized spectrum bands (0..=255 per band) for visualizers.
    pub spectrum_bands: Option<Vec<u8>>,
    pub spectrum_source: Option<String>,
    pub beat_strength: Option<f32>,
    pub beat_is_onset: Option<bool>,
    pub beat_bpm: Option<f32>,
    pub beat_source: Option<String>,
    /// Stereo min/max waveform-proxy reading at playhead (−1..=1).
    pub waveform_min_left: Option<f32>,
    pub waveform_max_left: Option<f32>,
    pub waveform_min_right: Option<f32>,
    pub waveform_max_right: Option<f32>,
    pub waveform_source: Option<String>,
    /// Recent (min_left, max_left, min_right, max_right) buckets, oldest first,
    /// for the scrolling waveform trace visualizer.
    pub waveform_history: Option<Vec<(f32, f32, f32, f32)>>,
    /// Compact analysis readiness (e.g. `ESBW`, `analyzing`, or None).
    pub analysis_status: Option<String>,
    pub visualizer_id: Option<String>,
    pub find_query: String,
    pub find_active: bool,
    pub confirm_clear: bool,
    pub input_mode: String,
    pub input_buffer: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_json_round_trip() {
        let cmd = Command::SetVolume { volume: 80 };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
        assert!(json.contains("set_volume"));
    }
}
