# Next Major Goal: Expand Playback Compatibility with FFmpeg-Backed PCM Streaming

## Goal

Broaden playback support beyond the formats handled by the in-process native
decoder. When a local media file contains an audio stream that the native route
rejects but the packaged FFmpeg helper can decode, the application should be
able to play it and drive the visualizers through the same normalized PCM
pipeline.

This is a major compatibility goal for the next phase of the project. It is
intended to make the player accept more real-world audio libraries without
adding a separate playback implementation for every codec or container.

The current supported-format policy is documented in
[`usage.md`](usage.md). The implementation should build on the existing
package-relative `tz-audio-decoder` helper rather than requiring a system
FFmpeg installation.

## Proposed architecture

```text
local media file
        |
        v
bounded probe and route selection
        |
        +--> native decoder ------------------+
        |                                     |
        +--> packaged FFmpeg helper ----------+--> normalized PCM
                                              |
                              +---------------+---------------+
                              |                               |
                         playback output                 visualizers /
                                                         analysis caches
```

The FFmpeg route should produce normalized raw PCM, preferably interleaved
`f32le` or `s16le` with an explicit sample rate and channel count. A WAV
container is not required between the decoder and the application: the PCM
format is already described by the decode response and avoids container-header
and streaming complications.

The decoder output should be streamed through a bounded buffer or ring buffer.
The application should not decode an entire long track into an in-memory WAV
before playback starts. For reference, three minutes of stereo 48 kHz `f32`
PCM is approximately 66 MiB, while one hour is approximately 1.65 GiB.

## Scope

The first implementation should cover:

- routing content to the helper when native probing or decoding rejects it;
- streaming helper PCM into the active playback backend;
- exposing the same PCM stream to live levels and visualizers;
- preserving duration, metadata, natural-end, pause, resume, and stop behavior;
- bounded buffering and cancellation when a track is replaced or stopped;
- seeking by restarting or repositioning the helper with a bounded seek request;
- clear diagnostics when the helper cannot decode the file or exits unexpectedly;
- tests for representative helper-only formats, malformed files, long streams,
  cancellation, and repeated track transitions.

## Recommended phases

1. **Define the shared PCM contract.** Document sample format, sample rate,
   channel layout, frame boundaries, duration, and end-of-stream behavior.
2. **Connect helper output to playback.** Add a playback source backed by the
   helper stream and keep native decoding as the fast path for native formats.
3. **Fan out decoded PCM.** Feed playback and visualization/analysis consumers
   without decoding the file twice. Use bounded queues and define behavior when
   a consumer falls behind.
4. **Add transport controls.** Implement seek, pause/resume, stop, natural end,
   repeat, and track replacement under helper-backed playback.
5. **Harden and measure.** Add resource limits, process cleanup, format-matrix
   tests, latency measurements, and package smoke tests on every supported
   platform.

## Design constraints

- The packaged helper remains the runtime dependency; system FFmpeg is not
  required for normal playback.
- FFmpeg processes must have bounded input/output work, wall-clock limits, and
  reliable cancellation and reaping.
- Playback must not silently switch to the fake backend after a per-track
  decode failure.
- The native route remains preferred for formats it handles well.
- The feature is for local files; network streams and streaming-service
  protocols remain out of scope.
- The application should prefer streaming PCM over whole-track memory caching.
  A disk-backed cache may be considered later for repeated analysis, but is not
  required for the first playback implementation.

## Completion criteria

This goal is complete when a packaged build can play a representative set of
helper-only formats through the normal Audio backend, update the visualizers
from the decoded stream, seek and stop without orphaning decoder processes,
remain within documented memory/resource limits, and pass the workspace and
distribution test gates.

