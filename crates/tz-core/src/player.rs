//! Playback orchestration between UI intent and backend engines.

use std::sync::Arc;
use std::time::Instant;

use rand::seq::IndexedRandom;
use tokio::sync::Mutex;
use tz_control::TransportSnapshot;
use tz_db::{PlaylistRow, PlaylistStore};
use tz_playback::{
    BackendKind, BackendStatus, FakePlaybackBackend, LevelSample as PlaybackLevelSample,
    PlaybackBackend, PlaybackError, PlaybackLevelProvider, RodioPlaybackBackend,
    VlcPlaybackBackend,
};

use crate::{clamp_speed, SPEED_MAX, SPEED_MIN, SPEED_STEP};

/// Repeat mode (Python OFF/ONE/ALL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

impl RepeatMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::One => "one",
            Self::All => "all",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "one" | "ONE" => Self::One,
            "all" | "ALL" => Self::All,
            _ => Self::Off,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::One,
            Self::One => Self::All,
            Self::All => Self::Off,
        }
    }
}

/// Serializable transport state for UI / control snapshots.
#[derive(Debug, Clone)]
pub struct PlayerState {
    pub status: BackendStatus,
    pub playlist_id: Option<i64>,
    pub item_id: Option<i64>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: u8,
    pub speed: f64,
    pub repeat_mode: RepeatMode,
    pub shuffle: bool,
    pub track_path: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub error: Option<String>,
    pub backend: BackendKind,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: BackendStatus::Idle,
            playlist_id: None,
            item_id: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 100,
            speed: 1.0,
            repeat_mode: RepeatMode::Off,
            shuffle: false,
            track_path: None,
            title: None,
            artist: None,
            album: None,
            error: None,
            backend: BackendKind::Vlc,
        }
    }
}

enum Engine {
    Fake(FakePlaybackBackend),
    Rodio(RodioPlaybackBackend),
    Vlc(VlcPlaybackBackend),
}

impl Engine {
    fn as_backend_mut(&mut self) -> &mut dyn PlaybackBackend {
        match self {
            Self::Fake(b) => b,
            Self::Rodio(b) => b,
            Self::Vlc(b) => b,
        }
    }

    async fn level_sample(&self) -> Option<PlaybackLevelSample> {
        match self {
            Self::Rodio(backend) => backend.get_level_sample().await,
            Self::Fake(_) | Self::Vlc(_) => None,
        }
    }
}

/// Owns playlist-aware transport and backend lifecycle.
pub struct PlayerService {
    store: PlaylistStore,
    engine: Engine,
    state: Arc<Mutex<PlayerState>>,
    stop_requested: bool,
    default_duration_ms: u64,
    /// Last position actually reported by the backend, and when we observed
    /// it. Backends like libVLC only refresh their internal clock every
    /// ~250-300ms; between real readings we extrapolate from this anchor so
    /// displayed/sampled position doesn't visibly stall.
    real_position_ms: u64,
    real_observed_at: Option<Instant>,
}

impl PlayerService {
    pub fn new(store: PlaylistStore, backend: BackendKind) -> Self {
        let engine = match backend {
            BackendKind::Fake => Engine::Fake(FakePlaybackBackend::new()),
            BackendKind::Rodio => Engine::Rodio(RodioPlaybackBackend::new()),
            BackendKind::Vlc => Engine::Vlc(VlcPlaybackBackend::new()),
        };
        let state = PlayerState {
            backend,
            ..Default::default()
        };
        Self {
            store,
            engine,
            state: Arc::new(Mutex::new(state)),
            stop_requested: false,
            default_duration_ms: 180_000,
            real_position_ms: 0,
            real_observed_at: None,
        }
    }

    pub fn store(&self) -> &PlaylistStore {
        &self.store
    }

    pub async fn start(&mut self) -> Result<(), PlaybackError> {
        self.engine.as_backend_mut().start().await?;
        let volume = self.state.lock().await.volume;
        let speed = self.state.lock().await.speed;
        let _ = self.engine.as_backend_mut().set_volume(volume).await;
        let _ = self.engine.as_backend_mut().set_speed(speed).await;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), PlaybackError> {
        self.engine.as_backend_mut().shutdown().await
    }

    pub async fn snapshot(&self) -> PlayerState {
        self.state.lock().await.clone()
    }

    /// Return a backend-native live level when the selected engine exposes
    /// one. Higher-order spectrum/beat/waveform data remains in `tz-analysis`.
    pub async fn live_level_sample(&self) -> Option<PlaybackLevelSample> {
        self.engine.level_sample().await
    }

    /// Restore the selected track's display context without starting playback.
    ///
    /// Persisted state stores the last selected playlist item, but the playback
    /// backend intentionally starts idle. Hydrating the descriptive fields here
    /// lets the transport and visualizer header show that selection immediately.
    pub async fn restore_item_context(
        &mut self,
        playlist_id: i64,
        item_id: i64,
    ) -> Result<(), PlayerError> {
        let row = self
            .store
            .get_item_row(playlist_id, item_id)
            .map_err(|e| PlayerError::Db(e.to_string()))?
            .ok_or_else(|| PlayerError::Message("saved playlist item no longer exists".into()))?;
        let mut state = self.state.lock().await;
        state.playlist_id = Some(playlist_id);
        state.item_id = Some(item_id);
        state.track_path = Some(row.path.to_string_lossy().into_owned());
        state.title = row.title;
        state.artist = row.artist;
        state.album = row.album;
        if let Some(duration_ms) = row
            .duration_ms
            .and_then(|duration| u64::try_from(duration).ok())
            .filter(|duration| *duration > 0)
        {
            state.duration_ms = duration_ms;
        }
        Ok(())
    }

    pub async fn transport_snapshot(
        &self,
        cursor_index: usize,
        playlist_count: usize,
    ) -> TransportSnapshot {
        TransportSnapshot {
            cursor_index,
            playlist_count,
            ..Default::default()
        }
        // Prefer transport_snapshot_from for full fields.
    }

    /// Build a full transport snapshot from player state + analysis overlays.
    pub async fn transport_snapshot_from(&self, base: TransportSnapshot) -> TransportSnapshot {
        let s = self.state.lock().await;
        TransportSnapshot {
            status: s.status.as_str().into(),
            position_ms: s.position_ms,
            duration_ms: s.duration_ms,
            volume: s.volume,
            speed: s.speed,
            repeat_mode: s.repeat_mode.as_str().into(),
            shuffle: s.shuffle,
            item_id: s.item_id,
            track_path: s.track_path.clone(),
            title: s.title.clone(),
            artist: s.artist.clone(),
            album: s.album.clone(),
            error: s.error.clone(),
            backend: s.backend.as_str().into(),
            playlist_id: s.playlist_id,
            playlist_count: base.playlist_count,
            cursor_index: base.cursor_index,
            level_left: base.level_left,
            level_right: base.level_right,
            level_source: base.level_source,
            spectrum_bands: base.spectrum_bands,
            spectrum_source: base.spectrum_source,
            beat_strength: base.beat_strength,
            beat_is_onset: base.beat_is_onset,
            beat_bpm: base.beat_bpm,
            beat_source: base.beat_source,
            waveform_min_left: base.waveform_min_left,
            waveform_max_left: base.waveform_max_left,
            waveform_min_right: base.waveform_min_right,
            waveform_max_right: base.waveform_max_right,
            waveform_source: base.waveform_source,
            waveform_history: base.waveform_history,
            analysis_status: base.analysis_status,
            visualizer_id: base.visualizer_id,
            find_query: base.find_query,
            find_active: base.find_active,
            confirm_clear: base.confirm_clear,
            input_mode: base.input_mode,
            input_buffer: base.input_buffer,
        }
    }

    pub async fn poll_position(&mut self) {
        if let Ok((pos, dur, status)) = self.engine.as_backend_mut().get_transport_snapshot().await
        {
            let mut s = self.state.lock().await;
            if matches!(
                s.status,
                BackendStatus::Playing | BackendStatus::Paused | BackendStatus::Loading
            ) {
                if dur > 0 {
                    s.duration_ms = dur;
                }
                // Every backend reports natural completion as Stopped at the
                // known end position. Explicit Stop is excluded separately.
                if status == BackendStatus::Stopped
                    && !self.stop_requested
                    && s.item_id.is_some()
                    && s.duration_ms > 0
                    && pos >= s.duration_ms.saturating_sub(50)
                {
                    drop(s);
                    let _ = self.advance_after_end().await;
                    return;
                }

                // Only re-anchor when the backend hands us a genuinely new
                // reading; some backends (libVLC) only update every ~250-300ms,
                // so re-anchoring on every poll would freeze interpolation.
                if pos != self.real_position_ms || self.real_observed_at.is_none() {
                    self.real_position_ms = pos;
                    self.real_observed_at = Some(Instant::now());
                }

                s.position_ms = if s.status == BackendStatus::Playing {
                    let elapsed_ms = self
                        .real_observed_at
                        .map(|t| t.elapsed().as_secs_f64() * 1000.0 * s.speed)
                        .unwrap_or(0.0);
                    let cap = if s.duration_ms > 0 {
                        s.duration_ms as f64
                    } else {
                        f64::MAX
                    };
                    (self.real_position_ms as f64 + elapsed_ms)
                        .clamp(self.real_position_ms as f64, cap) as u64
                } else {
                    self.real_position_ms
                };

                if !matches!(status, BackendStatus::Idle) {
                    s.status = status;
                }
            }
        }
    }

    async fn advance_after_end(&mut self) -> Result<(), PlayerError> {
        let (playlist_id, item_id, repeat, shuffle) = {
            let s = self.state.lock().await;
            (s.playlist_id, s.item_id, s.repeat_mode, s.shuffle)
        };
        let (Some(playlist_id), Some(item_id)) = (playlist_id, item_id) else {
            return Ok(());
        };
        match repeat {
            RepeatMode::One => self.play_item(playlist_id, item_id).await,
            RepeatMode::All => {
                if let Some(next) = self
                    .resolve_next(playlist_id, item_id, shuffle, true)
                    .await?
                {
                    self.play_item(playlist_id, next).await
                } else {
                    self.finish_natural_end().await;
                    Ok(())
                }
            }
            RepeatMode::Off => {
                if let Some(next) = self
                    .resolve_next(playlist_id, item_id, shuffle, false)
                    .await?
                {
                    self.play_item(playlist_id, next).await
                } else {
                    self.finish_natural_end().await;
                    Ok(())
                }
            }
        }
    }

    async fn finish_natural_end(&mut self) {
        let mut state = self.state.lock().await;
        state.status = BackendStatus::Stopped;
        state.position_ms = state.duration_ms;
        self.real_position_ms = state.duration_ms;
        self.real_observed_at = None;
    }

    pub async fn play_item(&mut self, playlist_id: i64, item_id: i64) -> Result<(), PlayerError> {
        self.stop_requested = false;
        self.real_position_ms = 0;
        self.real_observed_at = None;
        {
            let mut s = self.state.lock().await;
            s.status = BackendStatus::Loading;
            s.playlist_id = Some(playlist_id);
            s.item_id = Some(item_id);
            s.position_ms = 0;
            s.error = None;
            s.title = None;
            s.artist = None;
            s.album = None;
            s.track_path = None;
        }

        let row = self
            .store
            .get_item_row(playlist_id, item_id)
            .map_err(|e| PlayerError::Db(e.to_string()))?
            .ok_or_else(|| {
                PlayerError::Message(
                    "Failed to start playback for selected track.\n\
                     Likely cause: Track entry is missing, moved, or no longer readable.\n\
                     Next step: Verify the file path and refresh/remove the playlist entry."
                        .into(),
                )
            })?;

        let duration_ms = row
            .duration_ms
            .map(|d| d as u64)
            .filter(|d| *d > 0)
            .unwrap_or(self.default_duration_ms);

        {
            let mut s = self.state.lock().await;
            s.track_path = Some(row.path.to_string_lossy().into_owned());
            s.title = row.title.clone();
            s.artist = row.artist.clone();
            s.album = row.album.clone();
            s.duration_ms = duration_ms;
        }

        match self
            .engine
            .as_backend_mut()
            .play(item_id, &row.path, 0, Some(duration_ms))
            .await
        {
            Ok(()) => {
                let mut s = self.state.lock().await;
                s.status = BackendStatus::Playing;
                s.position_ms = 0;
                Ok(())
            }
            Err(e) => {
                let mut s = self.state.lock().await;
                s.status = BackendStatus::Error;
                s.error = Some(e.to_string());
                Err(PlayerError::Playback(e))
            }
        }
    }

    pub async fn toggle_pause(&mut self) -> Result<(), PlayerError> {
        let status = self.state.lock().await.status;
        if !matches!(status, BackendStatus::Playing | BackendStatus::Paused) {
            return Ok(());
        }
        self.engine
            .as_backend_mut()
            .toggle_pause()
            .await
            .map_err(PlayerError::Playback)?;
        let mut s = self.state.lock().await;
        s.status = if s.status == BackendStatus::Playing {
            BackendStatus::Paused
        } else {
            BackendStatus::Playing
        };
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), PlayerError> {
        self.stop_requested = true;
        if let Err(e) = self.engine.as_backend_mut().stop().await {
            self.stop_requested = false;
            return Err(PlayerError::Playback(e));
        }
        let mut s = self.state.lock().await;
        s.status = BackendStatus::Stopped;
        s.position_ms = 0;
        self.real_position_ms = 0;
        self.real_observed_at = None;
        Ok(())
    }

    /// Stop playback and clear the active transport context. Playlist editing
    /// uses this transactional boundary so a replaced list cannot retain a
    /// stale item id or path.
    pub async fn stop_and_clear_context(&mut self) -> Result<(), PlayerError> {
        self.stop().await?;
        let mut s = self.state.lock().await;
        s.playlist_id = None;
        s.item_id = None;
        s.track_path = None;
        s.title = None;
        s.artist = None;
        s.album = None;
        s.duration_ms = 0;
        Ok(())
    }

    pub async fn seek_ms(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        self.engine
            .as_backend_mut()
            .seek_ms(position_ms)
            .await
            .map_err(PlayerError::Playback)?;
        self.state.lock().await.position_ms = position_ms;
        self.real_position_ms = position_ms;
        self.real_observed_at = Some(Instant::now());
        Ok(())
    }

    pub async fn seek_relative(&mut self, delta_ms: i64) -> Result<(), PlayerError> {
        let pos = self.state.lock().await.position_ms as i64;
        let dur = self.state.lock().await.duration_ms as i64;
        let mut next = pos.saturating_add(delta_ms).max(0) as u64;
        if dur > 0 {
            next = next.min(dur as u64);
        }
        self.seek_ms(next).await
    }

    pub async fn set_volume(&mut self, volume: u8) -> Result<(), PlayerError> {
        let v = volume.min(100);
        self.engine
            .as_backend_mut()
            .set_volume(v)
            .await
            .map_err(PlayerError::Playback)?;
        self.state.lock().await.volume = v;
        Ok(())
    }

    pub async fn volume_delta(&mut self, delta: i16) -> Result<(), PlayerError> {
        let cur = self.state.lock().await.volume as i16;
        let next = (cur + delta).clamp(0, 100) as u8;
        self.set_volume(next).await
    }

    pub async fn set_speed(&mut self, speed: f64) -> Result<(), PlayerError> {
        let s = clamp_speed(speed);
        self.engine
            .as_backend_mut()
            .set_speed(s)
            .await
            .map_err(PlayerError::Playback)?;
        self.state.lock().await.speed = s;
        Ok(())
    }

    pub async fn speed_delta(&mut self, delta: f64) -> Result<(), PlayerError> {
        let cur = self.state.lock().await.speed;
        self.set_speed(cur + delta).await
    }

    pub async fn cycle_repeat(&mut self) {
        let mut s = self.state.lock().await;
        s.repeat_mode = s.repeat_mode.cycle();
    }

    pub async fn set_repeat(&mut self, mode: RepeatMode) {
        self.state.lock().await.repeat_mode = mode;
    }

    pub async fn toggle_shuffle(&mut self) {
        let mut s = self.state.lock().await;
        s.shuffle = !s.shuffle;
    }

    pub async fn set_shuffle(&mut self, shuffle: bool) {
        self.state.lock().await.shuffle = shuffle;
    }

    pub async fn next(&mut self) -> Result<(), PlayerError> {
        let (playlist_id, item_id, shuffle) = {
            let s = self.state.lock().await;
            (s.playlist_id, s.item_id, s.shuffle)
        };
        let (Some(playlist_id), Some(item_id)) = (playlist_id, item_id) else {
            return Ok(());
        };
        if let Some(next) = self
            .resolve_next(playlist_id, item_id, shuffle, true)
            .await?
        {
            self.play_item(playlist_id, next).await?;
        }
        Ok(())
    }

    pub async fn previous(&mut self) -> Result<(), PlayerError> {
        let (playlist_id, item_id, shuffle) = {
            let s = self.state.lock().await;
            (s.playlist_id, s.item_id, s.shuffle)
        };
        let (Some(playlist_id), Some(item_id)) = (playlist_id, item_id) else {
            return Ok(());
        };
        if shuffle {
            if let Some(next) = self.resolve_next(playlist_id, item_id, true, true).await? {
                return self.play_item(playlist_id, next).await;
            }
        }
        let prev = self
            .store
            .get_prev_item_id(playlist_id, item_id, true)
            .map_err(|e| PlayerError::Db(e.to_string()))?;
        if let Some(prev) = prev {
            self.play_item(playlist_id, prev).await?;
        }
        Ok(())
    }

    async fn resolve_next(
        &self,
        playlist_id: i64,
        item_id: i64,
        shuffle: bool,
        wrap: bool,
    ) -> Result<Option<i64>, PlayerError> {
        if shuffle {
            let mut ids = self
                .store
                .list_item_ids(playlist_id)
                .map_err(|e| PlayerError::Db(e.to_string()))?;
            if ids.is_empty() {
                return Ok(None);
            }
            ids.retain(|id| *id != item_id);
            if ids.is_empty() {
                return if wrap { Ok(Some(item_id)) } else { Ok(None) };
            }
            let mut rng = rand::rng();
            Ok(ids.choose(&mut rng).copied())
        } else {
            self.store
                .get_next_item_id(playlist_id, item_id, wrap)
                .map_err(|e| PlayerError::Db(e.to_string()))
        }
    }

    pub fn apply_row_meta(state: &mut PlayerState, row: &PlaylistRow) {
        state.title = row.title.clone();
        state.artist = row.artist.clone();
        state.album = row.album.clone();
        state.track_path = Some(row.path.to_string_lossy().into_owned());
        if let Some(d) = row.duration_ms {
            if d > 0 {
                state.duration_ms = d as u64;
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("{0}")]
    Message(String),
    #[error("db: {0}")]
    Db(String),
    #[error(transparent)]
    Playback(#[from] PlaybackError),
}

impl PlayerError {
    /// True only for failures in the selected audio backend — i.e. playback is
    /// genuinely disrupted. `Db`/`Message` cover data-layer problems (e.g. a
    /// missing playlist row) where audio itself is not necessarily broken.
    pub fn is_backend_failure(&self) -> bool {
        matches!(self, PlayerError::Playback(_))
    }
}

// Keep constants referenced for UI keybinding docs.
#[allow(dead_code)]
const _SPEED_BOUNDS: (f64, f64, f64) = (SPEED_MIN, SPEED_MAX, SPEED_STEP);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> (PlaylistStore, i64, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tz_player_svc_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let store = PlaylistStore::new(dir.join("db.sqlite"));
        store.initialize().unwrap();
        let pid = store.create_playlist("Q").unwrap();
        let mut paths = Vec::new();
        for i in 0..3 {
            let p = dir.join(format!("t{i}.mp3"));
            fs::write(&p, b"").unwrap();
            paths.push(p);
        }
        store.add_tracks(pid, &paths).unwrap();
        (store, pid, dir)
    }

    async fn rodio_contract_player() -> (PlayerService, i64, Vec<i64>, std::path::PathBuf) {
        let (store, playlist_id, dir) = temp_store();
        let item_ids = store.list_item_ids(playlist_id).unwrap();
        let mut player = PlayerService::new(store, BackendKind::Rodio);
        // Natural-end playlist behavior is backend-neutral. Use the fake
        // transport under a Rodio-labelled service so this contract remains
        // deterministic and does not require a CI audio device.
        player.engine = Engine::Fake(FakePlaybackBackend::new());
        player.default_duration_ms = 15;
        player.start().await.unwrap();
        (player, playlist_id, item_ids, dir)
    }

    #[tokio::test]
    async fn play_pause_next_fake() {
        let (store, pid, dir) = temp_store();
        let ids = store.list_item_ids(pid).unwrap();
        let mut player = PlayerService::new(store, BackendKind::Fake);
        player.start().await.unwrap();
        player.play_item(pid, ids[0]).await.unwrap();
        assert_eq!(player.snapshot().await.status, BackendStatus::Playing);
        player.toggle_pause().await.unwrap();
        assert_eq!(player.snapshot().await.status, BackendStatus::Paused);
        player.next().await.unwrap();
        assert_eq!(player.snapshot().await.item_id, Some(ids[1]));
        player.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn restore_item_context_hydrates_selection_without_playing() {
        let (store, pid, dir) = temp_store();
        let ids = store.list_item_ids(pid).unwrap();
        let mut player = PlayerService::new(store, BackendKind::Fake);
        player.start().await.unwrap();

        player.restore_item_context(pid, ids[1]).await.unwrap();

        let state = player.snapshot().await;
        assert_eq!(state.status, BackendStatus::Idle);
        assert_eq!(state.playlist_id, Some(pid));
        assert_eq!(state.item_id, Some(ids[1]));
        assert!(state
            .track_path
            .as_deref()
            .is_some_and(|path| path.ends_with("t1.mp3")));
        player.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rodio_contract_natural_end_honors_repeat_modes_exactly_once() {
        let (mut player, playlist_id, ids, dir) = rodio_contract_player().await;

        player.set_repeat(RepeatMode::Off).await;
        player.play_item(playlist_id, ids[0]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        player.poll_position().await;
        assert_eq!(player.snapshot().await.item_id, Some(ids[1]));
        player.poll_position().await;
        assert_eq!(
            player.snapshot().await.item_id,
            Some(ids[1]),
            "one natural end must not skip two tracks"
        );

        player.play_item(playlist_id, ids[2]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        player.poll_position().await;
        let stopped = player.snapshot().await;
        assert_eq!(stopped.item_id, Some(ids[2]));
        assert_eq!(stopped.status, BackendStatus::Stopped);
        assert_eq!(stopped.position_ms, stopped.duration_ms);

        player.set_repeat(RepeatMode::All).await;
        player.play_item(playlist_id, ids[2]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        player.poll_position().await;
        assert_eq!(player.snapshot().await.item_id, Some(ids[0]));

        player.set_repeat(RepeatMode::One).await;
        player.play_item(playlist_id, ids[1]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        player.poll_position().await;
        let repeated = player.snapshot().await;
        assert_eq!(repeated.item_id, Some(ids[1]));
        assert_eq!(repeated.status, BackendStatus::Playing);
        assert_eq!(repeated.backend, BackendKind::Rodio);

        player.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rodio_contract_next_previous_and_shuffle_keep_valid_items() {
        let (mut player, playlist_id, ids, dir) = rodio_contract_player().await;
        player.play_item(playlist_id, ids[1]).await.unwrap();

        player.next().await.unwrap();
        assert_eq!(player.snapshot().await.item_id, Some(ids[2]));
        player.previous().await.unwrap();
        assert_eq!(player.snapshot().await.item_id, Some(ids[1]));

        player.set_shuffle(true).await;
        player.next().await.unwrap();
        let shuffled = player.snapshot().await.item_id.unwrap();
        assert_ne!(shuffled, ids[1]);
        assert!(ids.contains(&shuffled));

        player.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rodio_real_output_natural_end_advances_once_when_device_exists() {
        if std::env::var("TZ_PLAYER_RODIO_OUTPUT_TESTS").as_deref() != Ok("1") {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "tz_player_rodio_service_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let store = PlaylistStore::new(dir.join("db.sqlite"));
        store.initialize().unwrap();
        let playlist_id = store.create_playlist("Rodio output").unwrap();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tz-playback")
            .join("tests")
            .join("fixtures");
        let paths = [fixtures.join("tone.wav"), fixtures.join("tone.flac")];
        store.add_tracks(playlist_id, &paths).unwrap();
        let ids = store.list_item_ids(playlist_id).unwrap();

        let mut player = PlayerService::new(store, BackendKind::Rodio);
        match player.start().await {
            Ok(()) => {}
            Err(PlaybackError::RodioUnavailable(_)) => {
                let _ = fs::remove_dir_all(dir);
                return;
            }
            Err(error) => panic!("unexpected Rodio startup result: {error}"),
        }
        player.set_volume(0).await.unwrap();
        player.set_speed(4.0).await.unwrap();
        player.play_item(playlist_id, ids[0]).await.unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while player.snapshot().await.item_id != Some(ids[1]) && Instant::now() < deadline {
            player.poll_position().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(player.snapshot().await.item_id, Some(ids[1]));
        for _ in 0..5 {
            player.poll_position().await;
        }
        assert_eq!(
            player.snapshot().await.item_id,
            Some(ids[1]),
            "one natural end must advance exactly one playlist item"
        );

        player.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repeat_cycle() {
        assert_eq!(RepeatMode::Off.cycle(), RepeatMode::One);
        assert_eq!(RepeatMode::One.cycle(), RepeatMode::All);
        assert_eq!(RepeatMode::All.cycle(), RepeatMode::Off);
    }
}
