use std::ffi::CStr;

use ffmpeg_sys_next as sys;

pub fn version() -> String {
    // FFmpeg returns a static, NUL-terminated string owned by the library.
    unsafe {
        CStr::from_ptr(sys::av_version_info())
            .to_string_lossy()
            .into_owned()
    }
}

pub fn configuration() -> String {
    unsafe {
        CStr::from_ptr(sys::avutil_configuration())
            .to_string_lossy()
            .into_owned()
    }
}

pub fn library_majors() -> std::collections::BTreeMap<String, u32> {
    // FFmpeg encodes the ABI major in the high 16 bits of each library's
    // integer version. These calls also prove that every required shared
    // library was loaded, not merely libavutil.
    unsafe {
        std::collections::BTreeMap::from([
            ("avcodec".into(), ffmpeg_next::ffi::avcodec_version() >> 16),
            (
                "avformat".into(),
                ffmpeg_next::ffi::avformat_version() >> 16,
            ),
            ("avutil".into(), ffmpeg_next::ffi::avutil_version() >> 16),
            (
                "swresample".into(),
                ffmpeg_next::ffi::swresample_version() >> 16,
            ),
        ])
    }
}
