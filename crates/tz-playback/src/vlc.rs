//! VLC/libVLC playback backend (listen path).
//!
//! Loads `libvlc` dynamically from a normal VLC install and drives the player
//! on a dedicated worker thread (same model as the Python backend).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::backend::{EventHandler, PlaybackBackend, PlaybackError};
use crate::events::{BackendEvent, MediaChanged, PositionUpdated, StateChanged};
use crate::status::BackendStatus;
use crate::vlc_engine::{EngineCmd, EngineEventKind, VlcWorker};

/// Result of probing the local machine for VLC/libVLC.
#[derive(Debug, Clone)]
pub struct VlcDiscovery {
    pub vlc_executable: Option<PathBuf>,
    pub libvlc_dir: Option<PathBuf>,
    pub notes: Vec<String>,
}

impl VlcDiscovery {
    pub fn is_usable(&self) -> bool {
        self.libvlc_dir.is_some()
    }
}

/// Probe common install locations for VLC on this platform.
pub fn discover_vlc() -> VlcDiscovery {
    let mut notes = Vec::new();
    let vlc_executable = which("vlc").or_else(|| which("vlc.exe"));
    #[cfg(windows)]
    let mut vlc_executable = vlc_executable;
    let mut libvlc_dir = None;

    #[cfg(windows)]
    {
        let candidates = [
            r"C:\Program Files\VideoLAN\VLC",
            r"C:\Program Files (x86)\VideoLAN\VLC",
        ];
        for dir in candidates {
            let dir = PathBuf::from(dir);
            let exe = dir.join("vlc.exe");
            let dll = dir.join("libvlc.dll");
            if exe.is_file() && vlc_executable.is_none() {
                vlc_executable = Some(exe);
            }
            if dll.is_file() {
                libvlc_dir = Some(dir);
                break;
            }
        }
        if libvlc_dir.is_none() {
            notes.push("libvlc.dll not found under Program Files\\VideoLAN\\VLC".into());
        }
    }

    #[cfg(not(windows))]
    {
        for dir in [
            "/usr/lib",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/local/lib",
            "/usr/lib64",
        ] {
            let p = PathBuf::from(dir);
            if p.join("libvlc.so").is_file()
                || p.join("libvlc.so.5").is_file()
                || p.join("libvlc.dylib").is_file()
            {
                libvlc_dir = Some(p);
                break;
            }
        }
        // macOS app bundle
        let mac = PathBuf::from("/Applications/VLC.app/Contents/MacOS/lib");
        if mac.join("libvlc.dylib").is_file() {
            libvlc_dir = Some(mac);
        }
        if libvlc_dir.is_none() {
            notes.push("libvlc shared library not found in common system paths".into());
        }
    }

    if vlc_executable.is_none() {
        notes.push("vlc executable not on PATH".into());
    }

    VlcDiscovery {
        vlc_executable,
        libvlc_dir,
        notes,
    }
}

/// Configure VLC's plugin lookup before the application starts any threads.
///
/// VLC's Windows distribution keeps its plugins beside `libvlc.dll` and uses
/// `VLC_PLUGIN_PATH` to find them when libVLC is embedded. Call this once from
/// synchronous process startup, before constructing a Tokio runtime. Unix VLC
/// packages provide their own compiled-in plugin paths, so this function is a
/// deliberate no-op on Linux and macOS.
pub fn configure_vlc_environment() {
    #[cfg(windows)]
    {
        if std::env::var_os("VLC_PLUGIN_PATH").is_some() {
            return;
        }
        let Some(lib_dir) = discover_vlc().libvlc_dir else {
            return;
        };
        let plugin_dir = lib_dir.join("plugins");
        if plugin_dir.is_dir() {
            // SAFETY: the binary calls this during single-threaded startup,
            // before the Tokio runtime or VLC worker is constructed.
            std::env::set_var("VLC_PLUGIN_PATH", plugin_dir);
        }
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let p = dir.join(name);
            p.is_file().then_some(p)
        })
    })
}

struct VlcInner {
    started: bool,
    status: BackendStatus,
    position_ms: u64,
    duration_ms: u64,
    volume: u8,
    speed: f64,
    discovery: VlcDiscovery,
    worker: Option<VlcWorker>,
}

/// VLC playback backend using dynamic libVLC + worker thread.
pub struct VlcPlaybackBackend {
    inner: Mutex<VlcInner>,
    handler: Option<EventHandler>,
}

impl VlcPlaybackBackend {
    pub fn new() -> Self {
        let discovery = discover_vlc();
        Self {
            inner: Mutex::new(VlcInner {
                started: false,
                status: BackendStatus::Idle,
                position_ms: 0,
                duration_ms: 0,
                volume: 100,
                speed: 1.0,
                discovery,
                worker: None,
            }),
            handler: None,
        }
    }

    pub fn discovery(&self) -> VlcDiscovery {
        discover_vlc()
    }

    async fn emit(&self, event: BackendEvent) {
        if let Some(handler) = &self.handler {
            handler(event).await;
        }
    }

    fn drain_events(inner: &mut VlcInner) -> Vec<BackendEvent> {
        let mut out = Vec::new();
        let Some(worker) = inner.worker.as_ref() else {
            return out;
        };
        while let Some(ev) = worker.try_recv_event() {
            match ev.kind {
                EngineEventKind::State(status) => {
                    inner.status = status;
                    out.push(BackendEvent::StateChanged(StateChanged { status }));
                }
                EngineEventKind::Media { duration_ms } => {
                    inner.duration_ms = duration_ms;
                    out.push(BackendEvent::MediaChanged(MediaChanged { duration_ms }));
                }
                EngineEventKind::Position {
                    position_ms,
                    duration_ms,
                } => {
                    inner.position_ms = position_ms;
                    if duration_ms > 0 {
                        inner.duration_ms = duration_ms;
                    }
                    out.push(BackendEvent::PositionUpdated(PositionUpdated {
                        position_ms,
                        duration_ms: inner.duration_ms,
                    }));
                }
                EngineEventKind::Error(message) => {
                    inner.status = BackendStatus::Error;
                    out.push(BackendEvent::Error(crate::events::BackendError { message }));
                }
            }
        }
        out
    }

    async fn submit<T, F>(&self, build: F) -> Result<T, PlaybackError>
    where
        T: Send + 'static,
        F: FnOnce(mpsc::Sender<Result<T, String>>) -> EngineCmd + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let cmd = build(tx);
        {
            let inner = self.inner.lock().await;
            if !inner.started {
                return Err(PlaybackError::NotStarted);
            }
            let worker = inner
                .worker
                .as_ref()
                .ok_or_else(|| PlaybackError::VlcUnavailable("VLC worker not running".into()))?;
            worker
                .cmd_tx()
                .send(cmd)
                .map_err(|_| PlaybackError::VlcUnavailable("VLC worker disconnected".into()))?;
        }
        // Wait for worker reply without blocking the async runtime hard.
        let result = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(Duration::from_secs(30))
                .map_err(|_| "VLC command timeout".to_string())
                .and_then(|r| r)
        })
        .await
        .map_err(|e| PlaybackError::Message(e.to_string()))?
        .map_err(PlaybackError::Message)?;

        // Drain any events produced around the command.
        let events = {
            let mut inner = self.inner.lock().await;
            Self::drain_events(&mut inner)
        };
        for ev in events {
            self.emit(ev).await;
        }
        Ok(result)
    }

    fn unavailable_msg(discovery: &VlcDiscovery) -> String {
        let mut msg = String::from(
            "VLC/libVLC is not available.\n\
             Likely cause: VLC is not installed or libvlc could not be loaded.\n\
             Next step: install VLC from https://www.videolan.org/vlc/ and re-run `tz-player doctor`.",
        );
        if !discovery.notes.is_empty() {
            msg.push_str("\nDetails: ");
            msg.push_str(&discovery.notes.join("; "));
        }
        msg
    }
}

impl Default for VlcPlaybackBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlaybackBackend for VlcPlaybackBackend {
    fn set_event_handler(&mut self, handler: EventHandler) {
        self.handler = Some(handler);
    }

    async fn start(&mut self) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        if inner.started {
            return Ok(());
        }
        let lib_dir = inner.discovery.libvlc_dir.clone().ok_or_else(|| {
            PlaybackError::VlcUnavailable(Self::unavailable_msg(&inner.discovery))
        })?;

        let quiet = std::env::var("TZ_PLAYER_VLC_VERBOSE").ok().as_deref() != Some("1");
        let worker = VlcWorker::spawn(lib_dir, quiet).map_err(|e| {
            PlaybackError::VlcUnavailable(format!(
                "{}\nDetails: {e}",
                Self::unavailable_msg(&inner.discovery)
            ))
        })?;

        inner.worker = Some(worker);
        inner.started = true;
        inner.status = BackendStatus::Idle;
        tracing::info!("VLC/libVLC backend started");
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), PlaybackError> {
        let mut inner = self.inner.lock().await;
        if let Some(worker) = inner.worker.take() {
            worker.shutdown();
        }
        inner.started = false;
        inner.status = BackendStatus::Idle;
        Ok(())
    }

    async fn play(
        &mut self,
        _item_id: i64,
        track_path: &Path,
        start_ms: u64,
        duration_ms: Option<u64>,
    ) -> Result<(), PlaybackError> {
        let path = track_path.to_string_lossy().into_owned();
        self.submit(move |reply| EngineCmd::Play {
            path,
            start_ms,
            reply,
        })
        .await?;

        {
            let mut inner = self.inner.lock().await;
            inner.status = BackendStatus::Playing;
            inner.position_ms = start_ms;
            if let Some(d) = duration_ms {
                if d > 0 {
                    inner.duration_ms = d;
                }
            }
        }
        // Re-apply volume/speed after media change (VLC may reset).
        let (vol, speed) = {
            let inner = self.inner.lock().await;
            (inner.volume, inner.speed)
        };
        let _ = self
            .submit(move |reply| EngineCmd::SetVolume { volume: vol, reply })
            .await;
        let _ = self
            .submit(move |reply| EngineCmd::SetSpeed { speed, reply })
            .await;

        self.emit(BackendEvent::StateChanged(StateChanged {
            status: BackendStatus::Playing,
        }))
        .await;
        Ok(())
    }

    async fn toggle_pause(&mut self) -> Result<(), PlaybackError> {
        self.submit(|reply| EngineCmd::TogglePause { reply })
            .await?;
        let mut inner = self.inner.lock().await;
        Self::drain_events(&mut inner);
        inner.status = match inner.status {
            BackendStatus::Playing => BackendStatus::Paused,
            BackendStatus::Paused => BackendStatus::Playing,
            other => other,
        };
        let status = inner.status;
        drop(inner);
        self.emit(BackendEvent::StateChanged(StateChanged { status }))
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PlaybackError> {
        self.submit(|reply| EngineCmd::Stop { reply }).await?;
        {
            let mut inner = self.inner.lock().await;
            inner.status = BackendStatus::Stopped;
            inner.position_ms = 0;
        }
        self.emit(BackendEvent::StateChanged(StateChanged {
            status: BackendStatus::Stopped,
        }))
        .await;
        Ok(())
    }

    async fn seek_ms(&mut self, position_ms: u64) -> Result<(), PlaybackError> {
        self.submit(move |reply| EngineCmd::SeekMs { position_ms, reply })
            .await?;
        let mut inner = self.inner.lock().await;
        inner.position_ms = position_ms;
        let dur = inner.duration_ms;
        drop(inner);
        self.emit(BackendEvent::PositionUpdated(PositionUpdated {
            position_ms,
            duration_ms: dur,
        }))
        .await;
        Ok(())
    }

    async fn set_volume(&mut self, volume: u8) -> Result<(), PlaybackError> {
        let volume = volume.min(100);
        self.submit(move |reply| EngineCmd::SetVolume { volume, reply })
            .await?;
        self.inner.lock().await.volume = volume;
        Ok(())
    }

    async fn set_speed(&mut self, speed: f64) -> Result<(), PlaybackError> {
        let speed = speed.clamp(0.5, 4.0);
        self.submit(move |reply| EngineCmd::SetSpeed { speed, reply })
            .await?;
        self.inner.lock().await.speed = speed;
        Ok(())
    }

    async fn get_position_ms(&self) -> Result<u64, PlaybackError> {
        let events = {
            let mut inner = self.inner.lock().await;
            Self::drain_events(&mut inner)
        };
        // Cannot emit from &self easily without handler clone — drain state only.
        let _ = events;
        Ok(self.inner.lock().await.position_ms)
    }

    async fn get_duration_ms(&self) -> Result<u64, PlaybackError> {
        let mut inner = self.inner.lock().await;
        let _ = Self::drain_events(&mut inner);
        Ok(inner.duration_ms)
    }

    async fn get_state(&self) -> Result<BackendStatus, PlaybackError> {
        let mut inner = self.inner.lock().await;
        let _ = Self::drain_events(&mut inner);
        Ok(inner.status)
    }

    async fn get_transport_snapshot(&self) -> Result<(u64, u64, BackendStatus), PlaybackError> {
        // Prefer a single worker round-trip when running.
        let started = self.inner.lock().await.started;
        if !started {
            return Err(PlaybackError::NotStarted);
        }

        let (tx, rx) = mpsc::channel();
        {
            let inner = self.inner.lock().await;
            let worker = inner
                .worker
                .as_ref()
                .ok_or_else(|| PlaybackError::VlcUnavailable("VLC worker not running".into()))?;
            worker
                .cmd_tx()
                .send(EngineCmd::GetTransport { reply: tx })
                .map_err(|_| PlaybackError::VlcUnavailable("VLC worker disconnected".into()))?;
        }

        let snap = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(Duration::from_secs(5))
                .map_err(|_| "transport snapshot timeout".to_string())
                .and_then(|r| r)
        })
        .await
        .map_err(|e| PlaybackError::Message(e.to_string()))?
        .map_err(PlaybackError::Message)?;

        {
            let mut inner = self.inner.lock().await;
            inner.position_ms = snap.0;
            if snap.1 > 0 {
                inner.duration_ms = snap.1;
            }
            inner.status = snap.2;
            let _ = Self::drain_events(&mut inner);
        }
        Ok(snap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_does_not_panic() {
        let d = discover_vlc();
        let _ = d.is_usable();
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_startup_does_not_mutate_vlc_plugin_path() {
        let before = std::env::var_os("VLC_PLUGIN_PATH");
        configure_vlc_environment();
        assert_eq!(std::env::var_os("VLC_PLUGIN_PATH"), before);
    }

    #[tokio::test]
    async fn start_requires_libvlc() {
        let mut b = VlcPlaybackBackend::new();
        // On machines with VLC installed this succeeds; without it, VlcUnavailable.
        match b.start().await {
            Ok(()) => {
                b.shutdown().await.unwrap();
            }
            Err(PlaybackError::VlcUnavailable(_)) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
