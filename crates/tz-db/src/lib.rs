//! SQLite persistence for playlists, metadata, and analysis caches.
//!
//! Schema targets Python tz-player `SCHEMA_VERSION = 7` semantic compatibility.

mod beat_store;
mod cache_pruner;
mod envelope_store;
mod error;
mod models;
mod path_util;
mod playlist_store;
mod schema;
mod spectrum_store;
mod waveform_store;

pub use beat_store::{BeatParams, BeatReading, BeatStore};
pub use cache_pruner::{AnalysisCachePruneResult, AnalysisCachePruner};
pub use envelope_store::EnvelopeStore;
pub use error::DbError;
pub use models::{
    DraftRow, MoveDirection, PlaylistRow, PlaylistSummary, TrackMeta, TrackMetaSnapshot,
    TrackRecord,
};
pub use playlist_store::PlaylistStore;
pub use schema::{
    create_schema, ensure_playlist_search_fts, open_connection, table_exists, user_version,
    SCHEMA_VERSION,
};
pub use spectrum_store::{SpectrumParams, SpectrumStore};
pub use waveform_store::{WaveformParams, WaveformReading, WaveformStore};

use rusqlite::Connection;
use std::path::Path;

/// Open (or create) the application database and apply migrations.
pub fn open_database(path: &Path) -> Result<Connection, DbError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DbError::Io(format!("create db parent {}: {e}", parent.display())))?;
    }
    let conn = open_connection(path)?;
    create_schema(&conn)?;
    // Idempotent: upgrades DBs created before full FTS trigger set.
    ensure_playlist_search_fts(&conn)?;
    Ok(conn)
}
