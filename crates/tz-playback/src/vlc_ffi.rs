//! Minimal dynamic libVLC bindings (load at runtime from VLC install).
//!
//! We intentionally avoid link-time dependency on libvlc so Windows users can
//! keep a normal VideoLAN install without a separate SDK.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_float, c_int};
use std::path::Path;

use libloading::{Library, Symbol};

pub type libvlc_instance_t = c_void;
pub type libvlc_media_t = c_void;
pub type libvlc_media_player_t = c_void;
pub type libvlc_time_t = i64;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum libvlc_state_t {
    NothingSpecial = 0,
    Opening = 1,
    Buffering = 2,
    Playing = 3,
    Paused = 4,
    Stopped = 5,
    Ended = 6,
    Error = 7,
}

type FnNew = unsafe extern "C" fn(c_int, *const *const c_char) -> *mut libvlc_instance_t;
type FnRelease = unsafe extern "C" fn(*mut libvlc_instance_t);
type FnMediaNewPath =
    unsafe extern "C" fn(*mut libvlc_instance_t, *const c_char) -> *mut libvlc_media_t;
type FnMediaNewLocation =
    unsafe extern "C" fn(*mut libvlc_instance_t, *const c_char) -> *mut libvlc_media_t;
type FnMediaRelease = unsafe extern "C" fn(*mut libvlc_media_t);
type FnPlayerNew = unsafe extern "C" fn(*mut libvlc_instance_t) -> *mut libvlc_media_player_t;
type FnPlayerRelease = unsafe extern "C" fn(*mut libvlc_media_player_t);
type FnPlayerSetMedia = unsafe extern "C" fn(*mut libvlc_media_player_t, *mut libvlc_media_t);
type FnPlayerPlay = unsafe extern "C" fn(*mut libvlc_media_player_t) -> c_int;
type FnPlayerStop = unsafe extern "C" fn(*mut libvlc_media_player_t);
type FnPlayerPause = unsafe extern "C" fn(*mut libvlc_media_player_t);
type FnPlayerGetState = unsafe extern "C" fn(*mut libvlc_media_player_t) -> libvlc_state_t;
type FnPlayerGetTime = unsafe extern "C" fn(*mut libvlc_media_player_t) -> libvlc_time_t;
/// VLC 3.x signature (void return).
type FnPlayerSetTime = unsafe extern "C" fn(*mut libvlc_media_player_t, libvlc_time_t);
type FnPlayerGetLength = unsafe extern "C" fn(*mut libvlc_media_player_t) -> libvlc_time_t;
type FnAudioSetVolume = unsafe extern "C" fn(*mut libvlc_media_player_t, c_int) -> c_int;
type FnPlayerSetRate = unsafe extern "C" fn(*mut libvlc_media_player_t, c_float) -> c_int;
type FnErrmsg = unsafe extern "C" fn() -> *const c_char;

/// Loaded libVLC API surface + owning library handle.
pub struct LibVlcApi {
    _lib: Library,
    pub new: FnNew,
    pub release: FnRelease,
    pub media_new_path: FnMediaNewPath,
    pub media_new_location: Option<FnMediaNewLocation>,
    pub media_release: FnMediaRelease,
    pub player_new: FnPlayerNew,
    pub player_release: FnPlayerRelease,
    pub player_set_media: FnPlayerSetMedia,
    pub player_play: FnPlayerPlay,
    pub player_stop: FnPlayerStop,
    pub player_pause: FnPlayerPause,
    pub player_get_state: FnPlayerGetState,
    pub player_get_time: FnPlayerGetTime,
    pub player_set_time: Option<FnPlayerSetTime>,
    pub player_get_length: FnPlayerGetLength,
    pub audio_set_volume: FnAudioSetVolume,
    pub player_set_rate: FnPlayerSetRate,
    pub errmsg: Option<FnErrmsg>,
}

impl LibVlcApi {
    /// Load libvlc from a directory containing `libvlc.dll` / `libvlc.so`.
    pub unsafe fn load(lib_dir: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        let lib_name = "libvlc.dll";
        #[cfg(target_os = "macos")]
        let lib_name = "libvlc.dylib";
        #[cfg(all(unix, not(target_os = "macos")))]
        let lib_name = "libvlc.so";

        let lib_path = lib_dir.join(lib_name);
        if !lib_path.is_file() {
            let lib =
                Library::new(lib_name).map_err(|e| format!("failed to load {lib_name}: {e}"))?;
            return Self::from_library(lib);
        }

        #[cfg(windows)]
        {
            prepend_dll_directory(lib_dir);
        }

        let lib = Library::new(&lib_path)
            .map_err(|e| format!("failed to load {}: {e}", lib_path.display()))?;
        Self::from_library(lib)
    }

    unsafe fn from_library(lib: Library) -> Result<Self, String> {
        unsafe fn req<T>(lib: &Library, name: &[u8]) -> Result<T, String>
        where
            T: Copy,
        {
            let sym: Symbol<T> = lib
                .get(name)
                .map_err(|e| format!("missing symbol {}: {e}", String::from_utf8_lossy(name)))?;
            Ok(*sym)
        }

        unsafe fn opt<T>(lib: &Library, name: &[u8]) -> Option<T>
        where
            T: Copy,
        {
            lib.get::<T>(name).ok().map(|s| *s)
        }

        Ok(Self {
            new: req(&lib, b"libvlc_new\0")?,
            release: req(&lib, b"libvlc_release\0")?,
            media_new_path: req(&lib, b"libvlc_media_new_path\0")?,
            media_new_location: opt(&lib, b"libvlc_media_new_location\0"),
            media_release: req(&lib, b"libvlc_media_release\0")?,
            player_new: req(&lib, b"libvlc_media_player_new\0")?,
            player_release: req(&lib, b"libvlc_media_player_release\0")?,
            player_set_media: req(&lib, b"libvlc_media_player_set_media\0")?,
            player_play: req(&lib, b"libvlc_media_player_play\0")?,
            player_stop: req(&lib, b"libvlc_media_player_stop\0")?,
            player_pause: req(&lib, b"libvlc_media_player_pause\0")?,
            player_get_state: req(&lib, b"libvlc_media_player_get_state\0")?,
            player_get_time: req(&lib, b"libvlc_media_player_get_time\0")?,
            // VLC 3.x: void set_time(player, time). VLC 4 changed ABI — optional.
            player_set_time: opt(&lib, b"libvlc_media_player_set_time\0"),
            player_get_length: req(&lib, b"libvlc_media_player_get_length\0")?,
            audio_set_volume: req(&lib, b"libvlc_audio_set_volume\0")?,
            player_set_rate: req(&lib, b"libvlc_media_player_set_rate\0")?,
            errmsg: opt(&lib, b"libvlc_errmsg\0"),
            _lib: lib,
        })
    }

    pub fn last_error(&self) -> Option<String> {
        let f = self.errmsg?;
        unsafe {
            let p = f();
            if p.is_null() {
                return None;
            }
            Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        }
    }
}

#[cfg(windows)]
fn prepend_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;

    if let Some(old) = std::env::var_os("PATH") {
        let mut new_path = dir.as_os_str().to_os_string();
        new_path.push(";");
        new_path.push(old);
        std::env::set_var("PATH", new_path);
    } else {
        std::env::set_var("PATH", dir);
    }

    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        if let Ok(k32) = Library::new("kernel32.dll") {
            type AddDllDirectory = unsafe extern "system" fn(*const u16) -> *mut c_void;
            if let Ok(f) = k32.get::<AddDllDirectory>(b"AddDllDirectory\0") {
                let _ = f(wide.as_ptr());
            }
            std::mem::forget(k32);
        }
    }
}
