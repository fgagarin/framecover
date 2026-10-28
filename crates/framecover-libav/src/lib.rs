//! In-process FFmpeg for framecover-core's `libav` backend (Android, and
//! Linux for parity tests), where starting `ffmpeg` processes is not an
//! option.
//!
//! * [`run_tool`] runs the real `ffmpeg` / `ffprobe` front ends, linked in
//!   as `ffmpeg_main` / `ffprobe_main`, with the same arguments the
//!   subprocess backend passes. Runs are serialised (the front ends keep
//!   their state in globals, which `fc_shim.c` resets before every run).
//! * [`Decoder`] decodes frames to RGB24 exactly like
//!   `ffmpeg -noautorotate [-ss S] -i IN -map 0:V:0 [-vf F] -pix_fmt rgb24`,
//!   and any number of decoders can run concurrently.
//!
//! Builds for Android and Linux only (empty elsewhere).
#![cfg(unix)]

use std::ffi::{CStr, CString, OsStr, c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::{Mutex, Once};

mod ffi {
    use std::ffi::{c_char, c_int};

    pub enum FcDecoder {}

    #[repr(C)]
    pub struct FcFrame {
        pub pts: i64,
        pub width: c_int,
        pub height: c_int,
        pub data: *const u8,
    }

    pub type LogFn = extern "C" fn(level: c_int, line: *const c_char);

    unsafe extern "C" {
        pub fn fc_init(sink: LogFn, level: c_int);
        pub fn fc_tool_run(tool: c_int, argc: c_int, argv: *mut *mut c_char) -> c_int;
        pub fn fc_codec_next(
            opaque: *mut *mut std::ffi::c_void,
            encoders: c_int,
            name: *mut *const c_char,
            long_name: *mut *const c_char,
            kind: *mut c_char,
        ) -> c_int;

        pub fn fc_decoder_open(
            path: *const c_char,
            filter: *const c_char,
            err: *mut c_char,
            err_len: usize,
        ) -> *mut FcDecoder;
        pub fn fc_decoder_close(d: *mut FcDecoder);
        pub fn fc_decoder_time_base(d: *const FcDecoder, num: *mut c_int, den: *mut c_int);
        pub fn fc_decoder_start_us(d: *const FcDecoder) -> i64;
        pub fn fc_decoder_seek(d: *mut FcDecoder, seek_us: i64, threshold: i64) -> c_int;
        pub fn fc_decoder_next(
            d: *mut FcDecoder,
            out: *mut FcFrame,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
    }
}

/// `AV_LOG_*` levels.
const AV_LOG_ERROR: c_int = 16;
const AV_LOG_WARNING: c_int = 24;
const AV_LOG_INFO: c_int = 32;

/// `AV_NOPTS_VALUE`.
const NOPTS: i64 = i64::MIN;

static INIT: Once = Once::new();

/// Log text of the running front end (`Some` while [`run_tool`] runs).
static CAPTURE: Mutex<Option<String>> = Mutex::new(None);

/// Serialises [`run_tool`].
static TOOL_LOCK: Mutex<()> = Mutex::new(());

fn init() {
    // SAFETY: called once, before any other libav use from this crate.
    INIT.call_once(|| unsafe { ffi::fc_init(log_line, AV_LOG_WARNING) });
}

extern "C" fn log_line(level: c_int, line: *const c_char) {
    if line.is_null() {
        return;
    }
    // SAFETY: fc_shim passes a NUL-terminated line valid for this call.
    let text = unsafe { CStr::from_ptr(line) }.to_string_lossy();
    if let Ok(mut capture) = CAPTURE.lock()
        && let Some(buf) = capture.as_mut()
    {
        buf.push_str(&text);
        return;
    }
    let text = text.trim_end();
    if level <= AV_LOG_ERROR {
        log::error!(target: "libav", "{text}");
    } else if level <= AV_LOG_WARNING {
        log::warn!(target: "libav", "{text}");
    } else if level <= AV_LOG_INFO {
        log::info!(target: "libav", "{text}");
    } else {
        log::debug!(target: "libav", "{text}");
    }
}

/// Which front end [`run_tool`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
}

/// Result of a [`run_tool`] call.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Exit status as a process would report it (`0..=255`).
    pub code: i32,
    /// Everything the run logged (what the process printed on stderr).
    pub stderr: String,
}

/// `bytes` as a C string; interior NULs (never present in paths or
/// arguments) are dropped rather than truncating.
fn c_string(bytes: &[u8]) -> CString {
    let bytes: Vec<u8> = bytes.iter().copied().filter(|&b| b != 0).collect();
    CString::new(bytes).expect("NUL bytes were removed")
}

/// Run `ffmpeg` / `ffprobe` with `args` (without the program name) in this
/// process. Output files named in `args` are written as usual; nothing is
/// written to this process's stdout (pass output paths, not `-`).
pub fn run_tool<S: AsRef<OsStr>>(tool: Tool, args: &[S]) -> ToolOutput {
    init();
    let program = match tool {
        Tool::Ffmpeg => "ffmpeg",
        Tool::Ffprobe => "ffprobe",
    };
    let owned: Vec<CString> = std::iter::once(c_string(program.as_bytes()))
        .chain(args.iter().map(|a| c_string(a.as_ref().as_bytes())))
        .collect();
    let mut argv: Vec<*mut c_char> = owned.iter().map(|a| a.as_ptr().cast_mut()).collect();
    argv.push(std::ptr::null_mut());

    let _guard = TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *CAPTURE.lock().unwrap_or_else(|e| e.into_inner()) = Some(String::new());
    let argc = c_int::try_from(owned.len()).unwrap_or(c_int::MAX);
    // SAFETY: argv holds `argc` valid C strings plus a terminating NULL and
    // outlives the call; TOOL_LOCK keeps runs from overlapping.
    let ret = unsafe { ffi::fc_tool_run(tool as c_int, argc, argv.as_mut_ptr()) };
    let stderr = CAPTURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default();
    ToolOutput {
        // A process exit status keeps the low byte (`return AVERROR(x)` from
        // main exits with 256 - x).
        code: ret & 0xff,
        stderr,
    }
}

/// A codec of this build, as `ffmpeg -encoders` / `-decoders` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codec {
    /// `'V'`, `'A'`, `'S'` or `'D'` (data).
    pub kind: char,
    pub name: String,
    pub long_name: String,
}

/// The encoders (`encoders == true`) or decoders compiled into this build.
pub fn codecs(encoders: bool) -> Vec<Codec> {
    init();
    let mut list = Vec::new();
    let mut opaque = std::ptr::null_mut();
    let (mut name, mut long_name, mut kind) = (std::ptr::null(), std::ptr::null(), 0 as c_char);
    // SAFETY: fc_codec_next fills the pointers with static C strings while
    // it returns non-zero; `opaque` is only touched by it.
    while unsafe {
        ffi::fc_codec_next(
            &mut opaque,
            c_int::from(encoders),
            &mut name,
            &mut long_name,
            &mut kind,
        )
    } != 0
    {
        // SAFETY: as above.
        let (name, long_name) = unsafe { (CStr::from_ptr(name), CStr::from_ptr(long_name)) };
        list.push(Codec {
            kind: char::from(kind as u8),
            name: name.to_string_lossy().into_owned(),
            long_name: long_name.to_string_lossy().into_owned(),
        });
    }
    list
}

/// One decoded RGB24 frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Timestamp in the stream's time base; `None` if the frame has none.
    pub pts: Option<i64>,
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGB24 rows.
    pub data: Vec<u8>,
}

/// Decodes the first video stream that is not a cover picture.
pub struct Decoder {
    raw: NonNull<ffi::FcDecoder>,
}

// SAFETY: an FcDecoder is only used through `&mut self` (one thread at a
// time) and owns all of its libav state.
unsafe impl Send for Decoder {}

const ERR_LEN: usize = 512;

fn error_text(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl Decoder {
    /// Open `path`; `filter` is the `-vf` chain applied before the
    /// conversion to RGB24.
    pub fn open(path: &Path, filter: Option<&str>) -> Result<Self, String> {
        init();
        let path = c_string(path.as_os_str().as_bytes());
        let filter = filter.map(|f| c_string(f.as_bytes()));
        let mut err = [0u8; ERR_LEN];
        // SAFETY: valid C strings (or NULL) and an error buffer of ERR_LEN.
        let raw = unsafe {
            ffi::fc_decoder_open(
                path.as_ptr(),
                filter.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
                err.as_mut_ptr().cast(),
                ERR_LEN,
            )
        };
        NonNull::new(raw)
            .map(|raw| Self { raw })
            .ok_or_else(|| error_text(&err))
    }

    /// The stream time base as `(num, den)`.
    pub fn time_base(&self) -> (i64, i64) {
        let (mut num, mut den) = (0, 0);
        // SAFETY: self.raw is a live decoder.
        unsafe { ffi::fc_decoder_time_base(self.raw.as_ptr(), &mut num, &mut den) };
        (i64::from(num), i64::from(den))
    }

    /// Container start time in µs (0 when unknown).
    pub fn start_us(&self) -> i64 {
        // SAFETY: self.raw is a live decoder.
        unsafe { ffi::fc_decoder_start_us(self.raw.as_ptr()) }
    }

    /// Seek like input `-ss seek_us` (no-op at 0) and drop frames whose
    /// timestamp is below `threshold` ticks. Call before the first frame.
    pub fn seek(&mut self, seek_us: u64, threshold: Option<i64>) {
        let seek = i64::try_from(seek_us).unwrap_or(i64::MAX);
        // SAFETY: self.raw is a live decoder.
        unsafe { ffi::fc_decoder_seek(self.raw.as_ptr(), seek, threshold.unwrap_or(NOPTS)) };
    }

    /// The next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let mut out = ffi::FcFrame {
            pts: NOPTS,
            width: 0,
            height: 0,
            data: std::ptr::null(),
        };
        let mut err = [0u8; ERR_LEN];
        // SAFETY: self.raw is a live decoder; `out` and `err` are valid.
        let ret = unsafe {
            ffi::fc_decoder_next(
                self.raw.as_ptr(),
                &mut out,
                err.as_mut_ptr().cast(),
                ERR_LEN,
            )
        };
        match ret {
            0 => {
                let (width, height) = (out.width.max(0) as u32, out.height.max(0) as u32);
                let len = width as usize * height as usize * 3;
                // SAFETY: on success `data` points to `len` bytes owned by the
                // decoder, valid until the next call.
                let data = unsafe { std::slice::from_raw_parts(out.data, len) }.to_vec();
                Ok(Some(Frame {
                    pts: (out.pts != NOPTS).then_some(out.pts),
                    width,
                    height,
                    data,
                }))
            }
            1 => Ok(None),
            _ => Err(error_text(&err)),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: self.raw came from fc_decoder_open and is closed once.
        unsafe { ffi::fc_decoder_close(self.raw.as_ptr()) };
    }
}
