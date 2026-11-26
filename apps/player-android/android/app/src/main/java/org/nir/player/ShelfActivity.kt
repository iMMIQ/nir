package org.nir.player

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.Log
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.widget.LinearLayout
import android.widget.PopupMenu
import android.widget.Toast
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.widget.SearchView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.RecyclerView
import androidx.swiperefreshlayout.widget.SwipeRefreshLayout
import com.google.android.material.appbar.AppBarLayout
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.button.MaterialButton
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.floatingactionbutton.ExtendedFloatingActionButton
import com.google.android.material.progressindicator.LinearProgressIndicator
import com.google.android.material.snackbar.Snackbar
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.Executors

/**
 * Native Material 3 Library / Shelf Activity for Universal NIR Player (`org.nir.player`).
 *
 * Delegates `.nirpak` header & cover decoding to Rust (`NirHostBridge.nativeScanShelfJson`)
 * so zero container/codec logic is duplicated in Kotlin, while delivering a true native Android
 * experience: Material 3 dark cinema UI, adaptive grid, live search, sorting, pull-to-refresh,
 * multi-select SAF `.nirpak` import, `ACTION_VIEW` launch, and seamless launch/return with
 * [NirGameActivity].
 */
class ShelfActivity : AppCompatActivity() {

    companion object {
        private const val TAG = "ShelfActivity"
        private const val PREFS_NAME = "nir_shelf_prefs"
        private const val KEY_SORT_MODE = "sort_mode"
        private val NIRPAK_MAGIC = byteArrayOf(
            'N'.code.toByte(),
            'I'.code.toByte(),
            'R'.code.toByte(),
            'P'.code.toByte()
        )
    }

    private val ioExecutor = Executors.newFixedThreadPool(2) { r ->
        Thread(r, "nir-shelf-io").apply { isDaemon = true }
    }

    private lateinit var rootView: View
    private lateinit var appBar: AppBarLayout
    private lateinit var toolbar: MaterialToolbar
    private lateinit var importProgress: LinearProgressIndicator
    private lateinit var swipeRefresh: SwipeRefreshLayout
    private lateinit var recyclerView: RecyclerView
    private lateinit var emptyContainer: LinearLayout
    private lateinit var fabImport: ExtendedFloatingActionButton
    private lateinit var adapter: ShelfAdapter

    private var allGames: List<ShelfGameItem> = emptyList()
    private var searchQuery: String = ""
    private var sortMode: ShelfSortMode = ShelfSortMode.RECENT

    private val importMultipleLauncher =
        registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            if (uris.isNotEmpty()) {
                importUrisOnBackground(uris, launchSingleImmediately = false)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Safety fallback: if this APK ever bundles `assets/game.nirpak`, jump straight to GameActivity.
        if (hasBundledGameAsset()) {
            super.onCreate(savedInstanceState)
            startActivity(
                Intent(this, NirGameActivity::class.java).apply {
                    putExtra(NirGameActivity.EXTRA_HAS_SHELF, false)
                }
            )
            finish()
            return
        }

        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_shelf)

        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val savedSort = prefs.getString(KEY_SORT_MODE, ShelfSortMode.RECENT.name)
        sortMode = ShelfSortMode.entries.firstOrNull { it.name == savedSort } ?: ShelfSortMode.RECENT

        rootView = findViewById(R.id.shelf_root)
        appBar = findViewById(R.id.app_bar)
        toolbar = findViewById(R.id.toolbar)
        importProgress = findViewById(R.id.import_progress)
        swipeRefresh = findViewById(R.id.swipe_refresh)
        recyclerView = findViewById(R.id.recycler_games)
        emptyContainer = findViewById(R.id.empty_container)
        fabImport = findViewById(R.id.fab_import)

        setSupportActionBar(toolbar)
        applyEdgeToEdgeInsets()

        adapter = ShelfAdapter(
            ioExecutor = ioExecutor,
            onLaunch = { item -> launchGame(item.path) },
            onMoreClick = { anchor, item -> showCardOverflowMenu(anchor, item) },
            onLongClick = { item -> showGameDetailsDialog(item) }
        )

        recyclerView.layoutManager = GridLayoutManager(this, computeSpanCount())
        recyclerView.adapter = adapter
        recyclerView.addOnScrollListener(object : RecyclerView.OnScrollListener() {
            override fun onScrolled(rv: RecyclerView, dx: Int, dy: Int) {
                if (dy > 12 && fabImport.isExtended) {
                    fabImport.shrink()
                } else if (dy < -12 && !fabImport.isExtended) {
                    fabImport.extend()
                }
            }
        })

        swipeRefresh.setColorSchemeResources(R.color.nir_primary)
        swipeRefresh.setProgressBackgroundColorSchemeResource(R.color.nir_surface_elevated)
        swipeRefresh.setOnRefreshListener { refreshShelf() }

        val openPicker = View.OnClickListener { launchNirpakImportPicker() }
        fabImport.setOnClickListener(openPicker)
        findViewById<MaterialButton>(R.id.btn_empty_import).setOnClickListener(openPicker)
        findViewById<MaterialButton>(R.id.btn_empty_rescan).setOnClickListener {
            swipeRefresh.isRefreshing = true
            refreshShelf()
        }

        handleIncomingIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncomingIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        // Refresh save counts and recently-played ordering whenever returning from NirGameActivity.
        refreshShelf()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        (recyclerView.layoutManager as? GridLayoutManager)?.spanCount = computeSpanCount()
    }

    override fun onDestroy() {
        ioExecutor.shutdownNow()
        super.onDestroy()
    }

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menuInflater.inflate(R.menu.menu_shelf, menu)
        val searchItem = menu.findItem(R.id.action_search)
        val searchView = searchItem?.actionView as? SearchView
        searchView?.queryHint = "搜索标题或文件名…"
        searchView?.setOnQueryTextListener(object : SearchView.OnQueryTextListener {
            override fun onQueryTextSubmit(query: String?): Boolean = true
            override fun onQueryTextChange(newText: String?): Boolean {
                searchQuery = newText.orEmpty().trim()
                applyFilterAndSort()
                return true
            }
        })
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        return when (item.itemId) {
            R.id.action_sort -> {
                sortMode = sortMode.next()
                getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                    .edit()
                    .putString(KEY_SORT_MODE, sortMode.name)
                    .apply()
                applyFilterAndSort()
                Toast.makeText(this, "排序：${sortMode.label}", Toast.LENGTH_SHORT).show()
                true
            }
            R.id.action_refresh -> {
                ShelfCoverCache.clear()
                swipeRefresh.isRefreshing = true
                refreshShelf()
                true
            }
            R.id.action_storage_help -> {
                showStorageHelpDialog()
                true
            }
            else -> super.onOptionsItemSelected(item)
        }
    }

    private fun hasBundledGameAsset(): Boolean =
        runCatching { assets.list("")?.contains("game.nirpak") == true }.getOrDefault(false)

    private fun computeSpanCount(): Int {
        val widthDp = resources.configuration.screenWidthDp
        return when {
            widthDp >= 900 -> 3
            widthDp >= 560 -> 2
            else -> 1
        }
    }

    private fun applyEdgeToEdgeInsets() {
        val baseFabMargin = (18 * resources.displayMetrics.density).toInt()
        val baseRvBottom = (96 * resources.displayMetrics.density).toInt()
        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val sys = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            appBar.updatePadding(top = sys.top, left = sys.left, right = sys.right)
            recyclerView.updatePadding(
                left = (12 * resources.displayMetrics.density).toInt() + sys.left,
                right = (12 * resources.displayMetrics.density).toInt() + sys.right,
                bottom = baseRvBottom + sys.bottom
            )
            (fabImport.layoutParams as? androidx.coordinatorlayout.widget.CoordinatorLayout.LayoutParams)?.let { lp ->
                lp.bottomMargin = baseFabMargin + sys.bottom
                lp.rightMargin = baseFabMargin + sys.right
                fabImport.layoutParams = lp
            }
            insets
        }
    }

    private fun primaryGamesDir(): File {
        val base = getExternalFilesDir(null) ?: filesDir
        return File(base, "games").apply { mkdirs() }
    }

    private fun scanDirectories(): List<File> {
        val dirs = mutableListOf<File>()
        getExternalFilesDir(null)?.let { ext ->
            dirs.add(File(ext, "games"))
            dirs.add(ext)
        }
        dirs.add(File(filesDir, "games"))
        return dirs
    }

    private fun refreshShelf() {
        val dirsLines = scanDirectories().joinToString("\n") { it.absolutePath }
        val storageRoot = filesDir.absolutePath
        val coverCacheDir = File(cacheDir, "shelf-covers").apply { mkdirs() }.absolutePath

        ioExecutor.execute {
            val json = runCatching {
                NirHostBridge.nativeScanShelfJson(dirsLines, storageRoot, coverCacheDir)
            }.onFailure { err ->
                Log.e(TAG, "nativeScanShelfJson failed", err)
            }.getOrDefault("[]")
            val parsed = ShelfGameItem.parseJsonList(json)
            runOnUiThread {
                allGames = parsed
                swipeRefresh.isRefreshing = false
                applyFilterAndSort()
            }
        }
    }

    private fun applyFilterAndSort() {
        val query = searchQuery.lowercase()
        val filtered = if (query.isEmpty()) {
            allGames
        } else {
            allGames.filter { item ->
                item.title.lowercase().contains(query) ||
                    item.gameId.lowercase().contains(query) ||
                    item.fileName.lowercase().contains(query)
            }
        }
        val sorted = when (sortMode) {
            ShelfSortMode.RECENT -> filtered.sortedBy { it.recentOrder }
            ShelfSortMode.TITLE -> filtered.sortedBy { it.title.lowercase() }
            ShelfSortMode.SIZE_DESC -> filtered.sortedByDescending { it.fileSizeBytes }
        }
        adapter.submitList(sorted)
        toolbar.subtitle = if (allGames.isEmpty()) {
            "支持导入任意 .nirpak 作品"
        } else if (query.isNotEmpty()) {
            "匹配 ${sorted.size} / ${allGames.size} 部  ·  ${sortMode.label}"
        } else {
            "${allGames.size} 部作品  ·  ${sortMode.label}"
        }
        emptyContainer.visibility = if (allGames.isEmpty()) View.VISIBLE else View.GONE
        recyclerView.visibility = if (allGames.isEmpty()) View.GONE else View.VISIBLE
    }

    private fun launchNirpakImportPicker() {
        runCatching {
            importMultipleLauncher.launch(arrayOf("*/*"))
        }.onFailure { err ->
            Log.w(TAG, "OpenMultipleDocuments failed", err)
            Snackbar.make(rootView, "无法打开系统文件选择器", Snackbar.LENGTH_SHORT).show()
        }
    }

    private fun handleIncomingIntent(intent: Intent?) {
        if (intent?.action == Intent.ACTION_VIEW) {
            intent.data?.let { uri ->
                importUrisOnBackground(listOf(uri), launchSingleImmediately = true)
            }
        }
    }

    private fun resolveDisplayName(uri: Uri): String? {
        runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (idx >= 0) return cursor.getString(idx)
                    }
                }
        }
        return uri.lastPathSegment?.substringAfterLast('/')
    }

    private fun sanitizeFileName(raw: String, fallback: String): String {
        val cleaned = raw.trim().replace(Regex("[^A-Za-z0-9._\\-\\u4E00-\\u9FFF\\u3040-\\u30FF]"), "_")
        return cleaned.ifBlank { fallback }
    }

    private fun importUrisOnBackground(uris: List<Uri>, launchSingleImmediately: Boolean) {
        importProgress.visibility = View.VISIBLE
        ioExecutor.execute {
            val gamesDir = primaryGamesDir()
            val importedPaths = mutableListOf<String>()
            val failedNames = mutableListOf<String>()

            for (uri in uris) {
                val rawName = resolveDisplayName(uri) ?: "imported-${System.currentTimeMillis()}.nirpak"
                val baseName = sanitizeFileName(rawName, "imported.nirpak")
                val finalName = if (baseName.lowercase().endsWith(".nirpak")) baseName else "$baseName.nirpak"
                val target = File(gamesDir, finalName)
                val tmp = File(gamesDir, "$finalName.part")

                val ok = runCatching {
                    contentResolver.openInputStream(uri)?.use { input ->
                        val header = ByteArray(4)
                        var readHeader = 0
                        while (readHeader < 4) {
                            val n = input.read(header, readHeader, 4 - readHeader)
                            if (n <= 0) break
                            readHeader += n
                        }
                        if (readHeader < 4 || !header.contentEquals(NIRPAK_MAGIC)) {
                            error("not a valid .nirpak file")
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
                    } ?: error("cannot open uri stream")

                    if (target.exists()) target.delete()
                    if (!tmp.renameTo(target)) {
                        tmp.copyTo(target, overwrite = true)
                        tmp.delete()
                    }
                    importedPaths.add(target.absolutePath)
                }.onFailure { err ->
                    tmp.delete()
                    Log.w(TAG, "Import failed for $rawName", err)
                    failedNames.add(rawName)
                }.isSuccess

                if (!ok && launchSingleImmediately && uris.size == 1) {
                    runOnUiThread {
                        importProgress.visibility = View.GONE
                        Snackbar.make(rootView, "所选文件不是有效的 .nirpak 游戏包", Snackbar.LENGTH_LONG).show()
                    }
                    return@execute
                }
            }

            runOnUiThread {
                importProgress.visibility = View.GONE
                refreshShelf()
                if (launchSingleImmediately && importedPaths.size == 1) {
                    launchGame(importedPaths.first())
                } else if (importedPaths.isNotEmpty()) {
                    val msg = if (failedNames.isEmpty()) {
                        "已导入 ${importedPaths.size} 部 .nirpak 作品"
                    } else {
                        "已导入 ${importedPaths.size} 部，${failedNames.size} 个非 .nirpak 文件已跳过"
                    }
                    val snack = Snackbar.make(rootView, msg, Snackbar.LENGTH_LONG)
                    if (importedPaths.size == 1) {
                        snack.setAction("立即开始") { launchGame(importedPaths.first()) }
                    }
                    snack.show()
                } else {
                    Snackbar.make(rootView, "未能导入有效 .nirpak 作品（请检查文件格式）", Snackbar.LENGTH_LONG).show()
                }
            }
        }
    }

    private fun launchGame(pakPath: String) {
        val intent = Intent(this, NirGameActivity::class.java).apply {
            putExtra(NirGameActivity.EXTRA_PAK_PATH, pakPath)
            putExtra(NirGameActivity.EXTRA_HAS_SHELF, true)
        }
        startActivity(intent)
    }

    private fun showCardOverflowMenu(anchor: View, item: ShelfGameItem) {
        val popup = PopupMenu(this, anchor)
        popup.menu.add(0, 1, 0, "启动作品")
        popup.menu.add(0, 2, 1, "作品详情")
        popup.menu.add(0, 3, 2, "从书架移除")
        popup.setOnMenuItemClickListener { menuItem ->
            when (menuItem.itemId) {
                1 -> launchGame(item.path)
                2 -> showGameDetailsDialog(item)
                3 -> confirmRemoveGame(item)
            }
            true
        }
        popup.show()
    }

    private fun showGameDetailsDialog(item: ShelfGameItem) {
        val locales = if (item.locales.isEmpty()) "默认" else item.locales.joinToString(", ")
        val info = buildString {
            appendLine("作品 ID：${item.gameId}")
            appendLine("版本：${item.version.ifBlank { "未知" }}")
            appendLine("语言：$locales")
            appendLine("文件大小：${item.formattedSize()}")
            appendLine("本地存档：${item.saveCount} 个")
            append("路径：${item.path}")
        }
        MaterialAlertDialogBuilder(this)
            .setTitle(item.title)
            .setMessage(info)
            .setPositiveButton("启动作品") { _, _ -> launchGame(item.path) }
            .setNeutralButton("移除") { _, _ -> confirmRemoveGame(item) }
            .setNegativeButton("关闭", null)
            .show()
    }

    private fun confirmRemoveGame(item: ShelfGameItem) {
        MaterialAlertDialogBuilder(this)
            .setTitle("移除《${item.title}》？")
            .setMessage("将删除应用书架中的 `.nirpak` 文件（${item.formattedSize()}），你的本地存档与进度会被安全保留。")
            .setPositiveButton("确认移除") { _, _ ->
                ioExecutor.execute {
                    val file = File(item.path)
                    val deleted = !file.exists() || file.delete()
                    runCatching {
                        NirHostBridge.nativeRemoveRecentGame(filesDir.absolutePath, item.path)
                    }
                    runOnUiThread {
                        if (deleted) {
                            Snackbar.make(rootView, "已从书架移除《${item.title}》", Snackbar.LENGTH_SHORT).show()
                        } else {
                            Snackbar.make(rootView, "无法删除文件（请检查外部目录权限）", Snackbar.LENGTH_SHORT).show()
                        }
                        refreshShelf()
                    }
                }
            }
            .setNegativeButton("取消", null)
            .show()
    }

    private fun showStorageHelpDialog() {
        val gamesDir = primaryGamesDir().absolutePath
        MaterialAlertDialogBuilder(this)
            .setTitle("如何添加 .nirpak 作品")
            .setMessage(
                "1. 推荐直接点击右下角「导入 .nirpak」通过 Android 系统文件选择器多选导入；\n\n" +
                    "2. 或在系统文件管理器 / 浏览器 / 聊天软件中直接点击 `.nirpak` 文件，选择用 NIR Player 打开；\n\n" +
                    "3. 高级用户也可通过 ADB / 文件工具将文件直接放入：\n$gamesDir"
            )
            .setPositiveButton("复制目录路径") { _, _ ->
                val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                cm?.setPrimaryClip(ClipData.newPlainText("NIR Games Dir", gamesDir))
                Toast.makeText(this, "已复制目录路径", Toast.LENGTH_SHORT).show()
            }
            .setNegativeButton("知道了", null)
            .show()
    }
}
