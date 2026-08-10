# tz-player Rust Specification (parity v1)

Derived from the Python project `SPEC.md` with media roles made explicit.

## 1. Purpose

Local-first terminal music player with a keyboard-first TUI.

## 2. Media policy

| Concern | Implementation |
|---------|----------------|
| Playback | VLC / libVLC (`--backend vlc`, default) |
| Experimental playback | Rodio / Symphonia / system audio (`--backend rodio`) |
| Fallback / CI | Fake backend (`--backend fake`) |
| Analysis for visualizers | FFmpeg (optional) + native WAV |
| Custom minimal FFmpeg | Future; analysis path only |

## 3. In scope (parity)

- Playlist management (SQLite)
- Playback via pluggable backend (VLC + experimental Rodio + fake)
- Keyboard-first TUI (ratatui)
- Persistent state (volume, speed, repeat, shuffle, backend, visualizer)
- Cached metadata
- Visualizer host + built-in plugins
- Lazy analysis caches (envelope, spectrum, beat, waveform)
- `doctor` and `setup` CLI
- Structured internal command API (headless-ready)

## 4. Out of scope (parity v1)

- Streaming services
- Multi-user/network sync
- Remote control / web UI (later)
- Voice / local AI (later)
- Replacing VLC with FFmpeg for listening
- Promoting Rodio to the default without a separate evaluated decision
- Python visualizer plugin compatibility

## 5. Workflows

Same acceptance intent as Python WF-01..WF-07:

1. Launch and recover state  
2. Navigate playlist  
3. Playback control  
4. Find/search focus  
5. Playlist editing  
6. Visualization  
7. Runtime config and diagnostics  

## 6. Speed limits

Playback speed clamp: **0.5x – 4.0x** (step 0.25).

Rodio rate changes also change pitch. Pitch-preserving time stretching is out
of scope for the parity contract.

## 7. Config precedence

1. CLI flags for current run  
2. Persisted state  
3. Built-in defaults  

## 8. Quality gates

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo audit`
- `cargo deny --locked check advisories bans licenses sources`
