//! SQLite cache for spectrum frames (analysis type = "spectrum").

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};

use crate::error::DbError;
use crate::path_util::{normalize_path, stat_path};
use crate::{create_schema, ensure_playlist_search_fts, open_connection};

const ANALYSIS_TYPE: &str = "spectrum";
const ANALYSIS_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
pub struct SpectrumParams {
    pub band_count: usize,
    pub hop_ms: u64,
}

impl Default for SpectrumParams {
    fn default() -> Self {
        Self {
            band_count: 48,
            hop_ms: 40,
        }
    }
}

/// Cached spectrum lookup / upsert for visualizers.
pub struct SpectrumStore {
    db_path: PathBuf,
    params: SpectrumParams,
}

impl SpectrumStore {
    pub fn new(db_path: impl Into<PathBuf>, params: SpectrumParams) -> Self {
        Self {
            db_path: db_path.into(),
            params: SpectrumParams {
                band_count: params.band_count.max(8),
                hop_ms: params.hop_ms.max(10),
            },
        }
    }

    pub fn params(&self) -> SpectrumParams {
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
        format!(
            r#"{{"band_count":{},"hop_ms":{}}}"#,
            self.params.band_count, self.params.hop_ms
        )
    }

    fn params_hash(&self) -> String {
        let mut h = Sha1::new();
        h.update(self.params_json().as_bytes());
        hex::encode(h.finalize())
    }

    pub fn has_spectrum(&self, track_path: &Path) -> Result<bool, DbError> {
        Ok(self.lookup_entry_id(track_path)?.is_some())
    }

    pub fn upsert_spectrum(
        &self,
        track_path: &Path,
        duration_ms: u64,
        frames: &[(u64, Vec<u8>)],
    ) -> Result<(), DbError> {
        if frames.is_empty() {
            return Ok(());
        }
        let path_norm = normalize_path(track_path);
        let (mtime_ns, size_bytes) = stat_path(track_path);
        let params_json = self.params_json();
        let params_hash = self.params_hash();
        let byte_size: i64 = frames.iter().map(|(_, b)| b.len() as i64).sum();

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
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
                frames.len() as i64,
                byte_size,
            ],
        )?;
        let entry_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO analysis_spectrum_frames (entry_id, frame_idx, position_ms, bands)
                VALUES (?1, ?2, ?3, ?4)
                "#,
            )?;
            for (idx, (pos, bands)) in frames.iter().enumerate() {
                let payload = normalize_bands(bands, self.params.band_count);
                stmt.execute(params![entry_id, idx as i64, *pos as i64, payload])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Nearest spectrum bands at or before position_ms.
    pub fn get_bands_at(
        &self,
        track_path: &Path,
        position_ms: u64,
    ) -> Result<Option<Vec<u8>>, DbError> {
        let Some(entry_id) = self.lookup_entry_id(track_path)? else {
            return Ok(None);
        };
        let conn = self.connect()?;
        let bands: Option<Vec<u8>> = conn
            .query_row(
                r#"
                SELECT bands FROM analysis_spectrum_frames
                WHERE entry_id = ?1 AND position_ms <= ?2
                ORDER BY position_ms DESC
                LIMIT 1
                "#,
                params![entry_id, position_ms as i64],
                |r| r.get(0),
            )
            .optional()?;
        if bands.is_some() {
            return Ok(bands);
        }
        let first = conn
            .query_row(
                r#"
                SELECT bands FROM analysis_spectrum_frames
                WHERE entry_id = ?1
                ORDER BY position_ms ASC
                LIMIT 1
                "#,
                [entry_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(first)
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

fn normalize_bands(raw: &[u8], band_count: usize) -> Vec<u8> {
    let mut out = vec![0u8; band_count];
    let n = raw.len().min(band_count);
    out[..n].copy_from_slice(&raw[..n]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn upsert_and_lookup() {
        let dir = std::env::temp_dir().join(format!(
            "tz_specstore_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let path = dir.join("song.mp3");
        std::fs::write(&path, b"x").unwrap();

        let store = SpectrumStore::new(&db, SpectrumParams::default());
        store.initialize().unwrap();
        let bands = vec![10u8; 48];
        store
            .upsert_spectrum(&path, 1000, &[(0, bands.clone()), (40, vec![20u8; 48])])
            .unwrap();
        assert!(store.has_spectrum(&path).unwrap());
        let got = store.get_bands_at(&path, 45).unwrap().unwrap();
        assert_eq!(got[0], 20);
        let _ = std::fs::remove_dir_all(dir);
    }
}
