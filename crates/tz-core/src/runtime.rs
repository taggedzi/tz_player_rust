//! Application runtime: wires store, player, levels, state, and control commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tz_control::{Command, ControlError, TransportSnapshot};
use tz_db::{MoveDirection, PlaylistRow, PlaylistStore};
use tz_playback::{BackendKind, BackendStatus};

use crate::levels::LevelService;
use crate::metadata::{read_track_meta, refresh_playlist_metadata};
use crate::paths::AppPaths;
use crate::player::{PlayerService, RepeatMode};
use crate::state::{load_state_with_notice, save_state, AppState};

const STATUS_TTL: Duration = Duration::from_secs(4);

/// Headless-capable app session used by TUI and CLI.
pub struct AppRuntime {
    pub paths: AppPaths,
    pub store: PlaylistStore,
    pub player: PlayerService,
    pub levels: Arc<LevelService>,
    pub app_state: AppState,
    pub playlist_id: i64,
    pub cursor_index: usize,
    pub quit_requested: bool,
    pub status_message: Option<String>,
    status_message_set_at: Option<Instant>,
    /// Filtered item ids when find is active; empty means show all.
    pub find_query: String,
    pub find_ids: Option<Vec<i64>>,
    pub confirm_clear: bool,
    /// "normal" | "find" | "add_path" | "help"
    pub input_mode: String,
    pub input_buffer: String,
    pub visualizer_id: String,
    /// Fallback notice when VLC could not start.
    pub backend_fallback_notice: Option<String>,
    last_level: Option<(f32, f32, String)>,
    last_spectrum: Option<(Vec<u8>, String)>,
    last_beat: Option<(f32, bool, f32, String)>,
    /// (min_l, max_l, min_r, max_r, source)
    last_waveform: Option<(f32, f32, f32, f32, String)>,
    last_waveform_history: Option<Vec<(f32, f32, f32, f32)>>,
    last_analysis_label: Option<String>,
}

/// Bootstrap data dirs, DB, default playlist, and player backend.
pub async fn open_runtime(
    paths: AppPaths,
    backend_override: Option<BackendKind>,
) -> Result<AppRuntime, RuntimeError> {
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| RuntimeError::Io(e.to_string()))?;
    std::fs::create_dir_all(&paths.config_dir).map_err(|e| RuntimeError::Io(e.to_string()))?;
    std::fs::create_dir_all(&paths.log_dir).map_err(|e| RuntimeError::Io(e.to_string()))?;

    let store = PlaylistStore::new(paths.db_file.clone());
    store
        .initialize()
        .map_err(|e| RuntimeError::Db(e.to_string()))?;

    let levels = Arc::new(LevelService::new(paths.db_file.clone()));

    let (mut app_state, state_notice) = load_state_with_notice(&paths.state_file);
    if let Some(kind) = backend_override {
        app_state.playback_backend = kind.as_str().into();
    }
    let backend = BackendKind::parse(&app_state.playback_backend).unwrap_or(BackendKind::Vlc);

    let playlist_id = store
        .ensure_playlist("Default")
        .map_err(|e| RuntimeError::Db(e.to_string()))?;
    app_state.playlist_id = Some(playlist_id);

    let mut backend_fallback_notice = None;
    let mut player = PlayerService::new(PlaylistStore::new(paths.db_file.clone()), backend);
    if let Err(e) = player.start().await {
        if matches!(backend, BackendKind::Vlc) {
            tracing::warn!("VLC backend failed to start ({e}); falling back to fake");
            player =
                PlayerService::new(PlaylistStore::new(paths.db_file.clone()), BackendKind::Fake);
            player
                .start()
                .await
                .map_err(|e| RuntimeError::Playback(e.to_string()))?;
            app_state.playback_backend = "fake".into();
            backend_fallback_notice = Some(format!(
                "VLC unavailable ({e}); using fake backend. Run: tz-player doctor"
            ));
        } else {
            return Err(RuntimeError::Playback(e.to_string()));
        }
    }

    let vol = (app_state.volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    let _ = player.set_volume(vol).await;
    let _ = player.set_speed(app_state.speed).await;
    player.set_shuffle(app_state.shuffle).await;
    player
        .set_repeat(RepeatMode::parse(&app_state.repeat_mode))
        .await;

    let mut cursor_index = 0usize;
    if let Some(item_id) = app_state.current_item_id {
        if let Ok(Some(idx)) = store.get_item_index(playlist_id, item_id) {
            cursor_index = idx.saturating_sub(1);
        }
    }

    let visualizer_id = app_state
        .visualizer_id
        .clone()
        .unwrap_or_else(|| "basic".into());

    let mut runtime = AppRuntime {
        paths,
        store,
        player,
        levels,
        app_state,
        playlist_id,
        cursor_index,
        quit_requested: false,
        status_message: None,
        status_message_set_at: None,
        find_query: String::new(),
        find_ids: None,
        confirm_clear: false,
        input_mode: "normal".into(),
        input_buffer: String::new(),
        visualizer_id,
        backend_fallback_notice,
        last_level: None,
        last_spectrum: None,
        last_beat: None,
        last_waveform: None,
        last_waveform_history: None,
        last_analysis_label: None,
    };
    if let Some(notice) = state_notice {
        // Prefer a short single-line status for the TUI footer.
        runtime.set_status("State file was invalid; settings reset to defaults");
        tracing::warn!("{notice}");
    } else if let Some(fb) = runtime.backend_fallback_notice.clone() {
        runtime.set_status(fb);
    } else if runtime.playlist_count() == 0 {
        runtime.set_status("Empty playlist — press a to add music, or ? for help");
    }
    Ok(runtime)
}

impl AppRuntime {
    /// Set a transient footer status message (auto-clears after a few seconds).
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_set_at = Some(Instant::now());
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_message_set_at = None;
    }

    fn expire_status(&mut self) {
        if let Some(at) = self.status_message_set_at {
            if at.elapsed() >= STATUS_TTL {
                self.clear_status();
            }
        }
    }

    pub fn playlist_count(&self) -> usize {
        if let Some(ids) = &self.find_ids {
            return ids.len();
        }
        self.store.count(self.playlist_id).unwrap_or(0)
    }

    pub fn fetch_rows(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<PlaylistRow>, RuntimeError> {
        if let Some(ids) = &self.find_ids {
            let slice: Vec<i64> = ids.iter().skip(offset).take(limit).copied().collect();
            return self
                .store
                .fetch_rows_by_item_ids(self.playlist_id, &slice)
                .map_err(|e| RuntimeError::Db(e.to_string()));
        }
        self.store
            .fetch_window(self.playlist_id, offset, limit)
            .map_err(|e| RuntimeError::Db(e.to_string()))
    }

    pub fn cursor_item_id(&self) -> Option<i64> {
        let rows = self.fetch_rows(self.cursor_index, 1).ok()?;
        rows.first().map(|r| r.item_id)
    }

    pub async fn snapshot(&self) -> TransportSnapshot {
        let (ll, lr, ls) = match &self.last_level {
            Some((l, r, s)) => (Some(*l), Some(*r), Some(s.clone())),
            None => (None, None, None),
        };
        let (bands, ssrc) = match &self.last_spectrum {
            Some((b, s)) => (Some(b.clone()), Some(s.clone())),
            None => (None, None),
        };
        let (bstr, onset, bpm, bsrc) = match &self.last_beat {
            Some((st, on, bpm, s)) => (Some(*st), Some(*on), Some(*bpm), Some(s.clone())),
            None => (None, None, None, None),
        };
        let (wmin_l, wmax_l, wmin_r, wmax_r, wsrc) = match &self.last_waveform {
            Some((a, b, c, d, s)) => (Some(*a), Some(*b), Some(*c), Some(*d), Some(s.clone())),
            None => (None, None, None, None, None),
        };
        let base = TransportSnapshot {
            cursor_index: self.cursor_index,
            playlist_count: self.playlist_count(),
            level_left: ll,
            level_right: lr,
            level_source: ls,
            spectrum_bands: bands,
            spectrum_source: ssrc,
            beat_strength: bstr,
            beat_is_onset: onset,
            beat_bpm: bpm,
            beat_source: bsrc,
            waveform_min_left: wmin_l,
            waveform_max_left: wmax_l,
            waveform_min_right: wmin_r,
            waveform_max_right: wmax_r,
            waveform_source: wsrc,
            waveform_history: self.last_waveform_history.clone(),
            visualizer_id: Some(self.visualizer_id.clone()),
            find_active: self.input_mode == "find",
            confirm_clear: self.confirm_clear,
            input_mode: self.input_mode.clone(),
            input_buffer: self.input_buffer.clone(),
            find_query: self.find_query.clone(),
            analysis_status: self.last_analysis_label.clone(),
            ..Default::default()
        };
        self.player.transport_snapshot_from(base).await
    }

    pub async fn tick(&mut self) {
        self.expire_status();
        self.player.poll_position().await;
        let snap = self.player.snapshot().await;
        if let Some(path) = snap.track_path.clone() {
            let p = PathBuf::from(&path);
            let sample = self
                .levels
                .sample_all(&p, snap.position_ms, snap.status.as_str());
            self.last_level = Some((
                sample.levels.left,
                sample.levels.right,
                sample.levels.source.as_str().to_string(),
            ));
            self.last_spectrum = match (sample.spectrum_bands, sample.spectrum_source) {
                (Some(b), Some(s)) => Some((b, s.to_string())),
                _ => None,
            };
            self.last_beat = match (
                sample.beat_strength,
                sample.beat_is_onset,
                sample.beat_bpm,
                sample.beat_source,
            ) {
                (Some(st), Some(on), Some(bpm), Some(s)) => Some((st, on, bpm, s.to_string())),
                _ => None,
            };
            self.last_waveform = match (
                sample.waveform_min_left,
                sample.waveform_max_left,
                sample.waveform_min_right,
                sample.waveform_max_right,
                sample.waveform_source,
            ) {
                (Some(a), Some(b), Some(c), Some(d), Some(s)) => Some((a, b, c, d, s.to_string())),
                _ => None,
            };
            self.last_waveform_history = sample.waveform_history;
            let (e, s, b, w) = self.levels.cache_flags(&p);
            let label = if self.levels.is_analyzing() {
                "analyzing".to_string()
            } else {
                format!(
                    "{}{}{}{}",
                    if e { 'E' } else { 'e' },
                    if s { 'S' } else { 's' },
                    if b { 'B' } else { 'b' },
                    if w { 'W' } else { 'w' },
                )
            };
            self.last_analysis_label = Some(label);
        } else {
            self.last_level = None;
            self.last_spectrum = None;
            self.last_beat = None;
            self.last_waveform = None;
            self.last_waveform_history = None;
            self.last_analysis_label = None;
        }
    }

    pub async fn handle(&mut self, cmd: Command) -> Result<(), ControlError> {
        // Input modes intercept most keys via TUI; still handle structured cmds.
        match cmd {
            Command::Quit => {
                self.persist().await;
                self.quit_requested = true;
            }
            Command::PlayPause => {
                let status = self.player.snapshot().await.status;
                if matches!(status, BackendStatus::Playing | BackendStatus::Paused) {
                    self.player
                        .toggle_pause()
                        .await
                        .map_err(|e| ControlError::Message(e.to_string()))?;
                } else if self.playlist_count() == 0 {
                    self.set_status("Playlist empty — press a to add music");
                } else if let Some(id) = self.cursor_item_id() {
                    self.try_play(id).await?;
                } else {
                    self.set_status("No track selected");
                }
            }
            Command::PlayCursor => {
                if self.playlist_count() == 0 {
                    self.set_status("Playlist empty — press a to add music");
                } else if let Some(id) = self.cursor_item_id() {
                    self.try_play(id).await?;
                } else {
                    self.set_status("No track selected");
                }
            }
            Command::PlayItem { item_id } => {
                self.try_play(item_id).await?;
            }
            Command::Stop => {
                self.player
                    .stop()
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::Next => {
                self.player
                    .next()
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
                self.schedule_analysis_for_current().await;
            }
            Command::Previous => {
                self.player
                    .previous()
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
                self.schedule_analysis_for_current().await;
            }
            Command::Seek { position_ms } => {
                self.player
                    .seek_ms(position_ms)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::SeekRelative { delta_ms } => {
                self.player
                    .seek_relative(delta_ms)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::SetVolume { volume } => {
                self.player
                    .set_volume(volume)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::VolumeDelta { delta } => {
                self.player
                    .volume_delta(delta)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::SetSpeed { speed } => {
                self.player
                    .set_speed(speed)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::SpeedDelta { delta } => {
                self.player
                    .speed_delta(delta)
                    .await
                    .map_err(|e| ControlError::Message(e.to_string()))?;
            }
            Command::CycleRepeat => self.player.cycle_repeat().await,
            Command::ToggleShuffle => self.player.toggle_shuffle().await,
            Command::CursorUp => {
                self.cursor_index = self.cursor_index.saturating_sub(1);
            }
            Command::CursorDown => {
                let n = self.playlist_count();
                if n > 0 && self.cursor_index + 1 < n {
                    self.cursor_index += 1;
                }
            }
            Command::PageUp => {
                self.cursor_index = self.cursor_index.saturating_sub(10);
            }
            Command::PageDown => {
                let n = self.playlist_count();
                if n > 0 {
                    self.cursor_index = (self.cursor_index + 10).min(n - 1);
                }
            }
            Command::AddPaths { paths } => {
                let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                self.add_paths_internal(&paths)?;
            }
            Command::RequestAddPath => {
                self.input_mode = "add_path".into();
                self.input_buffer.clear();
                self.set_status("Enter path (Enter=add, Esc=cancel)");
            }
            Command::RemoveSelected => {
                if let Some(id) = self.cursor_item_id() {
                    let mut set = HashSet::new();
                    set.insert(id);
                    match self.store.remove_items(self.playlist_id, &set) {
                        Ok(n) => {
                            self.refresh_find();
                            let count = self.playlist_count();
                            if count == 0 {
                                self.cursor_index = 0;
                            } else if self.cursor_index >= count {
                                self.cursor_index = count - 1;
                            }
                            self.set_status(format!("Removed {n} item(s)"));
                        }
                        Err(e) => self.set_status(format!("Remove failed: {e}")),
                    }
                } else {
                    self.set_status("Nothing to remove");
                }
            }
            Command::ClearPlaylist => {
                if self.playlist_count() == 0 {
                    self.set_status("Playlist already empty");
                } else {
                    self.confirm_clear = true;
                    self.set_status("Clear playlist? y=yes n=no");
                }
            }
            Command::ConfirmClear { yes } => {
                if yes && self.confirm_clear {
                    self.store
                        .clear_playlist(self.playlist_id)
                        .map_err(|e| ControlError::Message(e.to_string()))?;
                    self.cursor_index = 0;
                    self.find_ids = None;
                    self.find_query.clear();
                    self.set_status("Playlist cleared");
                } else if self.confirm_clear {
                    self.set_status("Clear cancelled");
                }
                self.confirm_clear = false;
            }
            Command::RefreshMetadata => {
                if self.playlist_count() == 0 {
                    self.set_status("No tracks to refresh");
                } else {
                    let n = refresh_playlist_metadata(&self.store, self.playlist_id, 2000)
                        .map_err(|e| ControlError::Message(e.to_string()))?;
                    self.set_status(format!("Refreshed metadata for {n} track(s)"));
                }
            }
            Command::CycleVisualizer => {
                // Actual cycle done in TUI host; runtime only records id when set.
                self.set_status(format!("Visualizer: {}", self.visualizer_id));
            }
            Command::SetFindQuery { query } => {
                self.find_query = query;
                self.refresh_find();
                self.cursor_index = 0;
            }
            Command::ClearFind => {
                self.find_query.clear();
                self.find_ids = None;
                self.input_mode = "normal".into();
                self.input_buffer.clear();
                self.set_status("Find cleared");
            }
        }
        Ok(())
    }

    fn refresh_find(&mut self) {
        let q = self.find_query.trim();
        if q.is_empty() {
            self.find_ids = None;
            return;
        }
        match self.store.search_item_ids(self.playlist_id, q, 2000) {
            Ok(ids) => {
                let n = ids.len();
                self.find_ids = Some(ids);
                self.set_status(format!("Find '{q}': {n} hit(s)"));
            }
            Err(e) => {
                self.set_status(format!("Find error: {e}"));
                self.find_ids = None;
            }
        }
    }

    pub fn set_visualizer_id(&mut self, id: &str) {
        self.visualizer_id = id.to_string();
        self.app_state.visualizer_id = Some(id.to_string());
    }

    fn add_paths_internal(&mut self, paths: &[PathBuf]) -> Result<(), ControlError> {
        let expanded = expand_media_paths(paths);
        let added = self
            .store
            .add_tracks(self.playlist_id, &expanded)
            .map_err(|e| ControlError::Message(e.to_string()))?;
        let _ = refresh_playlist_metadata(&self.store, self.playlist_id, 500);
        // Background-ish analysis for new paths (blocking for now, short tracks OK)
        for p in &expanded {
            let levels = self.levels.clone();
            let path = p.clone();
            let _ = std::thread::Builder::new()
                .name("tz-analyze".into())
                .spawn(move || {
                    let _ = levels.ensure_analysis(&path);
                });
        }
        self.refresh_find();
        if added == 0 {
            self.set_status("No media files found at that path");
        } else {
            self.set_status(format!("Added {added} track(s) — analyzing in background"));
        }
        Ok(())
    }

    async fn try_play(&mut self, item_id: i64) -> Result<(), ControlError> {
        match self.player.play_item(self.playlist_id, item_id).await {
            Ok(()) => {
                self.schedule_analysis_for_current().await;
                self.set_status("Playing");
                Ok(())
            }
            Err(e) => {
                self.set_status(e.to_string());
                Ok(())
            }
        }
    }

    async fn schedule_analysis_for_current(&self) {
        let snap = self.player.snapshot().await;
        if let Some(path) = snap.track_path {
            let p = PathBuf::from(&path);
            self.levels.set_active_track(Some(p.clone()));
            let levels = self.levels.clone();
            let _ = std::thread::Builder::new()
                .name("tz-analyze".into())
                .spawn(move || {
                    if let Err(e) = levels.ensure_analysis(&p) {
                        tracing::debug!("analysis: {e}");
                    }
                });
        }
    }

    /// Move cursor item one step (shift+up/down).
    pub fn move_cursor_item(&mut self, up: bool) -> Result<(), RuntimeError> {
        let Some(id) = self.cursor_item_id() else {
            return Ok(());
        };
        let dir = if up {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        };
        self.store
            .move_selection(self.playlist_id, dir, &[id], None)
            .map_err(|e| RuntimeError::Db(e.to_string()))?;
        if up {
            self.cursor_index = self.cursor_index.saturating_sub(1);
        } else {
            let n = self.playlist_count();
            if n > 0 {
                self.cursor_index = (self.cursor_index + 1).min(n - 1);
            }
        }
        Ok(())
    }

    pub async fn persist(&mut self) {
        let snap = self.player.snapshot().await;
        self.app_state.playlist_id = Some(self.playlist_id);
        self.app_state.current_item_id = self.cursor_item_id();
        self.app_state.volume = f64::from(snap.volume) / 100.0;
        self.app_state.speed = snap.speed;
        self.app_state.repeat_mode = snap.repeat_mode.as_str().into();
        self.app_state.shuffle = snap.shuffle;
        self.app_state.playback_backend = snap.backend.as_str().into();
        self.app_state.visualizer_id = Some(self.visualizer_id.clone());
        let _ = save_state(&self.paths.state_file, &self.app_state);
    }

    pub fn add_paths_cli(&mut self, paths: &[PathBuf]) -> Result<usize, RuntimeError> {
        let expanded = expand_media_paths(paths);
        let added = self
            .store
            .add_tracks(self.playlist_id, &expanded)
            .map_err(|e| RuntimeError::Db(e.to_string()))?;
        for path in &expanded {
            if let Ok(rows) = self.store.fetch_window(self.playlist_id, 0, 10_000) {
                if let Some(row) = rows.iter().find(|r| r.path == *path) {
                    let meta = read_track_meta(path);
                    let _ = self.store.upsert_track_meta(row.track_id, &meta);
                }
            }
            let _ = self.levels.ensure_analysis(path);
        }
        Ok(added)
    }
}

fn expand_media_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            collect_media_files_recursive(p, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out.sort();
    out
}

fn collect_media_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.is_dir() {
            collect_media_files_recursive(&path, out);
        } else if path.is_file() && is_media_extension(&path) {
            out.push(path);
        }
    }
}

fn is_media_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "mp3"
            | "flac"
            | "wav"
            | "ogg"
            | "opus"
            | "m4a"
            | "aac"
            | "wma"
            | "aiff"
            | "aif"
            | "ape"
            | "wv"
            | "mp4"
            | "m4b"
            | "mka"
            | "ac3"
            | "dts"
            | "mpc"
            | "tta"
            | "spx"
            | "caf"
            | "mid"
            | "midi"
    )
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("io: {0}")]
    Io(String),
    #[error("db: {0}")]
    Db(String),
    #[error("playback: {0}")]
    Playback(String),
    #[error("{0}")]
    Message(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tz_runtime_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn expand_media_paths_recurses_into_subfolders() {
        let dir = temp_dir("recurse");
        std::fs::write(dir.join("top.mp3"), b"").unwrap();
        let sub = dir.join("album");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("nested.flac"), b"").unwrap();
        let sub2 = sub.join("disc2");
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(sub2.join("deep.wav"), b"").unwrap();

        let found = expand_media_paths(std::slice::from_ref(&dir));

        assert!(found.contains(&dir.join("top.mp3")));
        assert!(found.contains(&sub.join("nested.flac")));
        assert!(found.contains(&sub2.join("deep.wav")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recognizes_additional_vlc_supported_extensions() {
        for ext in [
            "mp4", "m4b", "mka", "ac3", "dts", "mpc", "tta", "spx", "caf", "mid", "midi",
        ] {
            let path = Path::new("track").with_extension(ext);
            assert!(
                is_media_extension(&path),
                "expected {ext} to be recognized as a media extension"
            );
        }
    }
}
