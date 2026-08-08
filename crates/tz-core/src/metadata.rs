//! Tag reading via lofty (Python tinytag parity target).

use std::path::Path;

use lofty::file::AudioFile;
use lofty::prelude::*;
use lofty::probe::Probe;

use tz_db::TrackMeta;

/// Read tags from a local audio file into a `TrackMeta` payload.
pub fn read_track_meta(path: &Path) -> TrackMeta {
    let (mtime_ns, size_bytes) = match std::fs::metadata(path) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);
            (mtime, Some(m.len() as i64))
        }
        Err(_) => (None, None),
    };

    match Probe::open(path).and_then(|p| p.read()) {
        Ok(tagged) => {
            let props = tagged.properties();
            let duration_ms = props.duration().as_millis() as i64;
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
            let (title, artist, album, year) = if let Some(tag) = tag {
                (
                    tag.title().map(|s| s.to_string()),
                    tag.artist().map(|s| s.to_string()),
                    tag.album().map(|s| s.to_string()),
                    tag.year().map(|y| y as i32),
                )
            } else {
                (None, None, None, None)
            };
            TrackMeta {
                title,
                artist,
                album,
                year,
                duration_ms: if duration_ms > 0 {
                    Some(duration_ms)
                } else {
                    None
                },
                meta_valid: true,
                meta_error: None,
                mtime_ns,
                size_bytes,
            }
        }
        Err(e) => TrackMeta {
            title: None,
            artist: None,
            album: None,
            year: None,
            duration_ms: None,
            meta_valid: false,
            meta_error: Some(e.to_string()),
            mtime_ns,
            size_bytes,
        },
    }
}

/// Refresh metadata for all tracks currently missing valid meta in a playlist window.
pub fn refresh_playlist_metadata(
    store: &tz_db::PlaylistStore,
    playlist_id: i64,
    limit: usize,
) -> Result<usize, tz_db::DbError> {
    let rows = store.fetch_window(playlist_id, 0, limit)?;
    let mut updated = 0usize;
    for row in rows {
        if row.meta_valid == Some(true) {
            continue;
        }
        let meta = read_track_meta(&row.path);
        store.upsert_track_meta(row.track_id, &meta)?;
        updated += 1;
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn missing_file_marks_invalid() {
        let meta = read_track_meta(Path::new("definitely-missing-tz-player.mp3"));
        assert!(!meta.meta_valid);
        assert!(meta.meta_error.is_some());
    }

    #[test]
    fn empty_file_does_not_panic() {
        let dir = std::env::temp_dir().join(format!(
            "tz_meta_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.mp3");
        let mut f = std::fs::File::create(&path).unwrap();
        let _ = f.write_all(b"");
        let meta = read_track_meta(&path);
        // lofty may fail; just ensure no panic
        let _ = meta.meta_valid;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
