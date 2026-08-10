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

/// Summary row used by the saved-playlist chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub track_count: usize,
    pub updated_at: i64,
}

/// Joined transient editor-draft row. Unlike [`PlaylistRow`], the item may
/// refer to a path that has not yet been inserted into `tracks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub item_id: i64,
    pub pos_key: i64,
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub missing: bool,
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

/// Non-destructive playlist view order. Playback and editor ordering continue
/// to use `pos_key`; these modes only affect rows presented by frontends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlaylistSort {
    #[default]
    Playlist,
    Track,
    Artist,
    Album,
}

impl PlaylistSort {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "playlist" => Some(Self::Playlist),
            "track" | "title" => Some(Self::Track),
            "artist" => Some(Self::Artist),
            "album" => Some(Self::Album),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Track => "track",
            Self::Artist => "artist",
            Self::Album => "album",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Playlist => "Playlist",
            Self::Track => "Track",
            Self::Artist => "Artist",
            Self::Album => "Album",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Playlist => Self::Track,
            Self::Track => Self::Artist,
            Self::Artist => Self::Album,
            Self::Album => Self::Playlist,
        }
    }
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
