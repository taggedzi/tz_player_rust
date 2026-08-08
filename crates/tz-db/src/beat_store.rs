//! SQLite cache for beat/onset frames (analysis type = "beat").

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};

use crate::error::DbError;
use crate::path_util::{normalize_path, stat_path};
use crate::{create_schema, ensure_playlist_search_fts, open_connection};

const ANALYSIS_TYPE: &str = "beat";
const ANALYSIS_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
pub struct BeatParams {
    pub hop_ms: u64,
}

impl Default for BeatParams {
    fn default() -> Self {
        Self { hop_ms: 40 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BeatReading {
    pub strength: f32,
    pub is_onset: bool,
    pub bpm: f32,
}

pub struct BeatStore {
    db_path: PathBuf,
    params: BeatParams,
}

impl BeatStore {
    pub fn new(db_path: impl Into<PathBuf>, params: BeatParams) -> Self {
        Self {
            db_path: db_path.into(),
            params: BeatParams {
                hop_ms: params.hop_ms.max(10),
            },
        }
    }

    pub fn params(&self) -> BeatParams {
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

    fn params_json(&self) -> String {
        format!(r#"{{"hop_ms":{}}}"#, self.params.hop_ms)
    }

    fn params_hash(&self) -> String {
        let mut h = Sha1::new();
        h.update(self.params_json().as_bytes());
        hex::encode(h.finalize())
    }

    pub fn has_beats(&self, track_path: &Path) -> Result<bool, DbError> {
        Ok(self.lookup_entry_id(track_path)?.is_some())
    }

    pub fn upsert_beats(
        &self,
        track_path: &Path,
        duration_ms: u64,
        bpm: f64,
        frames: &[(u64, u8, bool)],
    ) -> Result<(), DbError> {
        if frames.is_empty() {
            return Ok(());
        }
        let path_norm = normalize_path(track_path);
        let (mtime_ns, size_bytes) = stat_path(track_path);
        let params_hash = self.params_hash();
        let params_json = format!(r#"{{"hop_ms":{},"bpm":{}}}"#, self.params.hop_ms, bpm);
        let byte_size = (frames.len() * 16) as i64;

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
                INSERT INTO analysis_beat_frames (
                    entry_id, frame_idx, position_ms, strength_u8, is_beat, bpm
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )?;
            for (idx, (pos, strength, is_beat)) in frames.iter().enumerate() {
                stmt.execute(params![
                    entry_id,
                    idx as i64,
                    *pos as i64,
                    *strength as i64,
                    if *is_beat { 1i64 } else { 0 },
                    bpm
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_beat_at(
        &self,
        track_path: &Path,
        position_ms: u64,
    ) -> Result<Option<BeatReading>, DbError> {
        let Some(entry_id) = self.lookup_entry_id(track_path)? else {
            return Ok(None);
        };
        let conn = self.connect()?;
        let row = conn
            .query_row(
                r#"
                SELECT strength_u8, is_beat, bpm
                FROM analysis_beat_frames
                WHERE entry_id = ?1 AND position_ms <= ?2
                ORDER BY position_ms DESC
                LIMIT 1
                "#,
                params![entry_id, position_ms as i64],
                |r| {
                    Ok(BeatReading {
                        strength: r.get::<_, i64>(0)? as f32 / 255.0,
                        is_onset: r.get::<_, i64>(1)? != 0,
                        bpm: r.get::<_, f64>(2)? as f32,
                    })
                },
            )
            .optional()?;
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
            "tz_beatstore_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let path = dir.join("s.mp3");
        std::fs::write(&path, b"x").unwrap();
        let store = BeatStore::new(&db, BeatParams::default());
        store.initialize().unwrap();
        store
            .upsert_beats(&path, 1000, 120.0, &[(0, 10, false), (40, 200, true)])
            .unwrap();
        assert!(store.has_beats(&path).unwrap());
        let b = store.get_beat_at(&path, 45).unwrap().unwrap();
        assert!(b.is_onset);
        assert!((b.bpm - 120.0).abs() < 0.1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
