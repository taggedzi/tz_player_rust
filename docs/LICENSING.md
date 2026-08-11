# Distribution licensing

This document records the project's dependency-license policy and the release
procedure that preserves third-party terms. It is an engineering compliance
check, not legal advice.

## Current conclusion

The source written for `tz-player` is MIT-licensed. The dependency graph locked
in `Cargo.lock` passes `cargo-deny`'s allowlist for the release build. A compiled
binary is not, however, composed exclusively of MIT code: third-party
components retain their own licenses.

The main non-MIT-only cases in the current release graph are:

- `cpal` and `hound`, licensed under Apache-2.0;
- `option-ext` and the embedded Symphonia codec crates, licensed under MPL-2.0;
- a small number of permissive BSD, ISC, Unicode, and Zlib terms; and
- the bundled SQLite core, which SQLite publishes as public-domain code.

This combination allows `tz-player`'s original code and the larger executable
to remain under MIT. Distribution must also preserve the third-party terms.
In particular, MPL-2.0 permits an executable larger work under other terms but
requires recipients to be told where the MPL-covered source can be obtained.
`THIRD_PARTY_LICENSES.html` includes the selected license texts and exact,
version-specific source links for that purpose.

The Linux executable dynamically links the recipient's system ALSA library,
and the default backend dynamically loads a separately installed libVLC. The
archive therefore carries `NATIVE_DEPENDENCIES.md` and an LGPL-2.1 license copy
while not bundling either library. FFmpeg is invoked as a separate executable
and is not included. Anyone who changes the package to bundle ALSA, VLC,
FFmpeg, codecs, or plugins must audit those exact builds separately.

## Automated checks

Install the same tools pinned in CI:

```powershell
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-about --version 0.9.1 --locked --features cli
```

Run the distribution audit:

```powershell
./scripts/check-distribution-licenses.ps1
```

The check:

1. rejects license expressions outside `deny.toml`;
2. regenerates the report from `Cargo.lock` for x86-64 and ARM64 Windows,
   Linux, and macOS targets and fails if `THIRD_PARTY_LICENSES.html` is stale;
3. fails if a shipped dependency adds a separate `NOTICE` file requiring
   manual preservation review; and
4. requires the native-dependency notice and LGPL license copy used by Linux
   ALSA and the optional libVLC runtime boundary.

When dependencies change, regenerate and review the report:

```powershell
cargo about generate --locked `
  --manifest-path crates/tz-player/Cargo.toml `
  about.hbs `
  --output-file THIRD_PARTY_LICENSES.html
./scripts/check-distribution-licenses.ps1
```

Do not treat a green automated check as permission to relabel third-party code
as MIT. It means the declared terms match the reviewed policy and the required
notice bundle is current. Review any new license, native library, media asset,
font, codec, or separate `NOTICE` file before release.

## Release format

Create a release archive with:

```powershell
./scripts/package-release.ps1
```

For cross-compilation, pass the Cargo target explicitly, for example
`-Target x86_64-pc-windows-msvc`. The archive contains the executable plus:

- `LICENSE` for `tz-player`'s MIT-licensed code;
- `THIRD_PARTY_LICENSES.html` for embedded Rust components;
- `NATIVE_DEPENDENCIES.md` and `licenses/LGPL-2.1.txt` for native/runtime
  boundaries; and
- `README.md` with runtime dependency and usage information.

Distribute the archive as a unit. Do not publish the executable by itself.
