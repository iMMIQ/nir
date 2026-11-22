//! Thin Android NDK cdylib entry for NIR v2.
//! Zero JNI `libplayer.so` self-hashing; reads `assets/game.nirpak` directly from AssetManager.
#![cfg(target_os = "android")]

use android_activity::AndroidApp;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    if let Err(err) = player_native::run_android(app) {
        eprintln!("NIR player startup failed: {err:#}");
    }
}
