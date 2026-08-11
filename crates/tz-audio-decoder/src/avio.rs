//! Owned custom-AVIO boundary for local files.
//!
//! FFmpeg receives only callbacks over an already-open `std::fs::File`; no
//! protocol or URL opener is exposed. `LocalInput` owns the demuxer, AVIO
//! context, callback buffer, and opaque file state in their required drop
//! order.

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::ptr;

use ffmpeg_next as ffmpeg;
use ffmpeg_next::format;
use ffmpeg_sys_next as sys;
use tz_audio::ExitCode;

const AVIO_BUFFER_SIZE: usize = 32 * 1024;

struct LocalIo {
    file: File,
}

pub(crate) struct LocalInput {
    input: Option<format::context::Input>,
    avio: *mut sys::AVIOContext,
    io: *mut LocalIo,
}

impl LocalInput {
    pub(crate) fn open(path: &Path) -> Result<Self, (ExitCode, String)> {
        let file = File::open(path).map_err(|error| {
            (
                ExitCode::Input,
                format!("could not open local media file: {error}"),
            )
        })?;
        let io = Box::into_raw(Box::new(LocalIo { file }));

        // SAFETY: FFmpeg owns the AVIO buffer after avio_alloc_context succeeds. The
        // LocalIo allocation remains alive until LocalInput::drop, after the format
        // context has stopped invoking callbacks.
        unsafe {
            let buffer = sys::av_malloc(AVIO_BUFFER_SIZE) as *mut u8;
            if buffer.is_null() {
                drop(Box::from_raw(io));
                return Err((
                    ExitCode::LibraryCompatibility,
                    "FFmpeg could not allocate the AVIO buffer".into(),
                ));
            }
            let mut avio = sys::avio_alloc_context(
                buffer,
                AVIO_BUFFER_SIZE as i32,
                0,
                io.cast::<c_void>(),
                Some(read_packet),
                None,
                Some(seek),
            );
            if avio.is_null() {
                sys::av_free(buffer.cast::<c_void>());
                drop(Box::from_raw(io));
                return Err((
                    ExitCode::LibraryCompatibility,
                    "FFmpeg could not allocate the AVIO context".into(),
                ));
            }

            let mut context = sys::avformat_alloc_context();
            if context.is_null() {
                sys::avio_context_free(&mut avio);
                drop(Box::from_raw(io));
                return Err((
                    ExitCode::LibraryCompatibility,
                    "FFmpeg could not allocate the format context".into(),
                ));
            }
            (*context).pb = avio;
            (*context).flags |= sys::AVFMT_FLAG_CUSTOM_IO;

            let open_result = sys::avformat_open_input(
                &mut context,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if open_result < 0 {
                if !context.is_null() {
                    sys::avformat_close_input(&mut context);
                }
                sys::avio_context_free(&mut avio);
                drop(Box::from_raw(io));
                return Err((
                    ExitCode::UnsupportedMedia,
                    format!(
                        "FFmpeg could not probe local media: {}",
                        ffmpeg::Error::from(open_result)
                    ),
                ));
            }

            let stream_result = sys::avformat_find_stream_info(context, ptr::null_mut());
            if stream_result < 0 {
                sys::avformat_close_input(&mut context);
                sys::avio_context_free(&mut avio);
                drop(Box::from_raw(io));
                return Err((
                    ExitCode::UnsupportedMedia,
                    format!(
                        "FFmpeg could not read stream information: {}",
                        ffmpeg::Error::from(stream_result)
                    ),
                ));
            }

            Ok(Self {
                input: Some(format::context::Input::wrap(context)),
                avio,
                io,
            })
        }
    }
}

impl Deref for LocalInput {
    type Target = format::context::Input;

    fn deref(&self) -> &Self::Target {
        self.input.as_ref().expect("local input is alive")
    }
}

impl DerefMut for LocalInput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.input.as_mut().expect("local input is alive")
    }
}

impl Drop for LocalInput {
    fn drop(&mut self) {
        // Drop the demuxer first so no callback can run after LocalIo is released.
        drop(self.input.take());
        // SAFETY: both pointers were allocated by LocalInput::open, are uniquely
        // owned here, and no FFmpeg callback can run after the input was dropped.
        unsafe {
            sys::avio_context_free(&mut self.avio);
            drop(Box::from_raw(self.io));
        }
    }
}

unsafe extern "C" fn read_packet(opaque: *mut c_void, buffer: *mut u8, size: i32) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if opaque.is_null() || buffer.is_null() || size <= 0 {
            return sys::AVERROR_EXTERNAL;
        }
        // SAFETY: FFmpeg passes the LocalIo pointer and writable buffer supplied
        // by LocalInput::open, and `size` was validated positive above.
        let io = unsafe { &mut *opaque.cast::<LocalIo>() };
        let output = unsafe { std::slice::from_raw_parts_mut(buffer, size as usize) };
        match io.file.read(output) {
            Ok(0) => sys::AVERROR_EOF,
            Ok(read) => read as i32,
            Err(_) => sys::AVERROR_EXTERNAL,
        }
    }))
    .unwrap_or(sys::AVERROR_EXTERNAL)
}

unsafe extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: i32) -> i64 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if opaque.is_null() {
            return i64::from(sys::AVERROR_EXTERNAL);
        }
        // SAFETY: FFmpeg passes the live LocalIo pointer supplied by
        // LocalInput::open and calls this callback serially for its input.
        let io = unsafe { &mut *opaque.cast::<LocalIo>() };
        if whence & sys::AVSEEK_SIZE != 0 {
            return io
                .file
                .metadata()
                .ok()
                .and_then(|metadata| i64::try_from(metadata.len()).ok())
                .unwrap_or(i64::from(sys::AVERROR_EXTERNAL));
        }
        let mode = match whence & !sys::AVSEEK_FORCE {
            0 => u64::try_from(offset).ok().map(SeekFrom::Start),
            1 => Some(SeekFrom::Current(offset)),
            2 => Some(SeekFrom::End(offset)),
            _ => None,
        };
        match mode.and_then(|mode| io.file.seek(mode).ok()) {
            Some(position) => i64::try_from(position).unwrap_or(i64::MAX),
            None => i64::from(sys::AVERROR_EXTERNAL),
        }
    }))
    .unwrap_or(i64::from(sys::AVERROR_EXTERNAL))
}
