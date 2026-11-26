package org.nir.player

import androidx.annotation.Keep

/**
 * JNI bridge for Kotlin <-> Rust calls implemented in `apps/player-android`.
 */
@Keep
object NirHostBridge {
    init {
        System.loadLibrary("player")
    }

    @JvmStatic
    external fun nativeOnPakImported(path: String, launchNow: Boolean)

    @JvmStatic
    external fun nativeOnSaveBackupImported(path: String)

    @JvmStatic
    external fun nativeOnAudioFocusChanged(paused: Boolean)

    @JvmStatic
    external fun nativeOnSafeInsetsChanged(left: Int, top: Int, right: Int, bottom: Int)

    @JvmStatic
    external fun nativeOnToastMessage(message: String)

    @JvmStatic
    external fun nativeScanShelfJson(
        scanDirsLines: String,
        storageRoot: String,
        coverCacheDir: String
    ): String

    @JvmStatic
    external fun nativeRemoveRecentGame(storageRoot: String, pakPath: String)
}
