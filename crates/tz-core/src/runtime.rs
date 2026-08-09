//! Application runtime: wires store, player, levels, state, and control commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tz_control::{Command, ControlError, TransportSnapshot};
use tz_db::{DraftRow, MoveDirection, PlaylistRow, PlaylistStore};
use tz_playback::{BackendKind, BackendStatus};

use crate::levels::LevelService;
use crate::metadata::{read_track_meta, refresh_playlist_metadata};
use crate::paths::AppPaths;
use crate::player::{PlayerError, PlayerService, RepeatMode};
use crate::state::{load_state_with_notice, save_state, AppState};

const STATUS_TTL: Duration = Duration::from_secs(4);
const VLC_START_ATTEMPTS: usize = 3;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Files,
    Playlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorOverlay {
    None,
    Help,
    SaveName,
    Load,
    Rename,
    DeleteConfirm,
    PartialScanConfirm,
    DiscardConfirm,
}

#[derive(Debug)]
struct ScanResult {
    paths: Vec<PathBuf>,
    warnings: Vec<String>,
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
    /// "normal" | "find" | "browse" | "editor" | "help"
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
    pub last_browse_dir: Option<PathBuf>,
    pub editor_session: Option<String>,
    pub editor_focus: EditorFocus,
    pub editor_playlist_cursor: usize,
    pub editor_playlist_scroll: usize,
    pub editor_saved_id: Option<i64>,
    pub editor_saved_name: Option<String>,
    pub editor_load_cursor: usize,
    pub editor_overlay: EditorOverlay,
    pub editor_pending_name: String,
    pub editor_save_as: bool,
    pub editor_pending_paths: Option<Vec<PathBuf>>,
    pub editor_pending_insert: Option<usize>,
    editor_scan_job: Option<tokio::task::JoinHandle<(String, usize, ScanResult)>>,
    metadata_refresh_job: Option<tokio::task::JoinHandle<Result<usize, String>>>,
    pub editor_scan_generation: u64,
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
    // A crash or forced terminal close must not leave old staged editor rows
    // visible to a later session.
    store
        .cleanup_editor_drafts()
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
    if matches!(backend, BackendKind::Vlc) {
        let mut failures = Vec::new();
        let mut started = false;
        for attempt in 1..=VLC_START_ATTEMPTS {
            match player.start().await {
                Ok(()) => {
                    started = true;
                    break;
                }
                Err(error) => {
                    failures.push(format!("attempt {attempt}: {error}"));
                    tracing::warn!(attempt, error = %error, "VLC backend failed to start");
                    if attempt < VLC_START_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(150 * attempt as u64)).await;
                        // Re-run discovery as well as dynamic loading on every attempt.
                        player = PlayerService::new(
                            PlaylistStore::new(paths.db_file.clone()),
                            BackendKind::Vlc,
                        );
                    }
                }
            }
        }
        if !started {
            let details = failures.join("; ");
            tracing::error!(%details, "all VLC startup attempts failed; falling back to fake");
            player =
                PlayerService::new(PlaylistStore::new(paths.db_file.clone()), BackendKind::Fake);
            player
                .start()
                .await
                .map_err(|e| RuntimeError::Playback(e.to_string()))?;
            backend_fallback_notice = Some(format!(
                "VLC failed after {VLC_START_ATTEMPTS} attempts ({details}); using fake backend"
            ));
        }
    } else {
        player
            .start()
            .await
            .map_err(|e| RuntimeError::Playback(e.to_string()))?;
    }

    let vol = (app_state.volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    let _ = player.set_volume(vol).await;
    let _ = player.set_speed(app_state.speed).await;
    player.set_shuffle(app_state.shuffle).await;
    player
        .set_repeat(RepeatMode::parse(&app_state.repeat_mode))
        .await;

    let mut cursor_index = 0usize;
    let mut restored_item_id = None;
    if let Some(item_id) = app_state.current_item_id {
        if let Ok(Some(idx)) = store.get_item_index(playlist_id, item_id) {
            cursor_index = idx.saturating_sub(1);
            if player
                .restore_item_context(playlist_id, item_id)
                .await
                .is_ok()
            {
                restored_item_id = Some(item_id);
            }
        }
    }

    let metadata_refresh_job = if let Some(item_id) = restored_item_id {
        let db_file = paths.db_file.clone();
        Some(tokio::task::spawn_blocking(move || {
            let store = PlaylistStore::new(db_file);
            let Some(row) = store
                .get_item_row(playlist_id, item_id)
                .map_err(|error| error.to_string())?
            else {
                return Ok(0);
            };
            if row.meta_valid == Some(true) {
                return Ok(0);
            }
            let metadata = read_track_meta(&row.path);
            store
                .upsert_track_meta(row.track_id, &metadata)
                .map_err(|error| error.to_string())?;
            Ok(1)
        }))
    } else {
        None
    };

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
        editor_session: None,
        editor_focus: EditorFocus::Files,
        editor_playlist_cursor: 0,
        editor_playlist_scroll: 0,
        editor_saved_id: None,
        editor_saved_name: None,
        editor_load_cursor: 0,
        editor_overlay: EditorOverlay::None,
        editor_pending_name: String::new(),
        editor_save_as: false,
        editor_pending_paths: None,
        editor_pending_insert: None,
        editor_scan_job: None,
        metadata_refresh_job,
        editor_scan_generation: 0,
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
    if restored_item_id.is_none() {
        runtime.app_state.current_item_id = None;
    }
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
        self.poll_editor_scan().await;
        self.poll_metadata_refresh().await;
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

    async fn poll_metadata_refresh(&mut self) {
        let Some(job) = self.metadata_refresh_job.as_ref() else {
            return;
        };
        if !job.is_finished() {
            return;
        }
        let job = self
            .metadata_refresh_job
            .take()
            .expect("metadata refresh job present");
        match job.await {
            Ok(Ok(updated)) => {
                tracing::info!(updated, "startup metadata refresh completed");
                let selected_item = self.player.snapshot().await.item_id;
                if let Some(item_id) = selected_item {
                    if let Err(error) = self
                        .player
                        .restore_item_context(self.playlist_id, item_id)
                        .await
                    {
                        tracing::warn!(%error, "could not refresh selected track context");
                    }
                }
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "startup metadata refresh failed");
                self.set_warning(format!("Metadata refresh failed: {error}"));
            }
            Err(error) => {
                tracing::warn!(%error, "startup metadata task failed");
                self.set_warning(format!("Metadata task failed: {error}"));
            }
        }
    }

    async fn poll_editor_scan(&mut self) {
        let Some(job) = self.editor_scan_job.as_ref() else {
            return;
        };
        if !job.is_finished() {
            return;
        }
        let job = self.editor_scan_job.take().expect("scan job present");
        match job.await {
            Ok((session, insert_at, result))
                if self.editor_session.as_deref() == Some(&session) =>
            {
                if result.warnings.is_empty() {
                    if let Err(e) = self.stage_scan_paths(&session, insert_at, &result.paths) {
                        self.set_warning(format!("Could not stage scan: {e}"));
                    }
                } else if result.paths.is_empty() {
                    self.set_warning(result.warnings.join("; "));
                } else {
                    self.editor_pending_paths = Some(result.paths);
                    self.editor_pending_insert = Some(insert_at);
                    self.editor_overlay = EditorOverlay::PartialScanConfirm;
                    self.set_warning(format!(
                        "Scan completed with warnings: {} — add partial result? (y/n)",
                        result.warnings.join("; ")
                    ));
                }
            }
            Ok(_) => {}
            Err(e) => self.set_warning(format!("Folder scan failed: {e}")),
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
            Command::EditorOpen => {
                self.open_editor()?;
            }
            Command::EditorTab => {
                self.editor_focus = match self.editor_focus {
                    EditorFocus::Files => EditorFocus::Playlist,
                    EditorFocus::Playlist => EditorFocus::Files,
                };
            }
            Command::EditorUp => self.editor_move_cursor(-1),
            Command::EditorDown => self.editor_move_cursor(1),
            Command::EditorPageUp => self.editor_move_cursor(-10),
            Command::EditorPageDown => self.editor_move_cursor(10),
            Command::EditorHome => self.editor_set_cursor(0),
            Command::EditorEnd => {
                let n = if self.editor_focus == EditorFocus::Files {
                    self.browse_entries.len()
                } else {
                    self.editor_draft_count().unwrap_or(0)
                };
                self.editor_set_cursor(n.saturating_sub(1));
            }
            Command::EditorParent => self.editor_parent(),
            Command::EditorDrives => {
                self.browse_dir = None;
                self.browse_entries = drive_list();
                self.browse_cursor = 0;
            }
            Command::EditorEnter => {
                if self.editor_focus == EditorFocus::Files {
                    if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                        if entry.is_dir {
                            self.set_editor_dir(entry.path);
                        } else {
                            self.editor_add_highlighted(false).await?;
                        }
                    }
                }
            }
            Command::EditorAppend => self.editor_add_highlighted(false).await?,
            Command::EditorInsert => self.editor_add_highlighted(true).await?,
            Command::EditorRemove => self.editor_remove().map_err(ControlError::Message)?,
            Command::EditorClear => self.editor_clear().map_err(ControlError::Message)?,
            Command::EditorMoveUp => self.editor_reorder(true).map_err(ControlError::Message)?,
            Command::EditorMoveDown => self.editor_reorder(false).map_err(ControlError::Message)?,
            Command::EditorApply => self.editor_apply().await?,
            Command::EditorCancel => self.editor_cancel(),
            Command::EditorSave => self.editor_save(false).map_err(ControlError::Message)?,
            Command::EditorSaveAs => self.editor_save(true).map_err(ControlError::Message)?,
            Command::EditorLoad => self.editor_load().map_err(ControlError::Message)?,
            Command::EditorRename => self.editor_rename().map_err(ControlError::Message)?,
            Command::EditorDelete => self.editor_delete().map_err(ControlError::Message)?,
            Command::EditorConfirm { yes } => self.editor_confirm(yes).await?,
            Command::RequestAddFolder => {
                let dir = self.last_browse_dir.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                self.browse_entries = list_dir(&dir);
                self.browse_dir = Some(dir.clone());
                self.last_browse_dir = Some(dir);
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
                        self.add_paths_internal(&[entry.path])?;
                        self.last_browse_dir = self.browse_dir.clone();
                        self.input_mode = "normal".into();
                    }
                }
            }
            Command::BrowseSelect => {
                if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                    self.add_paths_internal(&[entry.path])?;
                    if let Some(dir) = self.browse_dir.clone() {
                        self.last_browse_dir = Some(dir);
                    }
                    self.input_mode = "normal".into();
                }
            }
            Command::BrowseParent => {
                if let Some(dir) = self.browse_dir.clone() {
                    match dir.parent() {
                        Some(parent) => {
                            if std::fs::read_dir(parent).is_err() {
                                self.set_warning(format!("Can't open '{}'", parent.display()));
                            } else {
                                let parent = parent.to_path_buf();
                                self.browse_entries = list_dir(&parent);
                                self.browse_dir = Some(parent.clone());
                                self.last_browse_dir = Some(parent);
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
                    }
                }
            }
            Command::BrowseCancel => {
                if let Some(dir) = self.browse_dir.clone() {
                    self.last_browse_dir = Some(dir);
                }
                self.input_mode = "normal".into();
                self.browse_entries.clear();
                self.browse_cursor = 0;
                self.set_status("Cancelled");
            }
            Command::RemoveSelected => {
                if let Err(e) = self.player.stop_and_clear_context().await {
                    self.set_warning(format!("Could not stop playback; removal cancelled: {e}"));
                    return Ok(());
                }
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
                    if let Err(e) = self.player.stop_and_clear_context().await {
                        self.set_warning(format!("Could not stop playback; clear cancelled: {e}"));
                        self.confirm_clear = false;
                        return Ok(());
                    }
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

    pub fn editor_active(&self) -> bool {
        self.editor_session.is_some()
    }

    pub fn editor_draft_count(&self) -> Result<usize, String> {
        let Some(session) = self.editor_session.as_deref() else {
            return Ok(0);
        };
        self.store.draft_count(session).map_err(|e| e.to_string())
    }

    pub fn editor_fetch_rows(&self, offset: usize, limit: usize) -> Result<Vec<DraftRow>, String> {
        let Some(session) = self.editor_session.as_deref() else {
            return Ok(Vec::new());
        };
        self.store
            .fetch_draft_window(session, offset, limit)
            .map_err(|e| e.to_string())
    }

    fn open_editor(&mut self) -> Result<(), ControlError> {
        let session = format!(
            "editor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        self.store
            .draft_from_playlist(&session, self.playlist_id)
            .map_err(|e| ControlError::Message(e.to_string()))?;
        self.editor_session = Some(session);
        self.editor_focus = EditorFocus::Files;
        self.editor_playlist_cursor = 0;
        self.editor_playlist_scroll = 0;
        self.editor_saved_id = Some(self.playlist_id);
        self.editor_saved_name = self.store.playlist_name(self.playlist_id).ok().flatten();
        self.editor_overlay = EditorOverlay::None;
        self.editor_pending_name.clear();
        self.editor_pending_paths = None;
        self.editor_pending_insert = None;
        self.input_mode = "editor".into();
        let dir = self
            .last_browse_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.set_editor_dir(dir);
        self.set_status("Playlist editor — Tab switches panes, F10 applies, Esc cancels");
        Ok(())
    }

    fn set_editor_dir(&mut self, dir: PathBuf) {
        self.browse_dir = Some(dir.clone());
        self.last_browse_dir = Some(dir.clone());
        match list_dir_checked(&dir) {
            Ok(entries) => self.browse_entries = entries,
            Err(e) => {
                self.browse_entries.clear();
                self.set_warning(format!("Cannot read {}: {e}", dir.display()));
            }
        }
        self.browse_cursor = 0;
    }

    fn editor_move_cursor(&mut self, delta: isize) {
        let len = if self.editor_focus == EditorFocus::Files {
            self.browse_entries.len()
        } else {
            self.editor_draft_count().unwrap_or(0)
        };
        let cur = if self.editor_focus == EditorFocus::Files {
            self.browse_cursor
        } else {
            self.editor_playlist_cursor
        };
        let next = if delta.is_negative() {
            cur.saturating_sub(delta.unsigned_abs())
        } else {
            cur.saturating_add(delta as usize)
                .min(len.saturating_sub(1))
        };
        self.editor_set_cursor(next);
    }

    fn editor_set_cursor(&mut self, index: usize) {
        if self.editor_focus == EditorFocus::Files {
            self.browse_cursor = index.min(self.browse_entries.len().saturating_sub(1));
        } else {
            self.editor_playlist_cursor =
                index.min(self.editor_draft_count().unwrap_or(0).saturating_sub(1));
        }
    }

    fn editor_parent(&mut self) {
        let Some(dir) = self.browse_dir.clone() else {
            return;
        };
        if let Some(parent) = dir.parent().map(Path::to_path_buf) {
            self.set_editor_dir(parent);
        } else {
            self.browse_dir = None;
            self.browse_entries = drive_list();
            self.browse_cursor = 0;
        }
    }

    async fn editor_add_highlighted(&mut self, insert: bool) -> Result<(), ControlError> {
        if self.editor_focus != EditorFocus::Files {
            return Ok(());
        }
        let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() else {
            return Ok(());
        };
        let draft_count = self.editor_draft_count().unwrap_or(0);
        let insert_at = if insert {
            self.editor_playlist_cursor.min(draft_count)
        } else {
            self.editor_playlist_cursor
                .saturating_add(1)
                .min(draft_count)
        };
        if entry.is_dir {
            self.editor_scan_generation = self.editor_scan_generation.wrapping_add(1);
            let session = self
                .editor_session
                .clone()
                .ok_or_else(|| ControlError::Message("editor is not open".into()))?;
            let generation = self.editor_scan_generation;
            self.editor_scan_job = Some(tokio::task::spawn_blocking(move || {
                let result = collect_media_files_recursive_safe(&entry.path);
                (session, insert_at, result)
            }));
            self.set_status(format!("Scanning {}…", entry.name));
            let _ = generation;
        } else if let Some(session) = self.editor_session.clone() {
            self.stage_scan_paths(&session, insert_at, std::slice::from_ref(&entry.path))
                .map_err(ControlError::Message)?;
        }
        Ok(())
    }

    fn stage_scan_paths(
        &mut self,
        session: &str,
        insert_at: usize,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        if insert_at >= self.editor_draft_count().map_err(|e| e.to_string())? {
            self.store
                .append_draft_paths(session, paths)
                .map_err(|e| e.to_string())?;
        } else {
            self.store
                .insert_draft_paths(session, insert_at, paths)
                .map_err(|e| e.to_string())?;
        }
        self.editor_playlist_cursor = insert_at.min(self.editor_draft_count()?.saturating_sub(1));
        self.set_status(format!(
            "Staged {} item{}",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ));
        Ok(())
    }

    fn editor_remove(&mut self) -> Result<(), String> {
        let Some(session) = self.editor_session.as_deref() else {
            return Ok(());
        };
        if self.editor_focus != EditorFocus::Playlist {
            return Ok(());
        }
        if self
            .store
            .remove_draft_at(session, self.editor_playlist_cursor)
            .map_err(|e| e.to_string())?
        {
            self.editor_playlist_cursor = self.editor_playlist_cursor.saturating_sub(1);
            self.set_status("Removed staged item");
        }
        Ok(())
    }

    fn editor_clear(&mut self) -> Result<(), String> {
        let Some(session) = self.editor_session.as_deref() else {
            return Ok(());
        };
        self.store.clear_draft(session).map_err(|e| e.to_string())?;
        self.editor_playlist_cursor = 0;
        self.set_status("Staged playlist cleared (Apply to commit, Esc to undo)");
        Ok(())
    }

    fn editor_reorder(&mut self, up: bool) -> Result<(), String> {
        let Some(session) = self.editor_session.as_deref() else {
            return Ok(());
        };
        if self.editor_focus != EditorFocus::Playlist {
            return Ok(());
        }
        if self
            .store
            .move_draft_at(session, self.editor_playlist_cursor, up)
            .map_err(|e| e.to_string())?
        {
            if up {
                self.editor_playlist_cursor = self.editor_playlist_cursor.saturating_sub(1);
            } else {
                self.editor_playlist_cursor = self
                    .editor_playlist_cursor
                    .saturating_add(1)
                    .min(self.editor_draft_count()?.saturating_sub(1));
            }
        }
        Ok(())
    }

    fn editor_load(&mut self) -> Result<(), String> {
        self.editor_overlay = EditorOverlay::Load;
        self.editor_load_cursor = 0;
        self.set_status("Load playlist: Up/Down choose, Enter loads, Esc cancels");
        Ok(())
    }

    fn editor_save(&mut self, save_as: bool) -> Result<(), String> {
        self.editor_save_as = save_as;
        if self.editor_saved_id == Some(self.playlist_id) && !save_as {
            self.editor_pending_name = self.editor_saved_name.clone().unwrap_or_default();
        }
        self.editor_overlay = EditorOverlay::SaveName;
        self.input_buffer = self.editor_pending_name.clone();
        self.set_status("Enter playlist name, then press Enter to save");
        Ok(())
    }

    pub fn editor_playlist_summaries(&self) -> Result<Vec<tz_db::PlaylistSummary>, String> {
        self.store
            .list_playlists()
            .map(|lists| {
                lists
                    .into_iter()
                    .filter(|summary| summary.id != self.playlist_id)
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    pub fn editor_commit_name(&mut self, name: String, save_as: bool) -> Result<(), String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("Playlist name cannot be empty".into());
        }
        let target = if save_as { None } else { self.editor_saved_id };
        if target == Some(self.playlist_id) {
            return Err("The active Default playlist can only be changed with Apply".into());
        }
        let session = self.editor_session.as_deref().ok_or("editor is not open")?;
        let id = self
            .store
            .save_playlist_from_draft(session, &name, target)
            .map_err(|e| e.to_string())?;
        self.editor_saved_id = Some(id);
        self.editor_saved_name = Some(name.clone());
        self.editor_overlay = EditorOverlay::None;
        self.input_buffer.clear();
        self.set_status(format!("Saved playlist '{name}'"));
        Ok(())
    }

    pub fn editor_commit_rename(&mut self, name: String) -> Result<(), String> {
        let id = self.editor_saved_id.ok_or("no saved playlist selected")?;
        if id == self.playlist_id {
            return Err("The active Default playlist cannot be renamed".into());
        }
        let name = name.trim();
        if name.is_empty() {
            return Err("Playlist name cannot be empty".into());
        }
        self.store
            .rename_playlist(id, name)
            .map_err(|e| e.to_string())?;
        self.editor_saved_name = Some(name.to_string());
        self.editor_overlay = EditorOverlay::None;
        self.input_buffer.clear();
        self.set_status("Playlist renamed");
        Ok(())
    }

    pub fn editor_load_selected(&mut self) -> Result<(), String> {
        let lists = self.editor_playlist_summaries()?;
        let Some(summary) = lists.get(self.editor_load_cursor) else {
            return Ok(());
        };
        if summary.id == self.playlist_id {
            return Err("The active playlist is already loaded".into());
        }
        let session = self.editor_session.as_deref().ok_or("editor is not open")?;
        self.store
            .draft_from_playlist(session, summary.id)
            .map_err(|e| e.to_string())?;
        self.editor_saved_id = Some(summary.id);
        self.editor_saved_name = Some(summary.name.clone());
        self.editor_playlist_cursor = 0;
        self.editor_overlay = EditorOverlay::None;
        self.set_status(format!(
            "Loaded playlist '{}' into the staged editor",
            summary.name
        ));
        Ok(())
    }

    pub fn editor_move_load_cursor(&mut self, delta: isize) {
        let len = self
            .editor_playlist_summaries()
            .map(|v| v.len())
            .unwrap_or(0);
        if len == 0 {
            self.editor_load_cursor = 0;
            return;
        }
        self.editor_load_cursor = if delta.is_negative() {
            self.editor_load_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.editor_load_cursor
                .saturating_add(delta as usize)
                .min(len - 1)
        };
    }

    fn editor_rename(&mut self) -> Result<(), String> {
        if self.editor_saved_id == Some(self.playlist_id) {
            return Err("The active Default playlist cannot be renamed".into());
        }
        self.editor_overlay = EditorOverlay::Rename;
        self.input_buffer = self.editor_saved_name.clone().unwrap_or_default();
        Ok(())
    }

    fn editor_delete(&mut self) -> Result<(), String> {
        if self.editor_saved_id == Some(self.playlist_id) {
            return Err("The active playlist cannot be deleted".into());
        }
        self.editor_overlay = EditorOverlay::DeleteConfirm;
        self.set_status("Delete selected saved playlist? (y/n)");
        Ok(())
    }

    async fn editor_confirm(&mut self, yes: bool) -> Result<(), ControlError> {
        match self.editor_overlay {
            EditorOverlay::PartialScanConfirm => {
                if yes {
                    if let (Some(paths), Some(index), Some(session)) = (
                        self.editor_pending_paths.take(),
                        self.editor_pending_insert.take(),
                        self.editor_session.clone(),
                    ) {
                        self.stage_scan_paths(&session, index, &paths)
                            .map_err(ControlError::Message)?;
                    }
                } else {
                    self.editor_pending_paths = None;
                    self.editor_pending_insert = None;
                    self.set_status("Partial scan discarded");
                }
                self.editor_overlay = EditorOverlay::None;
            }
            EditorOverlay::DiscardConfirm => {
                if yes {
                    self.editor_cancel();
                } else {
                    self.editor_overlay = EditorOverlay::None;
                }
            }
            EditorOverlay::DeleteConfirm => {
                if yes {
                    if let Some(id) = self.editor_saved_id {
                        self.store
                            .delete_playlist(id)
                            .map_err(|e| ControlError::Message(e.to_string()))?;
                        self.editor_saved_id = Some(self.playlist_id);
                        self.editor_saved_name =
                            self.store.playlist_name(self.playlist_id).ok().flatten();
                        self.set_status("Playlist deleted");
                    }
                }
                self.editor_overlay = EditorOverlay::None;
            }
            _ => {}
        }
        Ok(())
    }

    fn editor_cancel(&mut self) {
        if let Some(session) = self.editor_session.take() {
            let _ = self.store.clear_draft(&session);
        }
        self.editor_scan_job = None;
        self.editor_pending_paths = None;
        self.editor_pending_insert = None;
        self.editor_overlay = EditorOverlay::None;
        self.input_mode = "normal".into();
        self.set_status("Playlist edits cancelled");
    }

    async fn editor_apply(&mut self) -> Result<(), ControlError> {
        let Some(session) = self.editor_session.clone() else {
            return Ok(());
        };
        if self.editor_scan_job.is_some() {
            self.set_warning("Please wait for the folder scan to finish");
            return Ok(());
        }
        if let Err(e) = self.player.stop_and_clear_context().await {
            self.set_warning(format!("Could not stop playback; edits not applied: {e}"));
            return Ok(());
        }
        self.store
            .replace_playlist_from_draft(self.playlist_id, &session)
            .map_err(|e| ControlError::Message(e.to_string()))?;
        self.store
            .clear_draft(&session)
            .map_err(|e| ControlError::Message(e.to_string()))?;
        self.editor_session = None;
        self.editor_overlay = EditorOverlay::None;
        self.input_mode = "normal".into();
        self.cursor_index = 0;
        self.find_ids = None;
        self.find_query.clear();
        self.app_state.current_item_id = None;
        self.set_status("Playlist applied; playback stopped");
        Ok(())
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
        // A transient VLC startup failure may use fake playback for this run,
        // but it must not silently replace the user's preferred backend.
        if self.backend_fallback_notice.is_none() {
            self.app_state.playback_backend = snap.backend.as_str().into();
        }
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

fn list_dir_checked(dir: &Path) -> Result<Vec<FsEntry>, String> {
    std::fs::read_dir(dir).map_err(|e| e.to_string()).map(|rd| {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for ent in rd.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().into_owned();
            if ent.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
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
        let sort = |a: &FsEntry, b: &FsEntry| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then(a.name.cmp(&b.name))
        };
        dirs.sort_by(sort);
        files.sort_by(sort);
        dirs.extend(files);
        dirs
    })
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
    out.extend(collect_media_files_recursive_safe(dir).paths);
}

fn collect_media_files_recursive_safe(root: &Path) -> ScanResult {
    let mut stack = vec![root.to_path_buf()];
    let mut visited = HashSet::new();
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    while let Some(dir) = stack.pop() {
        let key = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !visited.insert(key) {
            continue;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name().to_string_lossy().to_ascii_lowercase());
        for ent in entries.into_iter().rev() {
            let path = ent.path();
            let ft = match ent.file_type() {
                Ok(ft) => ft,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && is_media_extension(&path) {
                paths.push(path);
            }
        }
    }
    paths.sort_by(|a, b| {
        a.to_string_lossy()
            .to_ascii_lowercase()
            .cmp(&b.to_string_lossy().to_ascii_lowercase())
    });
    ScanResult { paths, warnings }
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
    async fn startup_restores_saved_track_context_without_playing() {
        let dir = temp_dir("restore_saved_track");
        let paths = AppPaths {
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            log_dir: dir.join("logs"),
            state_file: dir.join("state.json"),
            db_file: dir.join("db.sqlite3"),
        };
        let store = PlaylistStore::new(&paths.db_file);
        store.initialize().unwrap();
        let playlist_id = store.ensure_playlist("Default").unwrap();
        let track = dir.join("remembered.mp3");
        std::fs::write(&track, b"").unwrap();
        store
            .add_tracks(playlist_id, std::slice::from_ref(&track))
            .unwrap();
        let item_id = store.list_item_ids(playlist_id).unwrap()[0];
        let state = AppState {
            playlist_id: Some(playlist_id),
            current_item_id: Some(item_id),
            playback_backend: "fake".into(),
            ..Default::default()
        };
        save_state(&paths.state_file, &state).unwrap();

        let mut runtime = open_runtime(paths, Some(BackendKind::Fake)).await.unwrap();
        let restored = runtime.player.snapshot().await;
        assert_eq!(restored.status, BackendStatus::Idle);
        assert_eq!(restored.item_id, Some(item_id));
        assert_eq!(
            restored.track_path.as_deref(),
            Some(track.to_string_lossy().as_ref())
        );

        while runtime.metadata_refresh_job.is_some() {
            runtime.tick().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        runtime.player.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn editor_cancel_leaves_working_playlist_unchanged() {
        let mut runtime = test_runtime("editor_cancel").await;
        let dir = temp_dir("editor_cancel_files");
        std::fs::write(dir.join("song.mp3"), b"").unwrap();
        runtime
            .store
            .add_tracks(runtime.playlist_id, &[dir.join("old.mp3")])
            .unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::EditorOpen).await.unwrap();
        assert_eq!(runtime.editor_draft_count().unwrap(), 1);
        runtime.editor_focus = EditorFocus::Files;
        runtime.browse_cursor = 0;
        runtime.handle(Command::EditorAppend).await.unwrap();
        runtime.handle(Command::EditorCancel).await.unwrap();
        assert_eq!(runtime.store.count(runtime.playlist_id).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn editor_protects_active_default_from_saved_playlist_mutations() {
        let mut runtime = test_runtime("editor_protected_default").await;
        runtime.handle(Command::EditorOpen).await.unwrap();
        assert!(runtime.editor_commit_name("Default".into(), false).is_err());
        assert!(runtime.editor_commit_rename("Renamed".into()).is_err());
        assert!(runtime.editor_delete().is_err());
        runtime.editor_cancel();
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn editor_apply_stops_and_replaces_working_playlist() {
        let mut runtime = test_runtime("editor_apply").await;
        let dir = temp_dir("editor_apply_files");
        let song = dir.join("song.mp3");
        std::fs::write(&song, b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::EditorOpen).await.unwrap();
        runtime.editor_focus = EditorFocus::Files;
        runtime.browse_cursor = 0;
        runtime.handle(Command::EditorAppend).await.unwrap();
        assert_eq!(runtime.editor_draft_count().unwrap(), 1);
        runtime.handle(Command::EditorApply).await.unwrap();
        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.store.count(runtime.playlist_id).unwrap(), 1);
        assert!(runtime.player.snapshot().await.item_id.is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
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
        assert_eq!(
            runtime.input_mode, "browse",
            "descending should not close the modal"
        );

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
    async fn browse_parent_and_cancel_remember_directory_for_next_open() {
        // Regression test: `last_browse_dir` must update on every navigation
        // that leaves `browse_dir` in a new place, not just on descend. Here
        // the session never descends — it opens at `sub`, ascends to `root`
        // via BrowseParent, then cancels — so if BrowseParent/BrowseCancel
        // didn't update `last_browse_dir`, the next RequestAddFolder would
        // wrongly reopen at `sub`.
        let mut runtime = test_runtime("browse_remember").await;
        let root = temp_dir("browse_remember_root");
        let sub = root.join("Album");
        std::fs::create_dir_all(&sub).unwrap();
        runtime.last_browse_dir = Some(sub.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();

        runtime.handle(Command::BrowseParent).await.unwrap();
        assert_eq!(runtime.browse_dir, Some(root.clone()));

        runtime.handle(Command::BrowseCancel).await.unwrap();
        runtime.handle(Command::RequestAddFolder).await.unwrap();

        assert_eq!(
            runtime.browse_dir,
            Some(root.clone()),
            "reopening should resume at the directory last shown, not fall back to where the session started"
        );

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
