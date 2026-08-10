//! Version-gated dynamic LibVLC 3 bindings.
//!
//! We intentionally avoid a link-time dependency on LibVLC. The stable
//! `libvlc_get_version` symbol is loaded first; no ABI-specific symbol is
//! resolved or called until the major version is validated. VLC 4 changes
//! player construction, seeking signatures, and time units, so it is rejected
//! until a complete VLC 4 backend is implemented.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_void, CStr};
use std::os::raw::{c_char, c_float, c_int};
use std::path::Path;

use libloading::{Library, Symbol};

pub type libvlc_instance_t = c_void;
pub type libvlc_media_t = c_void;
pub type libvlc_media_player_t = c_void;
pub type libvlc_time_t = i64;

/// LibVLC ABI families that this binary can call safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibVlcAbi {
    /// LibVLC 3.x: millisecond clocks and two-argument, void `set_time`.
    V3,
}

impl LibVlcAbi {
    fn raw_time_to_ms(self, raw: libvlc_time_t) -> i64 {
        match self {
            Self::V3 => raw,
        }
    }

    fn ms_to_raw_time(self, milliseconds: i64) -> libvlc_time_t {
        match self {
            Self::V3 => milliseconds,
        }
    }
}

/// Validated values returned by LibVLC's C enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibVlcState {
    NothingSpecial,
    Opening,
    Buffering,
    Playing,
    Paused,
    Stopped,
    Ended,
    Error,
}

impl TryFrom<c_int> for LibVlcState {
    type Error = String;

    fn try_from(raw: c_int) -> Result<Self, String> {
        match raw {
            0 => Ok(Self::NothingSpecial),
            1 => Ok(Self::Opening),
            2 => Ok(Self::Buffering),
            3 => Ok(Self::Playing),
            4 => Ok(Self::Paused),
            5 => Ok(Self::Stopped),
            6 => Ok(Self::Ended),
            7 => Ok(Self::Error),
            _ => Err(format!("LibVLC returned unknown player state {raw}")),
        }
    }
}

type FnGetVersion = unsafe extern "C" fn() -> *const c_char;

// LibVLC 3.x function signatures. Do not reuse these aliases for another
// major: VLC 4 changes several otherwise identically named symbols.
type FnV3New = unsafe extern "C" fn(c_int, *const *const c_char) -> *mut libvlc_instance_t;
type FnV3Release = unsafe extern "C" fn(*mut libvlc_instance_t);
type FnV3MediaNewPath =
    unsafe extern "C" fn(*mut libvlc_instance_t, *const c_char) -> *mut libvlc_media_t;
type FnV3MediaNewLocation =
    unsafe extern "C" fn(*mut libvlc_instance_t, *const c_char) -> *mut libvlc_media_t;
type FnV3MediaRelease = unsafe extern "C" fn(*mut libvlc_media_t);
type FnV3PlayerNew = unsafe extern "C" fn(*mut libvlc_instance_t) -> *mut libvlc_media_player_t;
type FnV3PlayerRelease = unsafe extern "C" fn(*mut libvlc_media_player_t);
type FnV3PlayerSetMedia = unsafe extern "C" fn(*mut libvlc_media_player_t, *mut libvlc_media_t);
type FnV3PlayerPlay = unsafe extern "C" fn(*mut libvlc_media_player_t) -> c_int;
type FnV3PlayerStop = unsafe extern "C" fn(*mut libvlc_media_player_t);
type FnV3PlayerPause = unsafe extern "C" fn(*mut libvlc_media_player_t);
/// C enums cross the FFI boundary as their integer representation and are
/// validated before conversion to `LibVlcState`.
type FnV3PlayerGetState = unsafe extern "C" fn(*mut libvlc_media_player_t) -> c_int;
type FnV3PlayerGetTime = unsafe extern "C" fn(*mut libvlc_media_player_t) -> libvlc_time_t;
type FnV3PlayerSetTime = unsafe extern "C" fn(*mut libvlc_media_player_t, libvlc_time_t);
type FnV3PlayerGetLength = unsafe extern "C" fn(*mut libvlc_media_player_t) -> libvlc_time_t;
type FnV3AudioSetVolume = unsafe extern "C" fn(*mut libvlc_media_player_t, c_int) -> c_int;
type FnV3PlayerSetRate = unsafe extern "C" fn(*mut libvlc_media_player_t, c_float) -> c_int;
type FnV3Errmsg = unsafe extern "C" fn() -> *const c_char;

/// Loaded, version-validated LibVLC 3 API surface and owning library handle.
pub struct LibVlcApi {
    _lib: Library,
    abi: LibVlcAbi,
    version: String,
    pub new: FnV3New,
    pub release: FnV3Release,
    pub media_new_path: FnV3MediaNewPath,
    pub media_new_location: Option<FnV3MediaNewLocation>,
    pub media_release: FnV3MediaRelease,
    pub player_new: FnV3PlayerNew,
    pub player_release: FnV3PlayerRelease,
    pub player_set_media: FnV3PlayerSetMedia,
    pub player_play: FnV3PlayerPlay,
    pub player_stop: FnV3PlayerStop,
    pub player_pause: FnV3PlayerPause,
    player_get_state: FnV3PlayerGetState,
    player_get_time: FnV3PlayerGetTime,
    player_set_time: FnV3PlayerSetTime,
    player_get_length: FnV3PlayerGetLength,
    pub audio_set_volume: FnV3AudioSetVolume,
    pub player_set_rate: FnV3PlayerSetRate,
    errmsg: Option<FnV3Errmsg>,
}

impl LibVlcApi {
    /// Load LibVLC from a directory and reject unsupported major versions
    /// before resolving any major-specific function pointer.
    pub unsafe fn load(lib_dir: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        let lib_name = "libvlc.dll";
        #[cfg(target_os = "macos")]
        let lib_name = "libvlc.dylib";
        #[cfg(all(unix, not(target_os = "macos")))]
        let lib_name = "libvlc.so";

        let lib_path = lib_dir.join(lib_name);
        if !lib_path.is_file() {
            #[cfg(windows)]
            return Err(format!(
                "expected LibVLC library was not found at {}",
                lib_path.display()
            ));

            #[cfg(not(windows))]
            let lib = Library::new(lib_name)
                .map_err(|error| format!("failed to load {lib_name}: {error}"))?;
            #[cfg(not(windows))]
            return Self::from_library(lib);
        }

        let lib = load_vlc_library(&lib_path)
            .map_err(|error| format!("failed to load {}: {error}", lib_path.display()))?;
        Self::from_library(lib)
    }

    unsafe fn from_library(lib: Library) -> Result<Self, String> {
        unsafe fn required<T>(lib: &Library, name: &[u8]) -> Result<T, String>
        where
            T: Copy,
        {
            let symbol: Symbol<T> = lib.get(name).map_err(|error| {
                format!("missing symbol {}: {error}", String::from_utf8_lossy(name))
            })?;
            Ok(*symbol)
        }

        unsafe fn optional<T>(lib: &Library, name: &[u8]) -> Option<T>
        where
            T: Copy,
        {
            lib.get::<T>(name).ok().map(|symbol| *symbol)
        }

        // `libvlc_get_version(void)` is the only symbol called before the ABI
        // family is known. It has the same signature in supported discovery
        // targets and returns an owned-by-LibVLC static string.
        let get_version: FnGetVersion = required(&lib, b"libvlc_get_version\0")?;
        let version_ptr = get_version();
        if version_ptr.is_null() {
            return Err("libvlc_get_version returned null".into());
        }
        let version = CStr::from_ptr(version_ptr).to_string_lossy().into_owned();
        let abi = classify_libvlc_version(&version)?;

        match abi {
            LibVlcAbi::V3 => Ok(Self {
                new: required(&lib, b"libvlc_new\0")?,
                release: required(&lib, b"libvlc_release\0")?,
                media_new_path: required(&lib, b"libvlc_media_new_path\0")?,
                media_new_location: optional(&lib, b"libvlc_media_new_location\0"),
                media_release: required(&lib, b"libvlc_media_release\0")?,
                player_new: required(&lib, b"libvlc_media_player_new\0")?,
                player_release: required(&lib, b"libvlc_media_player_release\0")?,
                player_set_media: required(&lib, b"libvlc_media_player_set_media\0")?,
                player_play: required(&lib, b"libvlc_media_player_play\0")?,
                player_stop: required(&lib, b"libvlc_media_player_stop\0")?,
                player_pause: required(&lib, b"libvlc_media_player_pause\0")?,
                player_get_state: required(&lib, b"libvlc_media_player_get_state\0")?,
                player_get_time: required(&lib, b"libvlc_media_player_get_time\0")?,
                player_set_time: required(&lib, b"libvlc_media_player_set_time\0")?,
                player_get_length: required(&lib, b"libvlc_media_player_get_length\0")?,
                audio_set_volume: required(&lib, b"libvlc_audio_set_volume\0")?,
                player_set_rate: required(&lib, b"libvlc_media_player_set_rate\0")?,
                errmsg: optional(&lib, b"libvlc_errmsg\0"),
                _lib: lib,
                abi,
                version,
            }),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub unsafe fn player_state(
        &self,
        player: *mut libvlc_media_player_t,
    ) -> Result<LibVlcState, String> {
        LibVlcState::try_from((self.player_get_state)(player))
    }

    pub unsafe fn player_time_ms(&self, player: *mut libvlc_media_player_t) -> i64 {
        self.abi.raw_time_to_ms((self.player_get_time)(player))
    }

    pub unsafe fn player_length_ms(&self, player: *mut libvlc_media_player_t) -> i64 {
        self.abi.raw_time_to_ms((self.player_get_length)(player))
    }

    pub unsafe fn set_player_time_ms(&self, player: *mut libvlc_media_player_t, milliseconds: i64) {
        (self.player_set_time)(player, self.abi.ms_to_raw_time(milliseconds));
    }

    pub fn last_error(&self) -> Option<String> {
        let function = self.errmsg?;
        unsafe {
            let pointer = function();
            if pointer.is_null() {
                return None;
            }
            Some(CStr::from_ptr(pointer).to_string_lossy().into_owned())
        }
    }
}

fn classify_libvlc_version(version: &str) -> Result<LibVlcAbi, String> {
    let major_text = version
        .split('.')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("could not parse LibVLC version {version:?}"))?;
    let major = major_text
        .parse::<u32>()
        .map_err(|_| format!("could not parse LibVLC version {version:?}"))?;
    match major {
        3 => Ok(LibVlcAbi::V3),
        _ => Err(format!(
            "unsupported LibVLC major version {major} ({version}); tz-player currently supports VLC 3.x only"
        )),
    }
}

#[cfg(windows)]
fn windows_vlc_load_flags() -> u32 {
    use libloading::os::windows::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32};

    // Resolve libvlccore.dll and the other VLC runtime dependencies beside
    // libvlc.dll for this load only. This avoids both process-global PATH
    // mutation and the ineffective AddDllDirectory-without-search-flags path.
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
}

#[cfg(windows)]
unsafe fn load_vlc_library(path: &Path) -> Result<Library, libloading::Error> {
    use libloading::os::windows::Library as WindowsLibrary;

    WindowsLibrary::load_with_flags(path, windows_vlc_load_flags()).map(Into::into)
}

#[cfg(not(windows))]
unsafe fn load_vlc_library(path: &Path) -> Result<Library, libloading::Error> {
    Library::new(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_vlc_3_version_strings() {
        assert_eq!(
            classify_libvlc_version("3.0.21 Vetinari").unwrap(),
            LibVlcAbi::V3
        );
        assert_eq!(classify_libvlc_version("3.0.0-git").unwrap(), LibVlcAbi::V3);
    }

    #[test]
    fn rejects_unsupported_or_unparseable_versions() {
        let vlc4 = classify_libvlc_version("4.0.0-dev Otto Chriek").unwrap_err();
        assert!(vlc4.contains("unsupported LibVLC major version 4"));
        assert!(classify_libvlc_version("nightly").is_err());
        assert!(classify_libvlc_version("").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_load_is_scoped_to_vlc_and_system32() {
        use libloading::os::windows::{
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };

        let flags = windows_vlc_load_flags();
        assert_ne!(flags & LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, 0);
        assert_ne!(flags & LOAD_LIBRARY_SEARCH_SYSTEM32, 0);
    }

    #[test]
    fn validates_c_enum_states_before_conversion() {
        assert_eq!(LibVlcState::try_from(3).unwrap(), LibVlcState::Playing);
        assert_eq!(LibVlcState::try_from(7).unwrap(), LibVlcState::Error);
        assert!(LibVlcState::try_from(-1).is_err());
        assert!(LibVlcState::try_from(8).is_err());
        assert!(LibVlcState::try_from(i32::MAX).is_err());
    }

    #[test]
    fn vlc_3_time_conversion_is_explicitly_milliseconds() {
        assert_eq!(LibVlcAbi::V3.raw_time_to_ms(12_345), 12_345);
        assert_eq!(LibVlcAbi::V3.ms_to_raw_time(54_321), 54_321);
    }
}
