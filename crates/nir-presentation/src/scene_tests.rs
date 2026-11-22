use super::*;

fn text_packet(texts: impl IntoIterator<Item = String>) -> DrawPacket {
    let mut packet = DrawPacket {
        locale: "zh-Hans".into(),
        font_assets: vec!["font.reader".into()],
        font_plan_digest: "plan-a".into(),
        ..Default::default()
    };
    for text in texts {
        packet.text(text, 0., 0., 300., 18., [1.; 4]);
    }
    packet
}

fn reader_text_engine() -> TextEngine {
    let mut text = TextEngine::default();
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    )
    .unwrap();
    text
}

fn visual_packet() -> DrawPacket {
    let mut packet = DrawPacket {
        locale: "en".into(),
        font_assets: vec!["font.ui".into()],
        font_plan_digest: "ui-plan-a".into(),
        width: 640.,
        height: 360.,
        stage_size: [1280, 720],
        transition_layers: Some((
            vec![Quad {
                corners: None,
                rect: [1., 2., 30., 40.],
                color: [0.1, 0.2, 0.3, 1.],
                asset: Some("old-bg".into()),
                clip: None,
            }],
            vec![Quad {
                corners: None,
                rect: [4., 5., 60., 70.],
                color: [0.4, 0.5, 0.6, 1.],
                asset: Some("new-bg".into()),
                clip: Some([0., 0., 640., 360.]),
            }],
            0.25,
        )),
        ..Default::default()
    };
    packet.rect([0., 0., 640., 360.], [0.01, 0.02, 0.03, 1.]);
    packet.text("hello", 10., 20., 300., 18., [1.; 4]);
    packet
}

#[test]
fn appended_controls_do_not_reuse_retained_semantic_ids() {
    let mut p = DrawPacket::default();
    for _ in 0..6 {
        p.button(
            "control".into(),
            UiAction::Menu,
            [0., 0., 30., 30.],
            false,
            &Theme::default(),
        );
    }
    p.semantics.retain(|n| n.id == 4 || n.id == 5);
    p.button(
        "close".into(),
        UiAction::Close,
        [0., 0., 30., 30.],
        false,
        &Theme::default(),
    );
    let ids: std::collections::BTreeSet<_> = p.semantics.iter().map(|n| n.id).collect();
    assert_eq!(ids.len(), p.semantics.len());
    assert_eq!(p.semantics.last().unwrap().id, 6);
}
#[test]
fn shadow_is_paint_only_and_preserves_reading_geometry() {
    let mut p = DrawPacket::default();
    p.text("a\nb", 10., 20., 100., 18., [1., 1., 1., 0.5]);
    let r = &mut p.texts[0];
    r.shadow = Some(TextShadow {
        offset: [2., 3.],
        color: [0., 0., 0., 0.8],
    });
    r.emphasis = vec![(0, 1)];
    r.visible = Some(1);
    r.scroll = 7.;
    r.region = Some(ScrollRegion::Dialogue);
    let shadow = r.shadow_run().unwrap();
    assert_eq!((shadow.x, shadow.y), (12., 23.));
    assert_eq!(shadow.width, r.width);
    assert_eq!(shadow.emphasis, r.emphasis);
    assert_eq!(
        TextEngine::line_offsets(&shadow),
        TextEngine::line_offsets(r)
    );
    assert_eq!(shadow.visible, r.visible);
    assert_eq!(shadow.scroll, 7.);
    assert_eq!(shadow.color[3], 0.4);
    assert_eq!(shadow.region, None);
    assert!(shadow.shadow.is_none());
    assert!(shadow.monochrome);
    assert_ne!(TextEngine::key(&shadow), TextEngine::key(r));
    assert!(shadow
        .clip
        .unwrap()
        .iter()
        .zip([10., 20., 100., r.height])
        .all(|(a, b)| (*a - b).abs() < 0.00001));
}
#[test]
fn imported_line_height_fits_four_lines_and_has_its_own_shape_key() {
    let mut text = reader_text_engine();
    let mut packet = text_packet(["A\nB\nC\nD".to_owned()]);
    packet.texts[0].size = 32.;
    packet.texts[0].line_height = 40.;
    packet.texts[0].height = 175.;
    text.layout(&packet);
    let key = TextEngine::key(&packet.texts[0]);
    let lines: Vec<_> = text.buffers[&key].layout_runs().collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines.last().unwrap().line_top + lines.last().unwrap().line_height,
        160.
    );
    packet.texts[0].line_height = 48.;
    assert_ne!(key, TextEngine::key(&packet.texts[0]));
    text.layout(&packet);
    assert_eq!(text.cache_stats().entries, 2);
}

#[test]
fn shadow_has_identical_glyph_geometry_without_emphasis_colors() {
    let mut engine = reader_text_engine();
    let mut packet = text_packet(["rain after\nhello world".to_owned()]);
    let run = &mut packet.texts[0];
    run.emphasis = vec![(0, 4)];
    run.shadow = Some(TextShadow {
        offset: [2., 2.],
        color: [0., 0., 0., 1.],
    });
    let shadow = run.shadow_run().unwrap();
    let key = TextEngine::key(run);
    let shadow_key = TextEngine::key(&shadow);
    packet.texts.push(shadow);
    engine.layout(&packet);
    let geometry = |key: &str| {
        engine.buffers[key]
            .layout_runs()
            .flat_map(|r| {
                r.glyphs
                    .iter()
                    .map(|g| (g.start, g.end, g.x, g.y, g.w, g.glyph_id))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(geometry(&key), geometry(&shadow_key));
    assert!(engine.buffers[&shadow_key]
        .layout_runs()
        .all(|r| r.glyphs.iter().all(|g| g.color_opt.is_none())));
}

#[test]
fn text_cache_lru_keeps_a_frequently_used_entry_hot() {
    let mut text = reader_text_engine();
    let hot = text_packet(["hot entry".to_owned()]);
    let hot_key = TextEngine::key(&hot.texts[0]);
    text.layout(&hot);

    for index in 0..200 {
        text.layout(&hot);
        let cold = text_packet([format!("cold entry {index}")]);
        text.layout(&cold);
    }

    let stats = text.cache_stats();
    assert!(text.buffers.contains_key(&hot_key));
    assert_eq!(stats.entries, 128);
    assert_eq!(stats.hits, 200);
    assert_eq!(stats.misses, 201);
    assert_eq!(stats.evictions, 73);
}

#[test]
fn text_cache_protects_the_full_current_packet_then_converges() {
    let mut text = reader_text_engine();
    let packet = text_packet((0..140).map(|index| format!("packet entry {index}")));
    let keys: Vec<_> = packet.texts.iter().map(TextEngine::key).collect();
    text.layout(&packet);

    assert_eq!(text.cache_stats().entries, 140);
    assert_eq!(text.cache_stats().evictions, 0);
    assert!(keys.iter().all(|key| text.buffers.contains_key(key)));

    let mut next_packet = DrawPacket::default();
    next_packet.texts.push(packet.texts[139].clone());
    text.layout(&next_packet);

    let stats = text.cache_stats();
    assert_eq!(stats.entries, 128);
    assert_eq!(stats.evictions, 12);
    assert!(text.buffers.contains_key(&keys[139]));
    assert!(!text.buffers.contains_key(&keys[0]));
}

#[test]
fn font_load_invalidates_buffers_but_keeps_cumulative_stats() {
    let mut text = reader_text_engine();
    let packet = text_packet(["font invalidation".to_owned()]);
    text.layout(&packet);
    assert_eq!(text.cache_stats().misses, 1);
    assert_eq!(text.cache_stats().entries, 1);

    text.add_font_asset(
        "font.abe",
        include_bytes!("../../../examples/rain-letters/assets/fonts/ABeeZee-Regular.ttf").to_vec(),
    )
    .unwrap();
    assert_eq!(text.cache_stats().entries, 0);

    text.layout(&packet);
    let stats = text.cache_stats();
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.evictions, 0);
    assert_eq!(stats.entries, 1);
}

#[test]
fn visual_equality_ignores_semantic_only_changes() {
    let packet = visual_packet();
    let mut semantically_changed = visual_packet();
    semantically_changed.announcement = "accessible description".into();
    semantically_changed.announcement_locale = "zh-Hans".into();
    semantically_changed.semantics.push(SemanticNode {
        value: None,
        id: 7,
        label: "continue".into(),
        action: UiAction::Advance,
        enabled: false,
        rect: [1., 2., 3., 4.],
        locale: "zh-Hans".into(),
    });
    semantically_changed.scrolls.push(ScrollView {
        menu: None,
        region: ScrollRegion::Dialogue,
        rect: [0., 0., 10., 10.],
        offset: 4.,
        max: 12.,
        step: 8.,
    });
    semantically_changed.dialogue_hint = Some(0);

    assert!(packet.visual_eq(&semantically_changed));
}

#[test]
fn visual_equality_detects_geometry_fonts_text_stage_and_transition_changes() {
    let packet = visual_packet();

    let mut changed = visual_packet();
    changed.quads[0].color[0] += 0.1;
    assert!(!packet.visual_eq(&changed));

    let mut changed = visual_packet();
    changed.texts[0].text.push('!');
    assert!(!packet.visual_eq(&changed));

    let mut changed = visual_packet();
    changed.texts[0].size += 1.;
    assert!(!packet.visual_eq(&changed));

    let mut changed = visual_packet();
    changed.font_plan_digest.push_str("-b");
    assert!(!packet.visual_eq(&changed));

    let mut changed = visual_packet();
    changed.stage_size[0] += 1;
    assert!(!packet.visual_eq(&changed));

    let mut changed = visual_packet();
    changed.transition_layers.as_mut().unwrap().2 = 0.5;
    assert!(!packet.visual_eq(&changed));
}

#[test]
fn missing_explicit_font_reports_error_before_shaping() {
    let mut text = TextEngine::default();
    let mut packet = DrawPacket {
        locale: "zh-Hans".into(),
        font_assets: vec!["font.reader".into()],
        font_plan_digest: "plan-a".into(),
        ..Default::default()
    };
    packet.text("雨后", 0., 0., 300., 18., [1.; 4]);
    text.layout(&packet);
    assert!(text
        .missing_font
        .as_deref()
        .unwrap()
        .contains("font.reader"));
    assert!(text.buffers.is_empty());
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    )
    .unwrap();
    text.layout(&packet);
    assert!(text.missing_font.is_none());
    assert_eq!(text.shapes, 1);
}

#[test]
fn text_shape_cache_is_scoped_to_locale_and_font_plan() {
    let mut text = TextEngine::default();
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    )
    .unwrap();
    let mut packet = DrawPacket {
        width: 320.,
        height: 180.,
        locale: "zh-Hans".into(),
        font_assets: vec!["font.reader".into()],
        font_plan_digest: "plan-a".into(),
        ..Default::default()
    };
    packet.text("雨后", 0., 0., 300., 18., [1.; 4]);
    text.layout(&packet);
    assert_eq!(text.shapes, 1);
    packet.texts[0].font_plan_digest = "plan-b".into();
    text.layout(&packet);
    assert_eq!(text.shapes, 2);
    packet.texts[0].locale = "en".into();
    text.layout(&packet);
    assert_eq!(text.shapes, 3);
}

fn node(id: &str, parent: Option<&str>, order: i32) -> Node {
    Node {
        id: id.into(),
        parent: parent.map(str::to_owned),
        asset: Some(id.into()),
        x: 0.,
        y: 0.,
        width: 10.,
        height: 10.,
        scale: 1.,
        opacity: 1.,
        color: [1.; 4],
        order,
        clip: None,
        timeline_binding: None,
        inherit_existence: false,
        sprite_transform: None,
        bitmap_text: None,
        preserve_pose: vec![],
        offset: [0.; 2],
    }
}
#[test]
fn leaf_sprite_rotation_composes_with_ancestor_pose_and_fixed_parent_clip() {
    let mut group = node("group", None, 0);
    group.width = 0.;
    group.height = 0.;
    group.x = 10.;
    group.y = 20.;
    group.scale = 2.;
    group.clip = Some([0., 0., 50., 50.]);
    let mut child = node("sprite", Some("group"), 0);
    child.width = 32.;
    child.height = 24.;
    child.x = 3.;
    child.y = 4.;
    child.offset = [5., -2.];
    child.sprite_transform = Some(nir_format::SpriteTransform {
        origin: [24., 0.],
        basis_x: [0., 1.],
        basis_y: [-1., 0.],
    });
    let packet = DrawPacket {
        width: 1280.,
        height: 720.,
        ..Default::default()
    };
    let quads = scene_layout(&packet, &[group, child], [1280., 720.], 1.);
    assert_eq!(quads.len(), 1);
    assert_eq!(
        quads[0].1.corners,
        Some([[74., 24.], [74., 88.], [26., 24.], [26., 88.]])
    );
    assert_eq!(quads[0].1.clip, Some([10., 20., 100., 100.]));
    let changed = DrawPacket {
        width: 1280.,
        height: 720.,
        quads: quads.iter().map(|(_, q)| q.clone()).collect(),
        ..Default::default()
    };
    let mut other = DrawPacket {
        width: 1280.,
        height: 720.,
        quads: changed.quads.clone(),
        ..Default::default()
    };
    other.quads[0].corners.as_mut().unwrap()[0][0] += 1.;
    assert!(!changed.visual_eq(&other));
}
#[test]
fn group_order_and_nested_clip_are_composed() {
    let mut group = node("group", None, 0);
    group.width = 0.;
    group.x = 10.;
    group.scale = 2.;
    group.clip = Some([0., 0., 8., 8.]);
    let mut child = node("child", Some("group"), 99);
    child.x = 3.;
    child.clip = Some([0., 0., 10., 10.]);
    let front = node("front", None, 1);
    let mut p = DrawPacket {
        width: 100.,
        height: 100.,
        ..Default::default()
    };
    scene(&mut p, &[group, front, child], [100., 100.], 1.);
    assert_eq!(p.quads[0].asset.as_deref(), Some("child"));
    assert_eq!(p.quads[1].asset.as_deref(), Some("front"));
    assert_eq!(p.quads[0].rect, [16., 0., 20., 20.]);
    assert_eq!(p.quads[0].clip, Some([16., 0., 10., 16.]));
}
