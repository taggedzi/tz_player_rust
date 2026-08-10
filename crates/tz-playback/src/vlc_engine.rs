//! libVLC engine owned by a single worker thread.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::status::BackendStatus;
use crate::vlc_ffi::{LibVlcApi, LibVlcState};

pub enum EngineCmd {
    Play {
        path: String,
        start_ms: u64,
        reply: Sender<Result<(), String>>,
    },
    TogglePause {
        reply: Sender<Result<(), String>>,
    },
    Stop {
        reply: Sender<Result<(), String>>,
    },
    SeekMs {
        position_ms: u64,
        reply: Sender<Result<(), String>>,
    },
    SetVolume {
        volume: u8,
        reply: Sender<Result<(), String>>,
    },
    SetSpeed {
        speed: f64,
        reply: Sender<Result<(), String>>,
    },
    GetTransport {
        reply: Sender<Result<(u64, u64, BackendStatus), String>>,
    },
    Shutdown {
        reply: Sender<()>,
    },
}

pub struct EngineEvent {
    pub kind: EngineEventKind,
}

pub enum EngineEventKind {
    State(BackendStatus),
    Media { duration_ms: u64 },
    Position { position_ms: u64, duration_ms: u64 },
    Error(String),
}

pub struct VlcWorker {
    cmd_tx: Sender<EngineCmd>,
    event_rx: Receiver<EngineEvent>,
    join: Option<JoinHandle<()>>,
}

impl VlcWorker {
    /// Spawn worker that loads libVLC from `lib_dir` and processes commands.
    pub fn spawn(lib_dir: PathBuf, quiet: bool) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCmd>();
        let (event_tx, event_rx) = mpsc::channel::<EngineEvent>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let join = thread::Builder::new()
            .name("vlc-backend".into())
            .spawn(move || {
                worker_main(lib_dir, quiet, cmd_rx, event_tx, ready_tx);
            })
            .map_err(|e| format!("failed to spawn VLC worker: {e}"))?;

        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "VLC worker ready timeout".to_string())??;

        Ok(Self {
            cmd_tx,
            event_rx,
            join: Some(join),
        })
    }

    pub fn cmd_tx(&self) -> Sender<EngineCmd> {
        self.cmd_tx.clone()
    }

    pub fn try_recv_event(&self) -> Option<EngineEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn shutdown(mut self) {
        let (tx, rx) = mpsc::channel();
        let _ = self.cmd_tx.send(EngineCmd::Shutdown { reply: tx });
        let _ = rx.recv_timeout(Duration::from_secs(3));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for VlcWorker {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel();
        let _ = self.cmd_tx.send(EngineCmd::Shutdown { reply: tx });
        let _ = rx.recv_timeout(Duration::from_millis(500));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct Engine {
    api: LibVlcApi,
    instance: *mut crate::vlc_ffi::libvlc_instance_t,
    player: *mut crate::vlc_ffi::libvlc_media_player_t,
    // Keep CString argv alive for instance lifetime.
    _argv_storage: Vec<CString>,
}

// libVLC pointers are only used on the worker thread.
unsafe impl Send for Engine {}

fn worker_main(
    lib_dir: PathBuf,
    quiet: bool,
    cmd_rx: Receiver<EngineCmd>,
    event_tx: Sender<EngineEvent>,
    ready_tx: Sender<Result<(), String>>,
) {
    let mut engine = match Engine::new(&lib_dir, quiet) {
        Ok(e) => {
            let _ = ready_tx.send(Ok(()));
            e
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    let mut last_state = BackendStatus::Idle;
    let mut last_pos: i64 = -1;
    let mut last_duration: i64 = -1;
    let poll = Duration::from_millis(200);

    loop {
        let cmd = cmd_rx.recv_timeout(poll);
        match cmd {
            Ok(EngineCmd::Shutdown { reply }) => {
                engine.stop();
                engine.release();
                let _ = reply.send(());
                break;
            }
            Ok(other) => {
                if let Err(e) = engine.handle(other, &event_tx) {
                    let _ = event_tx.send(EngineEvent {
                        kind: EngineEventKind::Error(e),
                    });
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                engine.stop();
                engine.release();
                break;
            }
        }

        // Poll transport while active.
        let state = match engine.map_state() {
            Ok(state) => state,
            Err(error) => {
                if last_state != BackendStatus::Error {
                    let _ = event_tx.send(EngineEvent {
                        kind: EngineEventKind::Error(error),
                    });
                }
                BackendStatus::Error
            }
        };
        if state != last_state {
            last_state = state;
            let _ = event_tx.send(EngineEvent {
                kind: EngineEventKind::State(state),
            });
        }
        if matches!(state, BackendStatus::Playing | BackendStatus::Paused) {
            let pos = engine.get_time_ms();
            let duration = engine.get_length_ms();
            if duration != last_duration {
                last_duration = duration;
                if duration > 0 {
                    let _ = event_tx.send(EngineEvent {
                        kind: EngineEventKind::Media {
                            duration_ms: duration as u64,
                        },
                    });
                }
            }
            if pos != last_pos {
                last_pos = pos;
                let _ = event_tx.send(EngineEvent {
                    kind: EngineEventKind::Position {
                        position_ms: pos.max(0) as u64,
                        duration_ms: duration.max(0) as u64,
                    },
                });
            }
            if state == BackendStatus::Playing && duration > 0 && pos >= duration {
                engine.stop();
                last_state = BackendStatus::Stopped;
                let _ = event_tx.send(EngineEvent {
                    kind: EngineEventKind::State(BackendStatus::Stopped),
                });
            }
        }
    }
}

impl Engine {
    fn new(lib_dir: &Path, quiet: bool) -> Result<Self, String> {
        let api = unsafe { LibVlcApi::load(lib_dir)? };
        tracing::info!(version = api.version(), "validated LibVLC ABI");

        let mut args: Vec<String> = Vec::new();
        if quiet {
            args.push("--quiet".into());
        }
        args.push("--no-video".into());
        args.push("--intf".into());
        args.push("dummy".into());

        let argv_storage: Vec<CString> = args
            .iter()
            .map(|s| CString::new(s.as_str()).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let argv_ptrs: Vec<*const i8> = argv_storage.iter().map(|c| c.as_ptr()).collect();

        let instance = unsafe {
            (api.new)(
                argv_ptrs.len() as i32,
                if argv_ptrs.is_empty() {
                    ptr::null()
                } else {
                    argv_ptrs.as_ptr()
                },
            )
        };
        if instance.is_null() {
            let err = api
                .last_error()
                .unwrap_or_else(|| "libvlc_new returned null".into());
            return Err(format!("libvlc_new failed: {err}"));
        }

        let player = unsafe { (api.player_new)(instance) };
        if player.is_null() {
            unsafe { (api.release)(instance) };
            return Err("libvlc_media_player_new returned null".into());
        }

        Ok(Self {
            api,
            instance,
            player,
            _argv_storage: argv_storage,
        })
    }

    fn handle(&mut self, cmd: EngineCmd, _event_tx: &Sender<EngineEvent>) -> Result<(), String> {
        match cmd {
            EngineCmd::Play {
                path,
                start_ms,
                reply,
            } => {
                let r = self.play(&path, start_ms);
                let _ = reply.send(r);
            }
            EngineCmd::TogglePause { reply } => {
                unsafe {
                    (self.api.player_pause)(self.player);
                }
                let _ = reply.send(Ok(()));
            }
            EngineCmd::Stop { reply } => {
                self.stop();
                let _ = reply.send(Ok(()));
            }
            EngineCmd::SeekMs { position_ms, reply } => {
                let r = self.set_time_ms(position_ms as i64);
                let _ = reply.send(r);
            }
            EngineCmd::SetVolume { volume, reply } => {
                let code = unsafe { (self.api.audio_set_volume)(self.player, i32::from(volume)) };
                let r = if code == 0 {
                    Ok(())
                } else {
                    Err(format!("audio_set_volume failed ({code})"))
                };
                let _ = reply.send(r);
            }
            EngineCmd::SetSpeed { speed, reply } => {
                let code = unsafe { (self.api.player_set_rate)(self.player, speed as f32) };
                let r = if code == 0 {
                    Ok(())
                } else {
                    Err(format!("set_rate failed ({code})"))
                };
                let _ = reply.send(r);
            }
            EngineCmd::GetTransport { reply } => {
                let snap = self.raw_state().map(|raw_state| {
                    normalize_transport_snapshot(
                        raw_state,
                        self.get_time_ms(),
                        self.get_length_ms(),
                    )
                });
                let _ = reply.send(snap);
            }
            EngineCmd::Shutdown { reply } => {
                // Handled by outer loop.
                let _ = reply.send(());
            }
        }
        Ok(())
    }

    fn play(&mut self, path: &str, start_ms: u64) -> Result<(), String> {
        let c_path = CString::new(path).map_err(|e| e.to_string())?;
        let media = unsafe { (self.api.media_new_path)(self.instance, c_path.as_ptr()) };
        if media.is_null() {
            // Fallback: file URI
            if let Some(new_loc) = self.api.media_new_location {
                let uri = path_to_file_uri(path);
                let c_uri = CString::new(uri).map_err(|e| e.to_string())?;
                let media = unsafe { new_loc(self.instance, c_uri.as_ptr()) };
                if media.is_null() {
                    return Err(self
                        .api
                        .last_error()
                        .unwrap_or_else(|| "media_new failed".into()));
                }
                return self.play_media(media, start_ms);
            }
            return Err(self
                .api
                .last_error()
                .unwrap_or_else(|| "media_new_path failed".into()));
        }
        self.play_media(media, start_ms)
    }

    fn play_media(
        &mut self,
        media: *mut crate::vlc_ffi::libvlc_media_t,
        start_ms: u64,
    ) -> Result<(), String> {
        unsafe {
            (self.api.player_set_media)(self.player, media);
            (self.api.media_release)(media);
            let code = (self.api.player_play)(self.player);
            if code != 0 {
                return Err(self
                    .api
                    .last_error()
                    .unwrap_or_else(|| format!("play failed ({code})")));
            }
        }
        if start_ms > 0 {
            // Brief wait so seek sticks after play starts.
            thread::sleep(Duration::from_millis(50));
            let _ = self.set_time_ms(start_ms as i64);
        }
        Ok(())
    }

    fn stop(&mut self) {
        unsafe {
            (self.api.player_stop)(self.player);
        }
    }

    fn set_time_ms(&mut self, ms: i64) -> Result<(), String> {
        unsafe {
            self.api.set_player_time_ms(self.player, ms);
        }
        Ok(())
    }

    fn get_time_ms(&self) -> i64 {
        unsafe { self.api.player_time_ms(self.player) }
    }

    fn get_length_ms(&self) -> i64 {
        unsafe { self.api.player_length_ms(self.player) }
    }

    fn map_state(&self) -> Result<BackendStatus, String> {
        self.raw_state().map(map_vlc_state)
    }

    fn raw_state(&self) -> Result<LibVlcState, String> {
        unsafe { self.api.player_state(self.player) }
    }

    fn release(&mut self) {
        unsafe {
            if !self.player.is_null() {
                (self.api.player_release)(self.player);
                self.player = ptr::null_mut();
            }
            if !self.instance.is_null() {
                (self.api.release)(self.instance);
                self.instance = ptr::null_mut();
            }
        }
    }
}

fn map_vlc_state(state: LibVlcState) -> BackendStatus {
    match state {
        LibVlcState::Playing => BackendStatus::Playing,
        LibVlcState::Paused => BackendStatus::Paused,
        LibVlcState::Stopped | LibVlcState::Ended => BackendStatus::Stopped,
        LibVlcState::Opening | LibVlcState::Buffering => BackendStatus::Loading,
        LibVlcState::Error => BackendStatus::Error,
        LibVlcState::NothingSpecial => BackendStatus::Idle,
    }
}

/// libVLC can enter `Ended` before its relatively coarse position clock emits
/// the final sample. Preserve the public `Stopped` state while making a natural
/// end unambiguous to the playlist-advance logic. A user-requested VLC `Stopped`
/// state keeps its actual position and therefore remains distinguishable.
fn normalize_transport_snapshot(
    state: LibVlcState,
    position_ms: i64,
    duration_ms: i64,
) -> (u64, u64, BackendStatus) {
    let duration_ms = duration_ms.max(0) as u64;
    let position_ms = if state == LibVlcState::Ended && duration_ms > 0 {
        duration_ms
    } else {
        position_ms.max(0) as u64
    };
    (position_ms, duration_ms, map_vlc_state(state))
}

fn path_to_file_uri(path: &str) -> String {
    let p = Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    // Simple file URI; libVLC accepts Windows paths with forward slashes.
    let s = abs.to_string_lossy().replace('\\', "/");
    if s.starts_with("//") {
        format!("file:{s}")
    } else if abs.has_root() {
        format!("file:///{s}")
    } else {
        format!("file:{s}")
    }
}

#[allow(dead_code)]
fn cstr_lossy(p: *const i8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_snapshot_reaches_duration_when_vlc_clock_is_short() {
        assert_eq!(
            normalize_transport_snapshot(LibVlcState::Ended, 179_720, 180_000),
            (180_000, 180_000, BackendStatus::Stopped)
        );
    }

    #[test]
    fn explicit_stop_preserves_position_and_cannot_look_like_natural_end() {
        assert_eq!(
            normalize_transport_snapshot(LibVlcState::Stopped, 42_000, 180_000),
            (42_000, 180_000, BackendStatus::Stopped)
        );
    }
}
