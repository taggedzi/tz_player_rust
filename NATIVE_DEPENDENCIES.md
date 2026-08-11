# Native and external dependencies

`tz-player`'s own source is MIT-licensed. The Rust components compiled into the
executable and their terms are listed in `THIRD_PARTY_LICENSES.html`. The
following native/runtime boundaries are not fully represented by Cargo
metadata.

## ALSA / libasound (Linux)

The Linux executable dynamically links the system copy of ALSA's `libasound`.
ALSA is covered by the GNU Lesser General Public License version 2.1 or, at
your option, a later version. A copy of version 2.1 is included at
`licenses/LGPL-2.1.txt`; upstream source is available from
<https://github.com/alsa-project/alsa-lib>.

The release archive does not contain `libasound`. The dynamic link uses the
shared library already present on the recipient's system and permits an
interface-compatible modified replacement. The MIT terms for `tz-player` do
not prohibit reverse engineering for debugging modifications to the LGPL
library.

## VLC / libVLC 3

The default playback backend dynamically loads libVLC 3 from the recipient's
separate VLC installation. libVLC and most VLC modules are covered by
LGPL-2.1-or-later. The LGPL-2.1 text is included at
`licenses/LGPL-2.1.txt`; upstream source is available from
<https://code.videolan.org/videolan/vlc>.

No VLC library or plugin is included in this archive. A distributor who adds a
VLC runtime must audit the exact VLC build and every bundled plugin, preserve
their notices, and provide the corresponding source as their licenses require.

## FFmpeg

The optional analysis backend starts an independently installed `ffmpeg`
executable as a separate process. No FFmpeg code or binary is included in this
archive. FFmpeg builds can be LGPL, GPL, or non-redistributable depending on
their configuration and linked libraries. A distributor who adds FFmpeg to the
archive must audit that exact build separately.

## SQLite and operating-system libraries

The `rusqlite` bundled feature compiles SQLite core into the executable. SQLite
publishes its deliverable core code as public domain. Standard Windows and
macOS system frameworks are referenced from the operating system and are not
included in the archive.
