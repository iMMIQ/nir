package org.nir.player

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import org.json.JSONArray
import java.io.File
import java.util.Locale

data class ShelfGameItem(
    val path: String,
    val gameId: String,
    val title: String,
    val version: String,
    val locales: List<String>,
    val fileSizeBytes: Long,
    val saveCount: Int,
    val coverPath: String?,
    val recentOrder: Int
) {
    val fileName: String
        get() = File(path).name

    fun formattedSize(): String {
        val b = fileSizeBytes.coerceAtLeast(0L)
        return when {
            b >= 1024L * 1024L * 1024L -> String.format(Locale.US, "%.2f GB", b / (1024.0 * 1024.0 * 1024.0))
            b >= 1024L * 1024L -> String.format(Locale.US, "%.1f MB", b / (1024.0 * 1024.0))
            b >= 1024L -> String.format(Locale.US, "%.0f KB", b / 1024.0)
            else -> "$b B"
        }
    }

    fun formattedMeta(): String {
        val ver = if (version.isNotBlank()) "v$version" else "v0"
        val loc = if (locales.isNotEmpty()) locales.joinToString(" / ") else ""
        return if (loc.isNotEmpty()) "$ver  ·  $loc" else ver
    }

    companion object {
        fun parseJsonList(json: String): List<ShelfGameItem> {
            val out = mutableListOf<ShelfGameItem>()
            runCatching {
                val arr = JSONArray(json)
                for (i in 0 until arr.length()) {
                    val obj = arr.getJSONObject(i)
                    val localesArr = obj.optJSONArray("locales")
                    val locales = mutableListOf<String>()
                    if (localesArr != null) {
                        for (j in 0 until localesArr.length()) {
                            localesArr.optString(j)?.takeIf { it.isNotBlank() }?.let(locales::add)
                        }
                    }
                    val cover = obj.optString("coverPath", "").takeIf { it.isNotBlank() }
                    out.add(
                        ShelfGameItem(
                            path = obj.getString("path"),
                            gameId = obj.optString("gameId", ""),
                            title = obj.optString("title", "Untitled"),
                            version = obj.optString("version", ""),
                            locales = locales,
                            fileSizeBytes = obj.optLong("fileSizeBytes", 0L),
                            saveCount = obj.optInt("saveCount", 0),
                            coverPath = cover,
                            recentOrder = i
                        )
                    )
                }
            }
            return out
        }
    }
}

enum class ShelfSortMode(val label: String) {
    RECENT("最近游玩"),
    TITLE("按标题 A–Z"),
    SIZE_DESC("按体积降序");

    fun next(): ShelfSortMode = entries[(ordinal + 1) % entries.size]
}

/**
 * Memory-bounded LRU cache of decoded cover PNGs for smooth 120fps RecyclerView scrolling.
 */
object ShelfCoverCache {
    private val cache = object : LruCache<String, Bitmap>(16 * 1024) {
        override fun sizeOf(key: String, value: Bitmap): Int = (value.byteCount / 1024).coerceAtLeast(1)
    }

    fun get(path: String): Bitmap? = cache.get(path)

    fun loadSync(path: String): Bitmap? {
        cache.get(path)?.let { return it }
        val file = File(path)
        if (!file.isFile) return null
        val bmp = BitmapFactory.decodeFile(path) ?: return null
        cache.put(path, bmp)
        return bmp
    }

    fun clear() {
        cache.evictAll()
    }
}
