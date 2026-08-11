# FFmpeg source offer

The bundled helper is intended to be built from the exact FFmpeg source listed
in `native/ffmpeg/manifest.toml`:

- Release: FFmpeg 7.1.5
- Source: <https://ffmpeg.org/releases/ffmpeg-7.1.5.tar.xz>
- Signature: <https://ffmpeg.org/releases/ffmpeg-7.1.5.tar.xz.asc>
- SHA-256: `de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f`
- MD5 (upstream cross-check): `8a5e3d530be908235511f585ccaceafd`

The build applies the audited patch
`native/ffmpeg/patches/0001-speex-frame-size.patch`, which incorporates the
upstream Speex frame-size correction needed by FFmpeg 7.1.5 for wideband
streams. The same patch is shipped as `audio/FFMPEG_CHANGES.diff`.

The source archive and matching patch must be made available beside each
binary release. The build scripts verify the SHA-256 before extraction, apply
the patch exactly once, and reject forbidden configuration flags.
