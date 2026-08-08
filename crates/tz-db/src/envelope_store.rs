//! SQLite cache for scalar envelope frames (analysis type = "scalar").

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};

use crate::error::DbError;
use crate::path_util::{normalize_path, stat_path};
use crate::{create_schema, ensure_playlist_search_fts, open_connection};

const ANALYSIS_TYPE: &str = "scalar";
const ANALYSIS_VERSION: i64 = 1;

/// Cached envelope lookup / upsert for visualizer levels.
pub struct EnvelopeStore {
    db_path: PathBuf,
    bucket_ms: u64,
}

impl EnvelopeStore {
    pub fn new(db_path: impl Into<PathBuf>, bucket_ms: u64) -> Self {
        Self {
            db_path: db_path.into(),
            bucket_ms: bucket_ms.max(10),
        }
    }

    pub fn bucket_ms(&self) -> u64 {
        self.bucket_ms
    }

    pub fn initialize(&self) -> Result<(), DbError> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Io(format!("create db parent: {e}")))?;
        }
        let conn = open_connection(&self.db_path)?;
        create_schema(&conn)?;
        ensure_playlist_search_fts(&conn)?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection, DbError> {
        open_connection(&self.db_path)
    }

    fn params_json(&self) -> String {
        format!(r#"{{"bucket_ms":{}}}"#, self.bucket_ms)
    }

    fn params_hash(&self) -> String {
        let mut h = Sha1::new();
        h.update(self.params_json().as_bytes());
        hex::encode(h.finalize())
    }

    pub fn has_envelope(&self, track_path: &Path) -> Result<bool, DbError> {
        Ok(self.lookup_entry_id(track_path)?.is_some())
    }

    pub fn upsert_envelope(
        &self,
        track_path: &Path,
        duration_ms: u64,
        points: &[(u64, f32, f32)],
    ) -> Result<(), DbError> {
        if points.is_empty() {
            return Ok(());
        }
        let path_norm = normalize_path(track_path);
        let (mtime_ns, size_bytes) = stat_path(track_path);
        let params_json = self.params_json();
        let params_hash = self.params_hash();
        let byte_size = (points.len() * 24) as i64;

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        // Remove prior entry for same fingerprint+type
        tx.execute(
            r#"
            DELETE FROM analysis_cache_entries
            WHERE analysis_type = ?1
              AND path_norm = ?2
              AND analysis_version = ?3
              AND params_hash = ?4
            "#,
            params![ANALYSIS_TYPE, path_norm, ANALYSIS_VERSION, params_hash],
        )?;
        tx.execute(
            r#"
            INSERT INTO analysis_cache_entries (
                analysis_type, path_norm, mtime_ns, size_bytes,
                analysis_version, params_hash, params_json,
                duration_ms, frame_count, byte_size
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                ANALYSIS_TYPE,
                path_norm,
                mtime_ns,
                size_bytes,
                ANALYSIS_VERSION,
                params_hash,
                params_json,
                duration_ms as i64,
                points.len() as i64,
                byte_size,
            ],
        )?;
        let entry_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO analysis_scalar_frames (entry_id, position_ms, level_left, level_right)
                VALUES (?1, ?2, ?3, ?4)
                "#,
            )?;
            for (pos, l, r) in points {
                stmt.execute(params![
                    entry_id,
                    *pos as i64,
                    (*l).clamp(0.0, 1.0),
                    (*r).clamp(0.0, 1.0)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Nearest envelope sample at or before position_ms.
    pub fn get_level_at(
        &self,
        track_path: &Path,
        position_ms: u64,
    ) -> Result<Option<(f32, f32)>, DbError> {
        let Some(entry_id) = self.lookup_entry_id(track_path)? else {
            return Ok(None);
        };
        let conn = self.connect()?;
        let row: Option<(f32, f32)> = conn
            .query_row(
                r#"
                SELECT level_left, level_right
                FROM analysis_scalar_frames
                WHERE entry_id = ?1 AND position_ms <= ?2
                ORDER BY position_ms DESC
                LIMIT 1
                "#,
                params![entry_id, position_ms as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if row.is_none() {
            // fallback first frame
            let first = conn
                .query_row(
                    r#"
                    SELECT level_left, level_right
                    FROM analysis_scalar_frames
                    WHERE entry_id = ?1
                    ORDER BY position_ms ASC
                    LIMIT 1
                    "#,
                    [entry_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            return Ok(first);
        }
        Ok(row)
    }

    fn lookup_entry_id(&self, track_path: &Path) -> Result<Option<i64>, DbError> {
        let path_norm = normalize_path(track_path);
        let (mtime_ns, size_bytes) = stat_path(track_path);
        let params_hash = self.params_hash();
        let conn = self.connect()?;
        let id = conn
            .query_row(
                r#"
                SELECT id FROM analysis_cache_entries
                WHERE analysis_type = ?1
                  AND path_norm = ?2
                  AND analysis_version = ?3
                  AND params_hash = ?4
                  AND (mtime_ns IS ?5 OR (mtime_ns IS NULL AND ?5 IS NULL))
                  AND (size_bytes IS ?6 OR (size_bytes IS NULL AND ?6 IS NULL))
                LIMIT 1
                "#,
                params![
                    ANALYSIS_TYPE,
                    path_norm,
                    ANALYSIS_VERSION,
                    params_hash,
                    mtime_ns,
                    size_bytes
                ],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn upsert_and_lookup() {
        let dir = std::env::temp_dir().join(format!(
            "tz_envstore_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let path = dir.join("song.wav");
        std::fs::write(&path, b"x").unwrap();

        let store = EnvelopeStore::new(&db, 50);
        store.initialize().unwrap();
        store
            .upsert_envelope(
                &path,
                1000,
                &[(0, 0.1, 0.2), (50, 0.5, 0.6), (100, 0.3, 0.4)],
            )
            .unwrap();
        assert!(store.has_envelope(&path).unwrap());
        let (l, r) = store.get_level_at(&path, 60).unwrap().unwrap();
        assert!((l - 0.5).abs() < 0.001);
        assert!((r - 0.6).abs() < 0.001);
        let _ = std::fs::remove_dir_all(dir);
    }
}
