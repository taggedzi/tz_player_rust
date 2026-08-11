# Audio format fixtures

The audio files contain generated one-second 440 Hz tones, not third-party
recordings. One tiny generated black-frame MP4 exercises the no-audio-stream
error path. They may be used and redistributed with this repository. Normal
tests decode the committed files directly and never invoke a system FFmpeg or
require an audio output device.

The FFmpeg-generated files use FFmpeg 8.0
(`essentials_build-www.gyan.dev`) and one of these synthetic inputs:

```text
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1" -ac 1 ...
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" -ac 2 ...
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 ...
```

The release FFmpeg SDK deliberately contains no encoders. These development
tools are used only to generate fixtures, as permitted by the migration plan.

| File | Codec/container and generator | Route | SHA-256 |
|---|---|---|---|
| `tone.wav` | PCM s16le/WAV, FFmpeg `-c:a pcm_s16le` | native | `5c3f47fe154ec5bf6e6afde6c9f3d81647d0ad2275900bc96579d7df4444d975` |
| `tone.mp3` | MP3, FFmpeg `-c:a libmp3lame -b:a 48k` | native | `f5dee0c4c8000759d1772e67f276e8ecaa55357e2c32841a254f8859bfec77e3` |
| `tone.flac` | FLAC, FFmpeg `-c:a flac` | native | `3fa8751ce9940cde56e74255a5646186b03018305aecdf14ea758823077a32a4` |
| `tone.ogg` | Vorbis/Ogg, FFmpeg `-c:a libvorbis -q:a 3` | native | `c5fd53ce6c202db4a45957e64f7fd3e358623647b439270e59596c6b313f51ba` |
| `tone-aac.m4a` | AAC/M4A, FFmpeg `-c:a aac -b:a 48k` | native | `c5e2530ba314409f5c85f108847397ee20b2b1e00f6851875d90d4980cf7a894` |
| `tone-alac.m4a` | ALAC/M4A, FFmpeg `-c:a alac` | native | `2d3d1ce8912d3013f7cd1a842e3b5fb697950ccdcc85f7ab5f3a2b64a76acaf4` |
| `tone.aiff` | PCM s16be/AIFF, FFmpeg `-c:a pcm_s16be` | native | `4d93d01a28d57f6f2fc70a174cd73e2456a185f1ac216182cc8b9e337f3d7b8e` |
| `tone.caf` | PCM s16le/CAF, FFmpeg `-c:a pcm_s16le` | native | `2db9827793c5ebbf6d64c9b90141bab1f9c79c119022ed3ed57c810108a9aeee` |
| `tone.mka` | FLAC/Matroska, FFmpeg `-c:a flac` | native | `f6bb97770526c14db9e3a0b9798dab3124c64fac9ab332d3e54509af5fb2f4ee` |
| `tone-opus.ogg` | Opus/Ogg, FFmpeg `-c:a libopus -b:a 24k` | helper | `60d801997d1a8c779844f5004fdb31ed5bb8c1ab2939071f6eadaca4c7ed9b5e` |
| `tone-wma.wma` | WMA2/ASF, FFmpeg `-c:a wmav2 -b:a 32k` | helper | `3f176c224861afd59e3629e0cf2e69e6ff2faad39dba059f664933b9399c3667` |
| `tone-wavpack.wv` | WavPack, FFmpeg `-c:a wavpack` | helper | `2ff34030102fecbf57fddbb9da63108e8b0b52309b476707e12b6f1064115cf8` |
| `tone-ac3.ac3` | AC-3, FFmpeg `-c:a ac3 -b:a 192k` | helper | `bc1da4e634674ce1f3d8bd1a0f052d0a3d5386aa48e33a587f0a5191c93ea07f` |
| `tone-eac3.eac3` | E-AC-3, FFmpeg `-c:a eac3 -b:a 192k` | helper | `7f56702958b272c718596c51f9e0d556e769c2d1cab0385e07a2c7ac9bec3c62` |
| `tone-dts.dts` | DTS, FFmpeg `-strict -2 -c:a dca -b:a 768k` | helper | `282d15e11221524f6af40f4202ec6d2a3813d2a12a49de904e7cdfc0c4ce9bda` |
| `tone-tta.tta` | TTA, FFmpeg `-c:a tta` | helper | `70938f4eaf7fea753dca27aff151948f50bf6eea96cdeb82ece53d7c2cb7cd6b` |
| `tone-speex.ogg` | Speex/Ogg, FFmpeg `-c:a libspeex` | helper | `df367ae202122ad77b31408e081e00e4a10545563523a82bd10b7dfb5afa40b8` |
| `tone-ape.ape` | Monkey's Audio 13.24 normal (`MAC.exe ... -c2000`) from `tone.wav` | helper | `ee5a0ee73b4223f920cf740dcf2aede41391991cb2a053d46b87ef335b2eba65` |
| `tone-musepack7.mpc` | Musepack SV7 standard, MDT `mppenc` 1.15u, from a 44.1 kHz stereo WAV | helper | `7b61e197d6fb6094655029299842e460b6f809eedcdf8b701e1a56e60e4e249d` |
| `tone-musepack8.mpc` | Musepack SV8 standard, `mpcenc` 1.30.1, from the same WAV | helper | `1b9738a86b134809075d00f1187a6f328cd676bd3a67486f95382ac43227bd82` |
| `tone-video-only.mp4` | MPEG-4 video/MP4, FFmpeg 8.0 `color=c=black:s=16x16:r=1 -t 1 -an -c:v mpeg4 -q:v 31` | negative: no audio | `3a6476cd30accfb7ed784b422ad21d740c727aff3fd2deee9f6a7f9440329950` |

Monkey's Audio was built from the official 13.24 SDK. The SV7 executable came
from ReallyRareWares' archived Musepack Development Team release
(`mppenc-windows-1.15u.zip`, SHA-256
`15fa000c2a503c97972d3304669c5487bf1b3000ac81c1d61d7c0c642509ab62`).
The SV8 executable is a modern public build of the r475 source
(`mpcenc.exe`, SHA-256
`dd70c95cfcf39dcb15899493ceb274fe1e8bcab21170b9bb60685a5baec0fdc0`).

`corrupt.bin` is a hand-authored 40-byte invalid-media fixture (SHA-256
`09a0e5cd52fde79a6312f4133c7b9fafd86c32f35f09fda344ad92619472e062`).
