//! Playlist / track domain types shared by the store layer.

use std::path::PathBuf;

/// Joined playlist row including optional cached metadata fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistRow {
    pub item_id: i64,
    pub track_id: i64,
    pub pos_key: i64,
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: Option<i64>,
    pub meta_valid: Option<bool>,
    pub meta_error: Option<String>,
}

/// Minimal track record for metadata refresh workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    pub track_id: i64,
    pub path: PathBuf,
    pub mtime_ns: Option<i64>,
    pub size_bytes: Option<i64>,
}

/// Metadata payload persisted into `track_meta` (and file stats on `tracks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: Option<i64>,
    pub meta_valid: bool,
    pub meta_error: Option<String>,
    pub mtime_ns: Option<i64>,
    pub size_bytes: Option<i64>,
}

/// Read-only metadata snapshot used for change detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMetaSnapshot {
    pub track_id: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: Option<i64>,
    pub meta_valid: bool,
    pub meta_error: Option<String>,
}

/// Direction for one-step playlist reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    Up,
    Down,
}

impl MoveDirection {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}
