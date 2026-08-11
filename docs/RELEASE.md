# Release checklist (tz-player Rust)

## Preconditions

- [ ] Tree contains only intentional release changes.
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo audit`
- [ ] `cargo deny --locked --workspace --all-features check advisories bans licenses sources`
- [ ] `./scripts/check-distribution-licenses.ps1`
- [ ] No security exception in `docs/SECURITY.md` is expired.
- [ ] Exact native helper tests pass against the audited SDK:
      `cargo test -p tz-audio-decoder --features ffmpeg-native --locked`.
- [ ] Theme, mouse, state restore, playlist editing, and malformed-media
      regression smokes pass.

## Build the native SDK and package

The pinned source identity is FFmpeg 7.1.5 with SHA-256
`de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f`.
Prerequisites are in `native/ffmpeg/README.md`.

```powershell
# Windows
./native/ffmpeg/build.ps1

# Linux/macOS
./native/ffmpeg/build.sh

# All targets, after the target-local SDK exists
./scripts/package-release.ps1
```

The packager builds player/helper with `--release --locked`, audits dynamic
dependencies, validates helper capabilities against the manifest, stages all
licenses/source metadata, and emits:

- `target/dist/tz-player-<version>-<target>.(zip|tar.gz)` and `.sha256`;
- `target/dist/ffmpeg-7.1.5.tar.xz` and `.sha256`;
- `target/dist/ffmpeg-7.1.5-tz-player.patch` and `.sha256`.

Never publish a player-only archive. Put the matching source archive, patch,
and their checksums beside every binary package.

## Clean-package acceptance

```powershell
./scripts/test-staged-package.ps1 -Archive target/dist/<binary-package>
```

The archive-level smoke extracts under a Unicode/space path, runs the packaged
helper and hardware-independent doctor, proves fake `ffmpeg`/VLC commands on
`PATH` are ignored, removes each FFmpeg library and metadata file in turn to
prove fail-closed behavior, restores them, and reruns doctor.

For every supported release target, also verify from the extracted archive:

- [ ] TUI launches and exits cleanly.
- [ ] One native fixture plays, seeks, stops, and naturally advances.
- [ ] One helper-only fixture plays, seeks, stops, and naturally advances.
- [ ] Both fixtures create envelope/spectrum/beat/waveform cache products.
- [ ] Repeat, shuffle, pause/resume, volume, speed, replacement, and shutdown.
- [ ] Native dependency inspection reports only packaged/system libraries.
- [ ] Human audible native and helper-only smoke on representative hardware.

Hosted CI builds and smoke-tests Windows x86-64, Linux x86-64, and macOS ARM64.
A target is supported only after its CI package and human audible checks are
recorded; otherwise label it unverified in release notes.

## End-user runtime

Users need the operating system's normal audio runtime and a color terminal.
They do not install VLC or FFmpeg. The `audio/`, `licenses/`, notice, and source
metadata files are part of the application package and must remain intact.
Run `tz-player doctor` after extraction.

Linux source builds need ALSA development files such as `libasound2-dev`; this
is a build requirement, not an extra codec installation.

## Version, publish, and rollback

1. Bump the workspace/player version and update migration results/package hashes.
2. Commit intentional release changes and tag `vX.Y.Z`.
3. Attach every binary archive/checksum and matching FFmpeg source/checksum.
4. Include the exact source/configuration/license statement and audible-smoke
   target matrix in release notes.

Rust data uses the separate `tz-player-rs` identity; exact paths come from
`tz-player paths`. Roll back with a previous complete package. SQLite schema is
version 8, so do not mix it with the Python application's database.

Known product limitations: Fake emits no audio; speed changes pitch; detailed
analysis is cache-based; future headless/multi-process control remains deferred.
