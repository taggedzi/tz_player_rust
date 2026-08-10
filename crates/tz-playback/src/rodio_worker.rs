//! Rodio output device and player ownership on a dedicated worker thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::rodio_engine::{
    decode_file, RodioSnapshot, RodioTransport, TimelineHandle, TimelineSource,
};
use crate::BackendStatus;

pub(crate) enum RodioCmd {
    Play {
        item_id: i64,
        path: PathBuf,
        start_ms: u64,
        duration_ms: Option<u64>,
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
        reply: Sender<Result<RodioSnapshot, String>>,
    },
    Shutdown {
        reply: Sender<()>,
    },
}

pub(crate) struct RodioWorkerEvent {
    pub(crate) kind: RodioWorkerEventKind,
}

pub(crate) enum RodioWorkerEventKind {
    State(BackendStatus),
    Media { duration_ms: u64 },
    Position { position_ms: u64, duration_ms: u64 },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RodioOutputInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
}

pub(crate) struct RodioWorker {
    cmd_tx: Sender<RodioCmd>,
    event_rx: Receiver<RodioWorkerEvent>,
    join: Option<JoinHandle<()>>,
    output_info: RodioOutputInfo,
}

impl RodioWorker {
    pub(crate) fn spawn() -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let join = thread::Builder::new()
            .name("rodio-backend".into())
            .spawn(move || worker_main(cmd_rx, event_tx, ready_tx))
            .map_err(|error| format!("failed to spawn Rodio worker: {error}"))?;

        let output_info = match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(info)) => info,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(_) => return Err("Rodio worker ready timeout".into()),
        };

        Ok(Self {
            cmd_tx,
            event_rx,
            join: Some(join),
            output_info,
        })
    }

    pub(crate) fn cmd_tx(&self) -> Sender<RodioCmd> {
        self.cmd_tx.clone()
    }

    pub(crate) fn try_recv_event(&self) -> Option<RodioWorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    pub(crate) fn output_info(&self) -> RodioOutputInfo {
        self.output_info.clone()
    }

    pub(crate) fn shutdown(mut self) {
        self.request_shutdown(Duration::from_secs(3));
    }

    fn request_shutdown(&mut self, timeout: Duration) {
        let (reply, reply_rx) = mpsc::channel();
        let _ = self.cmd_tx.send(RodioCmd::Shutdown { reply });
        let acknowledged = reply_rx.recv_timeout(timeout).is_ok();
        if acknowledged {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        } else {
            // Dropping a JoinHandle detaches it. Do not block application exit
            // indefinitely if a platform audio call has wedged the worker.
            self.join.take();
        }
    }
}

impl Drop for RodioWorker {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.request_shutdown(Duration::from_millis(500));
        }
    }
}

fn worker_main(
    cmd_rx: Receiver<RodioCmd>,
    event_tx: Sender<RodioWorkerEvent>,
    ready_tx: Sender<Result<RodioOutputInfo, String>>,
) {
    let (stream_error_tx, stream_error_rx) = mpsc::channel::<String>();
    let builder = match DeviceSinkBuilder::from_default_device() {
        Ok(builder) => builder.with_error_callback(move |error| {
            let _ = stream_error_tx.send(error.to_string());
        }),
        Err(error) => {
            let _ = ready_tx.send(Err(format!("no usable default audio output: {error}")));
            return;
        }
    };
    let mut output = match builder.open_sink_or_fallback() {
        Ok(output) => output,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("failed to open default audio output: {error}")));
            return;
        }
    };
    output.log_on_drop(false);
    let config = output.config();
    let info = RodioOutputInfo {
        channels: config.channel_count().get(),
        sample_rate: config.sample_rate().get(),
        sample_format: format!("{:?}", config.sample_format()),
    };
    let _ = ready_tx.send(Ok(info));

    let mut engine = WorkerEngine::new(output, stream_error_rx);
    let poll_interval = Duration::from_millis(20);
    loop {
        engine.refresh(&event_tx);
        match cmd_rx.recv_timeout(poll_interval) {
            Ok(RodioCmd::Shutdown { reply }) => {
                engine.stop();
                let _ = reply.send(());
                break;
            }
            Ok(command) => engine.handle(command, &event_tx),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                engine.stop();
                break;
            }
        }
    }
}

struct WorkerEngine {
    output: MixerDeviceSink,
    player: Option<Player>,
    timeline: Option<TimelineHandle>,
    transport: RodioTransport,
    stream_error_rx: Receiver<String>,
    last_event_position_ms: u64,
}

impl WorkerEngine {
    fn new(output: MixerDeviceSink, stream_error_rx: Receiver<String>) -> Self {
        Self {
            output,
            player: None,
            timeline: None,
            transport: RodioTransport::default(),
            stream_error_rx,
            last_event_position_ms: 0,
        }
    }

    fn handle(&mut self, command: RodioCmd, events: &Sender<RodioWorkerEvent>) {
        match command {
            RodioCmd::Play {
                item_id,
                path,
                start_ms,
                duration_ms,
                reply,
            } => {
                let result = self.play(item_id, path, start_ms, duration_ms, events);
                let _ = reply.send(result);
            }
            RodioCmd::TogglePause { reply } => {
                let result = self.toggle_pause(events);
                let _ = reply.send(result);
            }
            RodioCmd::Stop { reply } => {
                self.stop();
                emit_state(events, BackendStatus::Stopped);
                let _ = reply.send(Ok(()));
            }
            RodioCmd::SeekMs { position_ms, reply } => {
                let result = self.seek(position_ms, events);
                let _ = reply.send(result);
            }
            RodioCmd::SetVolume { volume, reply } => {
                self.transport.set_volume(volume);
                if let Some(player) = &self.player {
                    player.set_volume(f32::from(volume.min(100)) / 100.0);
                }
                let _ = reply.send(Ok(()));
            }
            RodioCmd::SetSpeed { speed, reply } => {
                self.transport.set_speed(speed);
                if let Some(player) = &self.player {
                    player.set_speed(speed as f32);
                }
                let _ = reply.send(Ok(()));
            }
            RodioCmd::GetTransport { reply } => {
                self.refresh(events);
                let _ = reply.send(Ok(self.transport.snapshot()));
            }
            RodioCmd::Shutdown { reply } => {
                let _ = reply.send(());
            }
        }
    }

    fn play(
        &mut self,
        item_id: i64,
        path: PathBuf,
        start_ms: u64,
        fallback_duration_ms: Option<u64>,
        events: &Sender<RodioWorkerEvent>,
    ) -> Result<(), String> {
        self.player.take();
        self.timeline.take();
        self.transport
            .begin_load(item_id, start_ms, fallback_duration_ms);
        emit_state(events, BackendStatus::Loading);

        let result: Result<(), String> = (|| {
            let mut decoded = decode_file(&path).map_err(|error| error.to_string())?;
            let duration_ms = decoded.duration_ms;
            let start = Duration::from_millis(start_ms);
            if start_ms > 0 {
                decoded.decoder.try_seek(start).map_err(|error| {
                    format!(
                        "Rodio could not seek {} to {start_ms} ms: {error}",
                        path.display()
                    )
                })?;
            }

            let (source, timeline) = TimelineSource::new(decoded.decoder, start);
            let player = Player::connect_new(self.output.mixer());
            let snapshot = self.transport.snapshot();
            player.set_volume(f32::from(snapshot.volume) / 100.0);
            player.set_speed(snapshot.speed as f32);
            player.append(source);
            self.player = Some(player);
            self.timeline = Some(timeline);
            self.transport.loaded(duration_ms);
            self.last_event_position_ms = start_ms;

            let snapshot = self.transport.snapshot();
            let _ = events.send(RodioWorkerEvent {
                kind: RodioWorkerEventKind::Media {
                    duration_ms: snapshot.duration_ms,
                },
            });
            emit_state(events, BackendStatus::Playing);
            Ok(())
        })();

        if let Err(error) = &result {
            self.transport.fail(error.clone());
            let _ = events.send(RodioWorkerEvent {
                kind: RodioWorkerEventKind::Error(error.clone()),
            });
        }
        result
    }

    fn toggle_pause(&mut self, events: &Sender<RodioWorkerEvent>) -> Result<(), String> {
        let status = self
            .transport
            .toggle_pause()
            .map_err(|error| error.to_string())?;
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| "Rodio has no active player".to_string())?;
        match status {
            BackendStatus::Paused => player.pause(),
            BackendStatus::Playing => player.play(),
            _ => {}
        }
        emit_state(events, status);
        Ok(())
    }

    fn seek(&mut self, position_ms: u64, events: &Sender<RodioWorkerEvent>) -> Result<(), String> {
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| "Rodio cannot seek without an active track".to_string())?;
        let accepted = if self.transport.snapshot().duration_ms > 0 {
            position_ms.min(self.transport.snapshot().duration_ms)
        } else {
            position_ms
        };
        player
            .try_seek(Duration::from_millis(accepted))
            .map_err(|error| format!("Rodio seek to {accepted} ms failed: {error}"))?;
        self.transport.seek_accepted(accepted);
        self.last_event_position_ms = accepted;
        let duration_ms = self.transport.snapshot().duration_ms;
        let _ = events.send(RodioWorkerEvent {
            kind: RodioWorkerEventKind::Position {
                position_ms: accepted,
                duration_ms,
            },
        });
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            player.stop();
        }
        self.timeline.take();
        self.transport.stop();
        self.last_event_position_ms = 0;
    }

    fn refresh(&mut self, events: &Sender<RodioWorkerEvent>) {
        if let Ok(error) = self.stream_error_rx.try_recv() {
            if self.transport.snapshot().status != BackendStatus::Error {
                self.player.take();
                self.timeline.take();
                let message = format!("Rodio output device failed: {error}");
                self.transport.fail(message.clone());
                let _ = events.send(RodioWorkerEvent {
                    kind: RodioWorkerEventKind::Error(message),
                });
            }
            return;
        }

        if let Some(timeline) = &self.timeline {
            let position_ms = timeline.position_ms();
            self.transport.observe_position(position_ms);
            if position_ms != self.last_event_position_ms {
                self.last_event_position_ms = position_ms;
                let _ = events.send(RodioWorkerEvent {
                    kind: RodioWorkerEventKind::Position {
                        position_ms,
                        duration_ms: self.transport.snapshot().duration_ms,
                    },
                });
            }
        }

        if self.player.as_ref().is_some_and(Player::empty) && self.transport.observe_empty() {
            self.player.take();
            self.timeline.take();
            let snapshot = self.transport.snapshot();
            let _ = events.send(RodioWorkerEvent {
                kind: RodioWorkerEventKind::Position {
                    position_ms: snapshot.position_ms,
                    duration_ms: snapshot.duration_ms,
                },
            });
            emit_state(events, BackendStatus::Stopped);
        }
    }
}

fn emit_state(events: &Sender<RodioWorkerEvent>, status: BackendStatus) {
    let _ = events.send(RodioWorkerEvent {
        kind: RodioWorkerEventKind::State(status),
    });
}
