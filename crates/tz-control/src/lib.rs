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
    /// Cycle the non-destructive playlist view order.
    CyclePlaylistSort,
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
    /// Open the folder-browser modal (TUI fills its own navigation state).
    RequestAddFolder,
    /// Move the browser cursor up/down within the current directory listing.
    BrowseUp,
    BrowseDown,
    /// Descend into the highlighted directory, or add-and-close on a file.
    BrowseEnter,
    /// Add the highlighted file/folder (recursively, for a folder) and close.
    BrowseSelect,
    /// Go up one directory level (or to the drive list, at a drive root).
    BrowseParent,
    /// Close the browser without adding anything.
    BrowseCancel,
    /// Open the full-screen staged playlist editor.
    EditorOpen,
    EditorTab,
    EditorUp,
    EditorDown,
    EditorPageUp,
    EditorPageDown,
    EditorHome,
    EditorEnd,
    EditorParent,
    EditorDrives,
    EditorEnter,
    EditorAppend,
    EditorInsert,
    EditorRemove,
    EditorClear,
    EditorMoveUp,
    EditorMoveDown,
    EditorApply,
    EditorCancel,
    EditorSave,
    EditorSaveAs,
    EditorLoad,
    EditorRename,
    EditorDelete,
    /// Confirm or reject a pending editor action (partial scan, discard, etc.).
    EditorConfirm {
        yes: bool,
    },
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

    #[test]
    fn browse_commands_json_round_trip() {
        for cmd in [
            Command::RequestAddFolder,
            Command::BrowseUp,
            Command::BrowseDown,
            Command::BrowseEnter,
            Command::BrowseSelect,
            Command::BrowseParent,
            Command::BrowseCancel,
        ] {
            let json = serde_json::to_string(&cmd).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(cmd, back);
        }
        let json = serde_json::to_string(&Command::RequestAddFolder).unwrap();
        assert!(json.contains("request_add_folder"));
    }
}
