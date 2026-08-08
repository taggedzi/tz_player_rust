# Release checklist (tz-player Rust)

Use this when cutting a local or public release of the `tz-player` binary.

## Preconditions

- [ ] On a clean git tree (or only intentional release bumps)
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Manual smoke: `cargo run -p tz-player -- doctor`
- [ ] Manual smoke: add a track, play with VLC, cycle visualizers (`z`), quit, re-open (state restores)

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
.\target\release\tz-player.exe --backend fake
```

## Runtime dependencies (end user)

| Dependency | Required? | Role |
|------------|-----------|------|
| **VLC** (with libVLC) | Yes for real audio | Playback; loaded dynamically at runtime |
| **FFmpeg** on `PATH` | Optional | Offline analysis for spectrum/beat/waveform visualizers |
| Terminal with color | Recommended | Colored visualizers (still usable monochrome) |

Install hints: `tz-player setup` or platform package manager (`winget`, `brew`, distro packages).

## Data locations

Rust build uses identity **`tz-player-rs`** (separate from Python `tz-player`):

| Item | Typical Windows path |
|------|----------------------|
| Database | `%LOCALAPPDATA%\taggedzi\tz-player-rs\data\tz-player.sqlite3` |
| State | `%APPDATA%\taggedzi\tz-player-rs\config\state.json` |
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

- Fake backend is for CI / no-VLC machines; no real audio.
- Analysis is offline/cache-based, not a live PCM oscilloscope (see ADR-0009 intent).
- Headless control server and multi-process appliance remain deferred.

## Rollback

Delete or rename the data/state paths above, or restore a previous binary. Schema is SQLite v7; avoid mixing with Python DB files.
