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
use crate::player::{PlayerError, PlayerService, RepeatMode};
use crate::state::{load_state_with_notice, save_state, AppState};

const STATUS_TTL: Duration = Duration::from_secs(4);

/// Severity of the current footer status message. Only `Error` (reserved for
/// playback-backend failures — the audio path itself is disrupted) survives
/// past `STATUS_TTL`; it stays until explicitly dismissed. `Warn` and `Info`
/// both auto-clear, since neither indicates playback is actually broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Warn,
    Error,
}

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
    pub status_level: StatusLevel,
    status_message_set_at: Option<Instant>,
    /// Filtered item ids when find is active; empty means show all.
    pub find_query: String,
    pub find_ids: Option<Vec<i64>>,
    pub confirm_clear: bool,
    /// "normal" | "find" | "browse" | "help"
    pub input_mode: String,
    pub input_buffer: String,
    /// Current directory shown by the folder-browser modal. `None` means
    /// the synthetic drive-selection level (Windows only, reached by going
    /// up from a drive root — see `Command::BrowseParent`).
    pub browse_dir: Option<PathBuf>,
    pub browse_entries: Vec<FsEntry>,
    pub browse_cursor: usize,
    /// Directory the browser starts at on its *next* open this session.
    /// Not persisted to `AppState` — the first open of a run always starts
    /// at the current working directory.
    last_browse_dir: Option<PathBuf>,
    pub visualizer_id: String,
    /// Collapses the visualizer pane (playlist takes the full width) when
    /// true. Session-only — not persisted to `AppState`.
    pub visualizer_hidden: bool,
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
        status_level: StatusLevel::Info,
        status_message_set_at: None,
        find_query: String::new(),
        find_ids: None,
        confirm_clear: false,
        input_mode: "normal".into(),
        input_buffer: String::new(),
        browse_dir: None,
        browse_entries: Vec::new(),
        browse_cursor: 0,
        last_browse_dir: None,
        visualizer_id,
        visualizer_hidden: false,
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
        runtime.set_warning("State file was invalid; settings reset to defaults");
        tracing::warn!("{notice}");
    } else if let Some(fb) = runtime.backend_fallback_notice.clone() {
        // The playback backend itself failed to start — this is exactly the
        // "disrupting playback" case, so it stays until dismissed.
        runtime.set_error(fb);
    } else if runtime.playlist_count() == 0 {
        runtime.set_status("Empty playlist — press a to add music, or ? for help");
    }
    Ok(runtime)
}

impl AppRuntime {
    /// Set a transient, informational footer status message (auto-clears
    /// after a few seconds).
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.set_status_with_level(msg, StatusLevel::Info);
    }

    /// Set a transient warning (auto-clears like `set_status`, but rendered
    /// distinctly). For problems that don't disrupt playback itself.
    pub fn set_warning(&mut self, msg: impl Into<String>) {
        self.set_status_with_level(msg, StatusLevel::Warn);
    }

    /// Set an error that persists until dismissed (does not auto-clear).
    /// Reserved for playback-backend failures — the audio path is disrupted.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.set_status_with_level(msg, StatusLevel::Error);
    }

    fn set_status_with_level(&mut self, msg: impl Into<String>, level: StatusLevel) {
        self.status_message = Some(msg.into());
        self.status_level = level;
        self.status_message_set_at = Some(Instant::now());
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_level = StatusLevel::Info;
        self.status_message_set_at = None;
    }

    fn expire_status(&mut self) {
        if self.status_level == StatusLevel::Error {
            return;
        }
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
                    if let Err(e) = self.player.toggle_pause().await {
                        self.report_player_error(e);
                    }
                } else if self.playlist_count() == 0 {
                    self.set_status("Playlist empty — press a to add music");
                } else if let Some(id) = self.cursor_item_id() {
                    self.try_play(id).await;
                } else {
                    self.set_status("No track selected");
                }
            }
            Command::PlayCursor => {
                if self.playlist_count() == 0 {
                    self.set_status("Playlist empty — press a to add music");
                } else if let Some(id) = self.cursor_item_id() {
                    self.try_play(id).await;
                } else {
                    self.set_status("No track selected");
                }
            }
            Command::PlayItem { item_id } => {
                self.try_play(item_id).await;
            }
            Command::Stop => {
                if let Err(e) = self.player.stop().await {
                    self.report_player_error(e);
                }
            }
            Command::Next => match self.player.next().await {
                Ok(()) => self.schedule_analysis_for_current().await,
                Err(e) => self.report_player_error(e),
            },
            Command::Previous => match self.player.previous().await {
                Ok(()) => self.schedule_analysis_for_current().await,
                Err(e) => self.report_player_error(e),
            },
            Command::Seek { position_ms } => {
                if let Err(e) = self.player.seek_ms(position_ms).await {
                    self.report_player_error(e);
                }
            }
            Command::SeekRelative { delta_ms } => {
                if let Err(e) = self.player.seek_relative(delta_ms).await {
                    self.report_player_error(e);
                }
            }
            Command::SetVolume { volume } => {
                if let Err(e) = self.player.set_volume(volume).await {
                    self.report_player_error(e);
                }
            }
            Command::VolumeDelta { delta } => {
                if let Err(e) = self.player.volume_delta(delta).await {
                    self.report_player_error(e);
                }
            }
            Command::SetSpeed { speed } => {
                if let Err(e) = self.player.set_speed(speed).await {
                    self.report_player_error(e);
                }
            }
            Command::SpeedDelta { delta } => {
                if let Err(e) = self.player.speed_delta(delta).await {
                    self.report_player_error(e);
                }
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
            Command::LocatePlaying => self.locate_playing().await,
            Command::AddPaths { paths } => {
                let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                self.add_paths_internal(&paths)?;
            }
            Command::RequestAddFolder => {
                let dir = self.last_browse_dir.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                self.browse_entries = list_dir(&dir);
                self.browse_dir = Some(dir);
                self.browse_cursor = 0;
                self.input_mode = "browse".into();
                self.set_status(
                    "Browse: Enter=open/add file  a/Space=add folder  Backspace=up  Esc=cancel",
                );
            }
            Command::BrowseUp => {
                self.browse_cursor = self.browse_cursor.saturating_sub(1);
            }
            Command::BrowseDown => {
                if !self.browse_entries.is_empty() {
                    self.browse_cursor =
                        (self.browse_cursor + 1).min(self.browse_entries.len() - 1);
                }
            }
            Command::BrowseEnter => {
                if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                    if entry.is_dir {
                        if std::fs::read_dir(&entry.path).is_err() {
                            // Keep the browser at the current (still-valid)
                            // directory rather than switching into one we
                            // can't actually list — an empty pane with no
                            // explanation would look like a bug, not a
                            // permissions issue.
                            self.set_warning(format!("Can't open '{}'", entry.name));
                        } else {
                            self.browse_entries = list_dir(&entry.path);
                            self.browse_dir = Some(entry.path.clone());
                            self.last_browse_dir = Some(entry.path);
                            self.browse_cursor = 0;
                        }
                    } else {
                        let name = entry.name.clone();
                        self.add_paths_internal(&[entry.path])?;
                        self.input_mode = "normal".into();
                        self.set_status(format!("Added '{name}'"));
                    }
                }
            }
            Command::BrowseSelect => {
                if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                    let name = entry.name.clone();
                    self.add_paths_internal(&[entry.path])?;
                    self.input_mode = "normal".into();
                    self.set_status(format!("Added '{name}'"));
                }
            }
            Command::BrowseParent => match self.browse_dir.clone() {
                Some(dir) => match dir.parent() {
                    Some(parent) => {
                        if std::fs::read_dir(parent).is_err() {
                            self.set_warning(format!("Can't open '{}'", parent.display()));
                        } else {
                            let parent = parent.to_path_buf();
                            self.browse_entries = list_dir(&parent);
                            self.browse_dir = Some(parent);
                            self.browse_cursor = 0;
                        }
                    }
                    None => {
                        let drives = drive_list();
                        if !drives.is_empty() {
                            self.browse_entries = drives;
                            self.browse_dir = None;
                            self.browse_cursor = 0;
                        }
                    }
                },
                None => {}
            },
            Command::BrowseCancel => {
                self.input_mode = "normal".into();
                self.browse_entries.clear();
                self.browse_cursor = 0;
                self.set_status("Cancelled");
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
                    match self.store.clear_playlist(self.playlist_id) {
                        Ok(()) => {
                            self.cursor_index = 0;
                            self.find_ids = None;
                            self.find_query.clear();
                            self.set_status("Playlist cleared");
                        }
                        Err(e) => self.set_warning(format!("Clear failed: {e}")),
                    }
                } else if self.confirm_clear {
                    self.set_status("Clear cancelled");
                }
                self.confirm_clear = false;
            }
            Command::RefreshMetadata => {
                if self.playlist_count() == 0 {
                    self.set_status("No tracks to refresh");
                } else {
                    match refresh_playlist_metadata(&self.store, self.playlist_id, 2000) {
                        Ok(n) => self.set_status(format!("Refreshed metadata for {n} track(s)")),
                        Err(e) => self.set_warning(format!("Metadata refresh failed: {e}")),
                    }
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
        let added = match self.store.add_tracks(self.playlist_id, &expanded) {
            Ok(n) => n,
            Err(e) => {
                self.set_warning(format!("Add failed: {e}"));
                return Ok(());
            }
        };
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

    async fn try_play(&mut self, item_id: i64) {
        match self.player.play_item(self.playlist_id, item_id).await {
            Ok(()) => {
                self.schedule_analysis_for_current().await;
                self.set_status("Playing");
            }
            Err(e) => self.report_player_error(e),
        }
    }

    /// Surface a player-layer failure at the right severity: only backend
    /// (VLC/libVLC) failures actually disrupt playback and persist until
    /// dismissed; data-layer failures (missing/unreadable track row) are
    /// quiet, auto-clearing warnings.
    fn report_player_error(&mut self, e: PlayerError) {
        if e.is_backend_failure() {
            self.set_error(e.to_string());
        } else {
            self.set_warning(e.to_string());
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

    /// Move the cursor to the currently-playing (or last-played) track.
    ///
    /// `cursor_index` always indexes whatever list `fetch_rows` is currently
    /// reading, so when a find filter is active this must resolve within the
    /// filtered ids, not the full playlist's `pos_key` order. If the playing
    /// track isn't in the filtered set, the find is cleared so the jump can
    /// still land somewhere meaningful.
    async fn locate_playing(&mut self) {
        let Some(id) = self.player.snapshot().await.item_id else {
            self.set_status("Nothing has played yet");
            return;
        };
        if let Some(ids) = &self.find_ids {
            if let Some(idx) = ids.iter().position(|&x| x == id) {
                self.cursor_index = idx;
                self.set_status("Located now-playing track");
                return;
            }
            self.find_ids = None;
            self.find_query.clear();
        }
        match self.store.get_item_index(self.playlist_id, id) {
            Ok(Some(idx1)) => {
                self.cursor_index = idx1.saturating_sub(1);
                self.set_status("Located now-playing track");
            }
            Ok(None) => self.set_warning("Now-playing track is no longer in the playlist"),
            Err(e) => self.set_warning(format!("Locate failed: {e}")),
        }
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

/// One entry in a directory listing shown by the folder-browser modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// List `dir`'s contents for the folder-browser modal: every subdirectory,
/// plus files recognized by `is_media_extension` (non-media clutter stays
/// out of the pane). Directories sort first, then alphabetically
/// (case-insensitive) within each group. An unreadable or missing
/// directory yields an empty list rather than erroring — callers fall back
/// to the previous, still-valid directory.
pub fn list_dir(dir: &Path) -> Vec<FsEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for ent in rd.flatten() {
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            dirs.push(FsEntry {
                name,
                path,
                is_dir: true,
            });
        } else if path.is_file() && is_media_extension(&path) {
            files.push(FsEntry {
                name,
                path,
                is_dir: false,
            });
        }
    }
    let by_name_ci = |a: &FsEntry, b: &FsEntry| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    };
    dirs.sort_by(by_name_ci);
    files.sort_by(by_name_ci);
    dirs.extend(files);
    dirs
}

/// Windows drive letters currently mounted, as synthetic browse entries
/// (e.g. `C:\`). Reached only by going "up" from a drive root — no single
/// filesystem parent spans drives. Always empty on non-Windows targets,
/// where `/` has no such concept.
#[cfg(windows)]
pub fn drive_list() -> Vec<FsEntry> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let path = PathBuf::from(&root);
        if std::fs::metadata(&path).is_ok() {
            out.push(FsEntry {
                name: root,
                path,
                is_dir: true,
            });
        }
    }
    out
}

#[cfg(not(windows))]
pub fn drive_list() -> Vec<FsEntry> {
    Vec::new()
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

    async fn test_runtime(name: &str) -> AppRuntime {
        let dir = temp_dir(name);
        let paths = AppPaths {
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            log_dir: dir.join("logs"),
            state_file: dir.join("state.json"),
            db_file: dir.join("db.sqlite3"),
        };
        open_runtime(paths, Some(BackendKind::Fake)).await.unwrap()
    }

    #[tokio::test]
    async fn set_error_persists_past_ttl() {
        let mut runtime = test_runtime("error_ttl").await;
        runtime.set_error("playback backend failed");
        runtime.status_message_set_at =
            Some(Instant::now() - STATUS_TTL - Duration::from_millis(50));

        runtime.expire_status();

        assert_eq!(
            runtime.status_message.as_deref(),
            Some("playback backend failed")
        );
        assert_eq!(runtime.status_level, StatusLevel::Error);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn set_warning_expires_like_info() {
        let mut runtime = test_runtime("warning_ttl").await;
        runtime.set_warning("metadata refresh failed");
        runtime.status_message_set_at =
            Some(Instant::now() - STATUS_TTL - Duration::from_millis(50));

        runtime.expire_status();

        assert_eq!(runtime.status_message, None);
        assert_eq!(runtime.status_level, StatusLevel::Info);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn report_player_error_classifies_by_variant() {
        let mut runtime = test_runtime("player_error_classification").await;

        runtime.report_player_error(PlayerError::Db("row missing".into()));
        assert_eq!(runtime.status_level, StatusLevel::Warn);

        runtime.report_player_error(PlayerError::Playback(tz_playback::PlaybackError::message(
            "VLC crashed",
        )));
        assert_eq!(runtime.status_level, StatusLevel::Error);

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn clear_status_dismisses_an_active_error() {
        let mut runtime = test_runtime("error_dismiss").await;
        runtime.set_error("playback backend failed");

        runtime.clear_status();

        assert_eq!(runtime.status_message, None);
        assert_eq!(runtime.status_level, StatusLevel::Info);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    async fn seed_tracks(runtime: &mut AppRuntime, names: &[&str]) -> Vec<i64> {
        let dir = runtime.paths.data_dir.clone();
        let paths: Vec<PathBuf> = names
            .iter()
            .map(|n| {
                let p = dir.join(n);
                std::fs::write(&p, b"").unwrap();
                p
            })
            .collect();
        runtime
            .store
            .add_tracks(runtime.playlist_id, &paths)
            .unwrap();
        runtime.store.list_item_ids(runtime.playlist_id).unwrap()
    }

    #[tokio::test]
    async fn locate_playing_moves_cursor_to_currently_playing_track() {
        let mut runtime = test_runtime("locate_playing").await;
        let ids = seed_tracks(&mut runtime, &["a.mp3", "b.mp3", "c.mp3"]).await;
        runtime.try_play(ids[2]).await;
        runtime.cursor_index = 0;

        runtime.handle(Command::LocatePlaying).await.unwrap();

        // Assert against the same read path draw_playlist uses (fetch_rows),
        // not a hardcoded index, so this can't pass by coincidence if
        // get_item_index's pos_key order ever diverges from fetch_window's.
        let landed = runtime.fetch_rows(runtime.cursor_index, 1).unwrap();
        assert_eq!(landed[0].item_id, ids[2]);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn locate_playing_searches_within_active_find_results() {
        let mut runtime = test_runtime("locate_playing_find").await;
        let ids = seed_tracks(&mut runtime, &["a.mp3", "b.mp3", "c.mp3"]).await;
        runtime.try_play(ids[2]).await;
        runtime.find_ids = Some(vec![ids[1], ids[2]]);
        runtime.cursor_index = 0;

        runtime.handle(Command::LocatePlaying).await.unwrap();

        let landed = runtime.fetch_rows(runtime.cursor_index, 1).unwrap();
        assert_eq!(landed[0].item_id, ids[2]);
        assert!(
            runtime.find_ids.is_some(),
            "an active find that already contains the playing track should be preserved"
        );
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn locate_playing_clears_find_when_playing_track_is_filtered_out() {
        let mut runtime = test_runtime("locate_playing_find_miss").await;
        let ids = seed_tracks(&mut runtime, &["a.mp3", "b.mp3", "c.mp3"]).await;
        runtime.try_play(ids[0]).await;
        runtime.find_ids = Some(vec![ids[1], ids[2]]);
        runtime.cursor_index = 2;

        runtime.handle(Command::LocatePlaying).await.unwrap();

        assert!(
            runtime.find_ids.is_none(),
            "find should be cleared once it no longer contains the playing track"
        );
        let landed = runtime.fetch_rows(runtime.cursor_index, 1).unwrap();
        assert_eq!(landed[0].item_id, ids[0]);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn locate_playing_is_a_noop_when_nothing_has_played() {
        let mut runtime = test_runtime("locate_playing_none").await;
        seed_tracks(&mut runtime, &["a.mp3", "b.mp3"]).await;
        runtime.cursor_index = 1;

        runtime.handle(Command::LocatePlaying).await.unwrap();

        assert_eq!(runtime.cursor_index, 1);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[test]
    fn list_dir_sorts_directories_before_files_case_insensitively() {
        let dir = temp_dir("list_sort");
        std::fs::write(dir.join("zebra.mp3"), b"").unwrap();
        std::fs::write(dir.join("Alpha.mp3"), b"").unwrap();
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::create_dir_all(dir.join("beta")).unwrap();

        let entries = list_dir(&dir);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

        assert_eq!(names, vec!["beta", "Zeta", "Alpha.mp3", "zebra.mp3"]);
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir && !entries[3].is_dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_dir_filters_out_non_media_files() {
        let dir = temp_dir("list_filter");
        std::fs::write(dir.join("song.mp3"), b"").unwrap();
        std::fs::write(dir.join("cover.jpg"), b"").unwrap();
        std::fs::write(dir.join("readme.txt"), b"").unwrap();

        let entries = list_dir(&dir);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "song.mp3");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_dir_on_unreadable_or_missing_path_returns_empty() {
        let missing = std::env::temp_dir().join("tz_runtime_does_not_exist_12345");
        assert!(list_dir(&missing).is_empty());
    }

    #[tokio::test]
    async fn request_add_folder_opens_browser_at_last_dir_or_cwd() {
        let mut runtime = test_runtime("browse_open").await;
        let dir = temp_dir("browse_open_target");
        std::fs::write(dir.join("track.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());

        runtime.handle(Command::RequestAddFolder).await.unwrap();

        assert_eq!(runtime.input_mode, "browse");
        assert_eq!(runtime.browse_dir, Some(dir.clone()));
        assert_eq!(runtime.browse_cursor, 0);
        assert!(runtime.browse_entries.iter().any(|e| e.name == "track.mp3"));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_enter_descends_into_a_directory() {
        let mut runtime = test_runtime("browse_descend").await;
        let root = temp_dir("browse_descend_root");
        let sub = root.join("Album");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("song.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(root.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();
        assert_eq!(runtime.browse_cursor, 0); // "Album" sorts as the only entry

        runtime.handle(Command::BrowseEnter).await.unwrap();

        assert_eq!(runtime.browse_dir, Some(sub.clone()));
        assert_eq!(runtime.last_browse_dir, Some(sub.clone()));
        assert!(runtime.browse_entries.iter().any(|e| e.name == "song.mp3"));
        assert_eq!(runtime.input_mode, "browse", "descending should not close the modal");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_enter_on_a_file_adds_it_and_closes() {
        let mut runtime = test_runtime("browse_add_file").await;
        let dir = temp_dir("browse_add_file_target");
        std::fs::write(dir.join("only.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();

        runtime.handle(Command::BrowseEnter).await.unwrap();

        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.playlist_count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_select_adds_a_folder_recursively_and_closes() {
        let mut runtime = test_runtime("browse_add_folder").await;
        let root = temp_dir("browse_add_folder_root");
        let sub = root.join("Album");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("one.mp3"), b"").unwrap();
        std::fs::write(sub.join("two.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(root.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();
        assert_eq!(runtime.browse_cursor, 0); // cursor is on "Album"

        runtime.handle(Command::BrowseSelect).await.unwrap();

        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.playlist_count(), 2);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_parent_goes_up_one_level() {
        let mut runtime = test_runtime("browse_parent").await;
        let root = temp_dir("browse_parent_root");
        let sub = root.join("Album");
        std::fs::create_dir_all(&sub).unwrap();
        runtime.last_browse_dir = Some(sub.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();
        assert_eq!(runtime.browse_dir, Some(sub.clone()));

        runtime.handle(Command::BrowseParent).await.unwrap();

        assert_eq!(runtime.browse_dir, Some(root.clone()));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_cancel_closes_without_adding_anything() {
        let mut runtime = test_runtime("browse_cancel").await;
        let dir = temp_dir("browse_cancel_target");
        std::fs::write(dir.join("track.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();

        runtime.handle(Command::BrowseCancel).await.unwrap();

        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.playlist_count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn browse_up_and_down_clamp_cursor_to_entry_bounds() {
        let mut runtime = test_runtime("browse_clamp").await;
        let dir = temp_dir("browse_clamp_target");
        std::fs::write(dir.join("a.mp3"), b"").unwrap();
        std::fs::write(dir.join("b.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();

        runtime.handle(Command::BrowseUp).await.unwrap();
        assert_eq!(runtime.browse_cursor, 0, "cannot go above the first entry");

        runtime.handle(Command::BrowseDown).await.unwrap();
        runtime.handle(Command::BrowseDown).await.unwrap();
        assert_eq!(runtime.browse_cursor, 1, "cannot go past the last entry");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }
}
