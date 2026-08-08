# ADR-0001 — Rust crate architecture and media split

- Status: Accepted
- Date: 2026-04-07

## Context

We are rewriting Python tz-player in Rust for performance and a future headless multi-frontend design. The Python app uses **VLC for playback** and **FFmpeg for optional analysis/visualizer decode**. An earlier draft plan incorrectly suggested replacing VLC with a pure-Rust or FFmpeg listen path for v1.

## Decision

1. Use a Cargo workspace with crates: `tz-core`, `tz-playback`, `tz-analysis`, `tz-control`, `tz-db`, `tz-tui`, `tz-player`.
2. **Listen path** lives only in `tz-playback` and uses **VLC/libVLC** (plus Fake).
3. **Analysis path** lives only in `tz-analysis` and uses **FFmpeg** (plus native WAV).
4. TUI and other frontends talk to the core via `tz-control` commands; they never link VLC/FFmpeg directly.
5. A custom minimal FFmpeg build is a **later** optimization for analysis, not a v1 playback engine.

## Consequences

Positive:

- Format coverage for listening matches “whatever VLC plays.”
- Clear degradation: no FFmpeg ⇒ weaker visualizers; no VLC ⇒ Fake + setup help.
- Aligns with existing user installs and Python ADR-0005 external tooling policy for v1.

Negative:

- libVLC discovery/bindings on Windows remain a Phase 2 risk.
- Two external media dependencies to document and doctor-check.

## Alternatives considered

- Symphonia + cpal as default playback — rejected for v1 parity (reduced format set).
- FFmpeg as playback engine in v1 — rejected; confuses analysis vs listen roles.
