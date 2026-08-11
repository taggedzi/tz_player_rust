The goal is to distribute everything together so users download one package, unzip it, and run the player.

The important distinction is: **One download:** easy and recommended.

On Windows, the release could look like this:

```text
tz-player/
├── tz-player.exe
├── audio/
│   ├── tz-audio-decoder.exe
│   ├── avcodec-XX.dll
│   ├── avformat-XX.dll
│   ├── avutil-XX.dll
│   └── swresample-XX.dll
├── licenses/
│   ├── MIT.txt
│   ├── LGPL-2.1.txt
│   └── THIRD_PARTY_LICENSES.html
├── FFMPEG_BUILD.txt
├── README.md
└── NATIVE_DEPENDENCIES.md
```

Users would not:

- Install FFmpeg
- Modify `PATH`
- Download DLLs
- Link anything
- Verify separate downloads

They would simply run `tz-player.exe`.

The same idea works on Linux with `.so` files and macOS with `.dylib` files inside the application package.

## How I would connect it

Because FFmpeg is currently only needed for offline visualization analysis, I recommend a small helper program:

```text
tz-player
    ↓ starts
tz-audio-decoder
    ↓ uses bundled FFmpeg libraries
PCM audio stream
    ↓
existing Rust visualization analysis and cache
```

`tz-audio-decoder` would be a tiny program we control. Its interface could be something like:

```text
tz-audio-decoder decode song.mp3
```

It would write standardized stereo PCM to its output. This is almost exactly what the project already receives from `ffmpeg.exe`, so relatively little analysis code would need to change.

Advantages:

- FFmpeg crashes stay outside the main player.
- Existing timeout and memory protections remain useful.
- The helper exposes only the small feature set we need.
- Rust does not interact directly with FFmpeg’s complicated API.
- The user never needs a separate FFmpeg installation.
- The helper can later stream uncommon formats to Rodio for playback.

For offline analysis, the extra process and pipe should not cause a meaningful performance problem.

## How it enters the release

Your release process would do the work for the user:

1. Build a pinned, audio-only LGPL FFmpeg configuration.
2. Build `tz-audio-decoder` against those libraries.
3. Copy the helper and FFmpeg libraries into the release folder.
4. Copy the required licenses and build information.
5. Create one ZIP, installer, AppImage, or application bundle.
6. Test the packaged player on a machine without FFmpeg or VLC installed.

The existing [package-release.ps1](/E:/Home/Documents/Programming/tz_player3/scripts/package-release.ps1) could eventually perform those packaging steps.

The application should locate the helper relative to its own executable, never through `PATH`. That prevents it from accidentally running an unknown FFmpeg installation.

## What you must distribute for LGPL compliance

The release should include or link to:

- The FFmpeg LGPL license
- The exact FFmpeg version or commit
- The complete build configuration
- Any patches you made
- The exact corresponding FFmpeg source
- The source address in the README and About screen

The exact source archive can be another asset attached to the same GitHub release. It does not necessarily need to be inside the smaller player ZIP, but it must be clearly available alongside the binary release.

FFmpeg’s official recommendation is to use dynamic libraries, avoid GPL/nonfree components, and provide the matching source and build information. [FFmpeg license checklist](https://ffmpeg.org/legal.html).

## The tradeoff

Bundling removes all setup work from the user, but it transfers that responsibility to you as the distributor:

- You choose and verify the FFmpeg build.
- You audit its enabled components.
- You preserve its license information.
- You publish matching source.
- You build packages for each operating system and CPU type.

That can all be automated in the release workflow.

## My recommendation

Use this arrangement:

- Rodio/Symphonia remains the normal player.
- Native Rust analysis handles formats Symphonia decodes.
- Bundled `tz-audio-decoder` handles additional formats and produces visualization PCM.
- Bundled FFmpeg shared libraries remain isolated under `audio/`.
- Everything arrives in one user-friendly download.
- Your Rust code and helper wrapper remain MIT.
- The FFmpeg files remain LGPL.

This gives users the simple experience you want without changing the project’s MIT license.


```text
audio file
   ↓
tz_audio_decoder
   ├─→ PCM stream → live meters / visualizations
   └─→ analysis builder → cache → rich visualizations later
```

The PCM stream should not replace the cache.

The cache is useful because it lets the program:

- start visualizations quickly;
- seek forward and backward;
- avoid decoding the same song every time;
- show spectrum, beat, waveform, and history-based visualizations consistently.

The live PCM stream is useful for:

- volume meters;
- immediate reaction to the current sound;
- visualizations while a song is playing;
- avoiding a delay while the entire song is analyzed.

For your project, I recommend:

1. `tz_audio_decoder` decodes the song into normalized PCM.
2. It sends that PCM to the analyzer.
3. The analyzer creates the normal visualization cache.
4. During playback, a small portion of the PCM is also used for live visualizers.
5. The cache remains available for detailed visualizers and seeking.

There are two ways to implement this:

- Easier and safer: Rodio plays the file, while `tz_audio_decoder` separately analyzes it. This may decode the file twice.
- More efficient but more complicated: `tz_audio_decoder` produces PCM once, then feeds both Rodio and the visualizer. This requires careful buffering, pause handling, seeking, timing, and error recovery.

I would start with the first approach. It uses more CPU during analysis, but it is much easier to make reliable. Later, you can optimize common formats so one PCM stream feeds both playback and visualizations.

Your existing cache system already follows this general idea: FFmpeg currently produces PCM, and `tz-analysis` turns it into envelope, spectrum, beat, and waveform data. See [decode.rs](E:/Home/Documents/Programming/tz_player3/crates/tz-analysis/src/decode.rs) and [levels.rs](E:/Home/Documents/Programming/tz_player3/crates/tz-core/src/levels.rs).