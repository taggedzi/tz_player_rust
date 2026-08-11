# Distribution licensing

This is an engineering compliance policy, not legal advice.

## Current conclusion

Project-authored source is MIT-licensed. Rust dependencies retain their own
terms; the locked all-feature/all-target graph passes the reviewed
`cargo-deny` allowlist. `THIRD_PARTY_LICENSES.html` records exact crate versions,
license texts, and source links, including Apache-2.0, MPL-2.0, BSD, ISC,
Unicode, Zlib, and the WTFPL expression declared by `ffmpeg-next` /
`ffmpeg-sys-next` 8.1.0.

Release packages also dynamically link `tz-audio-decoder` to a project-built,
minimal LGPL FFmpeg 7.1.5 runtime (`avcodec`, `avformat`, `avutil`, and
`swresample`). It is configured without GPL/nonfree code and without external
libraries, programs, network, protocols, devices, filters, encoders, or muxers.
The package includes the LGPL text, exact build/configuration/component records,
patch record, source offer, and recognizable shared-library names.

Every binary release must publish the verified FFmpeg 7.1.5 source archive,
the exact `ffmpeg-7.1.5-tz-player.patch`, and both `.sha256` files beside the
binary archive. See `FFMPEG_SOURCE.md` and `native/ffmpeg/manifest.toml`. Do not distribute a bare player/helper binary or
substitute a third-party prebuilt FFmpeg.

Linux binaries use the recipient's normal system audio runtime (typically
ALSA through CPAL). System drivers/frameworks are not bundled. SQLite core is
compiled through `rusqlite`'s bundled feature and is published upstream as
public-domain code.

## Automated checks

```powershell
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-about --version 0.9.1 --locked --features cli
./scripts/check-distribution-licenses.ps1
```

The gate checks the workspace/all-feature dependency graph across supported
desktop targets, rejects unapproved licenses and forbidden FFmpeg flags,
regenerates and compares the Rust notice report, scans required notices, and
validates source/build metadata. A green result confirms policy consistency;
it does not relicense third-party code or replace a maintainer/legal review.

When dependencies change, regenerate and inspect the HTML diff:

```powershell
cargo about generate --locked --workspace --all-features `
  --manifest-path Cargo.toml about.hbs `
  --output-file THIRD_PARTY_LICENSES.html
./scripts/check-distribution-licenses.ps1
```

## Release format

```powershell
./scripts/package-release.ps1
./scripts/test-staged-package.ps1 -Archive target/dist/<package>
```

The package contains the player, helper, four versioned FFmpeg libraries,
`LICENSE`, `THIRD_PARTY_LICENSES.html`, `NATIVE_DEPENDENCIES.md`,
`FFMPEG_SOURCE.md`, `licenses/LGPL-2.1-or-later.txt`, and the generated FFmpeg
build/component/configuration/change records. The packager emits checksums for
the binary package, matching source archive, and standalone patch. Distribute
those assets as one release set.
