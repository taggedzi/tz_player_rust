# Native and external dependencies

`tz-player` source is MIT-licensed. The Rust dependencies are listed in
`THIRD_PARTY_LICENSES.html`.

## Audio output

Rodio/CPAL uses the operating system’s audio backend. Linux packages may
therefore require the system ALSA library. The package does not include a
system audio driver.

## Bundled FFmpeg helper

The release package contains `audio/tz-audio-decoder` and the audited shared
FFmpeg libraries `avcodec`, `avformat`, `avutil`, and `swresample`. The build
is shared-only and rejects GPL, nonfree, network, program, device, filter,
encoder, and muxer components. The exact source archive, checksum, configure
identity, and patch set are recorded in `audio/FFMPEG_BUILD.json`,
`audio/FFMPEG_COMPONENTS.json`, `audio/FFMPEG_CONFIGURE.log`, and
`audio/FFMPEG_CHANGES.diff`.

The corresponding source offer is documented in [FFMPEG_SOURCE.md](FFMPEG_SOURCE.md).
The package must include the LGPL notice and any license for an audited native
transitive dependency. The helper is loaded only from the package-relative
`audio/` directory; system FFmpeg and `PATH` are not consulted.

## SQLite and operating-system libraries

The `rusqlite` bundled feature compiles SQLite core into the executable. SQLite
publishes its deliverable core code as public domain. Standard operating-system
frameworks and audio drivers remain system dependencies.
