//! SQLite cache for waveform-proxy frames (analysis type = "waveform_proxy").

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};

use crate::error::DbError;
use crate::path_util::{normalize_path, stat_path};
use crate::{create_schema, ensure_playlist_search_fts, open_connection};

const ANALYSIS_TYPE: &str = "waveform_proxy";
const ANALYSIS_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
pub struct WaveformParams {
    pub hop_ms: u64,
}

impl Default for WaveformParams {
    fn default() -> Self {
        Self { hop_ms: 20 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WaveformReading {
    pub min_left: f32,
    pub max_left: f32,
    pub min_right: f32,
    pub max_right: f32,
}

pub struct WaveformStore {
    db_path: PathBuf,
    params: WaveformParams,
}

impl WaveformStore {
    pub fn new(db_path: impl Into<PathBuf>, params: WaveformParams) -> Self {
        Self {
            db_path: db_path.into(),
            params: WaveformParams {
                hop_ms: params.hop_ms.max(10),
            },
        }
    }

    pub fn params(&self) -> WaveformParams {
        self.params
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

    fn params_hash(&self) -> String {
        let json = format!(r#"{{"hop_ms":{}}}"#, self.params.hop_ms);
        let mut h = Sha1::new();
        h.update(json.as_bytes());
        hex::encode(h.finalize())
    }

    pub fn has_waveform(&self, track_path: &Path) -> Result<bool, DbError> {
        Ok(self.lookup_entry_id(track_path)?.is_some())
    }

    pub fn upsert_waveform(
        &self,
        track_path: &Path,
        duration_ms: u64,
        frames: &[(u64, i8, i8, i8, i8)],
    ) -> Result<(), DbError> {
        if frames.is_empty() {
            return Ok(());
        }
        let path_norm = normalize_path(track_path);
        let (mtime_ns, size_bytes) = stat_path(track_path);
        let params_hash = self.params_hash();
        let params_json = format!(r#"{{"hop_ms":{}}}"#, self.params.hop_ms);
        let byte_size = (frames.len() * 20) as i64;

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            DELETE FROM analysis_cache_entries
            WHERE analysis_type = ?1 AND path_norm = ?2
              AND analysis_version = ?3 AND params_hash = ?4
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
                frames.len() as i64,
                byte_size,
            ],
        )?;
        let entry_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO analysis_waveform_proxy_frames (
                    entry_id, frame_idx, position_ms,
                    min_left_i8, max_left_i8, min_right_i8, max_right_i8
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
            )?;
            for (idx, (pos, min_l, max_l, min_r, max_r)) in frames.iter().enumerate() {
                stmt.execute(params![
                    entry_id,
                    idx as i64,
                    *pos as i64,
                    *min_l as i64,
                    *max_l as i64,
                    *min_r as i64,
                    *max_r as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_waveform_at(
        &self,
        track_path: &Path,
        position_ms: u64,
    ) -> Result<Option<WaveformReading>, DbError> {
        let Some(entry_id) = self.lookup_entry_id(track_path)? else {
            return Ok(None);
        };
        let conn = self.connect()?;
        let row = conn
            .query_row(
                r#"
                SELECT min_left_i8, max_left_i8, min_right_i8, max_right_i8
                FROM analysis_waveform_proxy_frames
                WHERE entry_id = ?1 AND position_ms <= ?2
                ORDER BY position_ms DESC
                LIMIT 1
                "#,
                params![entry_id, position_ms as i64],
                |r| {
                    Ok(WaveformReading {
                        min_left: r.get::<_, i64>(0)? as f32 / 127.0,
                        max_left: r.get::<_, i64>(1)? as f32 / 127.0,
                        min_right: r.get::<_, i64>(2)? as f32 / 127.0,
                        max_right: r.get::<_, i64>(3)? as f32 / 127.0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Most recent buckets at or before `position_ms`, oldest first.
    pub fn get_waveform_range(
        &self,
        track_path: &Path,
        position_ms: u64,
        max_frames: usize,
    ) -> Result<Vec<WaveformReading>, DbError> {
        let Some(entry_id) = self.lookup_entry_id(track_path)? else {
            return Ok(Vec::new());
        };
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT min_left_i8, max_left_i8, min_right_i8, max_right_i8
            FROM analysis_waveform_proxy_frames
            WHERE entry_id = ?1 AND position_ms <= ?2
            ORDER BY position_ms DESC
            LIMIT ?3
            "#,
        )?;
        let rows = stmt.query_map(
            params![entry_id, position_ms as i64, max_frames as i64],
            |r| {
                Ok(WaveformReading {
                    min_left: r.get::<_, i64>(0)? as f32 / 127.0,
                    max_left: r.get::<_, i64>(1)? as f32 / 127.0,
                    min_right: r.get::<_, i64>(2)? as f32 / 127.0,
                    max_right: r.get::<_, i64>(3)? as f32 / 127.0,
                })
            },
        )?;
        let mut out: Vec<WaveformReading> = rows.collect::<Result<_, _>>()?;
        out.reverse();
        Ok(out)
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
            "tz_wfstore_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let path = dir.join("s.mp3");
        std::fs::write(&path, b"x").unwrap();
        let store = WaveformStore::new(&db, WaveformParams::default());
        store.initialize().unwrap();
        store
            .upsert_waveform(
                &path,
                1000,
                &[(0, -40, 40, -20, 20), (20, -60, 60, -30, 30)],
            )
            .unwrap();
        assert!(store.has_waveform(&path).unwrap());
        let r = store.get_waveform_at(&path, 25).unwrap().unwrap();
        assert!(r.max_left > 0.4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn range_returns_oldest_first_bounded_by_position_and_limit() {
        let dir = std::env::temp_dir().join(format!(
            "tz_wfstore_range_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let path = dir.join("s.mp3");
        std::fs::write(&path, b"x").unwrap();
        let store = WaveformStore::new(&db, WaveformParams::default());
        store.initialize().unwrap();
        store
            .upsert_waveform(
                &path,
                1000,
                &[
                    (0, -10, 10, -10, 10),
                    (20, -20, 20, -20, 20),
                    (40, -30, 30, -30, 30),
                    (60, -40, 40, -40, 40),
                    (80, -50, 50, -50, 50),
                ],
            )
            .unwrap();

        // Bounded by position: frames after position_ms=45 are excluded.
        let rows = store.get_waveform_range(&path, 45, 10).unwrap();
        assert_eq!(rows.len(), 3);
        // Oldest first: min_left should be increasing in magnitude toward the end.
        assert!(rows[0].min_left.abs() < rows[2].min_left.abs());

        // Bounded by limit: only the most recent `max_frames` are kept.
        let limited = store.get_waveform_range(&path, 80, 2).unwrap();
        assert_eq!(limited.len(), 2);
        // Last entry is the newest bucket (position 80, max_left=50/127).
        assert!((limited[1].max_left - 50.0 / 127.0).abs() < 1e-3);

        let _ = std::fs::remove_dir_all(dir);
    }
}
