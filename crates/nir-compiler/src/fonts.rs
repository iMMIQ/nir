//! Native-only font preparation. No font tools or system fonts are needed by authors.
use anyhow::{anyhow, bail, Context, Result};
use hb_subset::{Blob, FontFace, SubsetInput};
use nir_format::{AssetKind, Program};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
};

pub const FONT_TOOL: &str =
    "nir-font/1;hb-subset/0.3.0;harfbuzz/8.2.2;unicode-normalization/0.1.25";
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontRecipe {
    pub mode: FontMode,
    /// Zero-based face in an OTF/TTF/TTC source. Output always contains one face.
    #[serde(default)]
    pub face_index: u32,
    #[serde(default)]
    pub extra_characters: String,
    /// Relative to the asset catalog, with the same containment rules as source.
    pub license: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontMode {
    Subset,
    Full,
}
#[derive(Debug, Serialize)]
pub struct FontReport {
    pub tool: String,
    pub mode: FontMode,
    pub face_index: u32,
    pub source: String,
    pub source_digest: String,
    pub source_bytes: usize,
    pub object: String,
    pub output_bytes: usize,
    pub requested_characters: usize,
    pub retained_characters: usize,
    pub cache_key: String,
    pub cache_hit: bool,
    pub license: String,
    pub license_digest: String,
}

pub(crate) fn characters(p: &Program, title: &str) -> Result<BTreeSet<char>> {
    let mut chars: BTreeSet<char> = (' '..='~').collect();
    // All built-in UI locales travel with the compiler. These are source data,
    // not a dependency on presentation/GPU. Control symbols are Fluent too.
    for text in [
        title,
        include_str!("../../nir-presentation/messages/zh-Hans.ftl"),
        include_str!("../../nir-presentation/messages/en.ftl"),
    ] {
        chars.extend(text.chars());
    }
    fn visit(v: &serde_json::Value, chars: &mut BTreeSet<char>) {
        match v {
            serde_json::Value::Object(o) => {
                if o.get("type").and_then(|v| v.as_str()) == Some("string") {
                    if let Some(s) = o.get("value").and_then(|v| v.as_str()) {
                        chars.extend(s.chars());
                    }
                }
                // Span text, dialogue speaker, and logical text IDs (also used
                // in save labels). Walk every expression, including call args.
                for key in ["text", "speaker"] {
                    if let Some(s) = o.get(key).and_then(|v| v.as_str()) {
                        chars.extend(s.chars());
                    }
                }
                for v in o.values() {
                    visit(v, chars);
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    visit(v, chars);
                }
            }
            _ => {}
        }
    }
    visit(&serde_json::to_value(p)?, &mut chars);
    chars.retain(|c| !c.is_control());
    Ok(chars)
}

fn face(bytes: &[u8], index: u32) -> Result<ttf_parser::Face<'_>> {
    let f = ttf_parser::Face::parse(bytes, index)
        .map_err(|_| anyhow!("E_FONT: invalid font or face_index {index}"))?;
    // Bound this profile explicitly: OpenType static outlines and layout only.
    let raw = f.raw_face();
    if f.is_variable()
        || [
            b"fvar", b"morx", b"mort", b"Silf", b"SVG ", b"COLR", b"CBDT", b"sbix",
        ]
        .iter()
        .any(|tag| raw.table(ttf_parser::Tag::from_bytes(tag)).is_some())
        || (f.tables().glyf.is_none() && f.tables().cff.is_none())
    {
        bail!("E_FONT_CAPABILITY: expected static OpenType glyf/CFF outlines; variable, color, AAT and Graphite fonts are unsupported");
    }
    Ok(f)
}
fn missing<'a>(
    chars: impl Iterator<Item = &'a char>,
    faces: &[ttf_parser::Face<'_>],
) -> Vec<String> {
    chars
        .filter(|c| !c.is_whitespace() && !faces.iter().any(|f| f.glyph_index(**c).is_some()))
        .take(12)
        .map(|c| format!("{c} (U+{:04X})", *c as u32))
        .collect()
}
pub(crate) fn coverage(
    p: &Program,
    media: &BTreeMap<String, Vec<u8>>,
    chars: &BTreeSet<char>,
) -> Result<()> {
    let fonts: Vec<_> = p
        .assets
        .iter()
        .filter(|(_, a)| a.kind == AssetKind::Font)
        .map(|(id, _)| face(&media[id], 0))
        .collect::<Result<_>>()?;
    if fonts.is_empty() {
        bail!("E_FONT: register a font asset");
    }
    let absent = missing(chars.iter(), &fonts);
    if !absent.is_empty() {
        bail!(
            "E_FONT_COVERAGE: UI/body/title/interpolation lacks {}; provide a licensed master font",
            absent.join(", ")
        );
    }
    Ok(())
}

pub(crate) fn prepare(
    root: &Path,
    bytes: &[u8],
    recipe: &FontRecipe,
    chars: &BTreeSet<char>,
) -> Result<(Vec<u8>, FontReport)> {
    let original = face(bytes, recipe.face_index)?;
    let extra: BTreeSet<_> = recipe.extra_characters.chars().collect();
    let absent = missing(extra.iter(), std::slice::from_ref(&original));
    if !absent.is_empty() {
        bail!(
            "E_FONT_COVERAGE: extra_characters absent from source: {}",
            absent.join(", ")
        );
    }
    let mut wanted = chars.clone();
    wanted.extend(extra);
    let requested = wanted.len();
    // Shapers canonically normalize before applying GSUB. Retain decomposed
    // marks and every source character composable from them, including across
    // Span/Param boundaries. GSUB closure alone cannot preserve e.g. a + acute.
    let mut decomposed = BTreeSet::new();
    for c in &wanted {
        unicode_normalization::char::decompose_canonical(*c, |d| {
            decomposed.insert(d);
        });
    }
    wanted.extend(&decomposed);
    let source = FontFace::new_with_index(Blob::from_bytes(bytes)?, recipe.face_index)?;
    for c in source.covered_codepoints()?.iter() {
        let mut composable = true;
        unicode_normalization::char::decompose_canonical(c, |d| {
            composable &= decomposed.contains(&d);
        });
        if composable {
            wanted.insert(c);
        }
    }
    wanted.retain(|c| original.glyph_index(*c).is_some());
    let source_digest = nir_content::digest(bytes);
    // Include implementation identity as well as the exact pinned tool and data.
    // License paths, filenames and timestamps cannot affect generated font bytes.
    let cache_key = nir_content::digest(&serde_json::to_vec(&(
        FONT_TOOL,
        nir_content::digest(include_bytes!("fonts.rs")),
        &source_digest,
        recipe.mode,
        recipe.face_index,
        &wanted,
    ))?);
    let mut cache = root.to_path_buf();
    for part in [".nir", "cache", "fonts"] {
        cache.push(part);
        if let Ok(meta) = fs::symlink_metadata(&cache) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                bail!("E_PATH_ESCAPE: font cache directory must be a real project directory");
            }
        }
        fs::create_dir_all(&cache)?;
    }
    let path = cache.join(format!("{cache_key}.font"));
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("E_PATH_ESCAPE: font cache entry is a symlink");
    }
    // One atomic envelope prevents concurrent readers seeing a half-written pair.
    let cached = fs::metadata(&path)
        .ok()
        .filter(|m| m.len() <= 64 * 1024 * 1024 + 64)
        .and_then(|_| fs::read(&path).ok())
        .filter(|b| b.len() > 64 && nir_content::digest(&b[64..]).as_bytes() == &b[..64])
        .filter(|b| face(&b[64..], 0).is_ok_and(|f| missing(wanted.iter(), &[f]).is_empty()));
    let cache_hit = cached.is_some();
    let output = if let Some(b) = cached {
        b[64..].to_vec()
    } else {
        let mut input = SubsetInput::new().context("E_FONT_SUBSET: allocation")?;
        // Preserve all scripts/features and names; HarfBuzz computes glyph closure
        // through GSUB/GPOS/GDEF, including ligatures and mark positioning.
        input.keep_everything();
        input
            .flags()
            .remove_unrecognized_tables()
            .retain_legacy_names();
        if recipe.mode == FontMode::Subset {
            input.glyph_set().clear();
            input.unicode_set().clear();
            for c in &wanted {
                input.unicode_set().insert(*c);
            }
        }
        let subset = input
            .subset_font(&source)
            .context("E_FONT_SUBSET: OpenType subset failed")?;
        let output = subset.underlying_blob().to_vec();
        let f = face(&output, 0)?;
        if !missing(wanted.iter(), &[f]).is_empty() {
            bail!("E_FONT_SUBSET: generated font lost requested glyphs");
        }
        if output.len() > 64 * 1024 * 1024 {
            bail!("E_LIMIT: generated font exceeds 64 MiB");
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&cache)?;
        tmp.write_all(nir_content::digest(&output).as_bytes())?;
        tmp.write_all(&output)?;
        tmp.persist(&path).context("E_FONT_CACHE: commit")?;
        output
    };
    let retained = FontFace::new(Blob::from_bytes(&output)?)?
        .covered_codepoints()?
        .len();
    let report = FontReport {
        tool: FONT_TOOL.into(),
        mode: recipe.mode,
        face_index: recipe.face_index,
        source: String::new(),
        source_digest,
        source_bytes: bytes.len(),
        object: nir_content::digest(&output),
        output_bytes: output.len(),
        requested_characters: requested,
        retained_characters: retained,
        cache_key,
        cache_hit,
        license: String::new(),
        license_digest: String::new(),
    };
    Ok((output, report))
}
