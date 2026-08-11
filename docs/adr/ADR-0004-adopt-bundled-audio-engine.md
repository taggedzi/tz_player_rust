# ADR-0004: Adopt the bundled composite audio engine

**Status:** Accepted

**Date:** 2026-08-10

## Decision

The application uses one user-facing `Audio` backend. Rodio/CPAL owns output
and transport, native Symphonia decoding is attempted first, and the packaged
`tz-audio-decoder` helper is used for local formats rejected during native
initialization. The helper is package-relative and dynamically linked to a
pinned, shared-only LGPL FFmpeg build.

The helper uses `ffmpeg-next` 8.1.0 for the supported decode/resample API and
confines `ffmpeg-sys-next` 8.1.0 to custom AVIO and stream-specific seek calls.
Both binding crates declare WTFPL; that permissive license is explicitly
allowed by project policy and reproduced in the generated third-party notice.
The FFmpeg libraries remain separately governed by LGPL-2.1-or-later.

Offline analysis follows the same native-first/helper-second policy. The
analysis cache remains the persistent source for visualizers, and `Fake`
remains available for deterministic tests and no-audio fallback.

## Consequences

The player no longer depends on tools discovered through `PATH`. Packaging
must include the helper, audited native libraries, build/configuration metadata,
licenses, and the matching FFmpeg source offer. Helper process failures are
isolated from the TUI and are reported as bounded, actionable errors.

ADR-0003 remains historical evidence for the Rodio implementation, but its
VLC-default and experimental-backend conclusions are superseded by this ADR.
