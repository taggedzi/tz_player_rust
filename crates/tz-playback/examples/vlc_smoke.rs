//! Smoke test: play a generated sine WAV through libVLC.
//!
//! ```text
//! cargo run -p tz-playback --example vlc_smoke
//! cargo run -p tz-playback --example vlc_smoke -- --startup-only
//! ```

use std::f32::consts::PI;
use std::fs::File;
use std::io::Write;
use std::time::Duration;

use tz_playback::{configure_vlc_environment, PlaybackBackend, VlcPlaybackBackend};

fn write_sine_wav(path: &std::path::Path, seconds: f32, freq: f32) {
    let sample_rate = 44100u32;
    let n = (sample_rate as f32 * seconds) as usize;
    let mut data = Vec::with_capacity(44 + n * 2);
    let data_len = (n * 2) as u32;
    let file_len = 36 + data_len;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&file_len.to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&sample_rate.to_le_bytes());
    data.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        let t = i as f32 / sample_rate as f32;
        let sample = (0.2 * (2.0 * PI * freq * t).sin() * i16::MAX as f32) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
    }
    File::create(path).unwrap().write_all(&data).unwrap();
}

fn main() {
    let startup_only = std::env::args_os().any(|argument| argument == "--startup-only");
    configure_vlc_environment();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("async runtime");
    runtime.block_on(run(startup_only));
}

async fn run(startup_only: bool) {
    let mut backend = VlcPlaybackBackend::new();
    println!("discovery usable={}", backend.discovery().is_usable());
    backend.start().await.expect("start");
    if startup_only {
        backend.shutdown().await.expect("shutdown");
        println!("startup OK");
        return;
    }

    let dir = std::env::temp_dir().join("tz_vlc_smoke");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("beep.wav");
    write_sine_wav(&wav, 1.5, 440.0);
    println!("wav={}", wav.display());

    backend.set_volume(40).await.expect("vol");
    backend.play(1, &wav, 0, Some(1500)).await.expect("play");
    println!("playing...");
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (pos, dur, st) = backend.get_transport_snapshot().await.expect("snap");
        println!("state={st:?} pos={pos} dur={dur}");
    }
    backend.stop().await.expect("stop");
    backend.shutdown().await.expect("shutdown");
    println!("OK");
}
