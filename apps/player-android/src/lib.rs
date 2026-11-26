//! Android NDK cdylib entry for NIR v2 powered by Jetpack `GameActivity` (`org.nir.player.NirGameActivity`).
//!
//! Exports:
//! - `android_main`: invoked by `android-activity` (`game-activity` backend) on the native main thread
//! - `Java_org_nir_player_NirHostBridge_*`: JNI callbacks from Kotlin (`NirGameActivity`) for
//!   SAF `.nirpak` import, SAF `nir-backup-v1` save export/import, `ACTION_VIEW` file intents,
//!   AudioFocus / noisy-headphones pause, safe-area insets, and status toasts.
#![cfg(target_os = "android")]

use android_activity::AndroidApp;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_TRUE};
use jni::JNIEnv;
use std::path::PathBuf;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    if let Err(err) = player_native::run_android(app) {
        player_native::android_host::log_error(&format!("NIR player startup failed: {err:#}"));
    }
}

fn jstring_to_rust(env: &mut JNIEnv<'_>, s: JString<'_>) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(&s).ok().map(Into::into)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeOnPakImported(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    path: JString<'_>,
    launch_now: jboolean,
) {
    if let Some(p) = jstring_to_rust(&mut env, path) {
        player_native::android_host::push_host_event(
            player_native::android_host::AndroidHostEvent::PakImported {
                path: PathBuf::from(p),
                launch_now: launch_now == JNI_TRUE,
            },
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeOnSaveBackupImported(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    path: JString<'_>,
) {
    if let Some(p) = jstring_to_rust(&mut env, path) {
        player_native::android_host::push_host_event(
            player_native::android_host::AndroidHostEvent::SaveBackupImported(PathBuf::from(p)),
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeOnAudioFocusChanged(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    paused: jboolean,
) {
    player_native::android_host::push_host_event(
        player_native::android_host::AndroidHostEvent::AudioFocusPaused(paused == JNI_TRUE),
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeOnSafeInsetsChanged(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
) {
    player_native::android_host::push_host_event(
        player_native::android_host::AndroidHostEvent::SafeInsetsPx([
            left.max(0),
            top.max(0),
            right.max(0),
            bottom.max(0),
        ]),
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeOnToastMessage(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    message: JString<'_>,
) {
    if let Some(msg) = jstring_to_rust(&mut env, message) {
        player_native::android_host::push_host_event(
            player_native::android_host::AndroidHostEvent::Toast(msg),
        );
    }
}

/// Scans `.nirpak` directories using Rust's zero-copy `mmap` header/cover parser and returns a
/// JSON array string for the native Material 3 `ShelfActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeScanShelfJson(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    scan_dirs_lines: JString<'_>,
    storage_root: JString<'_>,
    cover_cache_dir: JString<'_>,
) -> jstring {
    let dirs_str = jstring_to_rust(&mut env, scan_dirs_lines).unwrap_or_default();
    let root_str = jstring_to_rust(&mut env, storage_root).unwrap_or_default();
    let cache_str = jstring_to_rust(&mut env, cover_cache_dir).unwrap_or_default();
    let scan_dirs: Vec<PathBuf> = dirs_str
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    let json = player_native::shelf::scan_shelf_entries_json(
        &scan_dirs,
        &PathBuf::from(root_str),
        &PathBuf::from(cache_str),
    );
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Removes a `.nirpak` entry from `recent_games.json` when deleted from the native Shelf.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_nir_player_NirHostBridge_nativeRemoveRecentGame(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    storage_root: JString<'_>,
    pak_path: JString<'_>,
) {
    if let (Some(root), Some(pak)) = (
        jstring_to_rust(&mut env, storage_root),
        jstring_to_rust(&mut env, pak_path),
    ) {
        let recent_file = PathBuf::from(root).join("nir").join("recent_games.json");
        player_native::shelf::remove_recent_game(&recent_file, &PathBuf::from(pak));
    }
}
