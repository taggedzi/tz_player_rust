use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::discovery::resolve_package_helper;
use crate::{
    read_decode_header, Capabilities, DecodeError, DecodeRequest, PcmSource, PcmSpec, SampleFormat,
};

const MAX_STDERR_BYTES: usize = 64 * 1024;
const PCM_CHUNK_BYTES: usize = 32 * 1024;
const MAX_PCM_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const PCM_QUEUE_TARGET_MS: u64 = 500;
const PCM_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_secs(2);
const FFMPEG_MANIFEST: &str = include_str!("../../../native/ffmpeg/manifest.toml");

#[derive(Debug, Clone)]
pub struct HelperConfig {
    pub executable: PathBuf,
    pub startup_timeout: Duration,
    pub pcm_stall_timeout: Duration,
    pub stop_grace: Duration,
    expected_configuration_hash: Option<String>,
}

impl HelperConfig {
    pub fn packaged() -> Result<Self, DecodeError> {
        #[cfg(debug_assertions)]
        if let Some(executable) = std::env::var_os("TZ_PLAYER_AUDIO_HELPER") {
            return Self::injected(PathBuf::from(executable));
        }
        let location = resolve_package_helper().map_err(DecodeError::Message)?;
        let build_path = location.package_root.join("audio/FFMPEG_BUILD.json");
        let build: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&build_path).map_err(|error| {
                DecodeError::Message(format!(
                    "bundled helper build metadata could not be read at {}: {error}",
                    build_path.display()
                ))
            })?)
            .map_err(|error| {
                DecodeError::Message(format!(
                    "bundled helper build metadata is invalid at {}: {error}",
                    build_path.display()
                ))
            })?;
        let configuration = build
            .get("configuration")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                DecodeError::Message(format!(
                    "bundled helper build metadata has no configuration at {}",
                    build_path.display()
                ))
            })?;
        Ok(Self {
            executable: location.path,
            startup_timeout: Duration::from_secs(5),
            pcm_stall_timeout: PCM_STALL_TIMEOUT,
            stop_grace: STOP_GRACE,
            expected_configuration_hash: Some(fnv1a(configuration.as_bytes())),
        })
    }

    pub fn injected(executable: PathBuf) -> Result<Self, DecodeError> {
        if !executable.is_absolute() {
            return Err(DecodeError::Message("helper path must be absolute".into()));
        }
        Ok(Self {
            executable,
            startup_timeout: Duration::from_secs(5),
            pcm_stall_timeout: PCM_STALL_TIMEOUT,
            stop_grace: STOP_GRACE,
            expected_configuration_hash: None,
        })
    }
}

pub fn capabilities(config: &HelperConfig) -> Result<Capabilities, DecodeError> {
    let mut command = Command::new(&config.executable);
    command
        .args(["capabilities", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| DecodeError::Message(format!("bundled helper launch failed: {error}")))?;
    let stdout = drain_bounded(child.stdout.take(), MAX_STDERR_BYTES);
    let stderr = drain_bounded(child.stderr.take(), MAX_STDERR_BYTES);
    let deadline = Instant::now() + config.startup_timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(DecodeError::Message(
                    "bundled helper capability check timed out after 5 seconds".into(),
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DecodeError::Message(format!(
                    "bundled helper status failed: {error}"
                )));
            }
        }
    };
    let stdout = stdout
        .join()
        .map_err(|_| DecodeError::Message("bundled helper stdout reader failed".into()))?;
    let stderr = stderr
        .join()
        .map_err(|_| DecodeError::Message("bundled helper stderr reader failed".into()))?;
    if !status.success() {
        return Err(DecodeError::Message(format!(
            "bundled helper capabilities failed: {}",
            sanitize_stderr(&stderr)
        )));
    }
    let capabilities: Capabilities = serde_json::from_slice(&stdout)
        .map_err(|error| DecodeError::Message(format!("invalid helper capabilities: {error}")))?;
    validate_capabilities(config, &capabilities)?;
    Ok(capabilities)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CapabilityCacheKey {
    executable: PathBuf,
    expected_configuration_hash: Option<String>,
}

fn ensure_capabilities(config: &HelperConfig) -> Result<(), DecodeError> {
    static CACHE: OnceLock<Mutex<HashMap<CapabilityCacheKey, Capabilities>>> = OnceLock::new();
    let key = CapabilityCacheKey {
        executable: config.executable.clone(),
        expected_configuration_hash: config.expected_configuration_hash.clone(),
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if cache.lock().unwrap().contains_key(&key) {
        return Ok(());
    }
    let value = capabilities(config)?;
    cache.lock().unwrap().insert(key, value);
    Ok(())
}

fn validate_capabilities(
    config: &HelperConfig,
    capabilities: &Capabilities,
) -> Result<(), DecodeError> {
    if capabilities.protocol_major != crate::PROTOCOL_MAJOR {
        return Err(DecodeError::Message(format!(
            "bundled helper protocol major {} is incompatible with {}",
            capabilities.protocol_major,
            crate::PROTOCOL_MAJOR
        )));
    }
    let expected_version = manifest_value("version");
    let expected_commit = manifest_value("ffmpeg_release_commit");
    if capabilities.helper_version != env!("CARGO_PKG_VERSION")
        || capabilities.ffmpeg_version != expected_version
        || capabilities.ffmpeg_commit != expected_commit
        || capabilities.demuxers != sorted_manifest_array("demuxers")
        || capabilities.decoders != sorted_manifest_array("decoders")
        || capabilities.library_majors != manifest_library_majors()
    {
        return Err(DecodeError::Message(
            "bundled helper capabilities do not match the pinned FFmpeg manifest".into(),
        ));
    }
    if capabilities.configuration_hash.is_empty()
        || !capabilities
            .configuration_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DecodeError::Message(
            "bundled helper capabilities have an invalid configuration hash".into(),
        ));
    }
    if config
        .expected_configuration_hash
        .as_deref()
        .is_some_and(|expected| {
            !capabilities
                .configuration_hash
                .eq_ignore_ascii_case(expected)
        })
    {
        return Err(DecodeError::Message(
            "bundled helper configuration does not match FFMPEG_BUILD.json".into(),
        ));
    }
    Ok(())
}

fn manifest_value(name: &str) -> String {
    let prefix = format!("{name} = \"");
    FFMPEG_MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'))
        .unwrap_or_else(|| panic!("FFmpeg manifest value is missing: {name}"))
        .to_owned()
}

fn sorted_manifest_array(name: &str) -> Vec<String> {
    let prefix = format!("{name} = [");
    let mut values = FFMPEG_MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix(']'))
        .unwrap_or_else(|| panic!("FFmpeg manifest array is missing: {name}"))
        .split(',')
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn manifest_library_majors() -> BTreeMap<String, u32> {
    let prefix = "library_majors = {";
    FFMPEG_MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix(prefix)?.strip_suffix('}'))
        .expect("FFmpeg manifest library majors are missing")
        .split(',')
        .map(|entry| {
            let (name, major) = entry
                .split_once('=')
                .expect("invalid FFmpeg manifest library major");
            (
                name.trim().to_owned(),
                major
                    .trim()
                    .parse::<u32>()
                    .expect("invalid FFmpeg library major"),
            )
        })
        .collect()
}

fn fnv1a(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:x}")
}

fn drain_bounded<R: Read + Send + 'static>(reader: Option<R>, limit: usize) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let Some(mut reader) = reader else {
            return Vec::new();
        };
        let mut collected = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let remaining = limit.saturating_sub(collected.len());
                    collected.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
        collected
    })
}

pub fn decode(
    config: &HelperConfig,
    path: &Path,
    start_frame: u64,
    spec: PcmSpec,
) -> Result<HelperPcmSource, DecodeError> {
    let metadata = path
        .metadata()
        .map_err(|error| DecodeError::Message(format!("input {}: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(DecodeError::Message(format!(
            "input is not a local regular file: {}",
            path.display()
        )));
    }
    ensure_capabilities(config)?;
    let (running, header) = start_decode(config, path, start_frame, spec)?;
    Ok(HelperPcmSource {
        config: config.clone(),
        path: path.to_path_buf(),
        running: Some(running),
        spec,
        duration_frames: header.duration_frames,
        frame_position: header.start_frame,
        pending_error: None,
        finished: false,
        last_pcm_progress: Instant::now(),
        underrun_count: 0,
    })
}

fn start_decode(
    config: &HelperConfig,
    path: &Path,
    start_frame: u64,
    spec: PcmSpec,
) -> Result<(RunningHelper, crate::DecodeHeader), DecodeError> {
    let request = DecodeRequest {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        format: SampleFormat::F32le,
        start_frame,
    };
    let start_ms = start_frame.saturating_mul(1_000) / u64::from(spec.sample_rate);
    let mut command = Command::new(&config.executable);
    command
        .args(["decode", "--input"])
        .arg(path)
        .args([
            "--start-ms",
            &start_ms.to_string(),
            "--sample-rate",
            &spec.sample_rate.to_string(),
            "--channels",
            &spec.channels.to_string(),
            "--format",
            "f32le",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| DecodeError::Message(format!("bundled helper launch failed: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DecodeError::Message("bundled helper stdout unavailable".into()))?;
    let stderr = drain_bounded(child.stderr.take(), MAX_STDERR_BYTES);
    let (header_tx, header_rx) = mpsc::sync_channel(1);
    let queue_bytes = pcm_queue_bytes(spec);
    let queue_chunks = queue_bytes.div_ceil(PCM_CHUNK_BYTES).max(1);
    let (pcm_tx, pcm_rx) = mpsc::sync_channel(queue_chunks);
    let reader = std::thread::spawn(move || {
        stream_stdout(stdout, request, spec, header_tx, pcm_tx);
    });
    let header = match header_rx.recv_timeout(config.startup_timeout) {
        Ok(Ok(header)) => header,
        Ok(Err(error)) => {
            terminate_child(&mut child);
            let _ = reader.join();
            let diagnostic = stderr.join().unwrap_or_default();
            let suffix = sanitize_stderr(&diagnostic);
            return Err(DecodeError::Message(if suffix.is_empty() {
                format!("bundled helper protocol error: {error}")
            } else {
                format!("bundled helper protocol error: {error}; {suffix}")
            }));
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            terminate_child(&mut child);
            let _ = reader.join();
            let _ = stderr.join();
            return Err(DecodeError::Message(
                "bundled helper startup timed out after 5 seconds".into(),
            ));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            terminate_child(&mut child);
            let _ = reader.join();
            let diagnostic = stderr.join().unwrap_or_default();
            return Err(DecodeError::Message(format!(
                "bundled helper stopped before its decode header: {}",
                sanitize_stderr(&diagnostic)
            )));
        }
    };
    Ok((
        RunningHelper {
            child,
            receiver: pcm_rx,
            reader: Some(reader),
            stderr: Some(stderr),
            current: Vec::new(),
            current_offset: 0,
            stop_grace: config.stop_grace,
        },
        header,
    ))
}

fn pcm_queue_bytes(spec: PcmSpec) -> usize {
    usize::try_from(
        u64::from(spec.sample_rate)
            .saturating_mul(u64::from(spec.channels))
            .saturating_mul(4)
            .saturating_mul(PCM_QUEUE_TARGET_MS)
            / 1_000,
    )
    .unwrap_or(usize::MAX)
    .clamp(PCM_CHUNK_BYTES, MAX_PCM_QUEUE_BYTES)
}

pub struct HelperPcmSource {
    config: HelperConfig,
    path: PathBuf,
    running: Option<RunningHelper>,
    spec: PcmSpec,
    duration_frames: Option<u64>,
    frame_position: u64,
    pending_error: Option<DecodeError>,
    finished: bool,
    last_pcm_progress: Instant,
    underrun_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRead {
    Data(usize),
    Underrun,
    Eof,
}

struct RunningHelper {
    child: Child,
    receiver: Receiver<Result<Vec<f32>, DecodeError>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    current: Vec<f32>,
    current_offset: usize,
    stop_grace: Duration,
}

impl PcmSource for HelperPcmSource {
    fn spec(&self) -> PcmSpec {
        self.spec
    }
    fn duration_frames(&self) -> Option<u64> {
        self.duration_frames
    }
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, DecodeError> {
        loop {
            match self.try_read_for_playback(output)? {
                PlaybackRead::Data(count) => return Ok(count),
                PlaybackRead::Eof => return Ok(0),
                PlaybackRead::Underrun => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }
    fn seek_to_frame(&mut self, frame: u64) -> Result<(), DecodeError> {
        self.stop_running();
        let (running, header) = start_decode(&self.config, &self.path, frame, self.spec)?;
        self.running = Some(running);
        self.duration_frames = header.duration_frames;
        self.frame_position = header.start_frame;
        self.pending_error = None;
        self.finished = false;
        self.last_pcm_progress = Instant::now();
        self.underrun_count = 0;
        Ok(())
    }
}

impl HelperPcmSource {
    /// Read without waiting on child-process I/O. Playback inserts silence for
    /// `Underrun`; a continuous five-second underrun becomes an error.
    pub fn try_read_for_playback(
        &mut self,
        output: &mut [f32],
    ) -> Result<PlaybackRead, DecodeError> {
        if !output.len().is_multiple_of(self.spec.frame_samples()) {
            return Err(DecodeError::UnalignedBuffer);
        }
        if output.is_empty() {
            return Ok(PlaybackRead::Data(0));
        }
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        if self.finished {
            return Ok(PlaybackRead::Eof);
        }
        let mut written = 0;
        while written < output.len() {
            let Some(running) = self.running.as_mut() else {
                break;
            };
            if running.current_offset < running.current.len() {
                let available = running.current.len() - running.current_offset;
                let count = available.min(output.len() - written);
                output[written..written + count].copy_from_slice(
                    &running.current[running.current_offset..running.current_offset + count],
                );
                running.current_offset += count;
                written += count;
                continue;
            }
            match running.receiver.try_recv() {
                Ok(Ok(samples)) => {
                    running.current = samples;
                    running.current_offset = 0;
                    self.last_pcm_progress = Instant::now();
                }
                Ok(Err(error)) => {
                    let _ = self.finish_running();
                    if written == 0 {
                        return Err(error);
                    }
                    self.pending_error = Some(error);
                    break;
                }
                Err(TryRecvError::Empty) => {
                    if self.last_pcm_progress.elapsed() >= self.config.pcm_stall_timeout {
                        self.stop_running();
                        let error = DecodeError::Message(format!(
                            "bundled helper PCM stalled for {} ms",
                            self.config.pcm_stall_timeout.as_millis()
                        ));
                        if written == 0 {
                            return Err(error);
                        }
                        self.pending_error = Some(error);
                        break;
                    }
                    if written == 0 {
                        self.underrun_count = self.underrun_count.saturating_add(1);
                        return Ok(PlaybackRead::Underrun);
                    }
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    let result = self.finish_running();
                    self.finished = result.is_ok();
                    if let Err(error) = result {
                        if written == 0 {
                            return Err(error);
                        }
                        self.pending_error = Some(error);
                    }
                    break;
                }
            }
        }
        self.frame_position = self
            .frame_position
            .saturating_add((written / self.spec.frame_samples()) as u64);
        if written == 0 && self.finished {
            Ok(PlaybackRead::Eof)
        } else {
            Ok(PlaybackRead::Data(written))
        }
    }

    pub fn underrun_count(&self) -> u64 {
        self.underrun_count
    }

    fn finish_running(&mut self) -> Result<(), DecodeError> {
        let Some(mut running) = self.running.take() else {
            return Ok(());
        };
        let status = running
            .child
            .wait()
            .map_err(|error| DecodeError::Message(format!("helper wait failed: {error}")))?;
        if let Some(reader) = running.reader.take() {
            let _ = reader.join();
        }
        let stderr = running
            .stderr
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            Err(DecodeError::Message(format!(
                "bundled helper exited with {}: {}",
                status.code().unwrap_or(70),
                sanitize_stderr(&stderr)
            )))
        }
    }

    fn stop_running(&mut self) {
        if let Some(running) = self.running.take() {
            stop_running(running);
        }
    }
}

impl Drop for HelperPcmSource {
    fn drop(&mut self) {
        self.stop_running();
    }
}

fn stream_stdout(
    stdout: ChildStdout,
    request: DecodeRequest,
    spec: PcmSpec,
    header: SyncSender<Result<crate::DecodeHeader, crate::ProtocolError>>,
    pcm: SyncSender<Result<Vec<f32>, DecodeError>>,
) {
    let mut reader = BufReader::new(stdout);
    match read_decode_header(&mut reader, &request) {
        Ok(parsed) => {
            if header.send(Ok(parsed)).is_err() {
                return;
            }
        }
        Err(error) => {
            let _ = header.send(Err(error));
            return;
        }
    }
    let frame_bytes = spec.frame_samples() * size_of::<f32>();
    let mut pending = Vec::with_capacity(PCM_CHUNK_BYTES + frame_bytes);
    let mut buffer = vec![0; PCM_CHUNK_BYTES];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => {
                if !pending.is_empty() {
                    let _ = pcm.send(Err(DecodeError::Message(
                        "helper returned a partial PCM frame".into(),
                    )));
                }
                return;
            }
            Ok(count) => count,
            Err(error) => {
                let _ = pcm.send(Err(DecodeError::Message(format!(
                    "helper PCM read failed: {error}"
                ))));
                return;
            }
        };
        pending.extend_from_slice(&buffer[..count]);
        let complete = pending.len() / frame_bytes * frame_bytes;
        if complete == 0 {
            continue;
        }
        let mut samples = Vec::with_capacity(complete / size_of::<f32>());
        for chunk in pending[..complete].chunks_exact(size_of::<f32>()) {
            let sample = f32::from_le_bytes(chunk.try_into().expect("four-byte sample"));
            match crate::clamp_sample(sample) {
                Ok(sample) => samples.push(sample),
                Err(error) => {
                    let _ = pcm.send(Err(error));
                    return;
                }
            }
        }
        pending.copy_within(complete.., 0);
        pending.truncate(pending.len() - complete);
        if pcm.send(Ok(samples)).is_err() {
            return;
        }
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn stop_running(running: RunningHelper) {
    let RunningHelper {
        mut child,
        receiver,
        mut reader,
        mut stderr,
        stop_grace,
        ..
    } = running;
    drop(receiver);
    let deadline = Instant::now() + stop_grace;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            if let Some(reader) = reader.take() {
                let _ = reader.join();
            }
            if let Some(stderr) = stderr.take() {
                let _ = stderr.join();
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    terminate_child(&mut child);
    if let Some(reader) = reader.take() {
        let _ = reader.join();
    }
    if let Some(stderr) = stderr.take() {
        let _ = stderr.join();
    }
}

fn sanitize_stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_STDERR_BYTES)]);
    crate::protocol::sanitize_diagnostic(&text, MAX_STDERR_BYTES)
}

fn configure_child_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_queue_targets_half_a_second_and_stays_bounded() {
        let spec = PcmSpec::new(48_000, 2).unwrap();
        assert_eq!(pcm_queue_bytes(spec), 192_000);
        assert!(pcm_queue_bytes(spec) <= MAX_PCM_QUEUE_BYTES);
        assert!(
            pcm_queue_bytes(spec)
                <= spec.sample_rate as usize * spec.frame_samples() * size_of::<f32>() * 2
        );
    }
}
