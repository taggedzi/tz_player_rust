# Audio Engine Migration Results

**Evidence date:** 2026-08-11  
**Implementation state:** Windows x86-64 and Linux x86-64 implementation/package paths verified; macOS ARM64 and human-audible acceptance remain open.

## Plan task status

| Plan task | Status | Note |
|---|---|---|
| 1. Decision and baseline | Complete | ADR-0004 is recorded; ADR-0003 is preserved and marked superseded; the historical Rodio baseline records startup/runtime/size evidence and fixture provenance. Pre-migration repository state remains reversible at `a38cdb8f57c1d122b4cc899929e0dda2b7f153a0`. |
| 2. PCM types and protocol | Complete | Shared streaming PCM contract and bounded protocol 1.1 implementation/tests pass. |
| 3. Pinned LGPL FFmpeg build | Complete | Exact 7.1.5 shared-only build automation, allowlists, source/patch verification, and audit metadata are implemented and pass on Windows/Linux. |
| 4. Native decoder helper | Complete | The custom seekable AVIO ownership boundary is isolated in its own module; decode/resample, capabilities, bounded diagnostics, malformed/truncated/no-audio/large-metadata inputs, unreadable and non-UTF-8 Unix paths, and the 20-fixture matrix pass on Windows/Linux. |
| 5. Helper discovery/lifecycle | Complete | Package-relative discovery, exact cached handshake, bounded queue/timeouts/stderr, cancellation, pre/post-header failures, old-PID reap before seek acknowledgement, and final child reap tests pass. |
| 6. Analysis routing | Complete | Native-first/helper-second one-pass analysis creates all four versioned cache products within byte/duration/time limits; preserved version-1 rows miss and rebuild lazily as version 2. Native-WAV DSP parity passes explicit 1 ms/one-quantization-step tolerances. |
| 7. Helper-backed playback | Complete | Nonblocking PCM consumption, underrun/error/EOF separation, transport, seek/replacement, levels, and natural-end tests pass. The opt-in muted real-device/helper-only output test also passes locally on Windows. |
| 8. Composite Audio backend | Complete | Public Audio API/default/state/CLI/doctor migration is complete; `rodio` remains only as a compatibility alias and internal implementation terminology. |
| 9. Packaging/licensing | Complete | Self-contained target-aware layout, dependency closure, notices/source offer, portable hashes, and extracted-package tamper/playback/cache tests are implemented. |
| 10. Parity and VLC removal | Complete with recorded sequencing deviation | VLC code/dependencies/discovery are fully removed and exact searches are clean. The comparison was reconstructed after removal from preserved pre-migration commit `a38cdb8f57c1d122b4cc899929e0dda2b7f153a0`; both engines passed the identical workflow. |
| 11. Cross-platform/final acceptance | Partial | CI/package/docs implementation is complete and local Windows/Linux evidence passes. Hosted macOS ARM64, hosted Linux package execution, per-OS audible smokes, and publication remain open. |

## Delivered architecture

- `Audio` is the default and only real public backend; `Fake` remains for hardware-independent operation. Persisted `vlc`, `rodio`, missing, and unknown values migrate to `audio`; the hidden `rodio` CLI alias resolves to Audio; explicit `vlc` reports that the backend was removed.
- The public playback API is `AudioPlaybackBackend`; its output diagnostics and startup errors are likewise Audio-named. Rodio/CPAL remains an internal output implementation detail.
- Playback and analysis share native-first, bundled-helper-second selection. Production code invokes neither system `ffmpeg` nor VLC and does not search `PATH` for either.
- Native decoding streams through Symphonia. The fallback helper opens a local `std::fs::File`, passes it through seekable custom FFmpeg AVIO, streams stereo `f32le`, and does not enable FFmpeg protocols or networking.
- Helper discovery is package-relative. Its queue targets 500 ms in 32 KiB chunks and is hard-bounded to the smaller of two seconds of PCM and 4 MiB. Capability/startup, PCM-stall, and playback seek/restart acknowledgement timeouts are five seconds; stderr is capped at 64 KiB. Stop, seek, replacement, timeout, error, and drop terminate and reap the child.
- Every decode performs one cached capability handshake and requires the exact helper version, protocol major, FFmpeg release/commit, library ABI majors, demuxer/decoder allowlists, and packaged configuration hash. Windows child processes use `CREATE_NO_WINDOW`.
- Offline analysis performs one decode for the envelope, spectrum, beat, and waveform products. Their analysis versions were bumped from 1 to 2 for lazy rebuilding.
- VLC implementation files, dynamic loading, discovery, packaging, and runtime dependencies were removed.

## Native runtime identity

| Item | Verified Windows value |
|---|---|
| FFmpeg source/release/commit | 7.1.5 |
| Source SHA-256 | `de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f` |
| Audited patch | `patches/0001-speex-frame-size.patch` |
| Patch SHA-256 | `eea0d6b95d92e84e70884dad960759bc73a4c6bca9052908190e8bf9d10bcd75` |
| Rust bindings | `ffmpeg-next` / `ffmpeg-sys-next` 8.1.0 |
| Helper protocol | 1.1 |
| Helper configuration hash | Windows `cf71db2da635edbe`; Linux `f213877da659305e` |
| Shared libraries | `avcodec-61`, `avformat-61`, `avutil-59`, `swresample-5` |
| License profile | Shared-only LGPL; GPL, nonfree, programs, network, protocols, devices, filters, encoders, muxers, and automatic external dependencies disabled |

The exact component allowlist and build switches are in
[`native/ffmpeg/manifest.toml`](../native/ffmpeg/manifest.toml). The release
archive includes the generated build manifest, component manifest, change
record, notices, source offer, and required shared libraries.

## Format evidence

The exact native helper was invoked on Windows and Linux for all 20 committed fixtures. Every entry decoded finite, frame-aligned PCM and passed a 100 ms seek with no more than one output frame of error:

- Native policy matrix: AAC/M4A, ALAC/M4A, Ogg Vorbis, FLAC, MP3, WAV/PCM, AIFF, CAF/PCM, and Matroska audio.
- Helper-required matrix: Ogg Opus, WMA/ASF, Monkey's Audio, WavPack, AC-3, E-AC-3, DTS, Musepack 7, Musepack 8, TTA, and Speex.

Fixture generation, provenance, license, route, duration, and SHA-256 values are recorded in
[`crates/tz-playback/tests/fixtures/README.md`](../crates/tz-playback/tests/fixtures/README.md).

Automated mixer tests cover helper-backed playback behavior, including pause/resume, seek/replacement, speed and volume state, levels, child cleanup, stalls/errors, recovery, underrun versus EOF, and exactly-once natural end. Fresh extracted Windows and Linux packages drive all nine native-policy and all eleven helper-required fixtures through the paced hardware-independent mixer. Every fixture reports duration, seeks, reaches natural completion, and creates envelope, spectrum, beat, and waveform caches; both package runs reported zero helper underruns. Human-audible coverage remains a separate final-acceptance gate.

The pre-migration direct-WAV DSP path and the new native route were compared on
a generated sine fixture. Duration differs by no more than 1 ms, decoded samples
and envelope points by no more than one 16-bit quantization step, and the
spectrum, beat, and waveform frame outputs match. The opt-in real-output test
also opened the local Windows default device, played the helper-only Opus
fixture muted at 4x, and reached natural end.

## Reconstructed VLC comparison

The pre-migration tree was materialized from preserved commit
`a38cdb8f57c1d122b4cc899929e0dda2b7f153a0` without modifying the final
worktree. One identical harness then exercised installed VLC 3 and final Audio
against native `tone.wav` and helper-policy `tone-opus.ogg`. Both runs passed:

- all four cache products for both fixtures;
- duration and bounded seek convergence;
- pause/resume, volume, speed, and explicit stop;
- native-to-Opus natural next exactly once;
- Repeat One and Repeat All natural-end behavior;
- shuffle next/previous with the only alternate item; and
- clean backend shutdown.

Observed implementation differences were timing-only: VLC seek position can
converge asynchronously after command acknowledgement, while Rodio's rate
converter can consume a bounded buffered span at the old speed after a live
rate change. The comparison therefore allowed up to two seconds for seek
convergence and reopened the fallback track at 1x before measuring seek. Both
engines met the same final position tolerance. This comparison was reconstructed
after VLC deletion rather than executed before it, which is retained as a
process-sequencing deviation rather than an untested behavior gap.

## Package and target matrix

| Target | Package/build | Extracted-package smoke | Human audible native + helper | Status |
|---|---|---|---|---|
| Windows x86-64 MSVC | Pass locally | Pass locally, including mixer/cache smoke | Not recorded | Provisionally verified; audible gate open |
| Linux x86-64 | Pass locally at Rust 1.89 | Pass locally, including mixer/cache and pseudo-terminal TUI launch/quit | Not recorded | Provisionally verified; CI/audible gates open |
| macOS ARM64 | Manual-only CI job implemented (`macos-15`, `run_macos`) | Not executed in this evidence run | Not recorded | Not yet supported |

Prepared artifacts:

- `target/dist/tz-player-0.1.0-x86_64-pc-windows-msvc.zip`  
  SHA-256 `42a68243e215c160dcf6c06d9c14325ab70bef33961eb65e9e0e564fc7db0665`
- `target/dist/tz-player-0.1.0-x86_64-unknown-linux-gnu.tar.gz`  
  SHA-256 `e4f3f7484c6e26a986157fada33f0a59745d1787653b9d56c7a9598110330702`
- `target/dist/ffmpeg-7.1.5.tar.xz`  
  SHA-256 `de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f`
- `target/dist/ffmpeg-7.1.5-tz-player.patch`  
  SHA-256 `eea0d6b95d92e84e70884dad960759bc73a4c6bca9052908190e8bf9d10bcd75`

The Windows archive was extracted beneath a path containing spaces and Unicode. Its helper capability check, `--backend fake doctor`, all-20-format packaged mixer playback, and all four analysis products passed. Fake `ffmpeg` and VLC commands on `PATH` were ignored. Corrupt input retained bounded native and helper failure context. Removing the helper made doctor fail while native playback/analysis continued; restoring it made doctor pass. Removing each FFmpeg library or each required metadata file made the staged check fail, and restoring it made the check pass. Native dependency inspection accepted only the four packaged FFmpeg libraries and expected Windows system libraries.

The Linux archive was staged beneath a path containing spaces and Unicode. It passed helper identity, fake-backend doctor, the all-20-format mixer/cache matrix with zero helper underruns, no-audio dual-context errors, missing-helper native operation, every missing-library and metadata tamper check, and pseudo-terminal TUI launch/quit. The native helper suite also passed a raw non-UTF-8 filename. `ldd` resolved the four FFmpeg libraries from the package through `$ORIGIN`; the remaining dependencies were the expected glibc loader, libc, libm, and libgcc_s. The archive content audit confirmed that no smoke database or other test output was included. The full workspace format/check/Clippy/test gates and native helper tests passed under Rust 1.89.0. This local WSL package was assembled with the same committed layout logic but not by the hosted CI job; hosted CI remains required before declaring the target supported.

The matching source archive and standalone patch/checksums are prepared locally
but have not been uploaded or published; publication requires a separately
authorized release action.

## Quality and policy gates

Passed on Windows and, except for the PowerShell release wrapper/license tools noted below, Linux Rust 1.89:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --all-targets --locked`
- `cargo test -p tz-audio-decoder --features ffmpeg-native --locked`
- `cargo clippy -p tz-audio-decoder --features ffmpeg-native --all-targets --locked -- -D warnings`
- `cargo audit` (one policy-allowed unmaintained warning for transitive `paste 1.0.15`; no failing vulnerability advisory)
- `cargo deny check` (`deny.toml` enables the full optional-feature graph)
- `scripts/check-distribution-licenses.ps1`
- `scripts/package-release.ps1`
- `scripts/test-staged-package.ps1`

Windows ran the committed license, package, native-dependency, and staged-package PowerShell gates. Linux ran the pinned FFmpeg build, full Rust gates, exact helper matrix, dependency inspection, staged doctor/mixer/cache smoke, and packaged TUI launch/quit locally; its committed PowerShell package wrapper remains to be exercised by hosted CI.

Protocol tests cover incompatible major versions, unknown minor fields, oversized/truncated/invalid headers, invalid PCM metadata, request mismatch, diagnostic bounds, timeout/error paths, and capability identity. Real-helper tests add empty/truncated media, no-audio media, legal 1 MiB leading metadata, spaces/Unicode, non-UTF-8 Unix names, and Unix unreadable input. Missing and permission-denied analysis input is rejected before helper launch. Lifecycle tests prove the old helper PID is gone before a replacement seek is acknowledged and that pre-/post-header failures remain bounded and terminal-safe. VLC/system-FFmpeg production-use searches are clean; remaining VLC text is historical or explicitly tests/documents migration and the obsolete CLI error.

## Acceptance criteria

| Criterion | Status | Evidence / remaining work |
|---|---|---|
| AC-01 | Automated pass | Windows and Linux packages are self-contained; all 20 families pass duration, seek, natural end, and four-product cache smoke on both. Human-audible confirmation remains part of AC-09. |
| AC-02 | Pass | Package-relative helper discovery; fake `ffmpeg`/VLC on `PATH` ignored. |
| AC-03 | Automated pass | Mixer/worker transport, timing, levels, underrun/error propagation, replacement, natural-end, and extracted-package native/helper smokes pass. Human audible confirmation remains part of AC-09. |
| AC-04 | Pass | Native/helper analysis routing creates the unchanged four cache products with one decode and byte/duration/time bounds; version-1 rows are preserved and lazily rebuilt as version 2. |
| AC-05 | Pass | Fixed queue/diagnostic bounds, five-second startup/stall/seek timeouts, and child reap paths are implemented and tested. |
| AC-06 | Pass | State and CLI migration tests pass; Audio is the sole documented real backend. |
| AC-07 | Pass with sequencing note | VLC code/dependencies/setup/package assumptions are removed. The identical VLC-versus-final workflow passes using the preserved pre-migration commit; it was reconstructed after deletion rather than run beforehand. |
| AC-08 | Partial | Package contents plus local matching source, standalone patch, and checksums pass. Those source assets have not been published beside a release. |
| AC-09 | Partial | Full local Windows and Linux Rust gates plus clean-package binary smokes pass. Hosted Linux package CI, macOS ARM64, and human-audible OS smokes remain unexecuted. |
| AC-10 | Pass | Permanent user/release/security/licensing/architecture docs describe the composite Audio engine and require no VLC or system FFmpeg installation. Historical documents are marked superseded. |

## Known limitations

- Speed control retains the existing resampling/pitch-shift behavior; pitch preservation is not part of this migration.
- Linux x86-64 and macOS ARM64 are not declared supported until their hosted package jobs and human-audible native/helper smokes are recorded.
- Publishing the release archive and matching FFmpeg source/patch assets is intentionally outside this implementation run.
