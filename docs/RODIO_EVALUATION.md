# Rodio Backend Compatibility Evaluation

Date: 2026-08-10
Decision owner: repository maintainer
Implementation baseline: `b876731`

## Recommendation

**Keep Rodio available as an experimental opt-in backend. Keep VLC as the
default.**

Rodio met the implemented playback contract on the evaluated Windows system,
passed the common-format matrix, recovered from rejected input, and removed the
need for a VLC or FFmpeg runtime for those formats. It should not become the
default yet: VLC still has materially broader codec/container coverage, Rodio
adds 2.71 MiB to the release binary in this build, and audible/output-device
testing on Linux and macOS plus wider real-library testing remain future
evidence. Promotion or VLC removal still requires a separate proposal and ADR.

## Evidence levels

The results deliberately distinguish what each check proves:

1. **Hardware-independent:** committed fixtures decode through the locked
   Rodio/Symphonia stack, report duration, seek in both directions, and exercise
   source-timeline math without an output device.
2. **Muted output-device:** the real Rodio worker opened the Windows default
   device, played sources at volume zero, and reported transport/natural-end
   state. This proves device-path behavior without making the test suite noisy.
3. **Explicit file smoke:** the release smoke tool played the generated WAV
   fixture to completion through the default output. The process reported a
   correct 1,000/1,000 ms completion; this evaluation does not claim that a
   human independently confirmed audibility.

## Evaluated environment

| Item | Value |
|------|-------|
| OS | Microsoft Windows `10.0.26200.8875`, x86-64 |
| Rust | `rustc 1.94.1`, MSVC host; workspace MSRV remains 1.89 |
| Rodio stack | Rodio 0.22.2, Symphonia 0.5.5, CPAL 0.17.3 |
| Default output | 2 channels, 48,000 Hz, F32 |
| Fixture generator | FFmpeg 8.0; not used by playback or test execution |

Fixtures are one-second generated 440 Hz mono sources with no third-party
recording. Their commands and provenance are in
`crates/tz-playback/tests/fixtures/README.md`.

## Format results

`cargo test -p tz-playback --test rodio_formats --locked` verified initialization,
duration, sample production, forward seek to 700 ms, and backward seek to 100
ms. A separate muted worker matrix opened the real output device, played every
supported fixture at 4x, and required one natural end at the reported duration.

| Family / fixture | Decode + duration | Seek both ways | Muted output + natural end | Result |
|------------------|-------------------|----------------|----------------------------|--------|
| WAV / PCM | Pass | Pass | Pass | Supported |
| MP3 | Pass | Pass | Pass | Supported |
| FLAC | Pass | Pass | Pass | Supported |
| Ogg Vorbis | Pass | Pass | Pass | Supported |
| AAC in M4A | Pass | Pass | Pass | Supported |
| ALAC in M4A | Pass | Pass | Pass | Supported |
| AIFF / PCM | Pass | Pass | Pass | Supported |
| CAF / PCM | Pass | Pass | Pass | Supported |
| Matroska / FLAC | Pass | Pass | Pass | Supported |
| Ogg Opus | Rejected promptly | N/A | N/A | Unsupported as documented |
| Corrupt non-media | Rejected promptly | N/A | N/A | Error as expected |

After both Ogg Opus and corrupt-input failures, a supported WAV opened and
played successfully; the worker remained usable and did not switch to VLC.

The global playlist also admits formats outside this native set, including WMA,
Monkey's Audio, WavPack, AC-3, DTS, Musepack, TTA, Speex, and MIDI. Those remain
VLC compatibility cases rather than Rodio support claims.

## Transport and playlist results

| Behavior | Evidence | Result |
|----------|----------|--------|
| Start / clean shutdown | Release startup-only smoke plus repeated shutdown test | Pass |
| Play / pause / resume / stop | Silent three-second source through real output | Pass |
| Seek forward / backward | Real output seek to 1,500 ms then 300 ms | Pass |
| Volume | Real worker accepted volume; output matrices ran at zero | Pass |
| Speed 0.5x / 1x / 2x / 4x | Source-time tests at each rate and across repeated rate changes | Pass |
| Source position | Counted before Rodio's rate filter; seek/rate tolerances passed | Pass |
| Natural end | Final position latched to duration exactly once | Pass |
| Repeat Off | Advances until the final item, then remains stopped | Pass |
| Repeat One / All | Replays one / wraps all | Pass |
| Shuffle / Next / Previous | Remains within valid non-current/adjacent items as applicable | Pass |
| Core integration | Real muted Rodio output advanced exactly one playlist item | Pass |
| Unsupported track recovery | Error state cleared by the next supported play | Pass |

The evaluation found and fixed a backend-neutral Repeat Off defect: the final
item previously wrapped to the first and could leave UI state marked Playing.
The corrected logic and regression tests apply to VLC, Rodio, and Fake.

## Platform and setup comparison

| Concern | VLC | Rodio |
|---------|-----|-------|
| Default status | Default | Experimental, explicit `--backend rodio` |
| Windows startup on this system | Pass; VLC 3 discovery/startup smoke passed | Pass; default output reported above |
| Windows file smoke | Generated 1.5 s WAV reached Stopped | Generated 1.0 s WAV reached 1,000/1,000 ms |
| Runtime installation | Complete matching VLC 3 library, core, and plugin tree | No separate codec runtime on Windows/macOS |
| Linux source build | Distribution VLC development/runtime packaging as applicable | ALSA development files, e.g. `libasound2-dev` on Ubuntu |
| Format breadth | Broader plugin-based compatibility | Common native set listed above |
| Playback-rate pitch | VLC pipeline-dependent | Pitch changes with speed |
| Automatic cross-fallback | None | None |

Copying only `libvlc.dll` beside the program is not equivalent to a VLC
installation because `libvlccore` and the matching plugin tree are also needed.
Rodio avoids that deployment requirement for its supported formats.

## Startup, resource, and binary observations

- Five release `rodio_smoke --startup-only` runs took 305.9 ms cold, then
  44.5-47.0 ms warm (45.8 ms warm median).
- A three-second silent WAV smoke completed in 3,130.9 ms and had a sampled
  peak working set of 11,362,304 bytes (10.84 MiB). The Windows CPU-time sample
  was below the measurement's useful resolution, so this is an observation,
  not a CPU benchmark.
- With the same compiler and release profile, pre-Rodio commit `1c81e61`
  produced a 7,435,264-byte (7.09 MiB) `tz-player.exe`. The evaluated binary was
  10,280,960 bytes (9.80 MiB): +2,845,696 bytes (+2.71 MiB, 38.3%).
- Playback streamed from files; the implementation does not allocate whole
  tracks. The committed fixture and malformed-input tests found no hang or
  unbounded retry.

## Cross-platform build evidence

GitHub Actions run
[`31407743446`](https://github.com/taggedzi/tz_player_rust/actions/runs/31407743446)
passed format, check, strict Clippy, and tests on Windows, Ubuntu, and macOS with
the locked dependency set; its dependency audit and policy job also passed.
Linux installs `libasound2-dev` before compiling Rodio. Normal hosted tests do
not probe a physical output device. Those muted device checks are explicitly
opted in with `TZ_PLAYER_RODIO_OUTPUT_TESTS=1` and were run sequentially on the
evaluated Windows system.

## Limitations and promotion criteria

This evaluation did not establish audible output on Linux/macOS, device route
switch recovery, hostile-codec robustness beyond the repository's bounded
fixtures, gapless playback, pitch-preserving rate control, or parity across a
large private music library. CPU and battery behavior also need a dedicated
benchmark rather than the short process observation above.

Before proposing Rodio as the default:

1. obtain audible Linux and macOS output checks on representative devices;
2. test a broader non-private library and document unsupported codec frequency;
3. benchmark idle/playback CPU, memory, and battery against VLC;
4. decide whether narrower formats are acceptable or VLC remains a required
   compatibility option; and
5. publish a separate proposal and ADR. This evaluation does not authorize a
   default change.
