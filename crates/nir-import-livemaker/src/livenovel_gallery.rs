//! Finite CG pages built from the source gallery inventory. Unlocks use the
//! canonical image identity shared with CREATECG/CHANGECG, never file ordering.
use super::*;

pub(super) fn build(adapter: &mut Adapter) -> Result<()> {
    let path = match adapter.source.path("グラフィック/CGモード/CGモード.TXT") {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };
    let bytes = read_binary(&path)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "E_IMPORT_GALLERY: inventory exceeds 1 MiB"
    );
    let inventory = super::super::lsb::decode(&bytes)?;
    let mut images = vec![];
    for source in inventory.lines().filter(|line| !line.is_empty()) {
        ensure!(
            images.len() < 2048,
            "E_IMPORT_GALLERY: more than 2048 images"
        );
        let (image, _) = adapter.image(source)?;
        adapter.cg_images.insert(image.clone());
        images.push(image);
    }
    if images.is_empty() {
        return Ok(());
    }
    let (_, script) = adapter.source.read("ノベルシステム/CGモード/■開始.lsb")?;
    let mut backgrounds = BTreeSet::new();
    for command in &script.commands {
        if command.muted || command.kind != 9 {
            continue;
        }
        if let Body::Object(properties) = &command.body {
            if properties.get(&1).and_then(|e| literal_string(e).ok()) == Some("CGモード背景")
            {
                backgrounds.insert(
                    literal_string(
                        properties
                            .get(&3)
                            .context("E_IMPORT_GALLERY: background source")?,
                    )?
                    .to_owned(),
                );
            }
        }
    }
    ensure!(
        backgrounds.len() == 1,
        "E_IMPORT_GALLERY: missing/dynamic background"
    );
    let (background, _) = adapter.image(backgrounds.iter().next().unwrap())?;
    let pages = images.len().div_ceil(9).max(1);
    let width = adapter.stage[0] as f32;
    let height = adapter.stage[1] as f32;
    let make_page = |background, elements| ImageMenu {
        builtin_navigation: true,
        story_exports: BTreeMap::new(),
        locals: BTreeMap::new(),
        background,
        elements,
        buttons: vec![],
        effects: None,
    };
    for page in 0..pages {
        let mut elements = vec![];
        for (cell, image) in images.iter().skip(page * 9).take(9).enumerate() {
            let index = page * 9 + cell;
            let key = format!("lm.cg.{image}");
            let viewer = format!("cg-view-{index}");
            let locked = adapter.locked_image(image);
            elements.push(menu_element(ImageButton {
                id: format!("cg-{index}"),
                label: format!("CG {}", index + 1),
                asset: image.clone(),
                hover_asset: None,
                locked_asset: Some(locked),
                requires: Some(key),
                rect: [
                    width * (0.04 + (cell % 3) as f32 * 0.32),
                    height * (0.04 + (cell / 3) as f32 * 0.27),
                    width * 0.28,
                    height * 0.23,
                ],
                action: ImageMenuAction::PushMenu {
                    menu: viewer.clone(),
                },
            }));
            adapter
                .menus
                .insert(viewer, make_page(image.clone(), vec![]));
        }
        for (id, label, destination, x) in [
            ("previous", "前", page.checked_sub(1), 0.08),
            ("next", "次", (page + 1 < pages).then_some(page + 1), 0.82),
        ] {
            if let Some(destination) = destination {
                elements.push(MenuElement {
                    id: id.into(),
                    parent: None,
                    rect: [width * x, height * 0.9, width * 0.1, height * 0.06],
                    scale: 1.,
                    opacity: 1.,
                    clip: None,
                    visible_when: vec![],
                    enabled_when: vec![],
                    text_local: None,
                    text_preference: None,
                    text_slot: None,
                    content: MenuContent::TextButton {
                        label: label.into(),
                        size: 24.,
                        color: [1.; 4],
                        hover_color: [1., 0.8, 0.3, 1.],
                        disabled_color: [0.5; 4],
                        requires: None,
                        action: ImageMenuAction::Menu {
                            menu: format!("cg-{destination}"),
                        },
                    },
                });
            }
        }
        adapter.menus.insert(
            format!("cg-{page}"),
            make_page(background.clone(), elements),
        );
    }
    adapter.warnings.insert("CG gallery uses a paginated NIR thumbnail grid and full-image pages; the source inventory order and per-image unlocks are preserved.".into());
    Ok(())
}
