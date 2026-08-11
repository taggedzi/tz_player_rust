# Pinned FFmpeg SDK

This directory describes the exact LGPL, shared-only FFmpeg build used by
`tz-audio-decoder`. It does not contain generated binaries. The source archive
hash is pinned to the official release archive; the build scripts refuse a
mismatch.

The build is deliberately separate from normal workspace compilation because
the helper’s native SDK is target-specific. Outputs are staged under a local
`build/` directory and include `FFMPEG_BUILD.json`, `FFMPEG_COMPONENTS.json`,
`FFMPEG_CONFIGURE.log`, and `FFMPEG_CHANGES.diff` for package review. Both
scripts stop before compilation if the generated decoder, demuxer, parser, or
bitstream-filter set differs from the manifest allowlist.

Both scripts apply `patches/0001-speex-frame-size.patch` exactly once before
configuration. This audited upstream correction prevents FFmpeg 7.1.5's
native Speex decoder from doubling wideband frame sizes. The patch is copied
verbatim to `FFMPEG_CHANGES.diff` for package and corresponding-source review.

On Windows, run this script from an MSYS2/Visual Studio environment containing
`bash`, `make`, `nasm`, `libclang`, and the target C toolchain. The script also detects NASM
under `%LOCALAPPDATA%\bin\NASM`, which is the normal per-user install used by
this project. On Linux/macOS, use the native compiler plus `make`, `nasm`,
`libclang`, `pkg-config`, `curl`, and Python 3.8 or newer. The installed
prefix must be supplied to the Rust helper build as `FFMPEG_DIR` (the release
packager maps this from `TZ_FFMPEG_PREFIX`); its `lib/pkgconfig` directory must
also be available through `PKG_CONFIG_PATH`.
