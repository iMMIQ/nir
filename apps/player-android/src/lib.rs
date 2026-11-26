//! Thin Android NDK cdylib entry for NIR v2.
//! Zero JNI `libplayer.so` self-hashing; reads 16 KiB-aligned STORED `assets/game.nirpak`
//! directly from `AssetManager` via zero-copy `mmap`, and configures immersive cutout mode
//! on the UI thread inside `android_on_create` without requiring any Java/DEX code!
#![cfg(target_os = "android")]

use android_activity::{AndroidApp, OnCreateState};

#[unsafe(no_mangle)]
fn android_on_create(state: &OnCreateState) {
    let _ = std::panic::catch_unwind(|| {
        player_native::android_host::apply_immersive_on_ui_thread(
            state.vm_as_ptr(),
            state.activity_as_ptr(),
        );
    });
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    if let Err(err) = player_native::run_android(app) {
        player_native::android_host::log_error(&format!("NIR player startup failed: {err:#}"));
    }
}
