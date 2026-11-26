package org.nir.player

import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import android.view.WindowManager
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.Keep
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * Primary NIR v2 Android Activity built on Jetpack [GameActivity].
 *
 * Handles Android platform responsibilities cleanly in strongly-typed Kotlin while delegating
 * rendering, VM, audio mixing, and `.nirpak` zero-copy playback to Rust (`libplayer.so`):
 * - Edge-to-edge immersive fullscreen + display cutout (`shortEdges`) + safe-area insets
 * - SAF `.nirpak` picker (`ACTION_OPEN_DOCUMENT`) with streaming copy & `NIRP` magic verification
 * - SAF `nir-backup-v1` save export (`ACTION_CREATE_DOCUMENT`) and import (`ACTION_OPEN_DOCUMENT`)
 * - `ACTION_VIEW` file association (cold start & `onNewIntent`) for `.nirpak` and `.json` backups
 * - `AudioManager` AudioFocus + `ACTION_AUDIO_BECOMING_NOISY` (headphone disconnect auto-pause)
 * - `MediaStore` Gallery screenshot export, `ClipboardManager`, `Vibrator`, and low-power hints
 */
@Keep
class NirGameActivity : GameActivity() {

    companion object {
        private const val TAG = "NirGameActivity"
        const val EXTRA_PAK_PATH = "org.nir.player.EXTRA_PAK_PATH"
        const val EXTRA_HAS_SHELF = "org.nir.player.EXTRA_HAS_SHELF"
        private val NIRPAK_MAGIC = byteArrayOf(
            'N'.code.toByte(),
            'I'.code.toByte(),
            'R'.code.toByte(),
            'P'.code.toByte()
        )

        init {
            System.loadLibrary("player")
        }
    }

    private val ioExecutor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "nir-android-io").apply { isDaemon = true }
    }

    @Volatile
    private var cachedSafeInsets = intArrayOf(0, 0, 0, 0)

    private val nativeReady = AtomicBoolean(false)
    private val pendingLaunchUri = AtomicReference<Uri?>(null)
    private val pendingExportJson = AtomicReference<String?>(null)

    private var audioManager: AudioManager? = null
    private var audioFocusRequest: AudioFocusRequest? = null
    private var noisyReceiverRegistered = false

    private val audioFocusListener = AudioManager.OnAudioFocusChangeListener { change ->
        if (!nativeReady.get()) return@OnAudioFocusChangeListener
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                NirHostBridge.nativeOnAudioFocusChanged(true)
            }
            AudioManager.AUDIOFOCUS_GAIN -> {
                NirHostBridge.nativeOnAudioFocusChanged(false)
            }
        }
    }

    private val noisyAudioReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY && nativeReady.get()) {
                NirHostBridge.nativeOnAudioFocusChanged(true)
                NirHostBridge.nativeOnToastMessage("耳机已断开，音频已暂停")
            }
        }
    }

    private val pickNirpakLauncher =
        registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) {
                importUriOnBackground(uri, expectSaveBackup = false, launchNow = true)
            }
        }

    private val exportSaveLauncher =
        registerForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri ->
            val payload = pendingExportJson.getAndSet(null)
            if (uri == null || payload == null) return@registerForActivityResult
            ioExecutor.execute {
                val ok = runCatching {
                    contentResolver.openOutputStream(uri, "wt")?.use { out ->
                        out.write(payload.toByteArray(Charsets.UTF_8))
                        out.flush()
                    } ?: error("openOutputStream returned null")
                }.onFailure { err ->
                    Log.w(TAG, "SAF save export failed", err)
                }.isSuccess
                if (nativeReady.get()) {
                    if (ok) {
                        val name = resolveDisplayName(uri) ?: "saves-backup.json"
                        NirHostBridge.nativeOnToastMessage("已导出存档备份：$name")
                    } else {
                        NirHostBridge.nativeOnToastMessage("导出存档失败：无法写入所选位置")
                    }
                }
            }
        }

    private val importSaveLauncher =
        registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) {
                importUriOnBackground(uri, expectSaveBackup = true, launchNow = false)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        audioManager = getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        configureImmersiveAndInsets()
        captureViewIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        captureViewIntent(intent)
        if (nativeReady.get()) {
            flushPendingLaunchUri()
        }
    }

    override fun onResume() {
        super.onResume()
        applyImmersiveFullscreen()
        registerNoisyReceiver()
    }

    override fun onPause() {
        unregisterNoisyReceiver()
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            applyImmersiveFullscreen()
        }
    }

    override fun onDestroy() {
        nativeReady.set(false)
        abandonAudioFocus()
        unregisterNoisyReceiver()
        ioExecutor.shutdownNow()
        super.onDestroy()
    }

    private fun configureImmersiveAndInsets() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes = window.attributes.apply {
                layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        }
        WindowCompat.setDecorFitsSystemWindows(window, false)
        ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, insets ->
            val cutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout())
            val next = intArrayOf(
                cutout.left.coerceAtLeast(0),
                cutout.top.coerceAtLeast(0),
                cutout.right.coerceAtLeast(0),
                cutout.bottom.coerceAtLeast(0)
            )
            cachedSafeInsets = next
            if (nativeReady.get()) {
                NirHostBridge.nativeOnSafeInsetsChanged(next[0], next[1], next[2], next[3])
            }
            ViewCompat.onApplyWindowInsets(view, insets)
        }
        applyImmersiveFullscreen()
    }

    private fun applyImmersiveFullscreen() {
        val controller = WindowInsetsControllerCompat(window, window.decorView)
        controller.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller.hide(WindowInsetsCompat.Type.systemBars())
    }

    private fun registerNoisyReceiver() {
        if (noisyReceiverRegistered) return
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        runCatching {
            ContextCompat.registerReceiver(
                this,
                noisyAudioReceiver,
                filter,
                ContextCompat.RECEIVER_NOT_EXPORTED
            )
            noisyReceiverRegistered = true
        }
    }

    private fun unregisterNoisyReceiver() {
        if (!noisyReceiverRegistered) return
        runCatching { unregisterReceiver(noisyAudioReceiver) }
        noisyReceiverRegistered = false
    }

    private fun captureViewIntent(intent: Intent?) {
        if (intent?.action == Intent.ACTION_VIEW) {
            intent.data?.let { uri ->
                pendingLaunchUri.set(uri)
            }
        }
    }

    private fun flushPendingLaunchUri() {
        val uri = pendingLaunchUri.getAndSet(null) ?: return
        val name = resolveDisplayName(uri)?.lowercase() ?: ""
        val isJson = name.endsWith(".json") ||
            (contentResolver.getType(uri)?.contains("json", ignoreCase = true) == true)
        importUriOnBackground(uri, expectSaveBackup = isJson, launchNow = !isJson)
    }

    private fun resolveDisplayName(uri: Uri): String? {
        runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (idx >= 0) {
                            return cursor.getString(idx)
                        }
                    }
                }
        }
        return uri.lastPathSegment?.substringAfterLast('/')
    }

    private fun sanitizeFileName(raw: String, fallback: String): String {
        val cleaned = raw.trim().replace(Regex("[^A-Za-z0-9._\\-\\u4E00-\\u9FFF\\u3040-\\u30FF]"), "_")
        return cleaned.ifBlank { fallback }
    }

    private fun importUriOnBackground(uri: Uri, expectSaveBackup: Boolean, launchNow: Boolean) {
        ioExecutor.execute {
            if (expectSaveBackup) {
                if (nativeReady.get()) {
                    NirHostBridge.nativeOnToastMessage("正在读取存档备份…")
                }
                val dest = File(cacheDir, "saf-import-backup.json")
                val ok = runCatching {
                    contentResolver.openInputStream(uri)?.use { input ->
                        FileOutputStream(dest).use { out -> input.copyTo(out, 65536) }
                    } ?: error("openInputStream returned null")
                }.onFailure { err ->
                    Log.w(TAG, "SAF save import failed", err)
                }.isSuccess
                if (nativeReady.get()) {
                    if (ok) {
                        NirHostBridge.nativeOnSaveBackupImported(dest.absolutePath)
                    } else {
                        NirHostBridge.nativeOnToastMessage("无法读取所选存档备份文件")
                    }
                }
                return@execute
            }

            if (nativeReady.get()) {
                NirHostBridge.nativeOnToastMessage("正在导入 .nirpak 作品…")
            }
            val baseDir = getExternalFilesDir(null) ?: filesDir
            val gamesDir = File(baseDir, "games").apply { mkdirs() }
            val rawName = resolveDisplayName(uri) ?: "imported-${System.currentTimeMillis()}.nirpak"
            val baseName = sanitizeFileName(rawName, "imported.nirpak")
            val finalName = if (baseName.lowercase().endsWith(".nirpak")) baseName else "$baseName.nirpak"
            val target = File(gamesDir, finalName)
            val tmp = File(gamesDir, "$finalName.part")

            val result = runCatching {
                contentResolver.openInputStream(uri)?.use { input ->
                    val header = ByteArray(4)
                    var readHeader = 0
                    while (readHeader < 4) {
                        val n = input.read(header, readHeader, 4 - readHeader)
                        if (n <= 0) break
                        readHeader += n
                    }
                    if (readHeader < 4 || !header.contentEquals(NIRPAK_MAGIC)) {
                        error("所选文件不是有效的 .nirpak 游戏包")
                    }
                    FileOutputStream(tmp).use { out ->
                        out.write(header, 0, 4)
                        val buf = ByteArray(256 * 1024)
                        while (true) {
                            val n = input.read(buf)
                            if (n <= 0) break
                            out.write(buf, 0, n)
                        }
                        out.flush()
                    }
                } ?: error("无法打开所选文件")

                if (target.exists()) target.delete()
                if (!tmp.renameTo(target)) {
                    tmp.copyTo(target, overwrite = true)
                    tmp.delete()
                }
                target.absolutePath
            }.onFailure {
                tmp.delete()
            }

            if (nativeReady.get()) {
                result.onSuccess { path ->
                    NirHostBridge.nativeOnPakImported(path, launchNow)
                }.onFailure { err ->
                    Log.w(TAG, "SAF nirpak import failed", err)
                    NirHostBridge.nativeOnToastMessage("导入失败：${err.message ?: "IO 错误"}")
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // Methods invoked from Rust (`apps/player-native/src/android_host.rs`)
    // -------------------------------------------------------------------------

    @Keep
    fun querySafeInsetsPx(): IntArray = cachedSafeInsets.copyOf()

    @Keep
    fun triggerHaptic(durationMs: Int) {
        val clamped = durationMs.coerceIn(5, 45).toLong()
        runCatching {
            val vibrator: Vibrator? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                (getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator
            } else {
                @Suppress("DEPRECATION")
                getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            }
            if (vibrator?.hasVibrator() == true) {
                val amp = if (vibrator.hasAmplitudeControl()) 72 else VibrationEffect.DEFAULT_AMPLITUDE
                vibrator.vibrate(VibrationEffect.createOneShot(clamped, amp))
            }
        }
    }

    @Keep
    fun isLowPowerMode(): Boolean {
        val pm = getSystemService(Context.POWER_SERVICE) as? PowerManager
        if (pm?.isPowerSaveMode == true) return true
        val bm = getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
        val capacity = bm?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: 100
        return capacity in 1..19
    }

    @Keep
    fun copyToClipboard(text: String): Boolean {
        if (text.isEmpty()) return false
        val ok = AtomicBoolean(false)
        val latch = CountDownLatch(1)
        runOnUiThread {
            runCatching {
                val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                if (cm != null) {
                    cm.setPrimaryClip(ClipData.newPlainText("NIR Dialogue", text))
                    ok.set(true)
                }
            }
            latch.countDown()
        }
        latch.await(500, TimeUnit.MILLISECONDS)
        return ok.get()
    }

    @Keep
    fun saveScreenshotToGallery(filename: String, pngBytes: ByteArray): Boolean {
        if (pngBytes.isEmpty() || Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false
        return runCatching {
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, filename)
                put(MediaStore.Images.Media.MIME_TYPE, "image/png")
                put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/NIR")
                put(MediaStore.Images.Media.IS_PENDING, 1)
            }
            val uri = contentResolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
                ?: return false
            contentResolver.openOutputStream(uri)?.use { out ->
                out.write(pngBytes)
                out.flush()
            } ?: run {
                contentResolver.delete(uri, null, null)
                return false
            }
            values.clear()
            values.put(MediaStore.Images.Media.IS_PENDING, 0)
            contentResolver.update(uri, values, null, null)
            true
        }.getOrDefault(false)
    }

    @Keep
    fun launchNirpakPicker() {
        runOnUiThread {
            runCatching {
                pickNirpakLauncher.launch(arrayOf("*/*"))
            }.onFailure { err ->
                Log.w(TAG, "launchNirpakPicker failed", err)
                if (nativeReady.get()) {
                    NirHostBridge.nativeOnToastMessage("无法打开系统文件选择器")
                }
            }
        }
    }

    @Keep
    fun launchSaveExport(suggestedFileName: String, jsonPayload: String) {
        pendingExportJson.set(jsonPayload)
        runOnUiThread {
            runCatching {
                exportSaveLauncher.launch(suggestedFileName)
            }.onFailure { err ->
                pendingExportJson.set(null)
                Log.w(TAG, "launchSaveExport failed", err)
                if (nativeReady.get()) {
                    NirHostBridge.nativeOnToastMessage("无法打开系统保存对话框")
                }
            }
        }
    }

    @Keep
    fun launchSaveImport() {
        runOnUiThread {
            runCatching {
                importSaveLauncher.launch(arrayOf("application/json", "text/plain", "*/*"))
            }.onFailure { err ->
                Log.w(TAG, "launchSaveImport failed", err)
                if (nativeReady.get()) {
                    NirHostBridge.nativeOnToastMessage("无法打开存档选择对话框")
                }
            }
        }
    }

    @Keep
    fun requestAudioFocus(): Boolean {
        val am = audioManager ?: return false
        val attrs = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_GAME)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build()
        val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attrs)
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener(audioFocusListener)
            .build()
        audioFocusRequest = req
        return am.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    @Keep
    fun abandonAudioFocus() {
        val am = audioManager ?: return
        audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
        audioFocusRequest = null
    }

    @Keep
    fun consumeInitialPendingIntent() {
        nativeReady.set(true)
        val insets = cachedSafeInsets
        if (insets.any { it > 0 }) {
            NirHostBridge.nativeOnSafeInsetsChanged(insets[0], insets[1], insets[2], insets[3])
        }
        flushPendingLaunchUri()
    }

    @Keep
    fun getInitialPakPath(): String? = intent?.getStringExtra(EXTRA_PAK_PATH)

    @Keep
    fun hasNativeShelf(): Boolean = intent?.getBooleanExtra(EXTRA_HAS_SHELF, false) ?: false

    @Keep
    fun finishToShelf() {
        runOnUiThread {
            finish()
        }
    }
}
