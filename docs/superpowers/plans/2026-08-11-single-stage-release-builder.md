# Single-Stage Cross-Platform Release Builder Plan

Status: Ready for review
Date: 2026-08-11
Backlog item: `docs/TODO.md` — Tier 3 single-stage release builder

## Goal

Replace the current maintainer sequence of release gates, native SDK setup,
packaging, and archive smoke testing with one documented action.

The implementation has two entry points backed by the same target-local
orchestrator:

- full supported set: dispatch one `release.yml` workflow with a version and
  optional macOS enablement;
- current host: run one PowerShell command with a version for fast local
  iteration.

Both entry points must build the pinned FFmpeg SDK, build the player and native
helper, package all required notices and native metadata, create checksums, run
the archive-level smoke test, and write a machine-readable result plus a short
human summary. The local path must not expose switches that skip packaging,
license validation, or the staged-package smoke test.

## Current state and gaps

The required checks mostly exist, but orchestration is spread across several
places:

- `docs/RELEASE.md` lists the repository gates and three separate build/package
  commands.
- `native/ffmpeg/build.ps1` and `native/ffmpeg/build.sh` build and audit the
  target-native FFmpeg SDK.
- `scripts/package-release.ps1` builds the Rust binaries, inspects native
  dependencies, validates FFmpeg identity/capabilities, stages notices, and
  creates archives/checksums.
- `scripts/test-staged-package.ps1` extracts and tests the finished archive in
  an isolated Unicode/space path and verifies fail-closed behavior.
- `.github/workflows/ci.yml` duplicates the glue needed to install tools, export
  SDK paths, invoke those scripts, locate the archive, smoke it, and upload it.

The main gaps are:

1. no single command owns the whole transaction;
2. SDK and metadata paths are partly fixed and partly supplied through manually
   exported environment variables;
3. the requested release version is not an explicit input or cross-checked
   against Cargo metadata;
4. the supported target inventory is encoded independently in CI, license
   checks, scripts, and prose;
5. CI uploads per-runner artifacts but does not assemble and verify one
   deduplicated, publish-ready release asset set;
6. there is no stable summary contract describing the commit, target, gates,
   archive, checksums, and macOS verification state.

## Decisions and boundaries

### Build natively, orchestrate centrally

FFmpeg and the Rust helper are target-native and the staged executable must run
during smoke testing. Do not introduce cross-compilation. Each target is built
and smoke-tested on its matching runner; the workflow is the single action that
fans out to all enabled targets.

### Use PowerShell as the common target-local entry point

Add `scripts/build-release.ps1`. PowerShell is already used by packaging,
license checks, native inspection, archive smoke tests, and all three CI runner
families. The orchestrator calls the existing Windows or Unix FFmpeg builder
and owns all path/environment setup for its child processes.

Keep the lower-level scripts independently callable for development and
diagnostics, but document only the orchestrator and the workflow as release
entry points.

### Keep platform data in one manifest

Add `scripts/release-targets.json` with one record per supported target. At
minimum, each record contains:

| ID | Rust target | Required host | Runner | Archive | Default release |
|---|---|---|---|---|---|
| `windows-x86_64` | `x86_64-pc-windows-msvc` | Windows x86-64 | `windows-latest` | `.zip` | yes |
| `linux-x86_64` | `x86_64-unknown-linux-gnu` | Linux x86-64 | `ubuntu-latest` | `.tar.gz` | yes |
| `macos-arm64` | `aarch64-apple-darwin` | macOS ARM64 | pinned ARM64 runner label | `.tar.gz` | no, until enabled |

The manifest is the source of truth for host validation, workflow matrix
generation, package suffixes, and distribution-license target scanning. CI
must assert that the resolved `rustc` host equals the manifest triple; runner
labels alone are not evidence of architecture.

### Treat version as validation, not mutation

`-Version X.Y.Z` must be valid SemVer and exactly equal the `tz-player` version
reported by `cargo metadata --locked`. The builder records the selected Git
commit and dirty-tree state but does not edit `Cargo.toml`, commit, tag, publish,
or create a release. The workflow checks out the ref selected at dispatch and
fails before native compilation when the input version does not match.

### Make output transactional and predictable

Use target-scoped work directories under `target/release-work/<rust-target>`.
Write publishable files to a temporary target-scoped dist directory and move
them into place only after the archive smoke passes. A failure must not leave a
new archive that looks publishable.

The final aggregate release asset set in `target/dist` contains only:

- one `tz-player-<version>-<rust-target>.(zip|tar.gz)` per selected target;
- one adjacent `.sha256` per binary archive;
- the single pinned FFmpeg source archive and checksum;
- the single standalone project patch and checksum;
- `SHA256SUMS` covering every publishable asset;
- `release-summary.json` and `RELEASE_SUMMARY.md`.

Temporary extraction, SDK, Cargo, and staging files remain outside
`target/dist`. The aggregate step must reject unexpected files, duplicate
target archives, version mismatches, checksum mismatches, or non-identical
copies of the shared FFmpeg source/patch assets.

## Proposed command contract

The final documented commands should be:

```powershell
# Current host; always runs all gates, package creation, and archive smoke.
pwsh ./scripts/build-release.ps1 -Version 0.1.0 -Target current

# Full enabled target set; macOS is included only when explicitly enabled.
gh workflow run release.yml --ref main -f version=0.1.0 -f include_macos=true
```

The manual Actions form is an equivalent one-action entry point. An explicit
target ID may be accepted locally for diagnostics, but it must match the
current host. Unsupported targets, host/target mismatches, and disabled macOS
selection fail during preflight.

The user-facing command must not offer `SkipBuild`, `SkipLicenseCheck`, or
`SkipSmoke` options. If CI needs to separate a shared gate job from target
jobs, use a private workflow-facing phase/receipt contract that binds the gate
result to the exact commit and lockfile; never make it the documented local
path.

## Proposed commit sequence

### Task 1 — Define the release target and result contracts

**Files:**

- Add `scripts/release-targets.json`
- Add `docs/adr/ADR-0005-single-stage-native-release-builds.md`
- Modify `scripts/check-distribution-licenses.ps1`
- Add focused validation tests under `scripts/tests/`

**Work:**

- Record why supported binary releases are built natively and why the workflow,
  rather than cross-compilation, owns the full target set.
- Define the three target records and macOS's explicit enablement state.
- Define the schema for `release-summary.json`: schema version, product/version,
  commit, dirty state, target ID/triple, host, timestamps/durations, FFmpeg
  version/source/patch hashes, completed gates, archive name/hash/size, smoke
  result, and warnings/manual-audio status.
- Make the license target scan read the same manifest. If broader architectures
  remain useful for policy-only scans, put them in a separately named
  `license_scan_targets` collection so they are not mistaken for supported
  releases.
- Validate unique IDs/triples, known archive types, and a single record for the
  current host.

**Verification:** malformed/duplicate manifest fixtures fail with actionable
messages; the current six-platform license scan remains intentional; each
release target resolves to its expected Rust triple.

**Commit:** `build(release): define supported native target contracts`

### Task 2 — Make native build and packaging paths explicit

**Files:**

- Modify `native/ffmpeg/build.ps1`
- Modify `native/ffmpeg/build.sh`
- Modify `scripts/package-release.ps1`
- Modify `scripts/inspect-native-dependencies.ps1` only if its target inference
  needs to become explicit

**Work:**

- Give both FFmpeg builders an explicit, absolute work/output root while
  preserving their existing default for direct developer use.
- Make the packager accept explicit `Version`, `Target`, `NativeBuildRoot`, and
  `OutputDirectory` inputs.
- Derive FFmpeg prefix, include, library, pkg-config, build metadata, source,
  and patch paths from `NativeBuildRoot`; remove the release path's dependency
  on maintainer-set `TZ_FFMPEG_*`, `FFMPEG_DIR`, and `PKG_CONFIG_PATH` values.
- Scope any loader-path variables to the exact helper test/build child process,
  then clear them before archive smoke testing.
- Fail when the requested version differs from Cargo metadata or when native
  build metadata identifies a different target/source/patch/configuration.
- Preserve the current archive layout, macOS `.app` layout, native dependency
  inspection, component allowlist, corresponding-source assets, and checksum
  format.

**Verification:** package each available host with no pre-set FFmpeg
environment; inject wrong version, wrong target, stale SDK metadata, missing
library, and missing notice failures; verify no child-specific environment
change leaks after the script returns.

**Commit:** `refactor(release): parameterize native SDK and package paths`

### Task 3 — Add comprehensive prerequisite preflight

**Files:**

- Add `scripts/build-release.ps1`
- Add helper code under `scripts/release/` only where it keeps the entry script
  reviewable
- Add/extend tests under `scripts/tests/`

**Work:**

- Resolve `current` through `rustc -vV` plus OS/architecture and require an
  exact manifest match.
- Validate the requested version, clean/dirty policy, Rust 1.89 toolchain,
  `rustfmt`, `clippy`, locked metadata, and target installation.
- Check all prerequisites before starting expensive work and report all missing
  items together, with platform-specific installation guidance from
  `native/ffmpeg/README.md`.
- Check pinned versions of `cargo-audit`, `cargo-deny`, and `cargo-about`.
- Check shared native tools (`git`, `tar`, `make`, `nasm`, Python, libclang,
  `pkg-config`, download/hash support) and platform requirements: MSVC plus
  MSYS2 on Windows, ALSA development headers on Linux, and Xcode command-line
  tools on macOS.
- Do not install or mutate workstation prerequisites. CI remains responsible
  for provisioning its ephemeral runners.
- Print a concise preflight table and exit before changing dist output when
  anything is missing.

**Verification:** command-not-found and wrong-version tests cover each tool
class; preflight on every CI runner reports the exact target triple and output
paths; no source download/build occurs during a failing preflight.

**Commit:** `feat(release): preflight every native release prerequisite`

### Task 4 — Implement the target-local single-stage transaction

**Files:**

- Modify `scripts/build-release.ps1`
- Modify `scripts/test-staged-package.ps1`
- Modify lower-level scripts only for structured result output

**Work:**

- Run, in order: preflight; format/check/clippy/test; audit/deny/license gates;
  native FFmpeg build; exact native-helper tests; helper playback/analysis
  tests; native-helper clippy; release player/helper build; package validation;
  archive creation/checksums; extracted archive smoke.
- Reuse a valid FFmpeg build only when a cache identity covers OS/architecture,
  target triple, FFmpeg manifest, both build scripts, and patch content. Always
  rerun metadata/component/capability validation after a cache hit.
- Extend the smoke script to accept/verify expected version and target, and to
  emit a structured pass record without weakening its isolated PATH and
  fail-closed tamper checks.
- Accumulate phase timings and results into the target summary.
- On success, atomically promote the archive and shared source assets into
  `target/dist`; on failure, remove only the transaction's temporary output and
  retain its logs under `target/release-work/<target>/logs`.
- Ensure `-Target current` follows this exact path. Lower-level diagnostic
  switches such as the packager's existing skips must not be forwarded.

**Verification:** run the command twice to cover clean-build and safe-cache
paths; introduce a smoke failure and prove no new publishable archive appears;
confirm license and smoke failures cannot be bypassed through the entry point.

**Commit:** `feat(release): build and smoke one host release in one command`

### Task 5 — Add the one-action full-target workflow

**Files:**

- Add `.github/workflows/release.yml`
- Modify `.github/dependabot.yml` only if the new workflow adds pinned actions

**Work:**

- Add `workflow_dispatch` inputs: required `version`, optional
  `include_macos`, and no skip-gate inputs.
- Use least privilege (`contents: read`) because this workflow builds assets but
  does not publish them.
- Generate the matrix from `scripts/release-targets.json`: Windows and Linux by
  default; add macOS ARM64 only when its manifest record is enabled and the
  dispatch input requests it.
- Pin every action to an immutable commit SHA.
- Provision the exact platform prerequisites, using caches only as performance
  optimizations. Cache keys include all identity inputs described in Task 4.
- Run the same orchestrator for every matrix target and assert the runner host
  triple before building.
- Upload each tested target result, including its JSON summary, as an internal
  workflow artifact.
- In a final aggregate job, download all target results, require the expected
  target set, deduplicate and compare the shared FFmpeg assets, verify every
  checksum, create `SHA256SUMS` and the combined summaries, reject unexpected
  files, and upload one `tz-player-<version>-release-assets` artifact.
- Make the workflow fail if any target, smoke, aggregation, or upload fails. A
  disabled/skipped macOS target is recorded explicitly, not shown as passed.

**Verification:** dispatch Windows+Linux, then all three targets; test an input
version mismatch; inspect the aggregate artifact from a clean extraction; prove
that removing or changing any matrix artifact makes aggregation fail.

**Commit:** `ci: add one-action cross-platform release builder`

### Task 6 — Reuse the builder from normal CI

**Files:**

- Modify `.github/workflows/ci.yml`
- Modify `scripts/build-release.ps1` only if CI-specific structured logging is
  needed

**Work:**

- Replace the duplicated native SDK/path/package/archive-location/smoke glue in
  `native-package` with the target-local orchestrator or its receipt-bound CI
  phase.
- Keep Windows x86-64 and Linux x86-64 package smokes on normal pushes/PRs.
- Keep macOS ARM64 manual-only until its target record is enabled by the
  project's support decision.
- Preserve dependency-policy and normal workspace matrix coverage. Avoid
  silently dropping any existing exact-helper, helper-only format, analysis
  cache, Clippy, license, or tamper smoke gate.
- Upload only successfully smoke-tested archives and summaries.

**Verification:** compare old and new CI gate inventories line by line; all
existing branch-protection jobs still have an equivalent result; a package
smoke failure blocks artifact upload.

**Commit:** `ci: route native packages through the release orchestrator`

### Task 7 — Document the single action and release-note evidence

**Files:**

- Modify `docs/RELEASE.md`
- Modify `native/ffmpeg/README.md`
- Modify `README.md`
- Modify `docs/usage.md`
- Modify `docs/LICENSING.md`
- Add `.github/RELEASE_NOTES_TEMPLATE.md` or the repository's chosen release
  note template
- Modify `docs/TODO.md` and `docs/PROGRESS.md` after acceptance passes

**Work:**

- Replace the multi-command checklist with the two commands in this plan and a
  description of their identical non-bypassable packaging/license/smoke gates.
- Document exact prerequisites and how preflight reports them, without asking
  maintainers to export intermediate SDK paths.
- Document the target enablement policy: Windows/Linux are normal; macOS ARM64
  is included only when enabled and explicitly requested until promoted.
- Explain the final `target/dist`/aggregate artifact contents and how to verify
  `SHA256SUMS` and `release-summary.json`.
- Make release notes consume the generated target matrix, FFmpeg identity,
  checksums, automated smoke status, manual audible-smoke status, and any
  macOS unsigned/unnotarized limitation rather than relying on memory.
- Mark the TODO complete only after every enabled target has a recorded clean
  workflow run and the current-host path passes independently.

**Verification:** all documented commands are copy/paste tested; search for
stale instructions referring to manual FFmpeg environment variables or the
old three-command release sequence.

**Commit:** `docs: make the single-stage release path canonical`

### Task 8 — Run final acceptance and retain evidence

**Files:**

- Add a dated result under `docs/releases/` or append the release evidence to
  `docs/PROGRESS.md`
- Make any discovered fixes in their owning task before recording acceptance

**Work and verification:**

1. From a clean checkout, dispatch the workflow for Windows x86-64 and Linux
   x86-64 with the current workspace version.
2. Enable and include macOS ARM64, or record it as disabled/unverified without
   claiming it is supported by that release.
3. Download the aggregate artifact, verify `SHA256SUMS`, and confirm its file
   inventory exactly matches the contract above.
4. Extract each archive on its native OS and rerun the packaged doctor and
   package smoke command without build SDK variables or system media tools.
5. Perform and record the human audible native/helper-only smoke matrix from
   `docs/RELEASE.md`; hosted CI evidence must not be described as audible.
6. Run the current-host command from a separate clean checkout and compare its
   archive checksum/summary fields with the corresponding workflow result. Do
   not require byte-for-byte reproducibility until timestamps/toolchain inputs
   are deliberately normalized.
7. Confirm `target/dist` contains no stage directories, SDKs, extraction trees,
   duplicate source assets, logs, or untested archives.

**Commit:** `docs: record single-stage release builder acceptance`

## Completion criteria

The TODO item is complete when all of the following are true:

- one documented local command produces a fully gated current-host release;
- one manual workflow action produces Windows x86-64 and Linux x86-64, plus
  macOS ARM64 whenever that target is enabled and requested;
- version, host, target, native SDK identity, capabilities, package contents,
  licenses, native dependencies, and checksums all fail closed;
- no documented release path requires manual SDK commands, environment
  variables, intermediate paths, archive discovery, or a second smoke command;
- a failed transaction cannot leave a new publishable archive;
- the aggregate artifact contains exactly the selected tested binaries, one
  corresponding-source set, checksums, and summaries;
- CI uses the same target-local implementation rather than maintaining a
  separate release recipe;
- release notes distinguish automated validation, manual audible validation,
  disabled targets, and any signing/notarization limitations.

## Non-goals

- Cross-compiling or smoke-testing a foreign target on the maintainer's host.
- Automatically changing versions, committing, tagging, or pushing.
- Creating or publishing a GitHub Release.
- Introducing code signing, Apple notarization, package-manager installers, or
  reproducible-build guarantees. The summary and release notes must state the
  current status of these controls; they can be added later without changing
  the one-action build contract.
