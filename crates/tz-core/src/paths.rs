//! Platform data / config / log directories.
//!
//! Uses a distinct app identity from the Python install so user data is not
//! corrupted during parallel use; import tools can bridge later.

use directories::{BaseDirs, ProjectDirs};
use std::path::PathBuf;

/// Resolved filesystem locations for the Rust tz-player.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub state_file: PathBuf,
    pub db_file: PathBuf,
}

/// Qualifier / organization / application for `directories`.
/// Distinct from Python `tz-player` platformdirs identity.
const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "taggedzi";
const APPLICATION: &str = "tz-player-rs";

/// Resolve default application paths for the current user.
pub fn app_paths() -> Option<AppPaths> {
    let project = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)?;
    let data_dir = project.data_dir().to_path_buf();
    let config_dir = project.config_dir().to_path_buf();
    let log_dir = data_dir.join("logs");
    Some(AppPaths {
        state_file: config_dir.join("state.json"),
        db_file: data_dir.join("tz-player.sqlite3"),
        log_dir,
        data_dir,
        config_dir,
    })
}

/// Fallback when ProjectDirs is unavailable (rare).
pub fn app_paths_or_cwd() -> AppPaths {
    if let Some(paths) = app_paths() {
        return paths;
    }
    let base = BaseDirs::new()
        .map(|b| b.home_dir().join(".tz-player-rs"))
        .unwrap_or_else(|| PathBuf::from(".tz-player-rs"));
    AppPaths {
        state_file: base.join("state.json"),
        db_file: base.join("tz-player.sqlite3"),
        log_dir: base.join("logs"),
        config_dir: base.clone(),
        data_dir: base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve() {
        let p = app_paths_or_cwd();
        assert!(p.db_file.file_name().is_some());
        assert!(p.state_file.extension().and_then(|e| e.to_str()) == Some("json"));
    }
}
