use nir_format::*;
use nir_player::Player;
use nir_presentation::{project, MenuPaint, Messages};
fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("ui.menu-elements.v1".into());
    p.theme.image_menus.insert("title".into(), serde_json::from_value(serde_json::json!({
        "background":"bg.station","buttons":[],"elements":[
          {"id":"group","rect":[100,100,0,0],"scale":2,"opacity":0.5,"clip":[0,0,80,60],"content":{"type":"group"}},
          {"id":"text","parent":"group","rect":[10,10,100,40],"content":{"type":"text","text":"Test","size":20,"color":[1,0,0,1]}},
          {"id":"cover","parent":"group","rect":[0,0,100,60],"content":{"type":"image","asset":"bg.river"}},
          {"id":"hit","parent":"group","rect":[10,10,100,40],"content":{"type":"hit_region","label":"Locked","action":{"type":"entry","function":"main"},"requires":"seen"}}
        ]
    })).unwrap());
    p
}
#[test]
fn composition_shares_transform_clip_opacity_and_preserves_paint_order() {
    let player = Player::new(program(), "release".into(), "Test".into()).unwrap();
    let mut model = player.model();
    model.stage = [1280., 720.];
    let p = project(&model, 1280., 720., &Messages::default());
    assert_eq!(p.menu_paint.len(), 2);
    let MenuPaint::Text(t) = p.menu_paint[0] else {
        panic!("text must paint first")
    };
    let MenuPaint::Quad(q) = p.menu_paint[1] else {
        panic!("image must cover earlier text")
    };
    assert_eq!(p.texts[t].x, 120.);
    assert_eq!(p.texts[t].y, 120.);
    assert_eq!(p.texts[t].size, 40.);
    assert_eq!(p.texts[t].color[3], 0.5);
    assert_eq!(p.quads[q].rect, [100., 100., 200., 120.]);
    assert_eq!(p.texts[t].clip, Some([100., 100., 160., 120.]));
    let hit = p.semantics.iter().find(|n| n.label == "Locked").unwrap();
    assert_eq!(hit.rect, [120., 120., 140., 80.]);
    assert!(!hit.enabled);
    assert_eq!(
        p.menu_controls.get(&hit.id).map(String::as_str),
        Some("hit")
    );
    assert!(p.hit(130., 130.).is_none());
    model.profile.insert("seen".into());
    let p = project(&model, 640., 360., &Messages::default());
    assert!(matches!(
        p.hit(65., 65.),
        Some(UiAction::MenuControl { .. })
    ));
    assert!(p.hit(140., 65.).is_none());
}
#[test]
fn invalid_hierarchies_capabilities_assets_and_entry_signatures_are_rejected() {
    let original = program();
    let mut p = original.clone();
    p.requires.retain(|v| v != "ui.menu-elements.v1");
    assert!(Player::new(p, "r".into(), "t".into()).is_err());
    for kind in [
        "cycle",
        "missing_parent",
        "non_group_parent",
        "duplicate",
        "missing_asset",
        "missing_entry",
    ] {
        let mut p = original.clone();
        let menu = p.theme.image_menus.get_mut("title").unwrap();
        match kind {
            "cycle" => menu.elements[0].parent = Some("group".into()),
            "missing_parent" => menu.elements[1].parent = Some("absent".into()),
            "non_group_parent" => menu.elements[1].parent = Some("cover".into()),
            "duplicate" => menu.elements[1].id = "cover".into(),
            "missing_asset" => {
                menu.elements[2].content = MenuContent::Image {
                    asset: "absent".into(),
                }
            }
            _ => {
                menu.elements[3].content = MenuContent::HitRegion {
                    label: "bad".into(),
                    action: ImageMenuAction::Entry {
                        function: "absent".into(),
                    },
                    requires: None,
                }
            }
        }
        assert!(Player::new(p, "r".into(), "t".into()).is_err(), "{kind}");
    }
}
#[test]
fn bounded_text_batches_and_asset_variants_are_validated() {
    let mut p = program();
    let menu = p.theme.image_menus.get_mut("title").unwrap();
    let text = menu.elements[1].clone();
    for i in 0..64 {
        let mut t = text.clone();
        t.id = format!("text{i}");
        menu.elements.push(t);
    }
    assert!(menu.validate_elements().is_err());
    let mut p = program();
    let menu = p.theme.image_menus.get_mut("title").unwrap();
    menu.elements[2].content = MenuContent::Button {
        label: "button".into(),
        asset: "normal".into(),
        hover_asset: Some("hover".into()),
        locked_asset: Some("locked".into()),
        action: ImageMenuAction::NewGame,
        requires: None,
    };
    for id in ["normal", "hover", "locked"] {
        assert!(p.theme.image_assets().contains(id));
    }
}

fn stack_program() -> Program {
    let mut p = program();
    p.requires.extend(
        [
            "ui.menu-stack.v1",
            "ui.menu-state.v1",
            "ui.menu-reading.v1",
            "ui.menu-services.v1",
        ]
        .map(str::to_owned),
    );
    p.theme.image_menus.insert("title".into(), serde_json::from_value(serde_json::json!({
        "background":"bg.station","buttons":[],
        "elements":[
          {"id":"list","rect":[100,100,200,200],"scale":2,"clip":[0,0,100,100],"content":{"type":"stack","gap":5}},
          {"id":"first","parent":"list","rect":[0,0,100,20],"scale":1.5,"visible_when":[{"type":"reading_available","mode":"auto","available":true}],"content":{"type":"group"}},
          {"id":"first.hit","parent":"first","rect":[0,0,100,20],"content":{"type":"hit_region","label":"Auto row","action":{"type":"close"}}},
          {"id":"second","parent":"list","rect":[0,0,100,20],"enabled_when":[{"type":"profile","key":"unlock","present":true}],"content":{"type":"hit_region","label":"Second","action":{"type":"close"}}},
          {"id":"third","parent":"list","rect":[0,0,100,20],"content":{"type":"hit_region","label":"Third","action":{"type":"close"}}}
        ]
    })).unwrap());
    p
}
#[test]
fn stack_reflows_visible_rows_with_shared_transform_clip_and_stable_control_ids() {
    let player = Player::new(stack_program(), "r".into(), "t".into()).unwrap();
    let mut m = player.model();
    m.stage = [1280., 720.];
    let first = project(&m, 1280., 720., &Messages::default());
    assert!(!first.semantics.iter().any(|s| s.label == "Auto row"));
    let second = first
        .semantics
        .iter()
        .find(|s| s.label == "Second")
        .unwrap();
    assert_eq!(second.rect, [100., 100., 200., 40.]);
    assert!(!second.enabled);
    let third = first.semantics.iter().find(|s| s.label == "Third").unwrap();
    assert_eq!(third.rect, [100., 150., 200., 40.]);
    let third_id = third.id;
    m.menu_reading_modes.insert(MenuReadingMode::Auto);
    let next = project(&m, 1280., 720., &Messages::default());
    assert_eq!(
        next.semantics
            .iter()
            .find(|s| s.label == "Auto row")
            .unwrap()
            .rect,
        [100., 100., 200., 60.]
    );
    assert_eq!(
        next.semantics
            .iter()
            .find(|s| s.label == "Second")
            .unwrap()
            .rect,
        [100., 170., 200., 40.]
    );
    let third = next.semantics.iter().find(|s| s.label == "Third").unwrap();
    assert_eq!(third.id, third_id);
    assert_eq!(third.rect, [100., 220., 200., 40.]);
    assert!(
        matches!(next.hit(120.,230.),Some(UiAction::MenuControl { control, .. }) if control == "third")
    );
    assert!(next.hit(120., 190.).is_none()); // disabled rows retain their place and consume hits
    let scaled = project(&m, 640., 360., &Messages::default());
    assert_eq!(
        scaled
            .semantics
            .iter()
            .find(|s| s.label == "Third")
            .unwrap()
            .rect,
        [50., 110., 100., 20.]
    );
    assert_eq!(
        serde_json::to_value(player.model().menu_locals).unwrap(),
        serde_json::to_value(m.menu_locals).unwrap()
    );
}
#[test]
fn stack_rejects_ambiguous_or_unbounded_row_geometry_and_missing_capabilities() {
    let original = stack_program();
    for cap in [
        "ui.menu-stack.v1",
        "ui.menu-reading.v1",
        "ui.menu-services.v1",
        "ui.menu-state.v1",
    ] {
        let mut p = original.clone();
        p.requires.retain(|c| c != cap);
        assert!(Player::new(p, "r".into(), "t".into()).is_err(), "{cap}");
    }
    for case in 0..5 {
        let mut p = original.clone();
        let menu = p.theme.image_menus.get_mut("title").unwrap();
        match case {
            0 => menu.elements[0].content = MenuContent::Stack { gap: f32::NAN },
            1 => menu.elements[0].content = MenuContent::Stack { gap: -1. },
            2 => menu.elements[1].rect[1] = 1.,
            3 => menu.elements[1].rect[3] = 0.,
            _ => {
                menu.elements[1].rect[3] = 8192.;
                menu.elements[1].scale = 2.;
            }
        }
        assert!(menu.validate_elements().is_err(), "{case}");
    }
}

#[test]
fn text_button_uses_one_identity_for_label_paint_hover_guard_and_hit() {
    let mut p = program();
    p.requires.push("ui.menu-text-button.v1".into());
    let menu = p.theme.image_menus.get_mut("title").unwrap();
    menu.elements.truncate(1);
    menu.elements.push(serde_json::from_value(serde_json::json!({
        "id":"text.button","parent":"group","rect":[10,10,100,40],
        "content":{"type":"text_button","label":"Action","size":20,"color":[1,0,0,1],"hover_color":[0,1,0,1],"disabled_color":[0,0,1,0.4],"action":{"type":"settings"},"requires":"unlocked"}
    })).unwrap());
    let player = Player::new(p.clone(), "r".into(), "t".into()).unwrap();
    let mut m = player.model();
    m.stage = [1280., 720.];
    m.hovered_image = Some("text.button".into());
    let locked = project(&m, 1280., 720., &Messages::default());
    let text = locked.texts.iter().find(|t| t.text == "Action").unwrap();
    assert_eq!(text.color, [0., 0., 1., 0.2]);
    assert_eq!(text.size, 40.);
    assert_eq!((text.x, text.y), (120., 120.));
    let control = locked
        .semantics
        .iter()
        .find(|s| s.label == "Action")
        .unwrap();
    assert!(!control.enabled);
    assert!(locked.hit(130., 130.).is_none());
    assert_eq!(control.rect, [120., 120., 140., 80.]);
    m.profile.insert("unlocked".into());
    let hover = project(&m, 1280., 720., &Messages::default());
    assert_eq!(
        hover
            .texts
            .iter()
            .find(|t| t.text == "Action")
            .unwrap()
            .color,
        [0., 1., 0., 0.5]
    );
    assert!(
        matches!(hover.hit(130.,130.),Some(UiAction::MenuControl {control,..}) if control=="text.button")
    );
    m.hovered_image = None;
    let plain = project(&m, 1280., 720., &Messages::default());
    assert_eq!(
        plain
            .texts
            .iter()
            .find(|t| t.text == "Action")
            .unwrap()
            .color,
        [1., 0., 0., 0.5]
    );
    p.requires.retain(|c| c != "ui.menu-text-button.v1");
    assert!(Player::new(p, "r".into(), "t".into()).is_err());
}
