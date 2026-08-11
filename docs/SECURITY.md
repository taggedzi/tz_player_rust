# Security policy

## Dependency policy

Pull requests run `cargo audit` and
`cargo deny --locked --workspace --all-features check advisories bans licenses sources` against the
committed lockfile. Vulnerabilities, unsound/yanked dependencies, unapproved
licenses, unknown registries, and Git dependencies fail unless an explicit,
time-bounded exception is recorded both here and in `deny.toml`.

## Runtime trust boundaries

The default Audio engine has two parser boundaries:

- Symphonia parses native-route media in the player process. Its exact Rust
  dependencies are pinned by `Cargo.lock`.
- `tz-audio-decoder` parses helper-route media in a child process using the
  four packaged FFmpeg 7.1.5 libraries. The helper is resolved relative to the
  running executable, never through the working directory, `PATH`, or a shell.

The helper opens the requested path with `std::fs::File` and gives FFmpeg only
a seekable custom AVIO context. The release configuration disables network,
protocol discovery, programs, devices, filters, encoders, muxers, GPL, nonfree,
and automatic external dependencies. Stdin is null; stdout is a length-bounded
binary protocol; stderr is fully drained but retained only up to 64 KiB.
Startup, PCM stalls, cancellation, replacement, seeking, analysis time, and
shutdown are bounded, and every child is killed/reaped when required.

Release packages must stay intact. Do not replace the helper or libraries with
system copies. `tz-player doctor` validates the package-relative helper,
protocol/FFmpeg identity, and source/build metadata. Fake `ffmpeg` or VLC files
on `PATH` are intentionally ignored.

## Untrusted media and resource limits

Treat filenames, tags, cover art, codec input, and helper diagnostics as
untrusted. Terminal-facing text has controls and directional formatting escaped.
Resource limits reduce denial-of-service exposure but are not a substitute for
keeping Rust dependencies and packaged FFmpeg builds patched.

| Limit | Default per track | Compiled ceiling |
|---|---:|---:|
| Decoded stereo PCM | 256 MiB | 1 GiB |
| Media duration | 1 hour | 6 hours |
| Decode wall time | 2 minutes | 15 minutes |
| Helper PCM queue | min(2 seconds, 4 MiB) | fixed |
| Helper diagnostic retention | 64 KiB | fixed |

The environment variables `TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES`,
`TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS`, and
`TZ_PLAYER_ANALYSIS_TIMEOUT_SECS` can change analysis defaults but not compiled
ceilings. Cover handling separately caps one picture at 8 MiB, all pictures in
a tag at 16 MiB, cumulative cover metadata at 32 MiB, and decoded artwork at
4096x4096 / 32 MiB. Use OS-level isolation for deliberately hostile files.

## Temporary dependency exceptions

Each exception needs an owner, rationale, compensating controls, and an expiry
within 90 days.

### RUSTSEC-2024-0436 (`paste` 1.0.15)

- **Status:** accepted, time-bounded technical risk
- **Owner:** Matthew Craig (repository maintainer)
- **Accepted:** 2026-08-10
- **Expires:** 2026-11-08
- **Dependency path:** `tz-player` -> `lofty` 0.25.0 -> `paste` 1.0.15
- **Reason:** Lofty still requires `paste`; Ratatui's former path was removed.
- **Risk:** the advisory reports unmaintained status, not a known vulnerability;
  `paste` is a build-time procedural macro.
- **Controls:** lockfile pinning, CI audits, explicit deny configuration, and
  release review.
- **Exit:** upgrade/replace Lofty or its macro dependency before expiry, or
  record a new review and deadline.
