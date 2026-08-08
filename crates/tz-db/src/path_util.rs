//! Path normalization helpers for track uniqueness.

use std::path::{Component, Path, PathBuf};

/// Normalize paths for uniqueness checks across case / relative variants.
///
/// Matches Python intent: expanduser-ish, resolve when possible, normcase on Windows.
pub fn normalize_path(path: &Path) -> String {
    let expanded = expand_user(path);
    let resolved = match expanded.canonicalize() {
        Ok(p) => p,
        Err(_) => absolute_path(&expanded),
    };
    let s = resolved.to_string_lossy();
    #[cfg(windows)]
    {
        s.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.into_owned()
    }
}

fn expand_user(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs_home() {
            return home;
        }
    }
    path.to_path_buf()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize_dot_components(path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize_dot_components(&cwd.join(path))
}

fn normalize_dot_components(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Stat mtime_ns and size; returns `(None, None)` if the path is unreadable.
pub fn stat_path(path: &Path) -> (Option<i64>, Option<i64>) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);
            (mtime_ns, Some(meta.len() as i64))
        }
        Err(_) => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_is_stable_for_same_path() {
        let p = Path::new("foo/bar.mp3");
        let a = normalize_path(p);
        let b = normalize_path(p);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }
}
