# Security policy

## Dependency policy

Pull requests run both `cargo audit` and
`cargo deny --locked check advisories bans licenses sources` against the
committed `Cargo.lock`. Known vulnerabilities, unsound advisories, yanked
crates, unmaintained crates, unapproved licenses, unknown registries, and Git
dependencies fail the policy unless they have an explicit exception in
`deny.toml`.

An exception in `deny.toml` is not sufficient on its own. It must have a
matching entry below with an owner and expiration date. Review both files
together whenever an exception is added, renewed, or removed.

## Runtime trust boundaries

### Playback and analysis code

The selected backend changes which trusted code parses playback media:

- **FFmpeg is executable code selected through `PATH`.** For non-WAV offline
  analysis, the first `ffmpeg` executable found through the process environment
  is spawned with the media path as an argument. FFmpeg is not used for
  listening. A malicious or accidentally shadowed executable on `PATH` runs
  with the same user permissions as `tz-player`.
- **LibVLC and its plugins are dynamically loaded.** The player discovers a
  normal VLC installation in the platform's common system locations and loads
  its shared library into the process. On Windows, `VLC_PLUGIN_PATH` selects
  the plugin directory; a pre-existing value is respected. Only the VLC 3.x
  ABI is supported. Other majors and unparseable versions are rejected before
  ABI-specific symbols are resolved.
- **Rodio, Symphonia, and CPAL are Rust dependencies compiled into the
  application.** They do not discover VLC or FFmpeg when Rodio is selected.
  Symphonia parses supported playback containers/codecs in the player process,
  and CPAL opens the operating system's default output device. Their versions
  are pinned by `Cargo.lock` and covered by the dependency policy above.

Install VLC 3.x and FFmpeg only from the operating-system package manager or
another source whose binaries and update channel you trust. Do not put
user-writable download directories ahead of trusted system directories on
`PATH`, and do not point `VLC_PLUGIN_PATH` at an untrusted directory. Run
`tz-player --backend <name> doctor` after installation or environment changes
to check only the selected playback backend plus FFmpeg availability.
Separately inspect the first `ffmpeg` on `PATH` and the installed VLC/FFmpeg
versions before playing media. Copying only `libvlc.dll` beside the executable
is not a supported deployment: libVLC also requires its matching core library
and plugin tree, and mismatching those files expands the loader/parser risk.

### Untrusted media

Treat filenames, tags, cover art, WAV data, and codec input as untrusted.
Metadata and native WAV parsing occur in the Rust process; LibVLC parses
playback media in-process when VLC is selected; Symphonia parses playback media
in-process when Rodio is selected; FFmpeg parses analysis media in a child
process. Resource limits reduce denial-of-service exposure, but they do not
make an outdated native codec or parser safe. Keep VLC, FFmpeg, and Rust
dependencies patched, and use OS-level isolation when examining deliberately
hostile files.

Offline analysis streams input under these limits:

| Limit | Default per track | Compiled ceiling |
|-------|-------------------|------------------|
| Decoded stereo PCM | 256 MiB | 1 GiB |
| Media duration | 1 hour | 6 hours |
| Decode wall time | 2 minutes | 15 minutes |

The environment variables `TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES`,
`TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS`, and
`TZ_PLAYER_ANALYSIS_TIMEOUT_SECS` may change the defaults but cannot exceed the
compiled ceilings. FFmpeg receives null stdin; a limit or timeout kills and
reaps the process. Cover parsing separately caps one picture at 8 MiB, all
pictures in a tag at 16 MiB, cumulative cover-metadata reads at 32 MiB, and a
decoded image at 4096x4096 / 32 MiB. Plain terminal output visibly escapes
terminal controls and directional formatting characters.

The release procedure in `docs/RELEASE.md` runs dependency audits and the
malformed/oversized-media regression tests before packaging.

## Temporary dependency exceptions

Exceptions are a last resort. Each exception must name an owner, explain why
the dependency remains, state the compensating controls, and expire within 90
days. The owner must remove, replace, or explicitly renew the exception before
its expiration date.

### RUSTSEC-2024-0436 (`paste` 1.0.15)

- **Status:** accepted, time-bounded technical risk
- **Owner:** Matthew Craig (repository maintainer)
- **Accepted:** 2026-08-10
- **Expires:** 2026-11-08
- **Dependency path:** `tz-player` -> `lofty` 0.25.0 -> `paste` 1.0.15
- **Reason:** the current Lofty release still has a mandatory dependency on
  `paste`. Ratatui was upgraded to 0.30.2, which removed its separate `paste`
  dependency, and Lofty was upgraded to 0.25.0, so this is the only remaining
  path.
- **Risk assessment:** RUSTSEC-2024-0436 reports that `paste` is unmaintained;
  it does not report a known vulnerability. `paste` is a build-time procedural
  macro and does not execute in the media-player process at runtime.
- **Compensating controls:** `Cargo.lock` pins `paste` to 1.0.15; CI audits the
  lockfile; dependency-policy configuration identifies this exact advisory;
  upgrades are reviewed before release.
- **Exit criteria:** upgrade Lofty to a release that removes `paste`, replace
  Lofty, or vendor a reviewed minimal alternative. Do not release after the
  expiration date without recording a new review and a new deadline.
