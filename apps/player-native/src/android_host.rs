//! Android platform host integrations for `player-native` (pure Rust + minimal JNI, zero Java/DEX!).
//!
//! Preserves `nir-apk`'s `hasCode="false"` zero-Gradle APK architecture while exposing:
//! - Direct `liblog` (`__android_log_print`) logcat diagnostics
//! - UI-thread immersive fullscreen + `shortEdges` display cutout setup (`android_on_create`)
//! - Accurate `WindowInsets` + `DisplayCutout` safe-area probing (API 28+ / 30+)
//! - Native haptic feedback (`Vibrator` / `VibrationEffect.createOneShot`, API 26+)
//! - Power-save & low-battery hint (`PowerManager.isPowerSaveMode` + `BatteryManager`)
//! - Native clipboard copy (`ClipboardManager`)
//! - Screenshot export to user-accessible external pictures / `MediaStore`

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
    use jni::objects::{JObject, JValue};
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
        // SAFETY: `activity_ptr` is a borrowed global reference owned by `NativeActivity`.
        // Wrapping in `ManuallyDrop` ensures we never call `DeleteLocalRef` on it.
        let activity = ManuallyDrop::new(unsafe { JObject::from_raw(activity_ptr.cast()) });
        match f(&mut env, &activity) {
            Ok(v) => Some(v),
            Err(_) => {
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

    fn sdk_int(env: &mut JNIEnv<'_>) -> i32 {
        env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
            .and_then(|v| v.i())
            .unwrap_or(26)
    }

    /// Configures immersive sticky fullscreen + `LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES`.
    /// Must be invoked on the Java UI thread (e.g. from `android_on_create`).
    pub fn apply_immersive_on_ui_thread(vm_ptr: *mut c_void, activity_ptr: *mut c_void) {
        let _ = with_raw_activity(vm_ptr, activity_ptr, |env, activity| {
            let api = sdk_int(env);
            let window = env
                .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
                .l()?;
            if window.is_null() {
                return Ok(());
            }

            // API 28+: Allow rendering into display cutout (notch / punch-hole) area so
            // `SafeInsets` cleanly frames content edge-to-edge without ugly black sidebars.
            if api >= 28 {
                if let Ok(lp_val) = env.call_method(
                    &window,
                    "getAttributes",
                    "()Landroid/view/WindowManager$LayoutParams;",
                    &[],
                ) {
                    if let Ok(lp) = lp_val.l() {
                        if !lp.is_null() {
                            // LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES = 1
                            let _ = env.set_field(
                                &lp,
                                "layoutInDisplayCutoutMode",
                                "I",
                                JValue::Int(1),
                            );
                            let _ = env.call_method(
                                &window,
                                "setAttributes",
                                "(Landroid/view/WindowManager$LayoutParams;)V",
                                &[JValue::Object(&lp)],
                            );
                        }
                    }
                }
                let _ = env.exception_clear();
            }

            let decor = env
                .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
                .l()?;
            if decor.is_null() {
                return Ok(());
            }

            if api >= 30 {
                // WindowInsetsController: hide status + navigation bars with transient swipe reveal.
                let _ = env.call_method(
                    &window,
                    "setDecorFitsSystemWindows",
                    "(Z)V",
                    &[JValue::Bool(0)],
                );
                if let Ok(ctrl_val) = env.call_method(
                    &decor,
                    "getWindowInsetsController",
                    "()Landroid/view/WindowInsetsController;",
                    &[],
                ) {
                    if let Ok(ctrl) = ctrl_val.l() {
                        if !ctrl.is_null() {
                            // WindowInsets.Type.systemBars() = 7 (statusBars=1 | navigationBars=2 | captionBar=4)
                            let bars = env
                                .call_static_method(
                                    "android/view/WindowInsets$Type",
                                    "systemBars",
                                    "()I",
                                    &[],
                                )
                                .and_then(|v| v.i())
                                .unwrap_or(7);
                            let _ = env.call_method(&ctrl, "hide", "(I)V", &[JValue::Int(bars)]);
                            // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE = 2
                            let _ = env.call_method(
                                &ctrl,
                                "setSystemBarsBehavior",
                                "(I)V",
                                &[JValue::Int(2)],
                            );
                        }
                    }
                }
                let _ = env.exception_clear();
            } else {
                // API 26-29: IMMERSIVE_STICKY | FULLSCREEN | HIDE_NAVIGATION | LAYOUT_*
                const FLAGS: i32 = 0x0000_0100 // LAYOUT_STABLE
                    | 0x0000_0200 // LAYOUT_HIDE_NAVIGATION
                    | 0x0000_0400 // LAYOUT_FULLSCREEN
                    | 0x0000_0002 // HIDE_NAVIGATION
                    | 0x0000_0004 // FULLSCREEN
                    | 0x0000_1000; // IMMERSIVE_STICKY
                let _ = env.call_method(
                    &decor,
                    "setSystemUiVisibility",
                    "(I)V",
                    &[JValue::Int(FLAGS)],
                );
                let _ = env.exception_clear();
            }
            Ok(())
        });
    }

    /// Queries real physical pixel safe insets `(left, top, right, bottom)` from
    /// `DecorView.getRootWindowInsets()` (display cutout + system gesture/bar insets).
    pub fn query_window_insets_px(app: &AndroidApp) -> Option<[i32; 4]> {
        with_activity(app, |env, activity| {
            let api = sdk_int(env);
            let window = env
                .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
                .l()?;
            if window.is_null() {
                return Ok(None);
            }
            let decor = env
                .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
                .l()?;
            if decor.is_null() {
                return Ok(None);
            }
            let insets = env
                .call_method(
                    &decor,
                    "getRootWindowInsets",
                    "()Landroid/view/WindowInsets;",
                    &[],
                )?
                .l()?;
            if insets.is_null() {
                return Ok(None);
            }

            let mut left = 0i32;
            let mut top = 0i32;
            let mut right = 0i32;
            let mut bottom = 0i32;

            if api >= 30 {
                // `displayCutout()` covers camera notches/punch-holes; `systemGestures()` or
                // `mandatorySystemGestures()` keeps HUD clear of edge-swipe zones while bars are hidden.
                let mask = env
                    .call_static_method(
                        "android/view/WindowInsets$Type",
                        "displayCutout",
                        "()I",
                        &[],
                    )
                    .and_then(|v| v.i())
                    .unwrap_or(128);
                if let Ok(ins_obj) = env
                    .call_method(
                        &insets,
                        "getInsets",
                        "(I)Landroid/graphics/Insets;",
                        &[JValue::Int(mask)],
                    )
                    .and_then(|v| v.l())
                {
                    if !ins_obj.is_null() {
                        left = left.max(env.get_field(&ins_obj, "left", "I")?.i().unwrap_or(0));
                        top = top.max(env.get_field(&ins_obj, "top", "I")?.i().unwrap_or(0));
                        right = right.max(env.get_field(&ins_obj, "right", "I")?.i().unwrap_or(0));
                        bottom =
                            bottom.max(env.get_field(&ins_obj, "bottom", "I")?.i().unwrap_or(0));
                    }
                }
                let _ = env.exception_clear();
            } else if api >= 28 {
                if let Ok(cutout) = env
                    .call_method(
                        &insets,
                        "getDisplayCutout",
                        "()Landroid/view/DisplayCutout;",
                        &[],
                    )
                    .and_then(|v| v.l())
                {
                    if !cutout.is_null() {
                        left = left.max(
                            env.call_method(&cutout, "getSafeInsetLeft", "()I", &[])?
                                .i()
                                .unwrap_or(0),
                        );
                        top = top.max(
                            env.call_method(&cutout, "getSafeInsetTop", "()I", &[])?
                                .i()
                                .unwrap_or(0),
                        );
                        right = right.max(
                            env.call_method(&cutout, "getSafeInsetRight", "()I", &[])?
                                .i()
                                .unwrap_or(0),
                        );
                        bottom = bottom.max(
                            env.call_method(&cutout, "getSafeInsetBottom", "()I", &[])?
                                .i()
                                .unwrap_or(0),
                        );
                    }
                }
                let _ = env.exception_clear();
            }

            Ok(Some([left.max(0), top.max(0), right.max(0), bottom.max(0)]))
        })
        .flatten()
    }

    /// Triggers a subtle Android haptic pulse (`duration_ms`, API 26+ `VibrationEffect.createOneShot`).
    /// Safe no-op if `VIBRATE` permission or hardware vibrator is unavailable.
    pub fn vibrate(app: &AndroidApp, duration_ms: u16) {
        if duration_ms == 0 {
            return;
        }
        let _ = with_activity(app, |env, activity| {
            let svc_name = env.new_string("vibrator")?;
            let vib = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&svc_name)],
                )?
                .l()?;
            if vib.is_null() {
                return Ok(());
            }
            let has = env
                .call_method(&vib, "hasVibrator", "()Z", &[])?
                .z()
                .unwrap_or(false);
            if !has {
                return Ok(());
            }
            // VibrationEffect.createOneShot(long milliseconds, int amplitude)
            // DEFAULT_AMPLITUDE = -1 (use subtle ~64 amplitude for crisp UI ticks on amplitude-capable motors).
            let has_amp = env
                .call_method(&vib, "hasAmplitudeControl", "()Z", &[])
                .and_then(|v| v.z())
                .unwrap_or(false);
            let amp: i32 = if has_amp { 72 } else { -1 };
            let effect = env
                .call_static_method(
                    "android/os/VibrationEffect",
                    "createOneShot",
                    "(JI)Landroid/os/VibrationEffect;",
                    &[
                        JValue::Long(i64::from(duration_ms.clamp(5, 45))),
                        JValue::Int(amp),
                    ],
                )?
                .l()?;
            if !effect.is_null() {
                let _ = env.call_method(
                    &vib,
                    "vibrate",
                    "(Landroid/os/VibrationEffect;)V",
                    &[JValue::Object(&effect)],
                );
            }
            Ok(())
        });
    }

    /// Checks whether Android is in Battery Saver mode (`PowerManager.isPowerSaveMode()`)
    /// or low battery (< 20% via `BatteryManager.BATTERY_PROPERTY_CAPACITY`).
    pub fn read_low_power_hint(app: &AndroidApp) -> bool {
        with_activity(app, |env, activity| {
            // 1. PowerManager.isPowerSaveMode()
            let pm_name = env.new_string("power")?;
            if let Ok(pm) = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&pm_name)],
                )
                .and_then(|v| v.l())
            {
                if !pm.is_null()
                    && env
                        .call_method(&pm, "isPowerSaveMode", "()Z", &[])
                        .and_then(|v| v.z())
                        .unwrap_or(false)
                {
                    return Ok(true);
                }
            }
            let _ = env.exception_clear();

            // 2. BatteryManager.getIntProperty(BATTERY_PROPERTY_CAPACITY = 4)
            let bm_name = env.new_string("batterymanager")?;
            if let Ok(bm) = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&bm_name)],
                )
                .and_then(|v| v.l())
            {
                if !bm.is_null() {
                    let cap = env
                        .call_method(&bm, "getIntProperty", "(I)I", &[JValue::Int(4)])
                        .and_then(|v| v.i())
                        .unwrap_or(100);
                    if (1..20).contains(&cap) {
                        return Ok(true);
                    }
                }
            }
            let _ = env.exception_clear();
            Ok(false)
        })
        .unwrap_or(false)
    }

    /// Copies text to Android system clipboard (`ClipboardManager.setPrimaryClip`).
    pub fn copy_to_clipboard(app: &AndroidApp, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        with_activity(app, |env, activity| {
            let svc = env.new_string("clipboard")?;
            let cm = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&svc)],
                )?
                .l()?;
            if cm.is_null() {
                return Ok(false);
            }
            let label = env.new_string("NIR Dialogue")?;
            let body = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[JValue::Object(&label), JValue::Object(&body)],
                )?
                .l()?;
            if clip.is_null() {
                return Ok(false);
            }
            env.call_method(
                &cm,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[JValue::Object(&clip)],
            )?;
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// Saves a PNG screenshot into Android's shared Gallery (`MediaStore.Images.Media` on API 29+),
    /// returning `true` on success (caller falls back to external app `screenshots/` directory).
    pub fn save_screenshot_mediastore(app: &AndroidApp, filename: &str, png_bytes: &[u8]) -> bool {
        if png_bytes.is_empty() {
            return false;
        }
        with_activity(app, |env, activity| {
            if sdk_int(env) < 29 {
                return Ok(false);
            }
            let resolver = env
                .call_method(
                    activity,
                    "getContentResolver",
                    "()Landroid/content/ContentResolver;",
                    &[],
                )?
                .l()?;
            if resolver.is_null() {
                return Ok(false);
            }
            let values = env.new_object("android/content/ContentValues", "()V", &[])?;
            let put_str = |env: &mut JNIEnv<'_>, k: &str, v: &str| -> jni::errors::Result<()> {
                let jk = env.new_string(k)?;
                let jv = env.new_string(v)?;
                env.call_method(
                    &values,
                    "put",
                    "(Ljava/lang/String;Ljava/lang/String;)V",
                    &[JValue::Object(&jk), JValue::Object(&jv)],
                )?;
                Ok(())
            };
            put_str(env, "_display_name", filename)?;
            put_str(env, "mime_type", "image/png")?;
            put_str(env, "relative_path", "Pictures/NIR")?;

            let external_uri = env
                .get_static_field(
                    "android/provider/MediaStore$Images$Media",
                    "EXTERNAL_CONTENT_URI",
                    "Landroid/net/Uri;",
                )?
                .l()?;
            if external_uri.is_null() {
                return Ok(false);
            }
            let uri = env
                .call_method(
                    &resolver,
                    "insert",
                    "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
                    &[JValue::Object(&external_uri), JValue::Object(&values)],
                )?
                .l()?;
            if uri.is_null() {
                return Ok(false);
            }
            let stream = env
                .call_method(
                    &resolver,
                    "openOutputStream",
                    "(Landroid/net/Uri;)Ljava/io/OutputStream;",
                    &[JValue::Object(&uri)],
                )?
                .l()?;
            if stream.is_null() {
                return Ok(false);
            }
            let jbytes = env.byte_array_from_slice(png_bytes)?;
            let write_res =
                env.call_method(&stream, "write", "([B)V", &[JValue::Object(&jbytes.into())]);
            let _ = env.call_method(&stream, "close", "()V", &[]);
            write_res?;
            Ok(true)
        })
        .unwrap_or(false)
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
}
