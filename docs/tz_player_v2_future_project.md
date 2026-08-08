# tz_player v2 — Future Project Design Brief

## Status

**Future project / concept design**

This document captures the intended direction for a future rewrite and expansion of `tz_player`.

The goal is not simply to make another desktop music player. The project should become a small, efficient, open-source music playback system that can run on hardware chosen by the user, including older or constrained devices.

---

# 1. Project Vision

`tz_player v2` is an **offline-first, open-source smart music appliance** focused primarily on playing music the user owns.

The default experience should require:

- no account
- no subscription
- no cloud service
- no permanent network connection
- no streaming provider
- no telemetry dependency
- no large desktop environment

A user should be able to place their music on local storage, point `tz_player` at it, and have a capable, searchable, pleasant music system.

Online services may be supported later through optional plugins, but they should never be required for the core player.

The project should remain usable if every external service disappears.

---

# 2. Core Philosophy

## Local by Default

Music files, metadata, playlists, listening history, indexing, search, playback state, voice processing, and future AI reasoning should be capable of remaining entirely on the user's device.

## Small and Efficient

The system should avoid carrying functionality that it does not need.

Prefer:

- small native components
- predictable resource use
- fast startup
- low idle CPU use
- low memory consumption
- minimal dependencies
- efficient native code
- hardware-specific optimized builds where useful

## Hardware Belongs to the User

`tz_player` should not assume one manufacturer's appliance or one supported board.

Possible targets include:

- modern desktop PCs
- old laptops
- thin clients
- mini PCs
- Raspberry Pi-class systems
- Orange Pi and similar SBCs
- ARM boards
- repurposed computers
- purpose-built portable players
- headless stereo components

The software should adapt to the available hardware rather than forcing the user to purchase a specific platform.

## Opinionated Default, Extensible Design

The base product has one clear job:

> Put your music on storage, point tz_player at it, and listen.

Features that expand beyond this purpose should generally be optional modules.

---

# 3. Proposed High-Level Architecture

```text
tz_player
|
+-- tz-core
|   +-- music library
|   +-- metadata
|   +-- indexing
|   +-- search
|   +-- playlists
|   +-- queue management
|   +-- playback state
|   +-- ratings / history
|
+-- tz-audio
|   +-- minimal FFmpeg backend
|   +-- decoding
|   +-- seeking
|   +-- resampling
|   +-- audio format abstraction
|
+-- tz-output
|   +-- WASAPI
|   +-- ALSA
|   +-- PipeWire
|   +-- direct DAC / platform-specific outputs
|
+-- tz-control
|   +-- structured command API
|   +-- local IPC
|   +-- optional local network API
|   +-- buttons
|   +-- rotary encoders
|   +-- serial/control interfaces
|
+-- frontends
|   +-- desktop GUI
|   +-- TUI
|   +-- tiny LCD/OLED UI
|   +-- web/LAN remote
|   +-- headless daemon
|
+-- tz-intelligence (optional/future)
    +-- wake-word detection
    +-- voice activity detection
    +-- speech recognition
    +-- deterministic intent parser
    +-- tiny music-focused reasoning model
```

The important architectural rule is:

**Playback and library management must not depend on a graphical interface.**

A display should be another client of the player core, not part of the core itself.

---

# 4. Rust Rewrite

A future `tz_player` rewrite should strongly consider Rust for the main application and service architecture.

Rust is well suited to:

- long-running services
- predictable memory usage
- concurrency
- native performance
- cross-platform development
- embedded/constrained targets
- safe interfaces around lower-level native libraries

The project does not need to eliminate C/C++ libraries merely because the application uses Rust.

In particular, mature media decoding should remain delegated to FFmpeg rather than reimplementing codecs.

---

# 5. Minimal FFmpeg Audio Backend

The current Python `tz_player` depends on FFmpeg DLLs for media playback.

For v2, FFmpeg can remain a separate upstream project while `tz_player` uses a **custom stripped FFmpeg build specifically for audio playback**.

The objective is not to fork FFmpeg unnecessarily.

Instead:

1. build only the FFmpeg components needed by `tz_player`
2. expose a very small stable interface to the Rust application
3. eliminate unrelated multimedia functionality

Likely required FFmpeg libraries:

```text
libavformat
libavcodec
libavutil
libswresample
```

Likely unnecessary components include:

```text
video encoders
video decoders
video scaling
subtitle processing
capture devices
most filters
media encoding
muxing not required for playback
ffmpeg CLI tools
ffplay
unneeded network protocols
unneeded codecs and containers
```

A minimal target might support formats such as:

```text
FLAC
MP3
AAC / M4A
ALAC
Opus
Vorbis
WAV / PCM
```

Additional formats should be enabled based on actual user need.

---

# 6. Stable Audio Boundary

Rather than exposing the full FFmpeg API throughout the Rust codebase, create a narrow audio abstraction.

Conceptually:

```text
tz_audio_open()
tz_audio_get_metadata()
tz_audio_read_frames()
tz_audio_seek()
tz_audio_get_position()
tz_audio_close()
```

The actual implementation may use Rust FFI, a small C wrapper, or an appropriate safe Rust binding.

Benefits:

- FFmpeg complexity stays isolated
- FFmpeg can be upgraded independently
- the rest of `tz_player` does not depend on FFmpeg internals
- alternative decoder backends could theoretically be added later
- testing becomes easier

---

# 7. Hardware-Specific Builds

The project should support a portable baseline build, while allowing optimized builds for particular hardware.

Possible build families:

```text
generic-x86_64
x86_64-v3 / AVX2
native-x86_64

generic-arm64
Cortex-A53
Cortex-A72
other ARM-specific targets
```

Potential optimization techniques include:

- compiler optimization
- native CPU targeting
- architecture-specific SIMD
- link-time optimization
- profile-guided optimization where worthwhile
- removal of unused FFmpeg codecs/features
- static versus dynamic linking experiments
- size optimization on constrained devices

Optimization should be benchmark-driven.

Audio decoding is inexpensive on modern desktop CPUs, so the primary benefits may be:

- smaller binaries
- lower RAM usage
- reduced dependencies
- lower startup overhead
- better performance on very weak hardware
- lower power consumption
- reduced attack surface

---

# 8. Library and Search System

The filesystem should not be the primary user interface.

`tz_player` should build a compact local searchable index, likely using SQLite.

Possible indexed fields:

```text
artist
album
album artist
title
genre
year
composer
track number
disc number
file path
duration
codec
sample rate
bitrate
user tags
rating
play count
last played
date added
```

This enables useful searches even on devices with minimal interfaces.

Examples:

```text
Pink Floyd
Pink Floyd live
jazz from the 1970s
unplayed Queen
instrumental tracks
highest rated jazz
songs longer than eight minutes
recently added albums
```

SQLite is attractive because it is:

- mature
- lightweight
- embedded
- fast
- portable
- available without a server
- suitable for constrained hardware

---

# 9. Headless Operation

Headless operation should be a first-class configuration.

Conceptually:

```bash
tz-player serve
```

The service should be able to:

- start automatically on boot
- discover/index configured music storage
- restore its queue and playback state
- accept local controls
- operate without a monitor
- run without a desktop environment

This would allow `tz_player` to function as a dedicated stereo component.

---

# 10. User Interface Options

Because the core is display-independent, multiple interfaces can coexist.

## Desktop

Full library browsing, artwork, search, playlists, queue editing, configuration, and device administration.

## Terminal / TUI

Useful for servers, SSH sessions, and lightweight systems.

## Small LCD/OLED

A device may expose only essential information:

```text
Pink Floyd
Time

03:17 / 06:53

▶ FLAC
```

Possible controls:

- play/pause
- next
- previous
- volume
- menu
- rotary encoder
- directional buttons

## Local Web Remote

A headless player could expose an optional LAN-only interface so a phone, tablet, or computer can act as a rich remote without requiring a cloud account.

---

# 11. Future Local Intelligence Layer

A later version may add optional local AI assistance.

This subsystem should be completely removable.

```text
voice
  |
  v
wake word / VAD
  |
  v
speech-to-text
  |
  v
deterministic command parser
  |
  +------ obvious command ------> tz-core
  |
  v
tiny reasoning model
  |
  v
validated structured command
  |
  v
tz-core
```

The key design principle:

**Do not use an LLM when ordinary code can solve the request.**

Commands such as:

```text
pause
stop
next
previous
volume up
shuffle
```

should be handled deterministically.

The reasoning model should primarily handle natural or ambiguous requests.

---

# 12. Domain-Focused AI

The future reasoning model does not need to be a general-purpose chatbot.

Its job is primarily to translate natural language into structured `tz_player` actions.

Example user request:

```text
"I want to listen to Pink Floyd. Play some of their hits."
```

Possible model output:

```json
{
  "intent": "play_music",
  "artist": "Pink Floyd",
  "selection": "popular_tracks",
  "count": 10,
  "shuffle": false
}
```

`tz_player`, not the model, performs the database search and playback action.

This greatly reduces the intelligence required from the model.

Potential requests include:

```text
Play some Pink Floyd.

Play something relaxing from the seventies.

Play more songs like this.

Skip this one.

Don't play this artist again today.

Put my highest-rated jazz on shuffle.

Play the album this song came from.

What song is this?

What have I listened to most this month?
```

Because the domain is narrow, a very small specialized model may eventually be sufficient.

A model in the hundreds-of-millions-of-parameters range may be worth investigating rather than assuming a multi-billion-parameter general-purpose LLM is necessary.

---

# 13. Voice Recognition

Speech recognition should also remain local where hardware permits.

Potential pipeline:

```text
microphone
  |
voice activity detection
  |
optional wake word
  |
small speech-to-text engine
  |
intent handling
```

The speech model and reasoning model should be independently replaceable.

This allows different hardware classes to use different components.

For extremely constrained hardware, voice support may simply be disabled.

---

# 14. Structured Command API

The structured command interface should exist **before** the AI features.

Every frontend should communicate with the same player core.

Example conceptual API:

```json
{
  "command": "play_artist",
  "artist": "Pink Floyd",
  "mode": "shuffle"
}
```

Potential command sources:

```text
desktop GUI
TUI
OLED controls
physical buttons
web remote
local API client
voice system
future AI system
```

This means AI can be added later without redesigning the player.

The AI becomes another client of an already mature command system.

---

# 15. Offline Intelligence and Privacy

A fully local configuration could keep all of the following on-device:

- music files
- music metadata
- search index
- playlists
- ratings
- listening history
- microphone processing
- speech recognition
- natural-language reasoning
- playback commands

No cloud provider needs to know:

- what the user owns
- what they listen to
- when they listen
- what they search for
- what they say to the player

Networking should be an optional capability, not an architectural requirement.

---

# 16. Optional Online Services

Being offline-first should not mean prohibiting network services.

An extension/plugin system could eventually allow support for:

- internet radio
- metadata providers
- lyrics providers
- network music libraries
- streaming providers
- remote control
- scrobbling
- synchronization services

These should remain optional.

Removing every online plugin should still leave a complete useful music player.

---

# 17. Revitalizing Old Hardware

One of the more interesting secondary goals is making obsolete computing hardware useful again.

Modern web applications may make old computers feel unusably slow even though the hardware remains easily capable of:

- decoding audio
- querying SQLite
- rendering a small interface
- controlling a DAC
- maintaining a music library
- running a lightweight service

Possible repurposed hardware:

```text
old laptops
office mini PCs
thin clients
old desktops
small ARM boards
single-board computers
industrial PCs
embedded x86 systems
```

A machine that is no longer pleasant for general-purpose computing may still make an excellent dedicated audio appliance.

---

# 18. Portable Hardware Possibility

Long term, the same architecture could support a modern equivalent of classic portable MP3 players.

Possible hardware:

```text
ARM SBC / compute module
flash storage or microSD
DAC / headphone amplifier
small OLED or LCD
physical buttons
rotary encoder
battery
optional microphone
optional Wi-Fi
```

The device could remain entirely functional with networking disabled.

A richer phone/desktop interface could optionally control it over a local connection.

---

# 19. What This Project Should Not Become

Avoid expanding the core project into a universal media platform.

The base system does not need to become:

- a video player
- a media transcoder
- a home automation system
- a cloud service
- a social network
- a streaming company
- a general-purpose AI assistant

Keeping scope narrow makes extremely small deployments possible.

---

# 20. Suggested Initial Development Scope

When development eventually begins, the first milestone should deliberately ignore AI.

A strong first target:

```text
Rust tz-core
+
SQLite music library
+
minimal FFmpeg audio backend
+
queue/playback engine
+
structured command API
+
one simple desktop or terminal interface
```

Then prove that the same core can run headless.

Only after those interfaces are stable should specialized hardware and local intelligence be added.

---

# 21. Long-Term Project Principle

The project can be summarized as:

> A small, efficient, intelligent music system for playing music you own, on hardware you control.

Or more practically:

> Put your music on storage, point tz_player at it, and listen.

Everything beyond that should build on top of the core rather than becoming a prerequisite for it.

---

# 22. Future Research Topics

When this project becomes active, investigate:

- Rust FFmpeg bindings versus a custom C ABI wrapper
- minimal FFmpeg configure/build options for audio-only playback
- FFmpeg licensing implications for binary distribution
- ALSA / PipeWire / WASAPI abstraction strategy
- SQLite full-text search versus conventional indexed queries
- cross-compilation for ARM
- x86-64-v2/v3/native build strategy
- reproducible optimized builds
- benchmark methodology for constrained hardware
- direct DAC/I2S output on SBC hardware
- low-power idle design
- wake-word engines
- small offline speech-to-text models
- sub-billion-parameter intent models
- grammar-constrained or schema-constrained model output
- deterministic natural-language command parsing
- plugin architecture for optional network services
- local remote-control protocol
- library/database synchronization between devices

---

# 23. Working Component Names

These names are provisional but useful for discussing architecture:

```text
tz_player          complete project/application
tz-core            music library and playback state
tz-audio           minimal native decoding layer
tz-output          platform audio output
tz-control         commands, IPC, remote control
tz-ui              graphical/display interfaces
tz-intelligence    optional local voice/reasoning system
```

---

# 24. Current Status

This project is intentionally deferred.

The architecture should be preserved so development can begin later without losing the design decisions behind it.

The important early decisions already established are:

1. Rust is the likely implementation language for v2.
2. FFmpeg remains an external upstream project.
3. A custom minimal audio-only FFmpeg build is preferred.
4. The core must work headless.
5. Displays and controls are interchangeable frontends.
6. SQLite provides the local music index/search layer.
7. Local ownership and offline use are the default.
8. Online services are optional extensions.
9. Hardware-specific optimized builds are encouraged.
10. Local voice/AI is an optional future client of the command API.
11. Simple commands should not require an LLM.
12. Constrained and repurposed hardware is an explicit project target.
