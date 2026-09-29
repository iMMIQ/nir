//! Android entry for the native player. Exports the `android-activity`
//! glue entrypoints, resolves the installed `libplayer.so` path for the
//! whole-player digest attestation over JNI, and hands control to the
//! shared desktop shell.
//!
//! On non-Android hosts the crate compiles to nothing; the shell itself
//! lives in `player-desktop` behind `cfg(target_os = "android")`.
#![cfg(target_os = "android")]
use android_activity::{AndroidApp, OnCreateState};
use std::{path::PathBuf, sync::OnceLock};

/// Resolved once on the Java main thread during `Activity.onCreate`, while
/// the application class loader can still find framework classes.
static NATIVE_LIBRARY: OnceLock<Option<PathBuf>> = OnceLock::new();

#[unsafe(no_mangle)]
fn android_on_create(state: &OnCreateState) {
    NATIVE_LIBRARY.get_or_init(|| native_library(state).ok());
}

/// ApplicationInfo.nativeLibraryDir joined with the packaged library name;
/// `run_android` hashes this file against the release manifest's player
/// digest on a helper thread, mirroring the desktop exe attestation.
fn native_library(state: &OnCreateState) -> anyhow::Result<PathBuf> {
    use jni::{jni_sig, jni_str, objects::JObject};
    let vm = unsafe { jni::JavaVM::from_raw(state.vm_as_ptr().cast()) };
    let activity = state.activity_as_ptr() as jni::sys::jobject;
    vm.attach_current_thread(move |env| -> jni::errors::Result<PathBuf> {
        // The reference is owned by the ongoing onCreate call; the cast only
        // borrows it for these lookups.
        let activity = unsafe { env.as_cast_raw::<JObject>(&activity)? };
        let info = env
            .call_method(
                &activity,
                jni_str!("getApplicationInfo"),
                jni_sig!(() -> android.content.pm.ApplicationInfo),
                &[],
            )?
            .l()?;
        let field = env
            .get_field(
                &info,
                jni_str!("nativeLibraryDir"),
                jni_sig!(java.lang.String),
            )?
            .l()?;
        let raw = field.as_raw();
        let dir = unsafe { env.as_cast_raw::<jni::objects::JString>(&raw)? };
        Ok(PathBuf::from(dir.to_string()).join("libplayer.so"))
    })
    .map_err(|e| anyhow::anyhow!("E_JNI: nativeLibraryDir lookup failed: {e:?}"))
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let native = NATIVE_LIBRARY.get().cloned().flatten();
    if let Err(error) = player_desktop::desktop::run_android(app, native) {
        // The I/O thread set up by android-activity routes this to logcat.
        eprintln!("无法启动 / Unable to start\n\n{error:#}");
    }
}
