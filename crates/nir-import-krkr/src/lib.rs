#![forbid(unsafe_code)]

//! KiriKiri2 / KAG3 engine importer (`nir-import-krkr`).
//!
//! Supports both unpacked game directories and standard `.xp3` archives (`data.xp3`
//! + `patch*.xp3` layered in KiriKiri2 priority order), lowering `.ks` scenarios
//!   cleanly onto NIR v2's `VnProject` (Persistent Scene Graph + `.nirs`).

pub mod kag;
pub mod xp3;

use anyhow::Result;
use kag::{ks_stem, lower_ks_script, AssetResolver};
use nir_import_core::{
    decode_text_bytes, sanitize_ident, ArchiveSource, CompositeVfs, ConversionReport, DirArchive,
    EngineImporter, EngineProbe, ImportOptions, VnProject, VnStmt,
};
use std::collections::BTreeMap;
use std::path::Path;
use xp3::Xp3Archive;

pub struct KrkrImporter;

impl EngineImporter for KrkrImporter {
    fn id(&self) -> &'static str {
        "krkr"
    }

    fn display_name(&self) -> &'static str {
        "KiriKiri2 / KAG3 (.ks / .xp3)"
    }

    fn detect(&self, source: &Path) -> Option<EngineProbe> {
        if source.is_file() {
            if source
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("xp3"))
            {
                return Some(EngineProbe {
                    engine_id: self.id().into(),
                    display_name: self.display_name().into(),
                    confidence: 96,
                    summary: "XP3 archive".into(),
                });
            }
            return None;
        }
        if !source.is_dir() {
            return None;
        }
        let dir = DirArchive::open(source).ok()?;
        let files = dir.list_files();
        let xp3_count = files.iter().filter(|f| f.ends_with(".xp3")).count();
        let ks_count = files.iter().filter(|f| f.ends_with(".ks")).count();
        let has_config = files.iter().any(|f| f.ends_with("config.tjs"));
        if xp3_count > 0 || ks_count > 0 {
            let confidence = if (xp3_count > 0 || has_config) && (ks_count > 0 || xp3_count > 0) {
                96
            } else {
                88
            };
            Some(EngineProbe {
                engine_id: self.id().into(),
                display_name: self.display_name().into(),
                confidence,
                summary: format!("{xp3_count} .xp3 archive(s), {ks_count} .ks script(s)"),
            })
        } else {
            None
        }
    }

    fn import(&self, source: &Path, opts: &ImportOptions) -> Result<(VnProject, ConversionReport)> {
        let mut vfs = CompositeVfs::new();
        let mut report = ConversionReport::new(self.id());

        if source.is_file() {
            let arc = Xp3Archive::open(source)?;
            if arc.protected_count > 0 {
                report.record_fallback(
                    source.display().to_string(),
                    format!("{} XP3 entries are DRM-protected", arc.protected_count),
                );
            }
            vfs.mount(arc);
        } else {
            let dir = DirArchive::open(source)?;
            // Mount `.xp3` archives first (`data.xp3`, then `patch*.xp3` alphabetically)
            let mut xp3s: Vec<String> = dir
                .list_files()
                .into_iter()
                .filter(|f| f.ends_with(".xp3"))
                .collect();
            xp3s.sort_by(|a, b| {
                let a_patch = a.contains("patch");
                let b_patch = b.contains("patch");
                a_patch.cmp(&b_patch).then_with(|| a.cmp(b))
            });
            for rel in xp3s {
                if let Some(real) = dir.real_path(&rel) {
                    match Xp3Archive::open(real) {
                        Ok(arc) => {
                            if arc.protected_count > 0 {
                                report.record_fallback(
                                    &rel,
                                    format!("{} protected entries in XP3", arc.protected_count),
                                );
                            }
                            vfs.mount(arc);
                        }
                        Err(e) => {
                            report.record_fallback(&rel, format!("failed to open XP3: {e}"));
                        }
                    }
                }
            }
            // Loose files in directory override archives (KiriKiri2 semantics)
            vfs.mount(dir);
        }

        let title_fallback = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("KAG3 Game")
            .to_owned();
        let mut project = VnProject {
            id: format!("org.nir.krkr.{}", sanitize_ident(&title_fallback)),
            title: title_fallback,
            source_locale: opts.source_locale.clone().unwrap_or_else(|| "ja".into()),
            stage_width: 1280,
            stage_height: 720,
            ..VnProject::default()
        };

        // Parse Config.tjs hints (scWidth / scHeight / windowTitle)
        let files = vfs.list_files();
        if let Some(cfg_path) = files
            .iter()
            .find(|f| f == &"config.tjs" || f.ends_with("/config.tjs"))
        {
            if let Ok(bytes) = vfs.read(cfg_path) {
                let (text, _) = decode_text_bytes(&bytes);
                parse_config_tjs(&text, &mut project);
            }
        }

        // Collect all `.ks` files, prioritizing `first.ks` as entry
        let mut ks_files: Vec<String> = files
            .iter()
            .filter(|f| f.ends_with(".ks"))
            .cloned()
            .collect();
        if let Some(pos) = ks_files
            .iter()
            .position(|f| f == "first.ks" || f.ends_with("/first.ks"))
        {
            let first = ks_files.remove(pos);
            ks_files.insert(0, first);
        }
        ks_files.truncate(opts.max_scripts);

        let mut macros = BTreeMap::new();
        let mut asset_cache = BTreeMap::new();

        for (idx, ks_path) in ks_files.iter().enumerate() {
            let bytes = match vfs.read(ks_path) {
                Ok(b) => b,
                Err(e) => {
                    report.record_fallback(ks_path, format!("failed to read script: {e}"));
                    continue;
                }
            };
            let (text, _) = decode_text_bytes(&bytes);
            let stem = ks_stem(ks_path);
            let script_key = if idx == 0 {
                "main".to_owned()
            } else {
                stem.clone()
            };

            let mut new_assets = BTreeMap::new();
            let mut resolver = KrkrResolver {
                vfs: &vfs,
                assets: &mut new_assets,
                cache: &mut asset_cache,
                missing: Vec::new(),
            };
            let stmts = lower_ks_script(
                &stem,
                &text,
                &mut project,
                &mut report,
                &mut resolver,
                &mut macros,
            );
            for (loc, msg) in resolver.missing {
                report.record_fallback(loc, msg);
            }
            project.assets.extend(new_assets);
            report.scripts_converted += 1;
            report.statements_emitted += stmts.len();
            project.scripts.insert(script_key, stmts);
        }

        if project.scripts.is_empty() {
            project.scripts.insert(
                "main".into(),
                vec![VnStmt::End {
                    outcome: "end".into(),
                }],
            );
        }
        report.assets_converted = project.assets.len();
        Ok((project, report))
    }
}

struct KrkrResolver<'a> {
    vfs: &'a CompositeVfs,
    assets: &'a mut BTreeMap<String, Vec<u8>>,
    cache: &'a mut BTreeMap<String, String>,
    missing: Vec<(String, String)>,
}

impl<'a> KrkrResolver<'a> {
    fn import_generic(
        &mut self,
        src_loc: &str,
        storage: &str,
        folder: &str,
        exts: &[&str],
    ) -> Option<String> {
        let Some(vfs_path) = self.vfs.find_by_stem(storage, exts) else {
            self.missing
                .push((src_loc.to_owned(), format!("missing asset `{storage}`")));
            return None;
        };
        if let Some(existing) = self.cache.get(&vfs_path) {
            return Some(existing.clone());
        }
        let bytes = match self.vfs.read(&vfs_path) {
            Ok(b) => b,
            Err(e) => {
                self.missing
                    .push((src_loc.to_owned(), format!("reading `{vfs_path}`: {e}")));
                return None;
            }
        };
        let without_ext = vfs_path
            .rsplit_once('.')
            .map(|(a, _)| a)
            .unwrap_or(&vfs_path);
        let stem = sanitize_ident(without_ext);
        let ext = vfs_path.rsplit('.').next().unwrap_or("bin");

        let (rel, out_bytes) = if ext == "bmp" {
            match image::load_from_memory(&bytes) {
                Ok(img) => {
                    let mut png = Vec::new();
                    let mut cur = std::io::Cursor::new(&mut png);
                    if img.write_to(&mut cur, image::ImageFormat::Png).is_ok() {
                        (format!("{folder}/{stem}.png"), png)
                    } else {
                        (format!("{folder}/{stem}.{ext}"), bytes)
                    }
                }
                Err(_) => (format!("{folder}/{stem}.{ext}"), bytes),
            }
        } else {
            (format!("{folder}/{stem}.{ext}"), bytes)
        };
        self.assets.insert(rel.clone(), out_bytes);
        self.cache.insert(vfs_path, rel.clone());
        Some(rel)
    }
}

impl<'a> AssetResolver for KrkrResolver<'a> {
    fn resolve_image(&mut self, src_loc: &str, storage: &str) -> Option<String> {
        self.import_generic(
            src_loc,
            storage,
            "img",
            &["png", "webp", "jpg", "jpeg", "bmp"],
        )
    }

    fn resolve_audio(&mut self, src_loc: &str, storage: &str) -> Option<String> {
        self.import_generic(src_loc, storage, "audio", &["ogg", "mp3", "wav"])
    }

    fn resolve_video(&mut self, src_loc: &str, storage: &str) -> Option<String> {
        self.import_generic(src_loc, storage, "video", &["webm", "mp4"])
    }
}

fn parse_config_tjs(text: &str, project: &mut VnProject) {
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("//") {
            continue;
        }
        if let Some(rest) = t.strip_prefix(";scWidth") {
            if let Some(val) = parse_tjs_int_assign(rest) {
                if val >= 320 {
                    project.stage_width = val;
                }
            }
        } else if let Some(rest) = t.strip_prefix(";scHeight") {
            if let Some(val) = parse_tjs_int_assign(rest) {
                if val >= 240 {
                    project.stage_height = val;
                }
            }
        } else if let Some(rest) = t.strip_prefix(";System.title") {
            if let Some(q1) = rest.find('"') {
                if let Some(q2) = rest[q1 + 1..].find('"') {
                    let title = &rest[q1 + 1..q1 + 1 + q2];
                    if !title.trim().is_empty() {
                        project.title = title.to_owned();
                    }
                }
            }
        }
    }
}

fn parse_tjs_int_assign(rest: &str) -> Option<u32> {
    let (_, rhs) = rest.split_once('=')?;
    rhs.trim().trim_end_matches(';').trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_import_core::emit_nirs_project;
    use std::sync::Arc;

    fn tiny_png(rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba(rgba));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn krkr_xp3_end_to_end_compiles_and_executes_in_nir_vm() {
        let ks = r#"
; Sample KAG3 scenario
*start
[eval exp="f.score = 10"]
[image storage="bg_station" layer="base" page="fore"]
[image storage="aki" layer="0" left="320" top="60"]
[trans method="crossfade" time="300"]
【アキ】こんにちは、[ruby text="あめ"]雨ですね。[p]
[link target="*walk"]駅を出る[endlink][r]
[link target="*stay"]ホームに残る[endlink][r]
[s]

*walk
[eval exp="f.score += 5"]
外は涼しい。[p]
[s]

*stay
ホームは静かだ。[p]
[s]
"#;
        let bg = tiny_png([20, 30, 60, 255]);
        let ch = tiny_png([200, 180, 160, 255]);
        let cfg = b";scWidth = 1280;\n;scHeight = 720;\n;System.title = \"KAG Demo\";\n";

        let xp3_bytes = xp3::test_util::build_xp3(&[
            ("system/Config.tjs", cfg, true),
            ("scenario/first.ks", ks.as_bytes(), true),
            ("bgimage/bg_station.png", &bg, false),
            ("fgimage/aki.png", &ch, false),
        ]);

        let src_dir = tempfile::tempdir().unwrap();
        std::fs::write(src_dir.path().join("data.xp3"), xp3_bytes).unwrap();

        let importer = KrkrImporter;
        let probe = importer.detect(src_dir.path()).expect("detect krkr");
        assert_eq!(probe.engine_id, "krkr");

        let (project, report) = importer
            .import(src_dir.path(), &ImportOptions::default())
            .unwrap();
        assert_eq!(project.title, "KAG Demo");
        assert!(report.is_strict_clean(), "{}", report.summary_text());
        assert_eq!(project.assets.len(), 2);

        let out_dir = tempfile::tempdir().unwrap();
        emit_nirs_project(&project, out_dir.path()).unwrap();

        // Compile converted `.nirs` project with `nir-build` and execute in `nir-vm`!
        let built = nir_build::compile_project(
            out_dir.path(),
            &nir_build::optimize::OptimizeConfig {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        let mut vm = nir_vm::Vm::new(Arc::new(built.program), None).unwrap();

        // Step through bg+show+trans -> hits transition wait, finish it
        vm.step(1000).unwrap();
        vm.tick(500_000, 1.0, false);
        // Step to dialogue
        vm.step(1000).unwrap();
        assert!(vm
            .dialogue
            .as_ref()
            .unwrap()
            .full_text
            .contains("こんにちは"));
        vm.advance(false);
        vm.advance(false);
        // Step to choice menu!
        vm.step(1000).unwrap();
        assert_eq!(vm.choice.as_ref().unwrap().items.len(), 2);
        // Pick option 0 ("駅を出る" -> *walk)
        let _ = vm.choose(0);
        vm.step(1000).unwrap();
        assert!(vm
            .dialogue
            .as_ref()
            .unwrap()
            .full_text
            .contains("外は涼しい"));
        vm.advance(false);
        vm.advance(false);
        vm.step(1000).unwrap();
        assert_eq!(vm.outcome.as_deref(), Some("end"));
    }
}
