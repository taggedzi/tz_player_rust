//! Schema creation and migrations (Python SCHEMA_VERSION 7 parity baseline).

use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

use crate::error::DbError;

/// Current schema version (matches Python tz-player at rewrite start).
pub const SCHEMA_VERSION: i32 = 7;

/// Open a SQLite connection with foreign keys enabled.
pub fn open_connection(path: &Path) -> Result<Connection, DbError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(conn)
}

/// Create or migrate schema to [`SCHEMA_VERSION`].
pub fn create_schema(conn: &Connection) -> Result<(), DbError> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if version > SCHEMA_VERSION {
        return Err(DbError::UnsupportedVersion {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }

    let mut version = version;
    if version == 0 {
        create_schema_v1(conn)?;
        set_user_version(conn, 1)?;
        version = 1;
    }
    if version == 1 {
        migrate_v1_to_v2(conn)?;
        set_user_version(conn, 2)?;
        version = 2;
    }
    if version == 2 {
        migrate_v2_to_v3(conn)?;
        set_user_version(conn, 3)?;
        version = 3;
    }
    if version == 3 {
        migrate_v3_to_v4(conn)?;
        set_user_version(conn, 4)?;
        version = 4;
    }
    if version == 4 {
        migrate_v4_to_v5(conn)?;
        set_user_version(conn, 5)?;
        version = 5;
    }
    if version == 5 {
        migrate_v5_to_v6(conn)?;
        set_user_version(conn, 6)?;
        version = 6;
    }
    if version == 6 {
        migrate_v6_to_v7(conn)?;
        set_user_version(conn, SCHEMA_VERSION)?;
    }

    Ok(())
}

fn set_user_version(conn: &Connection, version: i32) -> Result<(), DbError> {
    conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    Ok(())
}

fn create_schema_v1(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            path_norm TEXT NOT NULL UNIQUE,
            mtime_ns INTEGER,
            size_bytes INTEGER,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE TABLE IF NOT EXISTS track_meta (
            track_id INTEGER PRIMARY KEY,
            title TEXT,
            artist TEXT,
            album TEXT,
            year INTEGER,
            duration_ms INTEGER,
            meta_loaded_at INTEGER,
            meta_valid INTEGER NOT NULL DEFAULT 0,
            meta_error TEXT,
            FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS playlists (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE TABLE IF NOT EXISTS playlist_items (
            playlist_id INTEGER NOT NULL,
            track_id INTEGER NOT NULL,
            pos_key INTEGER NOT NULL,
            added_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
            FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_tracks_path_norm ON tracks(path_norm);
        CREATE INDEX IF NOT EXISTS idx_track_meta_title ON track_meta(title);
        CREATE INDEX IF NOT EXISTS idx_track_meta_artist ON track_meta(artist);
        CREATE INDEX IF NOT EXISTS idx_track_meta_album ON track_meta(album);
        CREATE INDEX IF NOT EXISTS idx_track_meta_valid ON track_meta(meta_valid);
        CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist_pos ON playlist_items(playlist_id, pos_key);
        CREATE INDEX IF NOT EXISTS idx_playlist_items_track ON playlist_items(track_id);
        "#,
    )?;
    Ok(())
}

fn migrate_v1_to_v2(conn: &Connection) -> Result<(), DbError> {
    // Add stable playlist_items.id (Python v1→v2).
    let has_id: bool = conn
        .prepare("PRAGMA table_info(playlist_items)")?
        .query_map([], |row| {
            let name: String = row.get(1)?;
            Ok(name)
        })?
        .filter_map(|r| r.ok())
        .any(|n| n == "id");

    if has_id {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS playlist_items_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            playlist_id INTEGER NOT NULL,
            track_id INTEGER NOT NULL,
            pos_key INTEGER NOT NULL,
            added_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
            FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
        );
        INSERT INTO playlist_items_new (playlist_id, track_id, pos_key, added_at)
        SELECT playlist_id, track_id, pos_key, added_at FROM playlist_items;
        DROP TABLE playlist_items;
        ALTER TABLE playlist_items_new RENAME TO playlist_items;
        CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist_pos
            ON playlist_items(playlist_id, pos_key);
        CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist_track
            ON playlist_items(playlist_id, track_id);
        COMMIT;
        "#,
    )?;
    Ok(())
}

fn migrate_v2_to_v3(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS audio_envelopes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path_norm TEXT NOT NULL UNIQUE,
            mtime_ns INTEGER,
            size_bytes INTEGER,
            duration_ms INTEGER NOT NULL,
            analysis_version INTEGER NOT NULL,
            computed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE TABLE IF NOT EXISTS audio_envelope_points (
            envelope_id INTEGER NOT NULL,
            position_ms INTEGER NOT NULL,
            level_left REAL NOT NULL,
            level_right REAL NOT NULL,
            PRIMARY KEY (envelope_id, position_ms),
            FOREIGN KEY(envelope_id) REFERENCES audio_envelopes(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_audio_envelopes_path_norm ON audio_envelopes(path_norm);
        CREATE INDEX IF NOT EXISTS idx_audio_points_envelope_pos
            ON audio_envelope_points(envelope_id, position_ms);
        COMMIT;
        "#,
    )?;
    Ok(())
}

fn migrate_v3_to_v4(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS analysis_cache_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            analysis_type TEXT NOT NULL,
            path_norm TEXT NOT NULL,
            mtime_ns INTEGER,
            size_bytes INTEGER,
            analysis_version INTEGER NOT NULL,
            params_hash TEXT NOT NULL,
            params_json TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            frame_count INTEGER NOT NULL DEFAULT 0,
            byte_size INTEGER NOT NULL DEFAULT 0,
            computed_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            last_accessed_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            UNIQUE(path_norm, mtime_ns, size_bytes, analysis_type, analysis_version, params_hash)
        );
        CREATE TABLE IF NOT EXISTS analysis_scalar_frames (
            entry_id INTEGER NOT NULL,
            position_ms INTEGER NOT NULL,
            level_left REAL NOT NULL,
            level_right REAL NOT NULL,
            PRIMARY KEY (entry_id, position_ms),
            FOREIGN KEY(entry_id) REFERENCES analysis_cache_entries(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS analysis_spectrum_frames (
            entry_id INTEGER NOT NULL,
            frame_idx INTEGER NOT NULL,
            position_ms INTEGER NOT NULL,
            bands BLOB NOT NULL,
            PRIMARY KEY (entry_id, frame_idx),
            FOREIGN KEY(entry_id) REFERENCES analysis_cache_entries(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_analysis_cache_lookup
            ON analysis_cache_entries(analysis_type, path_norm, analysis_version, params_hash);
        CREATE INDEX IF NOT EXISTS idx_analysis_cache_access
            ON analysis_cache_entries(last_accessed_at);
        CREATE INDEX IF NOT EXISTS idx_analysis_cache_computed
            ON analysis_cache_entries(computed_at);
        CREATE INDEX IF NOT EXISTS idx_analysis_scalar_pos
            ON analysis_scalar_frames(entry_id, position_ms);
        CREATE INDEX IF NOT EXISTS idx_analysis_spectrum_pos
            ON analysis_spectrum_frames(entry_id, position_ms);
        COMMIT;
        "#,
    )?;
    Ok(())
}

fn migrate_v4_to_v5(conn: &Connection) -> Result<(), DbError> {
    ensure_playlist_search_fts(conn)?;
    Ok(())
}

/// Create FTS5 playlist search index + sync triggers (idempotent).
///
/// Mirrors Python `schema._create_playlist_search_fts` so LIKE fallback is only
/// needed when FTS5 is unavailable.
pub fn ensure_playlist_search_fts(conn: &Connection) -> Result<(), DbError> {
    if !table_exists(conn, "tracks")? || !table_exists(conn, "playlist_items")? {
        return Ok(());
    }

    // Drop incomplete early scaffold tables (missing `year` column).
    if table_exists(conn, "playlist_search")? && !fts_has_year_column(conn)? {
        tracing::info!("Recreating playlist_search FTS table with full column set");
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS playlist_search;
            DROP TRIGGER IF EXISTS trg_playlist_search_item_insert;
            DROP TRIGGER IF EXISTS trg_playlist_search_item_update;
            DROP TRIGGER IF EXISTS trg_playlist_search_item_delete;
            DROP TRIGGER IF EXISTS trg_playlist_search_track_path_update;
            DROP TRIGGER IF EXISTS trg_playlist_search_track_meta_insert;
            DROP TRIGGER IF EXISTS trg_playlist_search_track_meta_update;
            DROP TRIGGER IF EXISTS trg_playlist_search_track_meta_delete;
            "#,
        )?;
    }

    let create = conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS playlist_search USING fts5(
            item_id UNINDEXED,
            playlist_id UNINDEXED,
            title,
            artist,
            album,
            year,
            path,
            tokenize = 'unicode61 remove_diacritics 2'
        );
        "#,
    );
    if let Err(e) = create {
        let msg = e.to_string().to_ascii_lowercase();
        if msg.contains("fts5") || msg.contains("no such module") {
            tracing::warn!("FTS5 playlist_search unavailable: {e}");
            return Ok(());
        }
        return Err(DbError::Sqlite(e));
    }

    // Backfill only when empty (fresh create or rebuild).
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM playlist_search", [], |row| row.get(0))
        .unwrap_or(0);
    if count == 0 && table_exists(conn, "playlist_items")? {
        conn.execute_batch(
            r#"
            INSERT INTO playlist_search (
                rowid, item_id, playlist_id, title, artist, album, year, path
            )
            SELECT
                pi.id,
                pi.id,
                pi.playlist_id,
                COALESCE(tm.title, ''),
                COALESCE(tm.artist, ''),
                COALESCE(tm.album, ''),
                COALESCE(CAST(tm.year AS TEXT), ''),
                COALESCE(t.path, '')
            FROM playlist_items AS pi
            JOIN tracks AS t ON t.id = pi.track_id
            LEFT JOIN track_meta AS tm ON tm.track_id = t.id;
            "#,
        )?;
    }

    conn.execute_batch(
        r#"
        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_item_insert
        AFTER INSERT ON playlist_items
        BEGIN
            INSERT INTO playlist_search (
                rowid, item_id, playlist_id, title, artist, album, year, path
            )
            SELECT
                NEW.id,
                NEW.id,
                NEW.playlist_id,
                COALESCE(tm.title, ''),
                COALESCE(tm.artist, ''),
                COALESCE(tm.album, ''),
                COALESCE(CAST(tm.year AS TEXT), ''),
                COALESCE(t.path, '')
            FROM tracks AS t
            LEFT JOIN track_meta AS tm ON tm.track_id = t.id
            WHERE t.id = NEW.track_id;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_item_update
        AFTER UPDATE OF track_id, playlist_id ON playlist_items
        BEGIN
            INSERT OR REPLACE INTO playlist_search (
                rowid, item_id, playlist_id, title, artist, album, year, path
            )
            SELECT
                NEW.id,
                NEW.id,
                NEW.playlist_id,
                COALESCE(tm.title, ''),
                COALESCE(tm.artist, ''),
                COALESCE(tm.album, ''),
                COALESCE(CAST(tm.year AS TEXT), ''),
                COALESCE(t.path, '')
            FROM tracks AS t
            LEFT JOIN track_meta AS tm ON tm.track_id = t.id
            WHERE t.id = NEW.track_id;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_item_delete
        AFTER DELETE ON playlist_items
        BEGIN
            DELETE FROM playlist_search WHERE rowid = OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_track_path_update
        AFTER UPDATE OF path ON tracks
        BEGIN
            UPDATE playlist_search
            SET path = COALESCE(NEW.path, '')
            WHERE rowid IN (
                SELECT id FROM playlist_items WHERE track_id = NEW.id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_track_meta_insert
        AFTER INSERT ON track_meta
        BEGIN
            UPDATE playlist_search
            SET
                title = COALESCE(NEW.title, ''),
                artist = COALESCE(NEW.artist, ''),
                album = COALESCE(NEW.album, ''),
                year = COALESCE(CAST(NEW.year AS TEXT), '')
            WHERE rowid IN (
                SELECT id FROM playlist_items WHERE track_id = NEW.track_id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_track_meta_update
        AFTER UPDATE OF title, artist, album, year ON track_meta
        BEGIN
            UPDATE playlist_search
            SET
                title = COALESCE(NEW.title, ''),
                artist = COALESCE(NEW.artist, ''),
                album = COALESCE(NEW.album, ''),
                year = COALESCE(CAST(NEW.year AS TEXT), '')
            WHERE rowid IN (
                SELECT id FROM playlist_items WHERE track_id = NEW.track_id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_playlist_search_track_meta_delete
        AFTER DELETE ON track_meta
        BEGIN
            UPDATE playlist_search
            SET title = '', artist = '', album = '', year = ''
            WHERE rowid IN (
                SELECT id FROM playlist_items WHERE track_id = OLD.track_id
            );
        END;
        "#,
    )?;

    Ok(())
}

fn migrate_v5_to_v6(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS analysis_beat_frames (
            entry_id INTEGER NOT NULL,
            frame_idx INTEGER NOT NULL,
            position_ms INTEGER NOT NULL,
            strength_u8 INTEGER NOT NULL,
            is_beat INTEGER NOT NULL DEFAULT 0,
            bpm REAL NOT NULL DEFAULT 0.0,
            PRIMARY KEY (entry_id, frame_idx),
            FOREIGN KEY(entry_id) REFERENCES analysis_cache_entries(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_analysis_beat_pos
            ON analysis_beat_frames(entry_id, position_ms);
        COMMIT;
        "#,
    )?;
    Ok(())
}

fn migrate_v6_to_v7(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS analysis_waveform_proxy_frames (
            entry_id INTEGER NOT NULL,
            frame_idx INTEGER NOT NULL,
            position_ms INTEGER NOT NULL,
            min_left_i8 INTEGER NOT NULL,
            max_left_i8 INTEGER NOT NULL,
            min_right_i8 INTEGER NOT NULL,
            max_right_i8 INTEGER NOT NULL,
            PRIMARY KEY (entry_id, frame_idx),
            FOREIGN KEY(entry_id) REFERENCES analysis_cache_entries(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_analysis_waveform_proxy_pos
            ON analysis_waveform_proxy_frames(entry_id, position_ms);
        COMMIT;
        "#,
    )?;
    Ok(())
}

fn fts_has_year_column(conn: &Connection) -> Result<bool, DbError> {
    // fts5 virtual tables expose columns via pragma table_info.
    let mut stmt = conn.prepare("PRAGMA table_info(playlist_search)")?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    // Some SQLite builds return empty table_info for fts5; probe with a query.
    if cols.is_empty() {
        let probe = conn.execute("SELECT year FROM playlist_search LIMIT 0", []);
        return Ok(probe.is_ok());
    }
    Ok(cols.iter().any(|c| c == "year"))
}

/// Helper for tests: read current user_version.
pub fn user_version(conn: &Connection) -> Result<i32, DbError> {
    let v = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(v)
}

/// True if a table exists.
pub fn table_exists(conn: &Connection, name: &str) -> Result<bool, DbError> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_database;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tz_player3_schema_{nanos}.db"))
    }

    #[test]
    fn fresh_db_reaches_schema_v7() {
        let path = temp_db_path();
        let conn = open_database(&path).expect("open");
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(table_exists(&conn, "tracks").unwrap());
        assert!(table_exists(&conn, "playlist_items").unwrap());
        assert!(table_exists(&conn, "analysis_cache_entries").unwrap());
        assert!(table_exists(&conn, "analysis_beat_frames").unwrap());
        assert!(table_exists(&conn, "analysis_waveform_proxy_frames").unwrap());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn playlist_items_has_id_column() {
        let path = temp_db_path();
        let conn = open_database(&path).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(playlist_items)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.iter().any(|c| c == "id"));
        let _ = std::fs::remove_file(&path);
    }
}
