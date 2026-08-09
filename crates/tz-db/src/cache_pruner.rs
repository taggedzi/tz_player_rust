//! Retention pruning for the shared `analysis_cache_entries` table (and its
//! cascaded frame tables). Mirrors the Python reference's
//! `SqliteAnalysisCachePruner`: age-based eviction first (protecting the most
//! recently accessed rows), then oldest-accessed-first eviction until the
//! cache is back under its byte budget.

use std::collections::HashSet;
use std::path::PathBuf;

use rusqlite::{params, Connection};

use crate::error::DbError;
use crate::open_connection;

/// Summary of one prune run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisCachePruneResult {
    pub entries_pruned: i64,
    pub bytes_before: i64,
    pub bytes_after: i64,
}

impl AnalysisCachePruneResult {
    pub fn bytes_reclaimed(&self) -> i64 {
        (self.bytes_before - self.bytes_after).max(0)
    }
}

/// Prunes analysis cache entries by age and storage cap.
pub struct AnalysisCachePruner {
    db_path: PathBuf,
}

impl AnalysisCachePruner {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    fn connect(&self) -> Result<Connection, DbError> {
        open_connection(&self.db_path)
    }

    pub fn total_cache_bytes(&self) -> Result<i64, DbError> {
        let conn = self.connect()?;
        sum_bytes(&conn)
    }

    /// True once the cache is at or above `threshold` (0.0-1.0) of `max_cache_bytes`.
    /// True once the cache is at or above `threshold` (0.0-1.0) of `max_cache_bytes`.
    pub fn exceeds_threshold(&self, max_cache_bytes: i64, threshold: f64) -> Result<bool, DbError> {
        let limit = max_cache_bytes.max(0);
        if limit <= 0 {
            return Ok(false);
        }
        let threshold = threshold.clamp(0.0, 1.0);
        let current = self.total_cache_bytes()?;
        Ok(current as f64 >= limit as f64 * threshold)
    }

    pub fn prune(
        &self,
        max_cache_bytes: i64,
        max_age_days: i64,
        min_recent_tracks_protected: i64,
    ) -> Result<AnalysisCachePruneResult, DbError> {
        let max_cache_bytes = max_cache_bytes.max(0);
        let max_age_days = max_age_days.max(1);
        let min_recent_tracks_protected = min_recent_tracks_protected.max(0);

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let bytes_before = sum_bytes(&tx)?;
        let mut entries_pruned: i64 = 0;

        // Age-based prune first, protecting the most recently accessed rows.
        entries_pruned += tx.execute(
            "DELETE FROM analysis_cache_entries
             WHERE id NOT IN (
                 SELECT id FROM analysis_cache_entries
                 ORDER BY last_accessed_at DESC
                 LIMIT ?1
             )
             AND computed_at < (strftime('%s','now') - (?2 * 86400))",
            params![min_recent_tracks_protected, max_age_days],
        )? as i64;

        let mut total_bytes = sum_bytes(&tx)?;
        if total_bytes > max_cache_bytes {
            let mut protected_stmt = tx.prepare(
                "SELECT id FROM analysis_cache_entries ORDER BY last_accessed_at DESC LIMIT ?1",
            )?;
            let protected_ids: HashSet<i64> = protected_stmt
                .query_map(params![min_recent_tracks_protected], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            drop(protected_stmt);

            let mut rows_stmt = tx.prepare(
                "SELECT id, byte_size FROM analysis_cache_entries ORDER BY last_accessed_at ASC",
            )?;
            let rows: Vec<(i64, i64)> = rows_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            drop(rows_stmt);
            for (id, byte_size) in rows {
                if total_bytes <= max_cache_bytes {
                    break;
                }
                if protected_ids.contains(&id) {
                    continue;
                }
                tx.execute(
                    "DELETE FROM analysis_cache_entries WHERE id = ?1",
                    params![id],
                )?;
                entries_pruned += 1;
                total_bytes = (total_bytes - byte_size).max(0);
            }
        }

        let bytes_after = sum_bytes(&tx)?;
        tx.commit()?;
        Ok(AnalysisCachePruneResult {
            entries_pruned,
            bytes_before,
            bytes_after,
        })
    }
}

fn sum_bytes(conn: &Connection) -> Result<i64, DbError> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(byte_size), 0) FROM analysis_cache_entries",
        [],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_database;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tz_player3_cache_pruner_{name}_{nanos}.db"))
    }

    fn insert_entry(
        conn: &Connection,
        entry_id: i64,
        analysis_type: &str,
        byte_size: i64,
        computed_age_days: i64,
        access_age_days: i64,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO analysis_cache_entries (
                id, analysis_type, path_norm, mtime_ns, size_bytes,
                analysis_version, params_hash, params_json,
                duration_ms, frame_count, byte_size, computed_at, last_accessed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                entry_id,
                analysis_type,
                format!("/tmp/track-{entry_id}.mp3"),
                1000 + entry_id,
                2048 + entry_id,
                1,
                "hash",
                "{}",
                1000,
                4,
                byte_size,
                now - (computed_age_days * 86400),
                now - (access_age_days * 86400),
            ],
        )
        .unwrap();
    }

    #[test]
    fn exceeds_threshold_compares_against_cap() {
        let path = temp_db_path("threshold");
        let conn = open_database(&path).unwrap();
        insert_entry(&conn, 1, "scalar", 900, 1, 1);
        drop(conn);

        let pruner = AnalysisCachePruner::new(&path);
        assert!(pruner.exceeds_threshold(1000, 0.90).unwrap());
        assert!(!pruner.exceeds_threshold(2000, 0.90).unwrap());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prune_evicts_by_age_and_byte_budget() {
        let path = temp_db_path("prune");
        let conn = open_database(&path).unwrap();
        insert_entry(&conn, 1, "scalar", 300, 300, 300);
        insert_entry(&conn, 2, "spectrum", 400, 5, 5);
        insert_entry(&conn, 3, "spectrum", 500, 2, 2);
        drop(conn);

        let pruner = AnalysisCachePruner::new(&path);
        let result = pruner.prune(600, 180, 1).unwrap();

        assert!(result.entries_pruned >= 2);
        assert_eq!(result.bytes_before, 1200);
        assert!(result.bytes_after <= 600);
        assert!(result.bytes_reclaimed() >= 600);

        let conn = open_connection(&path).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM analysis_cache_entries", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 1);

        let _ = std::fs::remove_file(&path);
    }
}
