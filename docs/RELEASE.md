# Release checklist (tz-player Rust)

Use this when cutting a local or public release of the `tz-player` binary.

## Preconditions

- [ ] On a clean git tree (or only intentional release bumps)
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo audit` (every warning is absent or matches an unexpired exception
      in `docs/SECURITY.md` and `deny.toml`)
- [ ] `cargo deny --locked check advisories bans licenses sources`
- [ ] No temporary security exception is past its documented expiration date
- [ ] Malicious-media regression tests pass:
      `cargo test -p tz-analysis decode::tests`,
      `cargo test -p tz-core metadata::tests`, and
      `cargo test -p tz-tui cover_ascii::tests`
- [ ] Manual smoke: `cargo run -p tz-player -- doctor`
- [ ] Windows VLC loader smoke (no audio):
      `cargo run -p tz-playback --example vlc_smoke -- --startup-only`
- [ ] Rodio format matrix:
      `cargo test -p tz-playback --test rodio_formats --locked`
- [ ] Rodio output smoke (no audio):
      `cargo run -p tz-playback --example rodio_smoke -- --startup-only`
- [ ] Rodio output-device tests (muted; requires a usable default device):
      `$env:TZ_PLAYER_RODIO_OUTPUT_TESTS='1'; cargo test -p tz-playback rodio::tests --locked -- --test-threads=1; cargo test -p tz-core rodio_real_output --locked`
- [ ] Manual smoke: add a track, play with VLC, cycle visualizers (`z`), quit, re-open (state restores)
- [ ] Theme smoke: copy `docs/theme.example.json` to the `theme.json` path
      reported by `tz-player paths`, verify palette/formatting, then remove it
- [ ] Mouse smoke: click/select and double-click/play a row, wheel both main
      and editor lists, drag time/volume/speed, then repeat via keyboard
- [ ] Manual Rodio smoke with an explicit supported local file; verify pause,
      forward/backward seek, 0.5x/1x/2x/4x, live visualizer response, stop, and
      natural next-track advance

## Build

```powershell
cargo build --release -p tz-player
```

Artifact:

| OS | Path |
|----|------|
| Windows | `target/release/tz-player.exe` |
| Unix | `target/release/tz-player` |

Optional smoke with the release binary:

```powershell
.\target\release\tz-player.exe doctor
.\target\release\tz-player.exe --backend rodio doctor
.\target\release\tz-player.exe --backend fake
```

## Runtime dependencies (end user)

| Dependency | Required? | Role |
|------------|-----------|------|
| **VLC 3.x** (complete libVLC install) | Default real backend | Playback; library, core, and plugins are loaded dynamically; other majors fail closed |
| **Rodio/Symphonia/CPAL** | Built into release | Experimental real backend; uses the default OS output device and its documented format set |
| **FFmpeg** on `PATH` | Optional | First matching executable runs for offline spectrum/beat/waveform analysis |
| Terminal with color | Recommended | Colored visualizers (still usable monochrome) |

Linux source/CI builds need ALSA development files (for example,
`libasound2-dev` on Ubuntu); end users of a built binary need the normal system
audio runtime. Windows and macOS need no separate Rodio codec installation.

Install VLC and FFmpeg only from a trusted package manager or verified
distribution channel. Do not release with an unexpected FFmpeg path,
`VLC_PLUGIN_PATH`, LibVLC path, or VLC major. Run `tz-player doctor` on each
target OS for VLC and `tz-player --backend rodio doctor` for Rodio. Record the
discovered VLC location, Rodio output configuration, FFmpeg availability, and
whether playback was audible or startup-only. Separately record the package
versions and the first `ffmpeg` on `PATH` in the release notes. See
`docs/SECURITY.md` for the complete trust model and analysis/cover-art limits.

## Data locations

Rust build uses identity **`tz-player-rs`** (separate from Python `tz-player`):

| Item | Typical Windows path |
|------|----------------------|
| Database | `%LOCALAPPDATA%\taggedzi\tz-player-rs\data\tz-player.sqlite3` |
| State | `%APPDATA%\taggedzi\tz-player-rs\config\state.json` |
| Optional TUI theme | `%APPDATA%\taggedzi\tz-player-rs\config\theme.json` |
| Logs | under data dir `logs/` |

Exact paths: `tz-player paths`.

## Version

Binary version comes from `crates/tz-player/Cargo.toml` (`--version` / doctor banner).

Bump that crate version (and workspace if desired) before tagging.

## Tag / publish (optional)

1. Update `docs/PROGRESS.md` status line if needed.
2. Commit release notes / version bump.
3. Tag `vX.Y.Z` and push.
4. Attach `tz-player` release binary + short install notes (VLC + optional FFmpeg).

## Known limitations (document in release notes)

- Fake backend is for CI and no-audio diagnostics; it never produces real audio.
- Rodio is experimental, has a narrower format set than VLC, and changes pitch
  with playback speed.
- Analysis is offline/cache-based, not a live PCM oscilloscope (see ADR-0009 intent).
- Headless control server and multi-process appliance remain deferred.

## Rollback

Delete or rename the data/state paths above, or restore a previous binary. Schema is SQLite v8; avoid mixing with Python DB files.
