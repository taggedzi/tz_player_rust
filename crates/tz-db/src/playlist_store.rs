//! SQLite-backed playlist and track metadata store.
//!
//! Synchronous API (call from a worker/blocking thread from the UI). Behavior
//! mirrors Python `PlaylistStore` for parity.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};

use crate::error::DbError;
use crate::models::{
    DraftRow, MoveDirection, PlaylistRow, PlaylistSort, PlaylistSummary, TrackMeta,
    TrackMetaSnapshot, TrackRecord,
};
use crate::path_util::{normalize_path, stat_path};
use crate::{create_schema, ensure_playlist_search_fts, open_connection};

/// Sparse position key step (allows local reorders without full renumber).
pub const POS_STEP: i64 = 10_000;

/// Playlist CRUD, ordering, search, and track metadata helpers.
pub struct PlaylistStore {
    db_path: PathBuf,
}

impl PlaylistStore {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Ensure parent dirs, schema, WAL, and FTS are ready.
    pub fn initialize(&self) -> Result<(), DbError> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Io(format!("create db parent: {e}")))?;
        }
        let conn = self.connect()?;
        create_schema(&conn)?;
        ensure_playlist_search_fts(&conn)?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection, DbError> {
        open_connection(&self.db_path)
    }

    pub fn create_playlist(&self, name: &str) -> Result<i64, DbError> {
        let conn = self.connect()?;
        conn.execute("INSERT INTO playlists (name) VALUES (?1)", [name])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn ensure_playlist(&self, name: &str) -> Result<i64, DbError> {
        let conn = self.connect()?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1 LIMIT 1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        conn.execute("INSERT INTO playlists (name) VALUES (?1)", [name])?;
        Ok(conn.last_insert_rowid())
    }

    /// List user-visible playlists. The caller can exclude its protected
    /// working playlist by id; transient editor rows are never stored here.
    pub fn list_playlists(&self) -> Result<Vec<PlaylistSummary>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.updated_at,
                    (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id)
             FROM playlists p ORDER BY lower(p.name), p.id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PlaylistSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    updated_at: row.get(2)?,
                    track_count: row.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn playlist_name(&self, playlist_id: i64) -> Result<Option<String>, DbError> {
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT name FROM playlists WHERE id = ?1",
                [playlist_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn find_playlist_by_name_ci(&self, name: &str) -> Result<Option<i64>, DbError> {
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT id FROM playlists WHERE lower(name) = lower(?1) ORDER BY id LIMIT 1",
                [name],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn rename_playlist(&self, playlist_id: i64, name: &str) -> Result<(), DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let conflict: Option<i64> = tx
            .query_row(
                "SELECT id FROM playlists WHERE lower(name) = lower(?1) AND id != ?2 LIMIT 1",
                params![name, playlist_id],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            return Err(DbError::Schema(format!(
                "playlist name already exists: {name}"
            )));
        }
        let changed = tx.execute(
            "UPDATE playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, unix_now(), playlist_id],
        )?;
        if changed == 0 {
            return Err(DbError::Schema("playlist does not exist".into()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_playlist(&self, playlist_id: i64) -> Result<(), DbError> {
        let conn = self.connect()?;
        let changed = conn.execute("DELETE FROM playlists WHERE id = ?1", [playlist_id])?;
        if changed == 0 {
            return Err(DbError::Schema("playlist does not exist".into()));
        }
        Ok(())
    }

    /// Replace a session draft with the ordered contents of a playlist.
    pub fn draft_from_playlist(&self, session_id: &str, playlist_id: i64) -> Result<(), DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM playlist_editor_draft_items WHERE session_id = ?1",
            [session_id],
        )?;
        tx.execute(
            "INSERT INTO playlist_editor_draft_items (session_id, path, path_norm, pos_key)
             SELECT ?1, t.path, t.path_norm,
                    ROW_NUMBER() OVER (ORDER BY pi.pos_key) * ?2
             FROM playlist_items pi JOIN tracks t ON t.id = pi.track_id
             WHERE pi.playlist_id = ?3",
            params![session_id, POS_STEP, playlist_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn draft_count(&self, session_id: &str) -> Result<usize, DbError> {
        let conn = self.connect()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM playlist_editor_draft_items WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )? as usize)
    }

    pub fn fetch_draft_window(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<DraftRow>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT d.id, d.pos_key, d.path, tm.title, tm.artist, tm.album,
                    tm.duration_ms, CASE WHEN t.id IS NULL THEN 1 ELSE 0 END
             FROM playlist_editor_draft_items d
             LEFT JOIN tracks t ON t.path_norm = d.path_norm
             LEFT JOIN track_meta tm ON tm.track_id = t.id
             WHERE d.session_id = ?1 ORDER BY d.pos_key
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![session_id, limit as i64, offset as i64], |row| {
                Ok(DraftRow {
                    item_id: row.get(0)?,
                    pos_key: row.get(1)?,
                    path: PathBuf::from(row.get::<_, String>(2)?),
                    title: row.get(3)?,
                    artist: row.get(4)?,
                    album: row.get(5)?,
                    duration_ms: row.get(6)?,
                    missing: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn append_draft_paths(
        &self,
        session_id: &str,
        paths: &[PathBuf],
    ) -> Result<usize, DbError> {
        if paths.is_empty() {
            return Ok(0);
        }
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let max_pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(pos_key), 0) FROM playlist_editor_draft_items WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        insert_draft_paths(&tx, session_id, paths, max_pos)?;
        tx.commit()?;
        Ok(paths.len())
    }

    pub fn insert_draft_paths(
        &self,
        session_id: &str,
        index: usize,
        paths: &[PathBuf],
    ) -> Result<usize, DbError> {
        if paths.is_empty() {
            return Ok(0);
        }
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before: Option<i64> = tx
            .query_row(
                "SELECT pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
                 ORDER BY pos_key LIMIT 1 OFFSET ?2",
                params![session_id, index as i64],
                |row| row.get(0),
            )
            .optional()?;
        let after: i64 = if index == 0 {
            0
        } else {
            tx.query_row(
                "SELECT pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
                 ORDER BY pos_key LIMIT 1 OFFSET ?2",
                params![session_id, (index - 1) as i64],
                |row| row.get(0),
            )
            .unwrap_or(0)
        };
        let upper = before.unwrap_or(after.saturating_add(POS_STEP));
        let gap = upper.saturating_sub(after);
        if gap <= paths.len() as i64 {
            renumber_draft_tx(
                &tx,
                session_id,
                POS_STEP.saturating_mul(paths.len() as i64 + 1),
            )?;
        }
        let (lower, upper) = draft_bounds_tx(&tx, session_id, index)?;
        let step = ((upper - lower) / (paths.len() as i64 + 1)).max(1);
        for (i, path) in paths.iter().enumerate() {
            insert_draft_path_tx(&tx, session_id, path, lower + step * (i as i64 + 1))?;
        }
        tx.commit()?;
        Ok(paths.len())
    }

    pub fn remove_draft_at(&self, session_id: &str, index: usize) -> Result<bool, DbError> {
        let conn = self.connect()?;
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM playlist_editor_draft_items WHERE session_id = ?1
                 ORDER BY pos_key LIMIT 1 OFFSET ?2",
                params![session_id, index as i64],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(false) };
        Ok(conn.execute(
            "DELETE FROM playlist_editor_draft_items WHERE id = ?1",
            [id],
        )? > 0)
    }

    pub fn move_draft_at(&self, session_id: &str, index: usize, up: bool) -> Result<bool, DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let target: Option<(i64, i64)> = tx
            .query_row(
                "SELECT id, pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
                 ORDER BY pos_key LIMIT 1 OFFSET ?2",
                params![session_id, index as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((target_id, target_pos)) = target else {
            return Ok(false);
        };
        let neighbor_index = if up {
            index.saturating_sub(1)
        } else {
            index + 1
        };
        if up && index == 0 {
            return Ok(false);
        }
        let neighbor: Option<(i64, i64)> = tx
            .query_row(
                "SELECT id, pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
                 ORDER BY pos_key LIMIT 1 OFFSET ?2",
                params![session_id, neighbor_index as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((neighbor_id, neighbor_pos)) = neighbor else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE playlist_editor_draft_items SET pos_key = ?1 WHERE id = ?2",
            params![neighbor_pos, target_id],
        )?;
        tx.execute(
            "UPDATE playlist_editor_draft_items SET pos_key = ?1 WHERE id = ?2",
            params![target_pos, neighbor_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn clear_draft(&self, session_id: &str) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM playlist_editor_draft_items WHERE session_id = ?1",
            [session_id],
        )?;
        Ok(())
    }

    pub fn cleanup_editor_drafts(&self) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM playlist_editor_draft_items", [])?;
        Ok(())
    }

    pub fn load_draft_from_playlist(
        &self,
        session_id: &str,
        playlist_id: i64,
    ) -> Result<(), DbError> {
        self.draft_from_playlist(session_id, playlist_id)
    }

    pub fn replace_playlist_from_draft(
        &self,
        working_playlist_id: i64,
        session_id: &str,
    ) -> Result<(), DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            [working_playlist_id],
        )?;
        insert_draft_into_playlist_tx(&tx, session_id, working_playlist_id)?;
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![unix_now(), working_playlist_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_playlist_from_draft(
        &self,
        session_id: &str,
        name: &str,
        target_id: Option<i64>,
    ) -> Result<i64, DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let conflict: Option<i64> = tx
            .query_row(
                "SELECT id FROM playlists WHERE lower(name) = lower(?1) LIMIT 1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        let playlist_id = match (target_id, conflict) {
            (Some(id), Some(found)) if id == found => id,
            (Some(_), Some(_)) => {
                return Err(DbError::Schema(format!(
                    "playlist name already exists: {name}"
                )))
            }
            (Some(id), None) => {
                tx.execute(
                    "UPDATE playlists SET name = ?1 WHERE id = ?2",
                    params![name, id],
                )?;
                id
            }
            (None, Some(_)) => {
                return Err(DbError::Schema(format!(
                    "playlist name already exists: {name}"
                )))
            }
            (None, None) => {
                tx.execute("INSERT INTO playlists (name) VALUES (?1)", [name])?;
                tx.last_insert_rowid()
            }
        };
        tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            [playlist_id],
        )?;
        insert_draft_into_playlist_tx(&tx, session_id, playlist_id)?;
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![unix_now(), playlist_id],
        )?;
        tx.commit()?;
        Ok(playlist_id)
    }

    pub fn clear_playlist(&self, playlist_id: i64) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            [playlist_id],
        )?;
        Ok(())
    }

    /// Insert track references and append playlist items in stable order.
    /// Duplicate paths create multiple playlist items sharing one track row.
    pub fn add_tracks(&self, playlist_id: i64, paths: &[PathBuf]) -> Result<usize, DbError> {
        if paths.is_empty() {
            return Ok(0);
        }
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let max_pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(pos_key), 0) FROM playlist_items WHERE playlist_id = ?1",
            [playlist_id],
            |row| row.get(0),
        )?;
        let mut next_pos = max_pos + POS_STEP;
        let mut added = 0usize;

        for path in paths {
            let path_value = path.to_string_lossy().to_string();
            let path_norm = normalize_path(path);
            let track_id = match get_track_id(&tx, &path_norm)? {
                Some(id) => id,
                None => {
                    let (mtime_ns, size_bytes) = stat_path(path);
                    tx.execute(
                        r#"
                        INSERT OR IGNORE INTO tracks (path, path_norm, mtime_ns, size_bytes)
                        VALUES (?1, ?2, ?3, ?4)
                        "#,
                        params![path_value, path_norm, mtime_ns, size_bytes],
                    )?;
                    match get_track_id(&tx, &path_norm)? {
                        Some(id) => id,
                        None => continue,
                    }
                }
            };
            tx.execute(
                r#"
                INSERT INTO playlist_items (playlist_id, track_id, pos_key)
                VALUES (?1, ?2, ?3)
                "#,
                params![playlist_id, track_id, next_pos],
            )?;
            next_pos += POS_STEP;
            added += 1;
        }

        tx.commit()?;
        Ok(added)
    }

    pub fn remove_items(
        &self,
        playlist_id: i64,
        item_ids: &HashSet<i64>,
    ) -> Result<usize, DbError> {
        if item_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.connect()?;
        let placeholders = item_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let mut params: Vec<i64> = Vec::with_capacity(1 + item_ids.len());
        params.push(playlist_id);
        params.extend(item_ids.iter().copied());
        let sql =
            format!("DELETE FROM playlist_items WHERE playlist_id = ? AND id IN ({placeholders})");
        let n = conn.execute(&sql, params_from_iter(params))?;
        Ok(n)
    }

    pub fn count(&self, playlist_id: i64) -> Result<usize, DbError> {
        let conn = self.connect()?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1",
            [playlist_id],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn fetch_window(
        &self,
        playlist_id: i64,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<PlaylistRow>, DbError> {
        self.fetch_window_sorted(playlist_id, offset, limit, PlaylistSort::Playlist)
    }

    pub fn fetch_window_sorted(
        &self,
        playlist_id: i64,
        offset: usize,
        limit: usize,
        sort: PlaylistSort,
    ) -> Result<Vec<PlaylistRow>, DbError> {
        let conn = self.connect()?;
        let sql = format!(
            r#"
            SELECT
                playlist_items.id AS item_id,
                playlist_items.track_id,
                playlist_items.pos_key,
                tracks.path,
                track_meta.title,
                track_meta.artist,
                track_meta.album,
                track_meta.year,
                track_meta.duration_ms,
                track_meta.meta_valid,
                track_meta.meta_error
            FROM playlist_items
            JOIN tracks ON tracks.id = playlist_items.track_id
            LEFT JOIN track_meta ON track_meta.track_id = tracks.id
            WHERE playlist_items.playlist_id = ?1
            ORDER BY {}
            LIMIT ?2 OFFSET ?3
            "#,
            playlist_order_by(sort)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                params![playlist_id, limit as i64, offset as i64],
                map_playlist_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_item_row(
        &self,
        playlist_id: i64,
        item_id: i64,
    ) -> Result<Option<PlaylistRow>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT
                playlist_items.id AS item_id,
                playlist_items.track_id,
                playlist_items.pos_key,
                tracks.path,
                track_meta.title,
                track_meta.artist,
                track_meta.album,
                track_meta.year,
                track_meta.duration_ms,
                track_meta.meta_valid,
                track_meta.meta_error
            FROM playlist_items
            JOIN tracks ON tracks.id = playlist_items.track_id
            LEFT JOIN track_meta ON track_meta.track_id = tracks.id
            WHERE playlist_items.playlist_id = ?1 AND playlist_items.id = ?2
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query_map(params![playlist_id, item_id], map_playlist_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn fetch_rows_by_track_ids(
        &self,
        playlist_id: i64,
        track_ids: &[i64],
    ) -> Result<Vec<PlaylistRow>, DbError> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let placeholders = track_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let mut params: Vec<i64> = Vec::with_capacity(1 + track_ids.len());
        params.push(playlist_id);
        params.extend_from_slice(track_ids);
        let sql = format!(
            r#"
            SELECT
                playlist_items.id AS item_id,
                playlist_items.track_id,
                playlist_items.pos_key,
                tracks.path,
                track_meta.title,
                track_meta.artist,
                track_meta.album,
                track_meta.year,
                track_meta.duration_ms,
                track_meta.meta_valid,
                track_meta.meta_error
            FROM playlist_items
            JOIN tracks ON tracks.id = playlist_items.track_id
            LEFT JOIN track_meta ON track_meta.track_id = tracks.id
            WHERE playlist_items.playlist_id = ?
              AND playlist_items.track_id IN ({placeholders})
            ORDER BY playlist_items.pos_key
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(params), map_playlist_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn fetch_rows_by_item_ids(
        &self,
        playlist_id: i64,
        item_ids: &[i64],
    ) -> Result<Vec<PlaylistRow>, DbError> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let placeholders = item_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let order_cases = item_ids
            .iter()
            .enumerate()
            .map(|(idx, _)| format!("WHEN playlist_items.id = ? THEN {idx}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut params: Vec<i64> = Vec::with_capacity(1 + item_ids.len() * 2);
        params.push(playlist_id);
        params.extend_from_slice(item_ids);
        params.extend_from_slice(item_ids);
        let sql = format!(
            r#"
            SELECT
                playlist_items.id AS item_id,
                playlist_items.track_id,
                playlist_items.pos_key,
                tracks.path,
                track_meta.title,
                track_meta.artist,
                track_meta.album,
                track_meta.year,
                track_meta.duration_ms,
                track_meta.meta_valid,
                track_meta.meta_error
            FROM playlist_items
            JOIN tracks ON tracks.id = playlist_items.track_id
            LEFT JOIN track_meta ON track_meta.track_id = tracks.id
            WHERE playlist_items.playlist_id = ?
              AND playlist_items.id IN ({placeholders})
            ORDER BY CASE {order_cases} END
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(params), map_playlist_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Tokenized AND search across title/artist/album/year/path (FTS with LIKE fallback).
    pub fn search_item_ids(
        &self,
        playlist_id: i64,
        query: &str,
        limit: usize,
    ) -> Result<Vec<i64>, DbError> {
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        if has_playlist_search_fts(&conn)? {
            match search_item_ids_fts(&conn, playlist_id, &tokens, limit) {
                Ok(ids) => return Ok(ids),
                Err(e) => {
                    tracing::debug!("FTS search failed, using LIKE fallback: {e}");
                }
            }
        }
        search_item_ids_like(&conn, playlist_id, &tokens, limit)
    }

    pub fn get_next_item_id(
        &self,
        playlist_id: i64,
        item_id: i64,
        wrap: bool,
    ) -> Result<Option<i64>, DbError> {
        let conn = self.connect()?;
        let pos: Option<i64> = conn
            .query_row(
                "SELECT pos_key FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
                params![playlist_id, item_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(pos_key) = pos else {
            return Ok(None);
        };
        let next: Option<i64> = conn
            .query_row(
                r#"
                SELECT id FROM playlist_items
                WHERE playlist_id = ?1 AND pos_key > ?2
                ORDER BY pos_key ASC LIMIT 1
                "#,
                params![playlist_id, pos_key],
                |row| row.get(0),
            )
            .optional()?;
        if next.is_some() {
            return Ok(next);
        }
        if !wrap {
            return Ok(None);
        }
        let wrap_id = conn
            .query_row(
                r#"
                SELECT id FROM playlist_items
                WHERE playlist_id = ?1
                ORDER BY pos_key ASC LIMIT 1
                "#,
                [playlist_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(wrap_id)
    }

    pub fn get_prev_item_id(
        &self,
        playlist_id: i64,
        item_id: i64,
        wrap: bool,
    ) -> Result<Option<i64>, DbError> {
        let conn = self.connect()?;
        let pos: Option<i64> = conn
            .query_row(
                "SELECT pos_key FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
                params![playlist_id, item_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(pos_key) = pos else {
            return Ok(None);
        };
        let prev: Option<i64> = conn
            .query_row(
                r#"
                SELECT id FROM playlist_items
                WHERE playlist_id = ?1 AND pos_key < ?2
                ORDER BY pos_key DESC LIMIT 1
                "#,
                params![playlist_id, pos_key],
                |row| row.get(0),
            )
            .optional()?;
        if prev.is_some() {
            return Ok(prev);
        }
        if !wrap {
            return Ok(None);
        }
        let wrap_id = conn
            .query_row(
                r#"
                SELECT id FROM playlist_items
                WHERE playlist_id = ?1
                ORDER BY pos_key DESC LIMIT 1
                "#,
                [playlist_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(wrap_id)
    }

    /// Reorder selected items one step while preserving relative block order.
    pub fn move_selection(
        &self,
        playlist_id: i64,
        direction: MoveDirection,
        selection: &[i64],
        cursor: Option<i64>,
    ) -> Result<(), DbError> {
        let mut selection_ids: HashSet<i64> = selection.iter().copied().collect();
        if selection_ids.is_empty() {
            if let Some(c) = cursor {
                selection_ids.insert(c);
            }
        }
        if selection_ids.is_empty() {
            return Ok(());
        }

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(
            r#"
            SELECT id, pos_key FROM playlist_items
            WHERE playlist_id = ?1
            ORDER BY pos_key
            "#,
        )?;
        let pairs: Vec<(i64, i64)> = stmt
            .query_map([playlist_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        if pairs.is_empty() {
            return Ok(());
        }

        let mut item_ids: Vec<i64> = pairs.iter().map(|(id, _)| *id).collect();
        let pos_keys: Vec<i64> = pairs.iter().map(|(_, p)| *p).collect();

        match direction {
            MoveDirection::Up => {
                for index in 1..item_ids.len() {
                    if selection_ids.contains(&item_ids[index])
                        && !selection_ids.contains(&item_ids[index - 1])
                    {
                        item_ids.swap(index - 1, index);
                    }
                }
            }
            MoveDirection::Down => {
                for index in (0..item_ids.len().saturating_sub(1)).rev() {
                    if selection_ids.contains(&item_ids[index])
                        && !selection_ids.contains(&item_ids[index + 1])
                    {
                        item_ids.swap(index, index + 1);
                    }
                }
            }
        }

        for (i, item_id) in item_ids.iter().enumerate() {
            tx.execute(
                r#"
                UPDATE playlist_items SET pos_key = ?1
                WHERE playlist_id = ?2 AND id = ?3
                "#,
                params![pos_keys[i], playlist_id, item_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_track_id_for_item(
        &self,
        playlist_id: i64,
        item_id: i64,
    ) -> Result<Option<i64>, DbError> {
        let conn = self.connect()?;
        let id = conn
            .query_row(
                "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
                params![playlist_id, item_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// 1-based index of the item by pos_key order (Python parity).
    pub fn get_item_index(&self, playlist_id: i64, item_id: i64) -> Result<Option<usize>, DbError> {
        let conn = self.connect()?;
        let pos: Option<i64> = conn
            .query_row(
                "SELECT pos_key FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
                params![playlist_id, item_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(pos_key) = pos else {
            return Ok(None);
        };
        let count: i64 = conn.query_row(
            r#"
            SELECT COUNT(*) FROM playlist_items
            WHERE playlist_id = ?1 AND pos_key <= ?2
            "#,
            params![playlist_id, pos_key],
            |row| row.get(0),
        )?;
        Ok(Some(count as usize))
    }

    pub fn list_item_ids(&self, playlist_id: i64) -> Result<Vec<i64>, DbError> {
        self.list_item_ids_sorted(playlist_id, PlaylistSort::Playlist)
    }

    pub fn list_item_ids_sorted(
        &self,
        playlist_id: i64,
        sort: PlaylistSort,
    ) -> Result<Vec<i64>, DbError> {
        let conn = self.connect()?;
        let sql = format!(
            "SELECT playlist_items.id FROM playlist_items \
             JOIN tracks ON tracks.id = playlist_items.track_id \
             LEFT JOIN track_meta ON track_meta.track_id = tracks.id \
             WHERE playlist_items.playlist_id = ?1 ORDER BY {}",
            playlist_order_by(sort)
        );
        let mut stmt = conn.prepare(&sql)?;
        let ids = stmt
            .query_map([playlist_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn get_random_item_id(
        &self,
        playlist_id: i64,
        exclude_item_id: Option<i64>,
    ) -> Result<Option<i64>, DbError> {
        let conn = self.connect()?;
        match exclude_item_id {
            None => {
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1",
                    [playlist_id],
                    |row| row.get(0),
                )?;
                if total <= 0 {
                    return Ok(None);
                }
                let offset = fastrand_offset(total as usize);
                let id = conn
                    .query_row(
                        r#"
                        SELECT id FROM playlist_items
                        WHERE playlist_id = ?1
                        ORDER BY pos_key ASC
                        LIMIT 1 OFFSET ?2
                        "#,
                        params![playlist_id, offset as i64],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(id)
            }
            Some(exclude) => {
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1 AND id != ?2",
                    params![playlist_id, exclude],
                    |row| row.get(0),
                )?;
                if total <= 0 {
                    return Ok(None);
                }
                let offset = fastrand_offset(total as usize);
                let id = conn
                    .query_row(
                        r#"
                        SELECT id FROM playlist_items
                        WHERE playlist_id = ?1 AND id != ?2
                        ORDER BY pos_key ASC
                        LIMIT 1 OFFSET ?3
                        "#,
                        params![playlist_id, exclude, offset as i64],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(id)
            }
        }
    }

    /// Invalidate metadata for the given tracks, or all rows when `None` / empty.
    pub fn invalidate_metadata(&self, track_ids: Option<&HashSet<i64>>) -> Result<(), DbError> {
        let conn = self.connect()?;
        match track_ids {
            Some(ids) if !ids.is_empty() => {
                let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                let sql = format!(
                    "UPDATE track_meta SET meta_valid = 0, meta_error = NULL WHERE track_id IN ({placeholders})"
                );
                conn.execute(&sql, params_from_iter(ids.iter().copied()))?;
            }
            _ => {
                conn.execute(
                    "UPDATE track_meta SET meta_valid = 0, meta_error = NULL",
                    [],
                )?;
            }
        }
        Ok(())
    }

    pub fn renumber_playlist(&self, playlist_id: i64) -> Result<(), DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(
            r#"
            SELECT id FROM playlist_items
            WHERE playlist_id = ?1
            ORDER BY pos_key
            "#,
        )?;
        let ids: Vec<i64> = stmt
            .query_map([playlist_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut next_pos = POS_STEP;
        for id in ids {
            tx.execute(
                "UPDATE playlist_items SET pos_key = ?1 WHERE playlist_id = ?2 AND id = ?3",
                params![next_pos, playlist_id, id],
            )?;
            next_pos += POS_STEP;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_tracks_basic(&self, track_ids: &[i64]) -> Result<Vec<TrackRecord>, DbError> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let placeholders = track_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT id, path, mtime_ns, size_bytes FROM tracks WHERE id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(track_ids.iter().copied()), |row| {
                Ok(TrackRecord {
                    track_id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    mtime_ns: row.get(2)?,
                    size_bytes: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_track_meta_snapshot(
        &self,
        track_ids: &[i64],
    ) -> Result<HashMap<i64, TrackMetaSnapshot>, DbError> {
        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let conn = self.connect()?;
        let placeholders = track_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            r#"
            SELECT track_id, title, artist, album, year, duration_ms, meta_valid, meta_error
            FROM track_meta
            WHERE track_id IN ({placeholders})
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut map = HashMap::new();
        let rows = stmt.query_map(params_from_iter(track_ids.iter().copied()), |row| {
            let track_id: i64 = row.get(0)?;
            let meta_valid: i64 = row.get(6)?;
            Ok(TrackMetaSnapshot {
                track_id,
                title: row.get(1)?,
                artist: row.get(2)?,
                album: row.get(3)?,
                year: row.get(4)?,
                duration_ms: row.get(5)?,
                meta_valid: meta_valid != 0,
                meta_error: row.get(7)?,
            })
        })?;
        for row in rows {
            let snap = row?;
            map.insert(snap.track_id, snap);
        }
        Ok(map)
    }

    pub fn upsert_track_meta(&self, track_id: i64, meta: &TrackMeta) -> Result<(), DbError> {
        let now = unix_now();
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO track_meta (
                track_id, title, artist, album, year, duration_ms,
                meta_loaded_at, meta_valid, meta_error
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(track_id) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                year = excluded.year,
                duration_ms = excluded.duration_ms,
                meta_loaded_at = excluded.meta_loaded_at,
                meta_valid = excluded.meta_valid,
                meta_error = excluded.meta_error
            "#,
            params![
                track_id,
                meta.title,
                meta.artist,
                meta.album,
                meta.year,
                meta.duration_ms,
                now,
                i64::from(meta.meta_valid),
                meta.meta_error,
            ],
        )?;
        tx.execute(
            r#"
            UPDATE tracks
            SET mtime_ns = ?1, size_bytes = ?2, updated_at = ?3
            WHERE id = ?4
            "#,
            params![meta.mtime_ns, meta.size_bytes, now, track_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_meta_invalid(&self, track_id: i64, error: Option<&str>) -> Result<(), DbError> {
        let now = unix_now();
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO track_meta (track_id, meta_loaded_at, meta_valid, meta_error)
            VALUES (?1, ?2, 0, ?3)
            ON CONFLICT(track_id) DO UPDATE SET
                meta_loaded_at = excluded.meta_loaded_at,
                meta_valid = excluded.meta_valid,
                meta_error = excluded.meta_error
            "#,
            params![track_id, now, error],
        )?;
        tx.execute(
            "UPDATE tracks SET updated_at = ?1 WHERE id = ?2",
            params![now, track_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn playlist_order_by(sort: PlaylistSort) -> &'static str {
    const TRACK: &str = "LOWER(COALESCE(NULLIF(TRIM(track_meta.title), ''), tracks.path)), \
         playlist_items.pos_key, playlist_items.id";
    match sort {
        PlaylistSort::Playlist => "playlist_items.pos_key, playlist_items.id",
        PlaylistSort::Track => TRACK,
        PlaylistSort::Artist => {
            "CASE WHEN NULLIF(TRIM(track_meta.artist), '') IS NULL THEN 1 ELSE 0 END, \
             LOWER(COALESCE(track_meta.artist, '')), \
             LOWER(COALESCE(NULLIF(TRIM(track_meta.title), ''), tracks.path)), \
             playlist_items.pos_key, playlist_items.id"
        }
        PlaylistSort::Album => {
            "CASE WHEN NULLIF(TRIM(track_meta.album), '') IS NULL THEN 1 ELSE 0 END, \
             LOWER(COALESCE(track_meta.album, '')), \
             LOWER(COALESCE(NULLIF(TRIM(track_meta.title), ''), tracks.path)), \
             playlist_items.pos_key, playlist_items.id"
        }
    }
}

fn map_playlist_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlaylistRow> {
    let meta_valid: Option<i64> = row.get(9)?;
    Ok(PlaylistRow {
        item_id: row.get(0)?,
        track_id: row.get(1)?,
        pos_key: row.get(2)?,
        path: PathBuf::from(row.get::<_, String>(3)?),
        title: row.get(4)?,
        artist: row.get(5)?,
        album: row.get(6)?,
        year: row.get(7)?,
        duration_ms: row.get(8)?,
        meta_valid: meta_valid.map(|v| v != 0),
        meta_error: row.get(10)?,
    })
}

fn get_track_id(conn: &Connection, path_norm: &str) -> Result<Option<i64>, DbError> {
    let id = conn
        .query_row(
            "SELECT id FROM tracks WHERE path_norm = ?1",
            [path_norm],
            |row| row.get(0),
        )
        .optional()?;
    Ok(id)
}

fn insert_draft_path_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    path: &Path,
    pos_key: i64,
) -> Result<(), DbError> {
    let path_value = path.to_string_lossy().to_string();
    let path_norm = normalize_path(path);
    tx.execute(
        "INSERT INTO playlist_editor_draft_items (session_id, path, path_norm, pos_key)
         VALUES (?1, ?2, ?3, ?4)",
        params![session_id, path_value, path_norm, pos_key],
    )?;
    Ok(())
}

fn insert_draft_paths(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    paths: &[PathBuf],
    start_pos: i64,
) -> Result<(), DbError> {
    let mut pos = if start_pos == 0 {
        POS_STEP
    } else {
        start_pos + POS_STEP
    };
    for path in paths {
        insert_draft_path_tx(tx, session_id, path, pos)?;
        pos += POS_STEP;
    }
    Ok(())
}

fn draft_bounds_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    index: usize,
) -> Result<(i64, i64), DbError> {
    let lower: i64 = if index == 0 {
        0
    } else {
        tx.query_row(
            "SELECT pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
             ORDER BY pos_key LIMIT 1 OFFSET ?2",
            params![session_id, (index - 1) as i64],
            |row| row.get(0),
        )?
    };
    let upper: Option<i64> = tx
        .query_row(
            "SELECT pos_key FROM playlist_editor_draft_items WHERE session_id = ?1
             ORDER BY pos_key LIMIT 1 OFFSET ?2",
            params![session_id, index as i64],
            |row| row.get(0),
        )
        .optional()?;
    Ok((lower, upper.unwrap_or(lower.saturating_add(POS_STEP))))
}

fn renumber_draft_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    step: i64,
) -> Result<(), DbError> {
    let ids: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM playlist_editor_draft_items WHERE session_id = ?1 ORDER BY pos_key",
        )?;
        let result = stmt
            .query_map([session_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        result
    };
    for (index, id) in ids.into_iter().enumerate() {
        tx.execute(
            "UPDATE playlist_editor_draft_items SET pos_key = ?1 WHERE id = ?2",
            params![step.saturating_mul(index as i64 + 1), id],
        )?;
    }
    Ok(())
}

fn ensure_track_id_tx(tx: &rusqlite::Transaction<'_>, path: &Path) -> Result<i64, DbError> {
    let path_value = path.to_string_lossy().to_string();
    let path_norm = normalize_path(path);
    if let Some(id) = get_track_id(tx, &path_norm)? {
        return Ok(id);
    }
    let (mtime_ns, size_bytes) = stat_path(path);
    tx.execute(
        "INSERT OR IGNORE INTO tracks (path, path_norm, mtime_ns, size_bytes)
         VALUES (?1, ?2, ?3, ?4)",
        params![path_value, path_norm, mtime_ns, size_bytes],
    )?;
    get_track_id(tx, &normalize_path(path))?.ok_or_else(|| {
        DbError::Schema(format!("failed to create track row for {}", path.display()))
    })
}

fn insert_draft_into_playlist_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    playlist_id: i64,
) -> Result<(), DbError> {
    let mut stmt = tx.prepare(
        "SELECT path FROM playlist_editor_draft_items WHERE session_id = ?1 ORDER BY pos_key",
    )?;
    let mut rows = stmt.query([session_id])?;
    let mut pos = POS_STEP;
    while let Some(row) = rows.next()? {
        let path = PathBuf::from(row.get::<_, String>(0)?);
        let track_id = ensure_track_id_tx(tx, &path)?;
        tx.execute(
            "INSERT INTO playlist_items (playlist_id, track_id, pos_key) VALUES (?1, ?2, ?3)",
            params![playlist_id, track_id, pos],
        )?;
        pos += POS_STEP;
    }
    Ok(())
}

fn has_playlist_search_fts(conn: &Connection) -> Result<bool, DbError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = 'playlist_search' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn build_fts_query(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| {
            let escaped = t.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn search_item_ids_fts(
    conn: &Connection,
    playlist_id: i64,
    tokens: &[String],
    limit: usize,
) -> Result<Vec<i64>, DbError> {
    let fts_query = build_fts_query(tokens);
    let mut stmt = conn.prepare(
        r#"
        SELECT playlist_items.id AS item_id
        FROM playlist_search
        JOIN playlist_items ON playlist_items.id = playlist_search.rowid
        WHERE playlist_search MATCH ?1
          AND playlist_items.playlist_id = ?2
        ORDER BY playlist_items.pos_key
        LIMIT ?3
        "#,
    )?;
    let ids = stmt
        .query_map(params![fts_query, playlist_id, limit as i64], |row| {
            row.get(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

fn search_item_ids_like(
    conn: &Connection,
    playlist_id: i64,
    tokens: &[String],
    limit: usize,
) -> Result<Vec<i64>, DbError> {
    let field_exprs = [
        "LOWER(COALESCE(track_meta.title, ''))",
        "LOWER(COALESCE(track_meta.artist, ''))",
        "LOWER(COALESCE(track_meta.album, ''))",
        "LOWER(COALESCE(CAST(track_meta.year AS TEXT), ''))",
        "LOWER(COALESCE(tracks.path, ''))",
    ];
    let mut token_clauses = Vec::new();
    for _ in tokens {
        let or_parts = field_exprs
            .iter()
            .map(|expr| format!("{expr} LIKE ?"))
            .collect::<Vec<_>>()
            .join(" OR ");
        token_clauses.push(format!("({or_parts})"));
    }
    let where_clause = token_clauses.join(" AND ");
    let sql = format!(
        r#"
        SELECT playlist_items.id AS item_id
        FROM playlist_items
        JOIN tracks ON tracks.id = playlist_items.track_id
        LEFT JOIN track_meta ON track_meta.track_id = tracks.id
        WHERE playlist_items.playlist_id = ?
          AND {where_clause}
        ORDER BY playlist_items.pos_key
        LIMIT ?
        "#
    );

    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    params.push(rusqlite::types::Value::Integer(playlist_id));
    for token in tokens {
        let pat = format!("%{token}%");
        for _ in &field_exprs {
            params.push(rusqlite::types::Value::Text(pat.clone()));
        }
    }
    params.push(rusqlite::types::Value::Integer(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(params_from_iter(params), |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

fn fastrand_offset(total: usize) -> usize {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    if total == 0 {
        return 0;
    }
    let mut h = DefaultHasher::new();
    SystemTime::now().hash(&mut h);
    (h.finish() as usize) % total
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tz_player3_playlist_{n}.db"))
    }

    fn touch(path: &Path) {
        fs::write(path, b"").unwrap();
    }

    fn store_with_tracks(count: usize) -> (PlaylistStore, PathBuf, i64, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tz_player3_pl_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.sqlite");
        let store = PlaylistStore::new(&db);
        store.initialize().unwrap();
        let pid = store.create_playlist("Queue").unwrap();
        let mut paths = Vec::new();
        for i in 0..count {
            let p = dir.join(format!("track_{i}.mp3"));
            touch(&p);
            paths.push(p);
        }
        store.add_tracks(pid, &paths).unwrap();
        (store, db, pid, dir)
    }

    #[test]
    fn basic_add_count_remove() {
        let dir = std::env::temp_dir().join(format!(
            "tz_pl_basic_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.sqlite");
        let store = PlaylistStore::new(&db);
        store.initialize().unwrap();
        let pid = store.create_playlist("Favorites").unwrap();
        let paths: Vec<_> = (0..3)
            .map(|i| {
                let p = dir.join(format!("track_{i}.mp3"));
                touch(&p);
                p
            })
            .collect();
        assert_eq!(store.add_tracks(pid, &paths).unwrap(), 3);
        assert_eq!(store.count(pid).unwrap(), 3);
        let rows = store.fetch_window(pid, 0, 10).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, paths[0]);
        let removed = store
            .remove_items(pid, &HashSet::from([rows[0].item_id]))
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.count(pid).unwrap(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_view_sorts_are_stable_and_do_not_change_playlist_order() {
        let (store, _db, pid, dir) = store_with_tracks(3);
        let original = store.fetch_window(pid, 0, 10).unwrap();
        let metadata = [
            ("Zulu", "Beta", "Gamma"),
            ("Alpha", "Zulu", "Beta"),
            ("Mike", "Alpha", "Zulu"),
        ];
        for (row, (title, artist, album)) in original.iter().zip(metadata) {
            store
                .upsert_track_meta(
                    row.track_id,
                    &TrackMeta {
                        title: Some(title.into()),
                        artist: Some(artist.into()),
                        album: Some(album.into()),
                        year: None,
                        duration_ms: None,
                        meta_valid: true,
                        meta_error: None,
                        mtime_ns: None,
                        size_bytes: None,
                    },
                )
                .unwrap();
        }

        let titles = |sort| {
            store
                .fetch_window_sorted(pid, 0, 10, sort)
                .unwrap()
                .into_iter()
                .map(|row| row.title.unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(PlaylistSort::Track), ["Alpha", "Mike", "Zulu"]);
        assert_eq!(titles(PlaylistSort::Artist), ["Mike", "Zulu", "Alpha"]);
        assert_eq!(titles(PlaylistSort::Album), ["Alpha", "Zulu", "Mike"]);
        assert_eq!(
            store.list_item_ids(pid).unwrap(),
            original.iter().map(|row| row.item_id).collect::<Vec<_>>(),
            "view sorting must not mutate stored playlist order"
        );
        assert_eq!(
            store
                .list_item_ids_sorted(pid, PlaylistSort::Artist)
                .unwrap(),
            [
                original[2].item_id,
                original[0].item_id,
                original[1].item_id,
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_selection_cursor_up() {
        let (store, _db, pid, dir) = store_with_tracks(3);
        let rows = store.fetch_window(pid, 0, 10).unwrap();
        let cursor = rows[1].item_id;
        store
            .move_selection(pid, MoveDirection::Up, &[], Some(cursor))
            .unwrap();
        let updated = store.fetch_window(pid, 0, 10).unwrap();
        assert_eq!(updated[0].item_id, cursor);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_tracks_allowed() {
        let dir = std::env::temp_dir().join(format!(
            "tz_pl_dup_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let store = PlaylistStore::new(dir.join("library.sqlite"));
        store.initialize().unwrap();
        let pid = store.create_playlist("Dupes").unwrap();
        let track = dir.join("track.mp3");
        touch(&track);
        store
            .add_tracks(pid, &[track.clone(), track.clone()])
            .unwrap();
        assert_eq!(store.count(pid).unwrap(), 2);
        let rows = store.fetch_window(pid, 0, 10).unwrap();
        assert_eq!(rows[0].track_id, rows[1].track_id);
        assert_ne!(rows[0].item_id, rows[1].item_id);
        store
            .move_selection(pid, MoveDirection::Up, &[rows[1].item_id], None)
            .unwrap();
        let moved = store.fetch_window(pid, 0, 10).unwrap();
        assert_eq!(moved[0].item_id, rows[1].item_id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_by_path_and_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "tz_pl_search_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let store = PlaylistStore::new(dir.join("library.sqlite"));
        store.initialize().unwrap();
        let pid = store.create_playlist("Search").unwrap();
        let paths = [
            dir.join("moon_song.mp3"),
            dir.join("sun_song.mp3"),
            dir.join("moonlight.flac"),
        ];
        for p in &paths {
            touch(p);
        }
        store.add_tracks(pid, &paths).unwrap();
        let rows = store.fetch_window(pid, 0, 10).unwrap();
        let match_ids = store.search_item_ids(pid, "moon", 1000).unwrap();
        assert_eq!(
            match_ids,
            vec![rows[0].item_id, rows[2].item_id],
            "search should find moon_song and moonlight"
        );

        store
            .upsert_track_meta(
                rows[1].track_id,
                &TrackMeta {
                    title: Some("Neon Pulse".into()),
                    artist: Some("Test Artist".into()),
                    album: Some("Perf Set".into()),
                    year: Some(2026),
                    duration_ms: Some(180_000),
                    meta_valid: true,
                    meta_error: None,
                    mtime_ns: None,
                    size_bytes: None,
                },
            )
            .unwrap();
        let neon = store.search_item_ids(pid, "neon", 1000).unwrap();
        assert_eq!(neon, vec![rows[1].item_id]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_prev_wrap() {
        let (store, _db, pid, dir) = store_with_tracks(3);
        let ids = store.list_item_ids(pid).unwrap();
        assert_eq!(
            store.get_next_item_id(pid, ids[0], false).unwrap(),
            Some(ids[1])
        );
        assert_eq!(store.get_next_item_id(pid, ids[2], false).unwrap(), None);
        assert_eq!(
            store.get_next_item_id(pid, ids[2], true).unwrap(),
            Some(ids[0])
        );
        assert_eq!(
            store.get_prev_item_id(pid, ids[0], true).unwrap(),
            Some(ids[2])
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalidate_metadata() {
        let (store, _db, pid, dir) = store_with_tracks(1);
        let rows = store.fetch_window(pid, 0, 1).unwrap();
        let tid = rows[0].track_id;
        store
            .upsert_track_meta(
                tid,
                &TrackMeta {
                    title: Some("Song".into()),
                    artist: None,
                    album: None,
                    year: None,
                    duration_ms: None,
                    meta_valid: true,
                    meta_error: None,
                    mtime_ns: None,
                    size_bytes: None,
                },
            )
            .unwrap();
        store
            .invalidate_metadata(Some(&HashSet::from([tid])))
            .unwrap();
        let snap = store.get_track_meta_snapshot(&[tid]).unwrap();
        assert!(!snap[&tid].meta_valid);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_playlist() {
        let (store, _db, pid, dir) = store_with_tracks(2);
        store.clear_playlist(pid).unwrap();
        assert_eq!(store.count(pid).unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_playlist_reuses_name() {
        let db = temp_db();
        let store = PlaylistStore::new(&db);
        store.initialize().unwrap();
        let a = store.ensure_playlist("Default").unwrap();
        let b = store.ensure_playlist("Default").unwrap();
        assert_eq!(a, b);
        let _ = fs::remove_file(&db);
    }

    #[test]
    fn draft_is_windowed_editable_and_can_apply() {
        let (store, _db, pid, dir) = store_with_tracks(3);
        let session = "test-session";
        store.draft_from_playlist(session, pid).unwrap();
        assert_eq!(store.draft_count(session).unwrap(), 3);
        let first = store.fetch_draft_window(session, 0, 10).unwrap();
        assert_eq!(first.len(), 3);
        assert!(first[0].path.ends_with("track_0.mp3"));

        let extra = dir.join("extra.mp3");
        touch(&extra);
        store
            .append_draft_paths(session, std::slice::from_ref(&extra))
            .unwrap();
        store
            .insert_draft_paths(session, 1, std::slice::from_ref(&extra))
            .unwrap();
        assert_eq!(store.draft_count(session).unwrap(), 5);
        assert!(store.move_draft_at(session, 2, true).unwrap());
        assert!(store.remove_draft_at(session, 0).unwrap());
        assert_eq!(store.draft_count(session).unwrap(), 4);

        store.replace_playlist_from_draft(pid, session).unwrap();
        assert_eq!(store.count(pid).unwrap(), 4);
        store.clear_draft(session).unwrap();
        assert_eq!(store.draft_count(session).unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_playlist_crud_and_name_conflicts_are_transactional() {
        let (store, _db, pid, dir) = store_with_tracks(2);
        let session = "save-session";
        store.draft_from_playlist(session, pid).unwrap();
        let saved = store
            .save_playlist_from_draft(session, "Road Trip", None)
            .unwrap();
        assert_ne!(saved, pid);
        assert!(store
            .save_playlist_from_draft(session, "road trip", None)
            .is_err());
        store.rename_playlist(saved, "Road Trip Updated").unwrap();
        assert_eq!(
            store.playlist_name(saved).unwrap().as_deref(),
            Some("Road Trip Updated")
        );
        assert!(store.delete_playlist(saved).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
