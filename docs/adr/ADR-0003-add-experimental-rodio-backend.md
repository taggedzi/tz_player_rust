# ADR-0003 — Add an Experimental Rodio Playback Backend

- Status: Accepted
- Date: 2026-08-10
- Design: `docs/superpowers/specs/2026-08-10-rodio-backend-design.md`

## Context

ADR-0001 selected LibVLC as the only real listen-path backend for the initial
Python-to-Rust parity release. That choice preserved VLC's broad format support,
but it also retained discovery, native-library, plugin, installation, and ABI
requirements that are unnecessary for common local music formats.

Rust now has a viable high-level playback stack: Rodio supplies transport and
mixing, Symphonia supplies streaming container/codec decode, and CPAL supplies
cross-platform device output. Its format set is narrower than VLC's, so an
immediate replacement would be an unevidenced compatibility regression.

## Decision

1. Add `RodioPlaybackBackend` inside `tz-playback` as an opt-in experimental
   real-audio backend selected with `--backend rodio`.
2. Keep VLC as the default and Fake as the deterministic test/no-audio fallback.
3. Keep playback orchestration in `tz-core`; no frontend imports Rodio,
   Symphonia, CPAL, VLC, or FFmpeg APIs.
4. Keep FFmpeg/native-WAV analysis separate from every listen-path backend.
5. Enable only Rodio playback, tracing, Symphonia's complete stable
   format/codec set, and stable SIMD features. Do not enable recording.
6. Evaluate format and transport compatibility before proposing any default
   change. Removing VLC requires a separate decision.

This amends only ADR-0001's VLC-only listen-path choice. Its workspace
boundaries and playback/analysis separation remain accepted.

## Consequences

Positive:

- Common local music can play without a separately installed VLC runtime.
- The existing backend trait permits an incremental comparison without a TUI
  rewrite or an all-at-once migration.
- A future decoded-sample tap can support live visualizers without a second
  decoder, although that is outside this implementation.

Negative:

- Symphonia does not cover VLC's complete codec/container set.
- Linux builds acquire CPAL's ALSA development dependency.
- The application temporarily carries two real playback stacks.
- Playback-rate pitch and device-loss behavior can differ between backends and
  must be documented and tested.

## Promotion boundary

`docs/RODIO_EVALUATION.md` must record transport and representative-format
results. Making Rodio the default, narrowing the supported-media policy, or
removing VLC is not authorized by this ADR.
