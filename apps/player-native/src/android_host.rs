//! Android platform host bridge for `player-native`, backed by Jetpack `GameActivity`
//! (`org.nir.player.NirGameActivity` in `apps/player-android/android`).
//!
//! Replaces fragile hand-written JNI reflection with thin strongly-typed calls into
//! `NirGameActivity.kt` plus an asynchronous Kotlin -> Rust event bus for:
//! - SAF `.nirpak` picker & `ACTION_VIEW` file open
//! - SAF `nir-backup-v1` save export (`ACTION_CREATE_DOCUMENT`) & import (`ACTION_OPEN_DOCUMENT`)
//! - `AudioManager` AudioFocus & `ACTION_AUDIO_BECOMING_NOISY` (headphone unplug auto-pause)
//! - `WindowInsetsCompat` display cutout & gesture safe-area insets
//! - `MediaStore` Gallery screenshot saving, `ClipboardManager`, `Vibrator`, and power-save hints

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Asynchronous events pushed from Kotlin (`NirGameActivity` / `NirHostBridge`) into `NativeApp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidHostEvent {
    /// A `.nirpak` file was picked via SAF or opened via `ACTION_VIEW` and staged to a local path.
    PakImported { path: PathBuf, launch_now: bool },
    /// A `nir-backup-v1` save backup JSON was picked via SAF or `ACTION_VIEW` and staged locally.
    SaveBackupImported(PathBuf),
    /// System audio focus or noisy-output (`ACTION_AUDIO_BECOMING_NOISY`) changed pause state.
    AudioFocusPaused(bool),
    /// Updated physical-pixel safe insets `[left, top, right, bottom]` from `WindowInsetsCompat`.
    SafeInsetsPx([i32; 4]),
    /// Informational toast message emitted by the Kotlin host (e.g., SAF export completed / failed).
    Toast(String),
}

type EventWaker = Arc<dyn Fn() + Send + Sync>;

struct HostEventBus {
    queue: VecDeque<AndroidHostEvent>,
    waker: Option<EventWaker>,
}

fn event_bus() -> &'static Mutex<HostEventBus> {
    static BUS: OnceLock<Mutex<HostEventBus>> = OnceLock::new();
    BUS.get_or_init(|| {
        Mutex::new(HostEventBus {
            queue: VecDeque::new(),
            waker: None,
        })
    })
}

/// Registers a thread-safe waker (backed by `winit::EventLoopProxy`) invoked whenever Kotlin
/// enqueues a new [`AndroidHostEvent`].
pub fn set_event_waker(waker: Option<EventWaker>) {
    let wake_now = {
        let mut bus = event_bus().lock().unwrap_or_else(|e| e.into_inner());
        let has_pending = !bus.queue.is_empty();
        bus.waker = waker;
        has_pending.then(|| bus.waker.clone()).flatten()
    };
    if let Some(w) = wake_now {
        w();
    }
}

/// Enqueues an event from Kotlin (`NirHostBridge`) and wakes the native event loop.
pub fn push_host_event(ev: AndroidHostEvent) {
    let waker = {
        let mut bus = event_bus().lock().unwrap_or_else(|e| e.into_inner());
        bus.queue.push_back(ev);
        bus.waker.clone()
    };
    if let Some(w) = waker {
        w();
    }
}

/// Drains all pending [`AndroidHostEvent`] items into a caller-owned `Vec`.
pub fn drain_host_events() -> Vec<AndroidHostEvent> {
    let mut bus = event_bus().lock().unwrap_or_else(|e| e.into_inner());
    bus.queue.drain(..).collect()
}

/// Computes the Android GPU texture cache budget (bytes) based on backend and power state.
pub fn recommended_android_gpu_budget_bytes(is_gl: bool, eco_or_low_power: bool) -> u64 {
    match (is_gl, eco_or_low_power) {
        (true, true) => 64 * 1024 * 1024,
        (true, false) => 96 * 1024 * 1024,
        (false, true) => 80 * 1024 * 1024,
        (false, false) => 128 * 1024 * 1024,
    }
}

/// Logs an informational message to Android `logcat` (or `eprintln!` on non-Android targets).
#[cfg(not(target_os = "android"))]
pub fn log_info(_msg: &str) {}

#[cfg(not(target_os = "android"))]
pub fn log_warn(msg: &str) {
    eprintln!("player-native [WARN]: {msg}");
}

#[cfg(not(target_os = "android"))]
pub fn log_error(msg: &str) {
    eprintln!("player-native [ERR]: {msg}");
}

#[cfg(target_os = "android")]
mod imp {
    use android_activity::AndroidApp;
    use jni::objects::{JIntArray, JObject, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::mem::ManuallyDrop;

    const ANDROID_LOG_INFO: c_int = 4;
    const ANDROID_LOG_WARN: c_int = 5;
    const ANDROID_LOG_ERROR: c_int = 6;

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    fn logcat(prio: c_int, msg: &str) {
        static TAG: &[u8] = b"NIRPlayer\0";
        let sanitized: String = msg
            .chars()
            .map(|c| if c == '\0' { ' ' } else { c })
            .collect();
        if let Ok(cmsg) = CString::new(sanitized) {
            unsafe {
                __android_log_write(prio, TAG.as_ptr().cast::<c_char>(), cmsg.as_ptr());
            }
        }
    }

    pub fn log_info(msg: &str) {
        logcat(ANDROID_LOG_INFO, msg);
    }

    pub fn log_warn(msg: &str) {
        logcat(ANDROID_LOG_WARN, msg);
    }

    pub fn log_error(msg: &str) {
        logcat(ANDROID_LOG_ERROR, msg);
    }

    fn with_raw_activity<T, F>(vm_ptr: *mut c_void, activity_ptr: *mut c_void, f: F) -> Option<T>
    where
        F: FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
    {
        if vm_ptr.is_null() || activity_ptr.is_null() {
            return None;
        }
        let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        // SAFETY: `activity_ptr` is a borrowed global reference owned by `GameActivity`.
        // Wrapping in `ManuallyDrop` ensures we never call `DeleteLocalRef` on it.
        let activity = ManuallyDrop::new(unsafe { JObject::from_raw(activity_ptr.cast()) });
        match f(&mut env, &activity) {
            Ok(v) => Some(v),
            Err(err) => {
                log_warn(&format!("JNI host call failed: {err}"));
                let _ = env.exception_clear();
                None
            }
        }
    }

    fn with_activity<T, F>(app: &AndroidApp, f: F) -> Option<T>
    where
        F: FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
    {
        with_raw_activity(app.vm_as_ptr(), app.activity_as_ptr(), f)
    }

    /// Queries physical pixel safe insets `[left, top, right, bottom]` from `NirGameActivity.querySafeInsetsPx()`.
    pub fn query_window_insets_px(app: &AndroidApp) -> Option<[i32; 4]> {
        with_activity(app, |env, activity| {
            let obj = env
                .call_method(activity, "querySafeInsetsPx", "()[I", &[])?
                .l()?;
            if obj.is_null() {
                return Ok(None);
            }
            let arr = JIntArray::from(obj);
            let mut buf = [0i32; 4];
            if env.get_array_length(&arr)? >= 4 {
                env.get_int_array_region(&arr, 0, &mut buf)?;
                Ok(Some([
                    buf[0].max(0),
                    buf[1].max(0),
                    buf[2].max(0),
                    buf[3].max(0),
                ]))
            } else {
                Ok(None)
            }
        })
        .flatten()
    }

    /// Triggers a subtle Android haptic pulse via `NirGameActivity.triggerHaptic(durationMs)`.
    pub fn vibrate(app: &AndroidApp, duration_ms: u16) {
        if duration_ms == 0 {
            return;
        }
        let _ = with_activity(app, |env, activity| {
            env.call_method(
                activity,
                "triggerHaptic",
                "(I)V",
                &[JValue::Int(i32::from(duration_ms.clamp(5, 45)))],
            )?;
            Ok(())
        });
    }

    /// Checks Android Battery Saver / low battery (< 20%) via `NirGameActivity.isLowPowerMode()`.
    pub fn read_low_power_hint(app: &AndroidApp) -> bool {
        with_activity(app, |env, activity| {
            env.call_method(activity, "isLowPowerMode", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    /// Copies text to Android system clipboard via `NirGameActivity.copyToClipboard(text)`.
    pub fn copy_to_clipboard(app: &AndroidApp, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        with_activity(app, |env, activity| {
            let jtext = env.new_string(text)?;
            env.call_method(
                activity,
                "copyToClipboard",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&jtext)],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    /// Saves a PNG screenshot into Android Gallery (`MediaStore`) via `NirGameActivity.saveScreenshotToGallery`.
    pub fn save_screenshot_mediastore(app: &AndroidApp, filename: &str, png_bytes: &[u8]) -> bool {
        if png_bytes.is_empty() {
            return false;
        }
        with_activity(app, |env, activity| {
            let jname = env.new_string(filename)?;
            let jbytes = env.byte_array_from_slice(png_bytes)?;
            env.call_method(
                activity,
                "saveScreenshotToGallery",
                "(Ljava/lang/String;[B)Z",
                &[JValue::Object(&jname), JValue::Object(&jbytes.into())],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    /// Launches Android SAF file picker (`ACTION_OPEN_DOCUMENT`) to import & open a `.nirpak` package.
    pub fn launch_nirpak_picker(app: &AndroidApp) -> bool {
        with_activity(app, |env, activity| {
            env.call_method(activity, "launchNirpakPicker", "()V", &[])?;
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// Launches Android SAF save creator (`ACTION_CREATE_DOCUMENT`) to export a `nir-backup-v1` JSON.
    pub fn launch_save_export(
        app: &AndroidApp,
        suggested_filename: &str,
        json_payload: &str,
    ) -> bool {
        with_activity(app, |env, activity| {
            let jname = env.new_string(suggested_filename)?;
            let jbody = env.new_string(json_payload)?;
            env.call_method(
                activity,
                "launchSaveExport",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&jname), JValue::Object(&jbody)],
            )?;
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// Launches Android SAF file picker (`ACTION_OPEN_DOCUMENT`) to import a `nir-backup-v1` JSON.
    pub fn launch_save_import(app: &AndroidApp) -> bool {
        with_activity(app, |env, activity| {
            env.call_method(activity, "launchSaveImport", "()V", &[])?;
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// Requests system Android AudioFocus when active playback resumes.
    pub fn request_audio_focus(app: &AndroidApp) -> bool {
        with_activity(app, |env, activity| {
            env.call_method(activity, "requestAudioFocus", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    /// Releases system Android AudioFocus when the activity suspends.
    pub fn abandon_audio_focus(app: &AndroidApp) {
        let _ = with_activity(app, |env, activity| {
            env.call_method(activity, "abandonAudioFocus", "()V", &[])?;
            Ok(())
        });
    }

    /// Notifies `NirGameActivity` that Rust is ready to receive any cold-start `ACTION_VIEW` intent.
    pub fn consume_initial_pending_intent(app: &AndroidApp) {
        let _ = with_activity(app, |env, activity| {
            env.call_method(activity, "consumeInitialPendingIntent", "()V", &[])?;
            Ok(())
        });
    }
}

#[cfg(target_os = "android")]
pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_gpu_budget_scales_with_backend_and_eco_mode() {
        assert_eq!(
            recommended_android_gpu_budget_bytes(false, false),
            128 * 1024 * 1024
        );
        assert_eq!(
            recommended_android_gpu_budget_bytes(false, true),
            80 * 1024 * 1024
        );
        assert_eq!(
            recommended_android_gpu_budget_bytes(true, false),
            96 * 1024 * 1024
        );
        assert_eq!(
            recommended_android_gpu_budget_bytes(true, true),
            64 * 1024 * 1024
        );
    }

    #[test]
    fn host_event_bus_delivers_and_wakes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        set_event_waker(Some(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })));
        let _ = drain_host_events();
        let before = wakes.load(Ordering::SeqCst);
        push_host_event(AndroidHostEvent::AudioFocusPaused(true));
        push_host_event(AndroidHostEvent::Toast("SAF OK".into()));
        assert!(wakes.load(Ordering::SeqCst) >= before + 2);
        let drained = drain_host_events();
        assert_eq!(
            drained,
            vec![
                AndroidHostEvent::AudioFocusPaused(true),
                AndroidHostEvent::Toast("SAF OK".into()),
            ]
        );
        set_event_waker(None);
    }
}
