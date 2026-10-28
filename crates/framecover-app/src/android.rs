//! Android entry point and the JNI bridge to `MainActivity`
//! (`android/app/src/main/java/io/github/fgagarin/framecover/MainActivity.java`).
//!
//! The Java side does what needs intents or content URIs:
//! * the document picker: the chosen file is copied into the app's cache,
//!   because FFmpeg and the writers work on real paths;
//! * publishing saved copies to the gallery (`Movies/FrameCover`);
//! * opening links and published videos.
//!
//! Calls into Java go through the activity object (no class lookups, which
//! fail on native threads); Java calls back through `nativeOnPicked`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};

use jni_sys::{
    JNI_EDETACHED, JNI_OK, JNI_VERSION_1_6, JNIEnv, JavaVM, jclass, jobject, jstring, jvalue,
};
use slint::android::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

type PickCallback = Box<dyn FnOnce(PathBuf) + Send>;

/// What to do with the file of the picker that is open (at most one).
static PENDING_PICK: Mutex<Option<PickCallback>> = Mutex::new(None);

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("FrameCover"),
    );
    if let Some(dir) = app.internal_data_path() {
        // Temporary files (tempfile, the writers' staging files) go to the
        // app's own storage; Android sets no TMPDIR.
        let tmp = dir.join("tmp");
        if std::fs::create_dir_all(&tmp).is_ok() {
            // SAFETY: no other thread exists yet that could read the environment.
            unsafe { std::env::set_var("TMPDIR", &tmp) };
        }
        let _ = DATA_DIR.set(dir);
    }
    let _ = APP.set(app.clone());
    if let Err(err) = slint::android::init(app) {
        log::error!("could not start the Slint Android backend: {err}");
        std::process::exit(1);
    }
    crate::ui::run_mobile();
    // The activity is gone. Android may keep the process and start a new
    // activity in it, but `APP` and Slint's platform can only be set once:
    // end the process so the next launch starts clean.
    std::process::exit(0);
}

/// The app's private files directory (settings live here).
pub fn data_dir() -> Option<PathBuf> {
    DATA_DIR.get().cloned()
}

/// Show the system picker for `mime` (`video/*`, `image/*`); `on_pick`
/// receives the cached copy of the chosen file. Nothing happens on cancel.
pub fn pick_file(mime: &str, on_pick: impl FnOnce(PathBuf) + Send + 'static) {
    *PENDING_PICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(on_pick));
    let result = with_activity(|env, activity| unsafe {
        let mime = JavaString::new(env, mime)?;
        call_void(
            env,
            activity,
            "pickFile",
            "(Ljava/lang/String;)V",
            &[mime.arg()],
        )
    });
    if let Err(err) = result {
        log::error!("could not open the file picker: {err}");
        PENDING_PICK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

/// Called by `MainActivity` once a picked file is in the cache (`path`), or
/// with `null` when the picker was cancelled or copying failed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_fgagarin_framecover_MainActivity_nativeOnPicked(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
) {
    // SAFETY: called by the JVM with a valid env and a String (or null).
    let path = unsafe { read_string(env, path) };
    let callback = PENDING_PICK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let (Some(callback), Some(path)) = (callback, path) {
        callback(PathBuf::from(path));
    }
}

/// Copy the saved video at `path` into the shared `Movies/FrameCover`
/// collection, so the gallery and other apps see it.
pub fn publish_video(path: &Path) -> Result<(), String> {
    with_activity(|env, activity| unsafe {
        let path = JavaString::new(env, &path.to_string_lossy())?;
        call_void(
            env,
            activity,
            "publishVideo",
            "(Ljava/lang/String;)V",
            &[path.arg()],
        )
    })
}

/// Open the published copy of `path` in the gallery / a video player.
pub fn view_published(path: &Path) -> bool {
    with_activity(|env, activity| unsafe {
        let path = JavaString::new(env, &path.to_string_lossy())?;
        call_void(
            env,
            activity,
            "viewPublished",
            "(Ljava/lang/String;)V",
            &[path.arg()],
        )
    })
    .map_err(|err| log::warn!("could not show {}: {err}", path.display()))
    .is_ok()
}

/// Open `url` in the browser.
pub fn open_url(url: &str) -> bool {
    with_activity(|env, activity| unsafe {
        let url = JavaString::new(env, url)?;
        call_void(
            env,
            activity,
            "openUrl",
            "(Ljava/lang/String;)V",
            &[url.arg()],
        )
    })
    .map_err(|err| log::warn!("could not open {url}: {err}"))
    .is_ok()
}

// ---------------------------------------------------------------------------
// JNI plumbing
// ---------------------------------------------------------------------------

/// Run `f` with this thread attached to the JVM and the activity object
/// (attaching, and detaching afterwards, threads that were not attached).
fn with_activity<R>(
    f: impl FnOnce(*mut JNIEnv, jobject) -> Result<R, String>,
) -> Result<R, String> {
    let app = APP.get().ok_or("the activity is not running")?;
    let vm = app.vm_as_ptr().cast::<JavaVM>();
    let activity = app.activity_as_ptr() as jobject;
    // SAFETY: `vm` is the process's JavaVM, valid for the app's lifetime.
    unsafe {
        let mut env: *mut c_void = null_mut();
        let attached_here = match (**vm).GetEnv.expect("GetEnv")(vm, &mut env, JNI_VERSION_1_6) {
            JNI_OK => false,
            JNI_EDETACHED => {
                if (**vm).AttachCurrentThread.expect("AttachCurrentThread")(
                    vm,
                    &mut env,
                    null_mut(),
                ) != JNI_OK
                {
                    return Err("could not attach to the JVM".into());
                }
                true
            }
            code => return Err(format!("JNI GetEnv failed ({code})")),
        };
        let result = f(env.cast(), activity);
        if attached_here {
            (**vm).DetachCurrentThread.expect("DetachCurrentThread")(vm);
        }
        result
    }
}

/// A local reference to a new `java.lang.String`, deleted on drop.
struct JavaString {
    env: *mut JNIEnv,
    raw: jstring,
}

impl JavaString {
    unsafe fn new(env: *mut JNIEnv, text: &str) -> Result<Self, String> {
        // Modified UTF-8 differs from UTF-8 only for NUL and astral
        // characters, which paths, MIME types and URLs here do not contain.
        let text = CString::new(text).map_err(|_| "string contains NUL")?;
        // SAFETY: valid env and C string.
        let raw = unsafe { (**env).NewStringUTF.expect("NewStringUTF")(env, text.as_ptr()) };
        if raw.is_null() {
            return Err(unsafe { take_exception(env) }.unwrap_or_else(|| "out of memory".into()));
        }
        Ok(Self { env, raw })
    }

    fn arg(&self) -> jvalue {
        jvalue { l: self.raw }
    }
}

impl Drop for JavaString {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live local reference created with `env`.
        unsafe { (**self.env).DeleteLocalRef.expect("DeleteLocalRef")(self.env, self.raw) };
    }
}

/// Call `void name(sig)` on `object`, turning a Java exception into an error.
unsafe fn call_void(
    env: *mut JNIEnv,
    object: jobject,
    name: &str,
    sig: &str,
    args: &[jvalue],
) -> Result<(), String> {
    let (name_c, sig_c) = (CString::new(name).unwrap(), CString::new(sig).unwrap());
    // SAFETY: valid env, object and C strings; `args` matches `sig`.
    unsafe {
        let class = (**env).GetObjectClass.expect("GetObjectClass")(env, object);
        let method =
            (**env).GetMethodID.expect("GetMethodID")(env, class, name_c.as_ptr(), sig_c.as_ptr());
        (**env).DeleteLocalRef.expect("DeleteLocalRef")(env, class);
        if method.is_null() {
            let _ = take_exception(env);
            return Err(format!("MainActivity has no method {name}{sig}"));
        }
        (**env).CallVoidMethodA.expect("CallVoidMethodA")(env, object, method, args.as_ptr());
        match take_exception(env) {
            Some(err) => Err(format!("{name} failed: {err}")),
            None => Ok(()),
        }
    }
}

/// Clear a pending Java exception (logging it); `None` if there was none.
unsafe fn take_exception(env: *mut JNIEnv) -> Option<String> {
    // SAFETY: valid env.
    unsafe {
        if (**env).ExceptionCheck.expect("ExceptionCheck")(env) == 0 {
            return None;
        }
        (**env).ExceptionDescribe.expect("ExceptionDescribe")(env);
        (**env).ExceptionClear.expect("ExceptionClear")(env);
    }
    Some("Java exception (see logcat)".into())
}

/// The contents of a Java string, or `None` for `null`.
unsafe fn read_string(env: *mut JNIEnv, string: jstring) -> Option<String> {
    if string.is_null() {
        return None;
    }
    // SAFETY: valid env and String reference; the chars are released below.
    unsafe {
        let chars: *const c_char =
            (**env).GetStringUTFChars.expect("GetStringUTFChars")(env, string, null_mut());
        if chars.is_null() {
            return None;
        }
        let text = CStr::from_ptr(chars).to_string_lossy().into_owned();
        (**env)
            .ReleaseStringUTFChars
            .expect("ReleaseStringUTFChars")(env, string, chars);
        Some(text)
    }
}
