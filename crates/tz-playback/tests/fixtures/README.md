# Rodio format fixtures

These files are one-second, mono 440 Hz test tones generated from FFmpeg's
synthetic `sine` source. They contain no third-party recording and may be used
and redistributed with this repository.

They were generated with FFmpeg 8.0 on Windows using this source prefix:

```text
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1" -ac 1
```

The output-specific arguments were:

| File | Arguments | Expected result |
|---|---|---|
| `tone.wav` | `-c:a pcm_s16le` | supported WAV/PCM |
| `tone.mp3` | `-c:a libmp3lame -b:a 48k` | supported MP3 |
| `tone.flac` | `-c:a flac` | supported FLAC |
| `tone.ogg` | `-c:a libvorbis -q:a 3` | supported Ogg Vorbis |
| `tone-aac.m4a` | `-c:a aac -b:a 48k` | supported AAC in M4A |
| `tone-alac.m4a` | `-c:a alac` | supported ALAC in M4A |
| `tone.aiff` | `-c:a pcm_s16be` | supported AIFF/PCM |
| `tone.caf` | `-c:a pcm_s16le` | supported CAF/PCM |
| `tone.mka` | `-c:a flac` | supported FLAC in Matroska |
| `tone-opus.ogg` | `-c:a libopus -b:a 24k` | intentionally unsupported Ogg Opus |

Tests decode these committed fixtures directly, so normal test and CI runs do
not invoke FFmpeg and do not require an audio output device.
