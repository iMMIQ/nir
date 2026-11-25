use crate::{
    AssetId, AudioLoopRegion, FuncId, FuncMeta, LabelMeta, MenuTemplate, NirError, Op, Result,
    SpriteTrim, StrId, TextEntry, TimelineClipKind, TimelineDef, UiTheme, VarMeta,
};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};

pub const NIRPAK_MAGIC: &[u8; 4] = b"NIRP";
pub const NIRPAK_VERSION: u16 = 2;

/// Coarse hardware/media feature flags.
pub const FEATURE_WEBP: u32 = 1 << 0;
pub const FEATURE_MP3: u32 = 1 << 1;
pub const FEATURE_OGG: u32 = 1 << 2;
pub const FEATURE_KTX2: u32 = 1 << 3;
pub const FEATURE_VIDEO: u32 = 1 << 4;
pub const FEATURE_TRIMMED: u32 = 1 << 5;
pub const FEATURE_OPUS: u32 = 1 << 6;
pub const FEATURE_DELTA_IMAGE: u32 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AssetKind {
    Image = 0,
    Audio = 1,
    Font = 2,
    Video = 3,
    Raw = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MediaCodec {
    Png = 0,
    Webp = 1,
    Jpeg = 2,
    Ktx2 = 3,
    Wav = 4,
    Mp3 = 5,
    Ogg = 6,
    Flac = 7,
    Ttf = 8,
    Otf = 9,
    Mp4 = 10,
    Webm = 11,
    Raw = 12,
    Opus = 13,
}

impl MediaCodec {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Jpeg => "jpg",
            Self::Ktx2 => "ktx2",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::Ttf => "ttf",
            Self::Otf => "otf",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Raw => "bin",
            Self::Opus => "opus",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Ktx2 => "image/ktx2",
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Ogg => "audio/ogg",
            Self::Flac => "audio/flac",
            Self::Ttf => "font/ttf",
            Self::Otf => "font/otf",
            Self::Mp4 => "video/mp4",
            Self::Webm => "video/webm",
            Self::Raw => "application/octet-stream",
            Self::Opus => "audio/ogg; codecs=opus",
        }
    }
}

/// Differential image patch applied over `base` at `(x, y)` (depth-1 star topology).
///
/// - When `tile_mask` is empty (opaque CG/BG mode): every pixel inside the patch with `alpha > 0`
///   overwrites the corresponding pixel in `base`, while `alpha == 0` preserves `base`.
/// - When `tile_mask` is non-empty (16×16 tile-replace mode for transparent sprites): each bit
///   (row-major, LSB-first per byte) covers a 16×16 tile inside the patch bounding box. Bit `1`
///   replaces the tile's RGBA pixels verbatim from the patch (preserving alpha deletions!), while
///   bit `0` preserves `base`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDelta {
    pub base: AssetId,
    pub x: u32,
    pub y: u32,
    #[serde(default)]
    pub tile_mask: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub id: AssetId,
    pub name: StrId,
    pub kind: AssetKind,
    pub codec: MediaCodec,
    pub width: u32,
    pub height: u32,
    pub trim: Option<SpriteTrim>,
    pub duration_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub loop_region: Option<AudioLoopRegion>,
    /// Offset relative to the start of the `.nirpak` media blob section.
    pub offset: u64,
    pub size: u64,
    /// Optional differential patch referencing a full keyframe `base` image.
    #[serde(default)]
    pub delta: Option<ImageDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameMeta {
    pub game_id: String,
    pub title: String,
    pub version: String,
    pub stage_width: u32,
    pub stage_height: u32,
    pub default_locale: String,
    pub locales: Vec<String>,
    pub entry_func: FuncId,
    pub save_slots: u32,
    pub default_reveal_us: u64,
    pub title_bg: Option<AssetId>,
    pub title_bgm: Option<AssetId>,
    pub ui_fonts: Vec<AssetId>,
    pub text_fonts: Vec<AssetId>,
}

/// Complete compiled NIR v2 program (metadata + string pool + bytecode + texts + asset index).
/// Media payload bytes live outside this struct so native hosts can zero-copy mmap them
/// and web hosts can stream them on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NirProgram {
    pub features: u32,
    pub meta: GameMeta,
    pub strings: Vec<String>,
    pub vars: Vec<VarMeta>,
    pub funcs: Vec<FuncMeta>,
    pub labels: Vec<LabelMeta>,
    pub ops: Vec<Op>,
    /// `locales[locale_name][text_id]`
    pub texts: BTreeMap<String, Vec<TextEntry>>,
    pub ui: UiTheme,
    pub assets: Vec<AssetMeta>,
    /// Normalized lowercase relative path (`/`-separated) -> `AssetId` for dynamic runtime asset resolution.
    #[serde(default)]
    pub asset_lookup: BTreeMap<String, AssetId>,
    /// Reusable frame-accurate timelines (LiveCinema/LiveMotion/cut-ins).
    #[serde(default)]
    pub timelines: Vec<TimelineDef>,
    /// Reusable interactive image menus (e.g. `.lpm` templates).
    #[serde(default)]
    pub menus: Vec<MenuTemplate>,
}

impl NirProgram {
    #[inline]
    pub fn str(&self, id: StrId) -> &str {
        self.strings.get(id.0 as usize).map_or("", String::as_str)
    }

    #[inline]
    pub fn asset(&self, id: AssetId) -> Option<&AssetMeta> {
        self.assets.get(id.0 as usize)
    }

    #[inline]
    pub fn func(&self, id: FuncId) -> Option<&FuncMeta> {
        self.funcs.get(id.0 as usize)
    }

    pub fn text(&self, locale: &str, id: crate::TextId) -> Option<&TextEntry> {
        self.texts
            .get(locale)
            .or_else(|| self.texts.get(&self.meta.default_locale))
            .and_then(|table| table.get(id.0 as usize))
    }

    pub fn find_label(&self, name: &str) -> Option<&LabelMeta> {
        self.labels.iter().find(|l| self.str(l.name) == name)
    }

    pub fn find_asset_by_name(&self, name: &str) -> Option<AssetId> {
        self.assets
            .iter()
            .find(|a| self.str(a.name) == name)
            .map(|a| a.id)
    }

    /// Collects all characters appearing in UI labels, strings, and localized texts
    /// directly from the string pool and text tables (zero Program cloning!).
    pub fn collect_all_chars(&self) -> std::collections::BTreeSet<char> {
        collect_all_chars_parts(&self.meta.title, &self.strings, &self.texts)
    }
}

/// Collects all unique non-control characters directly from raw program parts
/// without requiring callers to construct or clone a `NirProgram`.
pub fn collect_all_chars_parts(
    title: &str,
    strings: &[String],
    texts: &BTreeMap<String, Vec<TextEntry>>,
) -> std::collections::BTreeSet<char> {
    let mut chars: std::collections::BTreeSet<char> = (' '..='~').collect();
    chars.extend(title.chars());
    for s in strings {
        chars.extend(s.chars());
    }
    for table in texts.values() {
        for entry in table {
            entry.collect_chars(&mut chars);
        }
    }
    for ftl in [
        crate::ui_messages::EN_FTL,
        crate::ui_messages::ZH_HANS_FTL,
        "鉴赏 / 回想Gallery",
    ] {
        chars.extend(ftl.chars());
    }
    chars.retain(|c| !c.is_control());
    chars
}

/// Header layout of a `.nirpak` container (32 bytes, little-endian):
/// - `0..4`:   magic `b"NIRP"`
/// - `4..6`:   version (`u16`, currently 2)
/// - `6..8`:   flags (`u16`, bit 0 = zlib compressed, bit 1 = compact binary Postcard index)
/// - `8..12`:  feature_flags (`u32`)
/// - `12..16`: program_crc32 (`u32`, checksum of program payload)
/// - `16..24`: program_len (`u64`, byte length of program payload starting at offset 32)
/// - `24..32`: media_len (`u64`, byte length of media blob starting at offset `32 + program_len`)
pub const NIRPAK_HEADER_LEN: usize = 32;
pub const FLAG_ZLIB: u16 = 1 << 0;
pub const FLAG_POSTCARD: u16 = 1 << 1;
const BLOB_ALIGN: u64 = 64;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    crc.sum()
}

/// Computes an optimal temporal-locality media layout order for `.nirpak`:
/// - **Zone 0 (Boot/UI Hot Zone)**: fonts, dialogue box skins, title BG & title BGM
/// - **Zone 1 (Story Execution Order)**: reachable bytecode CFG first-use order (putting delta bases first!)
/// - **Zone 2 (Gallery Thumbnails)**: packed contiguously for single-burst gallery opening
/// - **Zone 3 (Remaining / Dynamic)**: remaining assets in index order (delta bases first)
///
/// Maximizes OS `mmap` sequential readahead and enables Web HTTP Range coalescing!
pub fn compute_story_asset_order(program: &NirProgram) -> Vec<usize> {
    let n = program.assets.len();
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);

    let push_asset = |id: AssetId, seen: &mut [bool], order: &mut Vec<usize>| {
        let idx = id.0 as usize;
        if idx >= n || seen[idx] {
            return;
        }
        // Always lay down delta base immediately before its differential patch!
        if let Some(delta) = &program.assets[idx].delta {
            let bidx = delta.base.0 as usize;
            if bidx < n && !seen[bidx] {
                seen[bidx] = true;
                order.push(bidx);
            }
        }
        seen[idx] = true;
        order.push(idx);
    };

    // Zone 0: Fonts, Dialogue UI, Title Screen
    for &f in program
        .meta
        .ui_fonts
        .iter()
        .chain(program.meta.text_fonts.iter())
    {
        push_asset(f, &mut seen, &mut order);
    }
    if let Some(bg) = program.ui.dialogue_bg {
        push_asset(bg, &mut seen, &mut order);
    }
    for st in &program.ui.dialogue_styles {
        if let Some(bg) = st.dialogue_bg {
            push_asset(bg, &mut seen, &mut order);
        }
    }
    if let Some(bg) = program.meta.title_bg {
        push_asset(bg, &mut seen, &mut order);
    }
    if let Some(bgm) = program.meta.title_bgm {
        push_asset(bgm, &mut seen, &mut order);
    }

    // Zone 1: Story CFG execution order from `entry_func`
    let ops_len = program.ops.len();
    if ops_len > 0 {
        let mut visited_pc = vec![false; ops_len];
        let mut queue = VecDeque::new();
        let entry_pc = program
            .func(program.meta.entry_func)
            .map_or(0, |f| f.entry_pc as usize);
        if entry_pc < ops_len {
            queue.push_back(entry_pc);
        }
        while let Some(mut pc) = queue.pop_front() {
            while pc < ops_len && !visited_pc[pc] {
                visited_pc[pc] = true;
                match &program.ops[pc] {
                    Op::LayerSet { asset: Some(a), .. } | Op::LayerAsset { asset: Some(a), .. } => {
                        push_asset(*a, &mut seen, &mut order);
                    }
                    Op::StageCommit { transition, .. } => {
                        if let Some(m) = transition.mask_asset() {
                            push_asset(m, &mut seen, &mut order);
                        }
                    }
                    Op::Say { voice: Some(v), .. } => push_asset(*v, &mut seen, &mut order),
                    Op::AudioPlay { asset, .. } | Op::VideoPlay { asset, .. } => {
                        push_asset(*asset, &mut seen, &mut order);
                    }
                    Op::SetDialoguePortrait(Some(a)) => push_asset(*a, &mut seen, &mut order),
                    Op::TimelinePlay { timeline, .. } => {
                        if let Some(def) = program.timelines.get(timeline.0 as usize) {
                            for clip in &def.clips {
                                match &clip.kind {
                                    TimelineClipKind::Image { asset, .. }
                                    | TimelineClipKind::Sound { asset, .. } => {
                                        push_asset(*asset, &mut seen, &mut order);
                                    }
                                    TimelineClipKind::Box { .. } => {}
                                }
                            }
                        }
                    }
                    Op::ChoiceMenu {
                        menu,
                        hover_se,
                        click_se,
                        ..
                    } => {
                        if let Some(se) = hover_se {
                            push_asset(*se, &mut seen, &mut order);
                        }
                        if let Some(se) = click_se {
                            push_asset(*se, &mut seen, &mut order);
                        }
                        if let Some(tmpl) = program.menus.get(menu.0 as usize) {
                            if let Some(bg) = tmpl.background {
                                push_asset(bg, &mut seen, &mut order);
                            }
                            for item in &tmpl.items {
                                for a in [
                                    item.image,
                                    item.hover_image,
                                    item.disabled_image,
                                    item.preview_image,
                                ]
                                .into_iter()
                                .flatten()
                                {
                                    push_asset(a, &mut seen, &mut order);
                                }
                            }
                        }
                    }
                    Op::Jump(target) => {
                        pc = *target as usize;
                        continue;
                    }
                    Op::JumpIfFalse(target) => {
                        let t = *target as usize;
                        if t < ops_len && !visited_pc[t] {
                            queue.push_back(t);
                        }
                    }
                    Op::Call { func, .. } => {
                        if let Some(f) = program.func(*func) {
                            let fpc = f.entry_pc as usize;
                            if fpc < ops_len && !visited_pc[fpc] {
                                queue.push_back(fpc);
                            }
                        }
                    }
                    Op::Choice { items, .. } => {
                        for item in items {
                            if let Some(a) = item.image {
                                push_asset(a, &mut seen, &mut order);
                            }
                            if let Some(a) = item.hover_image {
                                push_asset(a, &mut seen, &mut order);
                            }
                            let t = item.target_pc as usize;
                            if t < ops_len && !visited_pc[t] {
                                queue.push_back(t);
                            }
                        }
                        break;
                    }
                    Op::Ret { .. } | Op::End { .. } => break,
                    _ => {}
                }
                pc += 1;
            }
        }
    }

    // Zone 2: Gallery thumbnails contiguous for fast single-burst gallery open!
    for item in &program.ui.gallery {
        if let Some(thumb) = item.thumb {
            push_asset(thumb, &mut seen, &mut order);
        }
    }

    // Zone 3: Remaining assets (unreached branches, dynamic assets, full CGs)
    for i in 0..n {
        push_asset(AssetId(i as u32), &mut seen, &mut order);
    }

    order
}

/// Metadata descriptor for one blob written through `write_nirpak_streaming`.
#[derive(Debug, Clone, Copy)]
pub struct StreamingBlobInfo {
    pub size: u64,
    /// Collision-free content digest (e.g. SHA-256) used to deduplicate blobs without holding them in RAM.
    pub hash: [u8; 32],
}

/// Zero-clone serialization view over `NirProgram` with patched `AssetMeta` offsets.
#[derive(Serialize)]
struct NirProgramRef<'a> {
    features: u32,
    meta: &'a GameMeta,
    strings: &'a [String],
    vars: &'a [VarMeta],
    funcs: &'a [FuncMeta],
    labels: &'a [LabelMeta],
    ops: &'a [Op],
    texts: &'a BTreeMap<String, Vec<TextEntry>>,
    ui: &'a UiTheme,
    assets: &'a [AssetMeta],
    asset_lookup: &'a BTreeMap<String, AssetId>,
    timelines: &'a [TimelineDef],
    menus: &'a [MenuTemplate],
}

fn serialize_program_payload(
    view: &NirProgramRef<'_>,
    compression: Compression,
) -> Result<(u16, Vec<u8>)> {
    let raw_bin = postcard::to_stdvec(view)
        .map_err(|e| NirError::new("E_PACK", "serialize", e.to_string()))?;
    if compression == Compression::none() {
        Ok((FLAG_POSTCARD, raw_bin))
    } else {
        let mut enc = ZlibEncoder::new(Vec::with_capacity(raw_bin.len() / 2), compression);
        enc.write_all(&raw_bin)
            .map_err(|e| NirError::new("E_PACK", "zlib", e.to_string()))?;
        let comp = enc
            .finish()
            .map_err(|e| NirError::new("E_PACK", "zlib", e.to_string()))?;
        Ok((FLAG_ZLIB | FLAG_POSTCARD, comp))
    }
}

/// Streams a `.nirpak` directly to any `Write` (e.g., `BufWriter<File>`) with story-ordered
/// layout, hash-based deduplication, 64-byte alignment, and O(1) media RAM overhead!
///
/// Returns `(total_bytes_written, patched_assets)`.
pub fn write_nirpak_streaming<W, F>(
    program: &NirProgram,
    blob_infos: &[StreamingBlobInfo],
    compression: Compression,
    writer: &mut W,
    mut write_blob: F,
) -> Result<(u64, Vec<AssetMeta>)>
where
    W: Write,
    F: FnMut(usize, &mut W) -> std::io::Result<()>,
{
    if blob_infos.len() != program.assets.len() {
        return Err(NirError::new(
            "E_PACK",
            "write_nirpak_streaming",
            format!(
                "blob info count {} != asset count {}",
                blob_infos.len(),
                program.assets.len()
            ),
        ));
    }

    let layout_order = compute_story_asset_order(program);
    let mut patched_assets = program.assets.clone();
    let mut dedup: HashMap<([u8; 32], u64), u64> = HashMap::with_capacity(blob_infos.len());
    let mut unique_writes: Vec<(usize, usize)> = Vec::with_capacity(blob_infos.len());
    let mut offset = 0u64;

    for idx in layout_order {
        let info = blob_infos[idx];
        let key = (info.hash, info.size);
        if let Some(&prev_off) = dedup.get(&key) {
            patched_assets[idx].offset = prev_off;
            patched_assets[idx].size = info.size;
            continue;
        }
        let pad = if offset.is_multiple_of(BLOB_ALIGN) {
            0
        } else {
            (BLOB_ALIGN - (offset % BLOB_ALIGN)) as usize
        };
        offset = offset.checked_add(pad as u64).ok_or_else(|| {
            NirError::new("E_PACK", "write_nirpak_streaming", "media size overflow")
        })?;
        patched_assets[idx].offset = offset;
        patched_assets[idx].size = info.size;
        dedup.insert(key, offset);
        unique_writes.push((idx, pad));
        offset = offset.checked_add(info.size).ok_or_else(|| {
            NirError::new("E_PACK", "write_nirpak_streaming", "media size overflow")
        })?;
    }

    let view = NirProgramRef {
        features: program.features,
        meta: &program.meta,
        strings: &program.strings,
        vars: &program.vars,
        funcs: &program.funcs,
        labels: &program.labels,
        ops: &program.ops,
        texts: &program.texts,
        ui: &program.ui,
        assets: &patched_assets,
        asset_lookup: &program.asset_lookup,
        timelines: &program.timelines,
        menus: &program.menus,
    };

    let (flags, prog_payload) = serialize_program_payload(&view, compression)?;
    let crc = crc32(&prog_payload);

    let io_err = |e: std::io::Error| NirError::new("E_PACK", "io", e.to_string());
    writer.write_all(NIRPAK_MAGIC).map_err(io_err)?;
    writer
        .write_all(&NIRPAK_VERSION.to_le_bytes())
        .map_err(io_err)?;
    writer.write_all(&flags.to_le_bytes()).map_err(io_err)?;
    writer
        .write_all(&program.features.to_le_bytes())
        .map_err(io_err)?;
    writer.write_all(&crc.to_le_bytes()).map_err(io_err)?;
    writer
        .write_all(&(prog_payload.len() as u64).to_le_bytes())
        .map_err(io_err)?;
    writer.write_all(&offset.to_le_bytes()).map_err(io_err)?;
    writer.write_all(&prog_payload).map_err(io_err)?;

    let zeros = [0u8; BLOB_ALIGN as usize];
    for (idx, pad) in unique_writes {
        if pad > 0 {
            writer.write_all(&zeros[..pad]).map_err(io_err)?;
        }
        write_blob(idx, writer).map_err(io_err)?;
    }
    writer.flush().map_err(io_err)?;

    let total_written = NIRPAK_HEADER_LEN as u64 + prog_payload.len() as u64 + offset;
    Ok((total_written, patched_assets))
}

/// Encodes a `NirProgram` and raw media blobs into a single zero-copy `.nirpak` buffer
/// with story-order layout, content-hash deduplication, Postcard binary index, and 64-byte aligned blobs.
pub fn encode_nirpak(program: &NirProgram, media_blobs: &[&[u8]]) -> Result<Vec<u8>> {
    encode_nirpak_with_compression(program, media_blobs, Compression::default())
}

/// Encodes a `.nirpak` buffer with configurable zlib compression level (e.g. `Compression::fast()`
/// or `Compression::none()` for instant dev hot-reload).
pub fn encode_nirpak_with_compression(
    program: &NirProgram,
    media_blobs: &[&[u8]],
    compression: Compression,
) -> Result<Vec<u8>> {
    if media_blobs.len() != program.assets.len() {
        return Err(NirError::new(
            "E_PACK",
            "encode_nirpak",
            format!(
                "media blob count {} != asset count {}",
                media_blobs.len(),
                program.assets.len()
            ),
        ));
    }

    // Story-order layout + content-hash deduplication + 64-byte alignment for unique media blobs!
    let layout_order = compute_story_asset_order(program);
    let mut patched_assets = program.assets.clone();
    let mut dedup: HashMap<(u32, usize), Vec<(usize, u64)>> =
        HashMap::with_capacity(media_blobs.len());
    let mut unique_writes: Vec<(usize, usize)> = Vec::with_capacity(media_blobs.len());
    let mut offset = 0u64;

    for idx in layout_order {
        let blob = media_blobs[idx];
        let size = blob.len() as u64;
        let key = (crc32(blob), blob.len());
        let existing = dedup.get(&key).and_then(|entries| {
            entries
                .iter()
                .find(|&&(prev_idx, _)| media_blobs[prev_idx] == blob)
                .map(|&(_, prev_off)| prev_off)
        });
        if let Some(prev_off) = existing {
            patched_assets[idx].offset = prev_off;
            patched_assets[idx].size = size;
            continue;
        }

        let pad = if offset.is_multiple_of(BLOB_ALIGN) {
            0
        } else {
            (BLOB_ALIGN - (offset % BLOB_ALIGN)) as usize
        };
        offset = offset
            .checked_add(pad as u64)
            .ok_or_else(|| NirError::new("E_PACK", "encode_nirpak", "media size overflow"))?;
        patched_assets[idx].offset = offset;
        patched_assets[idx].size = size;
        dedup.entry(key).or_default().push((idx, offset));
        unique_writes.push((idx, pad));
        offset = offset
            .checked_add(size)
            .ok_or_else(|| NirError::new("E_PACK", "encode_nirpak", "media size overflow"))?;
    }

    let view = NirProgramRef {
        features: program.features,
        meta: &program.meta,
        strings: &program.strings,
        vars: &program.vars,
        funcs: &program.funcs,
        labels: &program.labels,
        ops: &program.ops,
        texts: &program.texts,
        ui: &program.ui,
        assets: &patched_assets,
        asset_lookup: &program.asset_lookup,
        timelines: &program.timelines,
        menus: &program.menus,
    };

    let (flags, prog_payload) = serialize_program_payload(&view, compression)?;

    let crc = crc32(&prog_payload);
    let total_len = NIRPAK_HEADER_LEN + prog_payload.len() + offset as usize;
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(NIRPAK_MAGIC);
    out.extend_from_slice(&NIRPAK_VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&program.features.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(prog_payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&prog_payload);
    let zeros = [0u8; BLOB_ALIGN as usize];
    for (idx, pad) in unique_writes {
        if pad > 0 {
            out.extend_from_slice(&zeros[..pad]);
        }
        out.extend_from_slice(media_blobs[idx]);
    }
    Ok(out)
}

/// Decodes just the `NirProgram` index from a header-only prefix slice (does NOT require the
/// trailing media blob to be present in `bytes`), returning `(program, media_base, media_len)`.
/// Supports both compact binary Postcard (`FLAG_POSTCARD`) and legacy JSON `.nirpak` headers.
pub fn decode_nirpak_header_only(bytes: &[u8]) -> Result<(NirProgram, usize, u64)> {
    if bytes.len() < NIRPAK_HEADER_LEN {
        return Err(NirError::new("E_PACK", "header", "truncated nirpak header"));
    }
    if &bytes[0..4] != NIRPAK_MAGIC {
        return Err(NirError::new("E_PACK", "header", "invalid NIRP magic"));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != NIRPAK_VERSION {
        return Err(NirError::new(
            "E_VERSION",
            "header",
            format!("unsupported nirpak version {version}, expected {NIRPAK_VERSION}"),
        ));
    }
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    let _features = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let expected_crc = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let prog_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let media_len = u64::from_le_bytes(bytes[24..32].try_into().unwrap());

    let media_base = NIRPAK_HEADER_LEN
        .checked_add(prog_len)
        .ok_or_else(|| NirError::new("E_PACK", "header", "program length overflow"))?;
    if bytes.len() < media_base {
        return Err(NirError::new(
            "E_PACK",
            "header",
            "truncated nirpak program section",
        ));
    }
    let prog_slice = &bytes[NIRPAK_HEADER_LEN..media_base];
    if crc32(prog_slice) != expected_crc {
        return Err(NirError::new(
            "E_PACK",
            "crc",
            "program section CRC mismatch",
        ));
    }

    let is_postcard = flags & FLAG_POSTCARD != 0;
    let is_zlib = flags & FLAG_ZLIB != 0;
    let program: NirProgram = if is_postcard {
        if is_zlib {
            let mut raw_bin = Vec::with_capacity(prog_slice.len().saturating_mul(4));
            ZlibDecoder::new(prog_slice)
                .read_to_end(&mut raw_bin)
                .map_err(|e| NirError::new("E_PACK", "zlib", e.to_string()))?;
            postcard::from_bytes(&raw_bin)
                .map_err(|e| NirError::new("E_PACK", "program", e.to_string()))?
        } else {
            postcard::from_bytes(prog_slice)
                .map_err(|e| NirError::new("E_PACK", "program", e.to_string()))?
        }
    } else if is_zlib {
        let dec = ZlibDecoder::new(prog_slice);
        serde_json::from_reader(dec)
            .map_err(|e| NirError::new("E_PACK", "program", e.to_string()))?
    } else {
        serde_json::from_slice(prog_slice)
            .map_err(|e| NirError::new("E_PACK", "program", e.to_string()))?
    };
    Ok((program, media_base, media_len))
}

/// Decodes just the `NirProgram` index + returns the byte offset where the media blob begins.
/// Validates that the complete media payload is present in `bytes`.
pub fn decode_nirpak_header(bytes: &[u8]) -> Result<(NirProgram, usize)> {
    let (program, media_base, media_len) = decode_nirpak_header_only(bytes)?;
    let expected_total = media_base
        .checked_add(media_len as usize)
        .ok_or_else(|| NirError::new("E_PACK", "header", "media length overflow"))?;
    if bytes.len() < expected_total {
        return Err(NirError::new(
            "E_PACK",
            "header",
            "truncated nirpak payload",
        ));
    }
    Ok((program, media_base))
}

/// Zero-copy slice lookup for one asset inside an in-memory or `mmap`-backed `.nirpak`.
pub fn nirpak_asset_slice<'a>(
    pack_bytes: &'a [u8],
    media_base: usize,
    asset: &AssetMeta,
) -> Result<&'a [u8]> {
    let start = media_base
        .checked_add(asset.offset as usize)
        .ok_or_else(|| NirError::new("E_PACK", "asset", "offset overflow"))?;
    let end = start
        .checked_add(asset.size as usize)
        .ok_or_else(|| NirError::new("E_PACK", "asset", "size overflow"))?;
    pack_bytes
        .get(start..end)
        .ok_or_else(|| NirError::new("E_PACK", "asset", "asset slice out of bounds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinOp, LayerId, Transform2D, Value, VarId};

    #[test]
    fn nirpak_roundtrip_and_zero_copy_asset_slice() {
        let program = NirProgram {
            features: FEATURE_WEBP | FEATURE_MP3 | FEATURE_TRIMMED,
            meta: GameMeta {
                game_id: "org.nir.demo".into(),
                title: "Demo Story".into(),
                version: "0.2.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "zh-Hans".into(),
                locales: vec!["zh-Hans".into(), "en".into()],
                entry_func: FuncId(0),
                save_slots: 12,
                default_reveal_us: 30_000,
                title_bg: Some(AssetId(0)),
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec![
                "main".into(),
                "score".into(),
                "bg/station".into(),
                "Aki".into(),
            ],
            vars: vec![VarMeta {
                name: StrId(1),
                default: Value::Int(0),
            }],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::LayerSet {
                    layer: LayerId(0),
                    parent: None,
                    asset: Some(AssetId(0)),
                    transform: Box::new(Transform2D::default()),
                },
                Op::PushInt(1),
                Op::LoadVar(VarId(0)),
                Op::BinOp(BinOp::Add),
                Op::StoreVar(VarId(0)),
                Op::End { outcome: StrId(0) },
            ],
            texts: BTreeMap::from([("zh-Hans".into(), vec![TextEntry::plain("你好，新世界！")])]),
            ui: UiTheme::default(),
            assets: vec![AssetMeta {
                id: AssetId(0),
                name: StrId(2),
                kind: AssetKind::Image,
                codec: MediaCodec::Webp,
                width: 1280,
                height: 720,
                trim: Some(SpriteTrim {
                    x: 10,
                    y: 20,
                    w: 100,
                    h: 200,
                    orig_w: 1280,
                    orig_h: 720,
                }),
                duration_us: 0,
                sample_rate: 0,
                channels: 0,
                loop_region: None,
                offset: 0,
                size: 0,
                delta: None,
            }],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };

        let blob = b"fake-webp-bytes-1234567890";
        let packed = encode_nirpak(&program, &[blob.as_slice()]).unwrap();
        let (decoded, media_base) = decode_nirpak_header(&packed).unwrap();
        assert_eq!(decoded.meta.game_id, "org.nir.demo");
        assert_eq!(decoded.ops.len(), 6);
        assert_eq!(
            nirpak_asset_slice(&packed, media_base, &decoded.assets[0]).unwrap(),
            blob
        );
        let chars = decoded.collect_all_chars();
        assert!(chars.contains(&'你') && chars.contains(&'世'));

        // Corrupt a byte in compressed program section -> CRC rejects it.
        let mut bad = packed.clone();
        bad[NIRPAK_HEADER_LEN + 2] ^= 0xff;
        assert_eq!(decode_nirpak_header(&bad).unwrap_err().code, "E_PACK");

        // Header-only decode works with just the prefix (without media payload bytes)!
        let (hdr_prog, hdr_base, hdr_media_len) =
            decode_nirpak_header_only(&packed[..media_base]).unwrap();
        assert_eq!(hdr_prog.meta.game_id, "org.nir.demo");
        assert_eq!(hdr_base, media_base);
        assert_eq!(hdr_media_len, blob.len() as u64);
    }

    #[test]
    fn nirpak_deduplicates_identical_blobs_and_aligns_unique_blobs() {
        let mut program = NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.dedup".into(),
                title: "Dedup".into(),
                version: "1".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 10_000,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["a".into(), "b".into(), "c".into()],
            vars: vec![],
            funcs: vec![],
            labels: vec![],
            ops: vec![],
            texts: BTreeMap::new(),
            ui: UiTheme::default(),
            assets: (0..3)
                .map(|i| AssetMeta {
                    id: AssetId(i),
                    name: StrId(i),
                    kind: AssetKind::Raw,
                    codec: MediaCodec::Raw,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    offset: 0,
                    size: 0,
                    delta: None,
                })
                .collect(),
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };
        let blob_a = vec![0xABu8; 1000];
        let blob_c = vec![0xCDu8; 500];
        let packed = encode_nirpak(&program, &[&blob_a, &blob_a, &blob_c]).unwrap();
        let (decoded, media_base) = decode_nirpak_header(&packed).unwrap();
        // Assets 0 and 1 have identical content -> share exact offset & size!
        assert_eq!(decoded.assets[0].offset, decoded.assets[1].offset);
        assert_eq!(decoded.assets[0].size, decoded.assets[1].size);
        // Asset 2 starts on a 64-byte boundary!
        assert_eq!(decoded.assets[2].offset % BLOB_ALIGN, 0);
        assert_eq!(
            nirpak_asset_slice(&packed, media_base, &decoded.assets[1]).unwrap(),
            blob_a.as_slice()
        );
        assert_eq!(
            nirpak_asset_slice(&packed, media_base, &decoded.assets[2]).unwrap(),
            blob_c.as_slice()
        );
        // Total media length is far smaller than storing blob_a twice.
        let total_media = packed.len() - media_base;
        assert!(total_media < 1700);
        program.features = 1; // touch mut binding cleanly
    }

    #[test]
    fn story_order_and_streaming_nirpak_match_in_memory() {
        // Asset 0: late gallery thumb, Asset 1: delta child of 2, Asset 2: base CG used in story, Asset 3: title BG
        let mut program = NirProgram {
            features: FEATURE_WEBP | FEATURE_DELTA_IMAGE,
            meta: GameMeta {
                game_id: "org.nir.order".into(),
                title: "Order".into(),
                version: "1".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 10_000,
                title_bg: Some(AssetId(3)),
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec![
                "entry".into(),
                "a0".into(),
                "a1".into(),
                "a2".into(),
                "a3".into(),
            ],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::LayerAsset {
                    layer: crate::LayerId(0),
                    asset: Some(AssetId(1)),
                },
                Op::End { outcome: StrId(0) },
            ],
            texts: BTreeMap::new(),
            ui: UiTheme {
                gallery: vec![crate::GalleryItem {
                    id: StrId(1),
                    title: StrId(1),
                    thumb: Some(AssetId(0)),
                    unlock_flag: None,
                    replay_func: None,
                    cg_assets: vec![],
                }],
                ..UiTheme::default()
            },
            assets: (0..4)
                .map(|i| AssetMeta {
                    id: AssetId(i),
                    name: StrId(i + 1),
                    kind: AssetKind::Image,
                    codec: MediaCodec::Webp,
                    width: 64,
                    height: 64,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    offset: 0,
                    size: 0,
                    delta: if i == 1 {
                        Some(ImageDelta {
                            base: AssetId(2),
                            x: 8,
                            y: 8,
                            tile_mask: vec![0b0000_0001],
                        })
                    } else {
                        None
                    },
                })
                .collect(),
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };

        // Expected layout order: Zone 0 [3 (title_bg)], Zone 1 [2 (base of 1), 1], Zone 2 [0 (gallery)]
        let order = compute_story_asset_order(&program);
        assert_eq!(order, vec![3, 2, 1, 0]);

        let blobs: Vec<Vec<u8>> = vec![
            vec![0x10; 100],
            vec![0x20; 80],
            vec![0x30; 200],
            vec![0x40; 150],
        ];
        let refs: Vec<&[u8]> = blobs.iter().map(Vec::as_slice).collect();
        let mem_pak = encode_nirpak(&program, &refs).unwrap();

        let infos: Vec<StreamingBlobInfo> = blobs
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let mut h = [0u8; 32];
                h[0] = i as u8 + 1;
                StreamingBlobInfo {
                    size: b.len() as u64,
                    hash: h,
                }
            })
            .collect();
        let mut streamed = Vec::new();
        let (written, _) = write_nirpak_streaming(
            &program,
            &infos,
            Compression::default(),
            &mut streamed,
            |idx, w| w.write_all(&blobs[idx]),
        )
        .unwrap();
        assert_eq!(written as usize, streamed.len());
        assert_eq!(streamed, mem_pak);

        let (dec, mbase) = decode_nirpak_header(&streamed).unwrap();
        // Verify title_bg (3) sits at offset 0, base (2) before child (1), gallery (0) last!
        assert_eq!(dec.assets[3].offset, 0);
        assert!(dec.assets[2].offset < dec.assets[1].offset);
        assert!(dec.assets[1].offset < dec.assets[0].offset);
        assert_eq!(dec.assets[1].delta.as_ref().unwrap().base, AssetId(2));
        for (i, blob) in blobs.iter().enumerate() {
            assert_eq!(
                nirpak_asset_slice(&streamed, mbase, &dec.assets[i]).unwrap(),
                blob.as_slice()
            );
        }

        // Legacy JSON .nirpak (FLAG_POSTCARD unset) still decodes cleanly!
        let raw_json = serde_json::to_vec(&dec).unwrap();
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&raw_json).unwrap();
        let zjson = z.finish().unwrap();
        let mut legacy = Vec::new();
        legacy.extend_from_slice(NIRPAK_MAGIC);
        legacy.extend_from_slice(&NIRPAK_VERSION.to_le_bytes());
        legacy.extend_from_slice(&FLAG_ZLIB.to_le_bytes());
        legacy.extend_from_slice(&dec.features.to_le_bytes());
        legacy.extend_from_slice(&crc32(&zjson).to_le_bytes());
        legacy.extend_from_slice(&(zjson.len() as u64).to_le_bytes());
        legacy.extend_from_slice(&0u64.to_le_bytes());
        legacy.extend_from_slice(&zjson);
        let (legacy_prog, _, _) = decode_nirpak_header_only(&legacy).unwrap();
        assert_eq!(legacy_prog.meta.game_id, "org.nir.order");
        program.features = 0;
    }
}
