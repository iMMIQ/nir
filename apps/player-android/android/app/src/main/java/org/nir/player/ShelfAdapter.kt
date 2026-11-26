package org.nir.player

import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.TextView
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import java.util.concurrent.ExecutorService

class ShelfAdapter(
    private val ioExecutor: ExecutorService,
    private val onLaunch: (ShelfGameItem) -> Unit,
    private val onMoreClick: (View, ShelfGameItem) -> Unit,
    private val onLongClick: (ShelfGameItem) -> Unit
) : ListAdapter<ShelfGameItem, ShelfAdapter.GameViewHolder>(DIFF) {

    private val mainHandler = Handler(Looper.getMainLooper())

    companion object {
        private val DIFF = object : DiffUtil.ItemCallback<ShelfGameItem>() {
            override fun areItemsTheSame(oldItem: ShelfGameItem, newItem: ShelfGameItem): Boolean =
                oldItem.path == newItem.path

            override fun areContentsTheSame(oldItem: ShelfGameItem, newItem: ShelfGameItem): Boolean =
                oldItem == newItem
        }
    }

    inner class GameViewHolder(view: View) : RecyclerView.ViewHolder(view) {
        private val imgCover: ImageView = view.findViewById(R.id.img_cover)
        private val txtCoverGlyph: TextView = view.findViewById(R.id.txt_cover_glyph)
        private val txtSaveBadge: TextView = view.findViewById(R.id.txt_save_badge)
        private val txtSizeBadge: TextView = view.findViewById(R.id.txt_size_badge)
        private val txtTitle: TextView = view.findViewById(R.id.txt_title)
        private val txtMeta: TextView = view.findViewById(R.id.txt_meta)
        private val btnMore: ImageButton = view.findViewById(R.id.btn_more)

        fun bind(item: ShelfGameItem) {
            txtTitle.text = item.title
            txtMeta.text = item.formattedMeta()
            txtSizeBadge.text = item.formattedSize()
            if (item.saveCount > 0) {
                txtSaveBadge.text = "${item.saveCount} 存档"
                txtSaveBadge.setBackgroundResource(R.drawable.bg_badge_accent)
            } else {
                txtSaveBadge.text = "新作品"
                txtSaveBadge.setBackgroundResource(R.drawable.bg_badge_pill)
            }

            val coverPath = item.coverPath
            imgCover.tag = coverPath
            if (coverPath == null) {
                imgCover.setImageDrawable(null)
                txtCoverGlyph.visibility = View.VISIBLE
                txtCoverGlyph.text = item.title.trim().firstOrNull()?.uppercase() ?: "N"
            } else {
                val cached = ShelfCoverCache.get(coverPath)
                if (cached != null) {
                    imgCover.setImageBitmap(cached)
                    txtCoverGlyph.visibility = View.GONE
                } else {
                    imgCover.setImageDrawable(null)
                    txtCoverGlyph.visibility = View.VISIBLE
                    txtCoverGlyph.text = item.title.trim().firstOrNull()?.uppercase() ?: "N"
                    ioExecutor.execute {
                        val bmp = ShelfCoverCache.loadSync(coverPath)
                        if (bmp != null) {
                            mainHandler.post {
                                if (imgCover.tag == coverPath) {
                                    imgCover.setImageBitmap(bmp)
                                    txtCoverGlyph.visibility = View.GONE
                                }
                            }
                        }
                    }
                }
            }

            itemView.setOnClickListener { onLaunch(item) }
            itemView.setOnLongClickListener {
                onLongClick(item)
                true
            }
            btnMore.setOnClickListener { anchor -> onMoreClick(anchor, item) }
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): GameViewHolder {
        val view = LayoutInflater.from(parent.context)
            .inflate(R.layout.item_shelf_game, parent, false)
        return GameViewHolder(view)
    }

    override fun onBindViewHolder(holder: GameViewHolder, position: Int) {
        holder.bind(getItem(position))
    }
}
