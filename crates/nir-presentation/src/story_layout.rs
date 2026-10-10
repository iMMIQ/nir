use super::*;
fn model() -> UiModel {
    UiModel {
        advance_wait: false,
        transition_style: StageTransition::Dissolve,
        image_menu: Default::default(),
        authored_menu: Default::default(),
        menu_instance: Default::default(),
        menu_revision: Default::default(),
        menu_depth: Default::default(),
        menu_navigation_pending: Default::default(),
        menu_locals: Default::default(),
        hovered_image: Default::default(),
        profile: Default::default(),
        title: Default::default(),
        screen: Screen::Story,
        nodes: Default::default(),
        transition: Default::default(),
        stage: [1280., 720.],
        dialogue: Some(DialogueView {
            images: vec![],
            full_text: "A long line of readable words. ".repeat(30),
            visible_text: "A long line of readable words. ".repeat(30),
            speaker: "Aya".into(),
            ready: true,
            gate: false,
            locale: "en".into(),
            font_plan_digest: "fixture".into(),
            font_assets: vec!["font.reader".into()],
            emphasis: vec![],
            ruby: vec![],
        }),
        hidden_dialogue: Default::default(),
        window_transition: Default::default(),
        menu_transition: Default::default(),
        menu_element_animations: Default::default(),
        interface_hidden: Default::default(),
        menu_disabled: false,
        dialogue_appearance: Default::default(),
        dialogue_decorations: Default::default(),
        choices: Default::default(),
        choice_cancellable: Default::default(),
        prefs: Default::default(),
        ui_locale: "en".into(),
        ui_fonts: vec!["font.reader".into()],
        ui_font_plan_digest: "fixture".into(),
        text_locale: "en".into(),
        available_ui_locales: Default::default(),
        available_text_locales: Default::default(),
        text_fonts: vec!["font.reader".into()],
        text_font_plan_digest: "fixture".into(),
        locale_pending: Default::default(),
        locale_error: Default::default(),
        preflight_texts: Default::default(),
        theme: Default::default(),
        history: Default::default(),
        history_total: Default::default(),
        history_voice: Default::default(),
        character_voices: Default::default(),
        menu_history: Default::default(),
        menu_history_flow: Default::default(),
        menu_opacity: 1.,
        slots: Default::default(),
        save_confirmation: Default::default(),
        busy_slots: Default::default(),
        can_save: Default::default(),
        replay_active: Default::default(),
        menu_story: Default::default(),
        menu_reading_modes: Default::default(),
        paused: Default::default(),
        can_continue: Default::default(),
        loading: Default::default(),
        status: Default::default(),
        fault: Default::default(),
        retrying: Default::default(),
        fault_recovery: Default::default(),
        auto: Default::default(),
        skip: Default::default(),
        outcome: Default::default(),
        history_offset: Default::default(),
    }
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] - 0.01
        && b[0] < a[0] + a[2] - 0.01
        && a[1] < b[1] + b[3] - 0.01
        && b[1] < a[1] + a[3] - 0.01
}
fn toolbar(action: &UiAction) -> bool {
    matches!(
        action,
        UiAction::Menu
            | UiAction::History
            | UiAction::ToggleAuto
            | UiAction::ToggleSkip
            | UiAction::ToggleInterface
    )
}

fn reader_font() -> TextEngine {
    let mut text = TextEngine::default();
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    )
    .unwrap();
    text
}

#[test]
fn portrait_panel_fits_the_whole_sentence_and_does_not_jump_during_reveal() {
    let mut m = model();
    let d = m.dialogue.as_mut().unwrap();
    d.full_text = "A quiet evening.".into();
    d.visible_text.clear();
    let mut reading = ReadingState::default();
    let mut text = reader_font();
    let messages = Messages::default();
    let first = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
    let rect = first.builtin_dialogue.unwrap();
    assert!(rect[3] < 220. && rect[3] >= 140., "{rect:?}");
    let stage = first.stage_viewport.unwrap();
    assert!(stage[1] >= 112. && stage[1] + stage[3] < rect[1]);
    let shapes = text.shapes;
    for prefix in ["A", "A quiet", "A quiet evening."] {
        m.dialogue.as_mut().unwrap().visible_text = prefix.into();
        let p = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
        assert_eq!(p.builtin_dialogue, Some(rect));
        assert_eq!(p.stage_viewport, Some(stage));
        assert_eq!(
            text.shapes, shapes,
            "reveal must reuse full-sentence shapes"
        );
    }
}

#[test]
fn adaptive_reader_preserves_long_text_overflow_and_fixed_author_geometry() {
    let mut m = model();
    m.prefs.font_scale = 1.5;
    let mut text = reader_font();
    let mut reading = ReadingState::default();
    let messages = Messages::default();
    let p = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
    assert_eq!(p.builtin_dialogue.unwrap()[3], 330.);
    assert!(p
        .scrolls
        .iter()
        .any(|s| s.region == ScrollRegion::Dialogue && s.max > 0.));
    m.theme.dialogue.rect = Some([160., 450., 960., 200.]);
    m.theme.dialogue.text_rect = Some([200., 490., 880., 120.]);
    let fixed = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
    assert!(fixed.builtin_dialogue.is_none());
    assert!(fixed.stage_viewport.is_none());
    let body = fixed
        .texts
        .iter()
        .find(|r| r.region == Some(ScrollRegion::Dialogue))
        .unwrap();
    let scale = 390. / 1280.;
    assert_eq!(body.width, 880. * scale);
    assert!((body.y - ((844. - 720. * scale) / 2. + 490. * scale)).abs() < 0.01);
}

#[test]
fn input_hints_switch_without_changing_dialogue_geometry() {
    let mut m = model();
    m.dialogue.as_mut().unwrap().full_text = "A quiet evening.".into();
    let mut reading = ReadingState::default();
    let mut text = reader_font();
    let messages = Messages::default();
    let keyboard = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
    assert!(keyboard.texts.iter().any(|r| r.text == "SPACE / ↗"));
    assert!(reading.set_touch_input(true));
    assert!(!reading.set_touch_input(true));
    let touch = reading.project(&m, (1, 1), 390., 844., &messages, &mut text);
    assert!(touch.texts.iter().any(|r| r.text == "Tap to continue"));
    assert!(!touch.texts.iter().any(|r| r.text.contains("SPACE")));
    assert_eq!(touch.builtin_dialogue, keyboard.builtin_dialogue);
}
#[test]
fn dialogue_images_keep_stage_geometry_and_share_visibility_without_background_alpha() {
    use nir_format::{DialogueDecoration, DialogueDecorationSlot};
    let mut m = model();
    m.theme.dialogue.rect = Some([160., 450., 960., 200.]);
    m.theme.dialogue.text_rect = Some([200., 490., 880., 120.]);
    m.dialogue_appearance.background_opacity = 0.2;
    m.dialogue_decorations.insert(
        DialogueDecorationSlot::Portrait,
        DialogueDecoration {
            asset: "portrait".into(),
            rect: [10., 350., 80., 100.],
        },
    );
    let packet = project(&m, 640., 480., &Messages::default());
    let image = packet
        .quads
        .iter()
        .find(|q| q.asset.as_deref() == Some("portrait"))
        .unwrap();
    assert_eq!(image.rect, [5., 235., 40., 50.]);
    assert_eq!(image.color[3], 1.);
    m.prefs.font_scale = 1.4;
    let enlarged = project(&m, 640., 480., &Messages::default());
    assert_eq!(
        enlarged
            .quads
            .iter()
            .find(|q| q.asset.as_deref() == Some("portrait"))
            .unwrap()
            .rect,
        image.rect
    );
    m.hidden_dialogue = true;
    let dialogue = m.dialogue.take(); // Player omits hidden live text from UiModel.
    let hidden = project(&m, 640., 480., &Messages::default());
    assert!(!hidden
        .quads
        .iter()
        .any(|q| q.asset.as_deref() == Some("portrait")));
    m.hidden_dialogue = false;
    m.dialogue = dialogue;
    m.window_transition = Some(WindowTransition {
        style: StageTransition::Dissolve,
        to_visible: false,
        progress: 0.5,
    });
    let fading = project(&m, 640., 480., &Messages::default());
    assert_eq!(
        fading
            .quads
            .iter()
            .find(|q| q.asset.as_deref() == Some("portrait"))
            .unwrap()
            .color[3],
        0.5
    );
}
#[test]
fn inline_images_reserve_exact_width_reveal_and_use_the_text_clip() {
    let mut m = model();
    let d = m.dialogue.as_mut().unwrap();
    d.speaker.clear();
    d.full_text = "AA\u{fffc}BB".into();
    d.visible_text = "AA".into();
    d.images = vec![InlineImagePlacement {
        offset: 2,
        image: InlineImage {
            asset: "icon".into(),
            width: 21,
            height: 19,
            align: InlineImageAlign::Center,
            margins: [3, 5, 2, 4],
        },
    }];
    let mut engine = TextEngine::default();
    engine
        .add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
    let mut reading = ReadingState::default();
    let before = reading.project(&m, (1, 1), 1280., 720., &Messages::default(), &mut engine);
    assert!(!before
        .quads
        .iter()
        .any(|q| q.asset.as_deref() == Some("icon")));
    m.dialogue.as_mut().unwrap().visible_text = "AA\u{fffc}".into();
    let shown = reading.project(&m, (1, 1), 1280., 720., &Messages::default(), &mut engine);
    let text = shown
        .texts
        .iter()
        .find(|r| r.region == Some(ScrollRegion::Dialogue))
        .unwrap();
    let line = engine.buffers[&TextEngine::key(text)]
        .layout_runs()
        .next()
        .unwrap();
    let glyph = line.glyphs.iter().find(|g| g.start == 2).unwrap();
    assert!((glyph.w - 29.).abs() < 0.01, "image slot width {}", glyph.w);
    let image = shown
        .quads
        .iter()
        .find(|q| q.asset.as_deref() == Some("icon"))
        .unwrap();
    assert_eq!(&image.rect[2..], &[21., 19.]);
    assert!((image.rect[0] - (text.x + glyph.x + 3.)).abs() < 0.01);
    assert_eq!(image.clip, Some([text.x, text.y, text.width, text.height]));
    let after = line.glyphs.iter().find(|g| g.start == 5).unwrap();
    assert!(after.x >= glyph.x + 29. - 0.01);
}
#[test]
fn cancellable_image_choices_offer_touch_cancel_and_guard_right_click() {
    let mut m = model();
    m.choice_cancellable = true;
    m.choices = vec![ChoiceView {
        id: "option".into(),
        label: "Option".into(),
        enabled: true,
        selected: false,
        locale: "en".into(),
        font_plan_digest: "fixture".into(),
        font_assets: vec![],
        image: Some(ChoiceImage {
            asset: "picture".into(),
            hover_asset: None,
            disabled_asset: None,
            rect: [100., 100., 180., 120.],
        }),
    }];
    let p = project(&m, 1280., 720., &Messages::default());
    let cancel = p
        .semantics
        .iter()
        .find(|n| n.action == UiAction::CancelChoice)
        .unwrap();
    assert_eq!(
        pointer_action(&p, &m, cancel.rect[0] + 10., cancel.rect[1] + 10., 0),
        Some(UiAction::CancelChoice)
    );
    assert_eq!(
        pointer_action(&p, &m, 20., 20., 2),
        Some(UiAction::CancelChoice)
    );
    m.loading = true;
    assert_ne!(
        pointer_action(&p, &m, 20., 20., 2),
        Some(UiAction::CancelChoice)
    );
    m.loading = false;
    m.paused = true;
    assert_ne!(
        pointer_action(&p, &m, 20., 20., 2),
        Some(UiAction::CancelChoice)
    );
    m.paused = false;
    m.choice_cancellable = false;
    let p = project(&m, 1280., 720., &Messages::default());
    assert!(!p
        .semantics
        .iter()
        .any(|n| n.action == UiAction::CancelChoice));
    assert_eq!(pointer_action(&p, &m, 20., 20., 2), Some(UiAction::Menu));
}

#[test]
fn toolbar_controls_have_touch_targets_in_wide_landscape() {
    let m = model();
    let messages = Messages::default();
    for (width, height) in [(844., 260.), (1024., 768.)] {
        let p = project(&m, width, height, &messages);
        let controls: Vec<_> = p.semantics.iter().filter(|n| toolbar(&n.action)).collect();
        assert_eq!(controls.len(), 5);
        for n in controls {
            assert!(
                n.rect[2] >= 44. && n.rect[3] >= 44.,
                "{:?}: {:?}",
                n.action,
                n.rect
            );
        }
    }
}
#[test]
fn compact_story_keeps_speaker_body_and_scroll_controls_separate() {
    let messages = Messages::default();
    for (width, height) in [(240., 240.), (320., 240.), (844., 240.), (844., 260.)] {
        for font_scale in [0.8, 1., 1.5] {
            for slot in [DialogueComponent::Bottom, DialogueComponent::Top] {
                let mut m = model();
                m.prefs.font_scale = font_scale;
                m.theme.slots.dialogue = slot;
                let mut text = TextEngine::default();
                text.add_font_asset(
                    "font.reader",
                    include_bytes!(
                        "../../../examples/rain-letters/assets/fonts/ABeeZee-Regular.ttf"
                    )
                    .to_vec(),
                )
                .unwrap();
                let p = ReadingState::default().project(
                    &m,
                    (1, 1),
                    width,
                    height,
                    &messages,
                    &mut text,
                );
                let body = p
                    .texts
                    .iter()
                    .find(|r| r.region == Some(ScrollRegion::Dialogue))
                    .unwrap();
                assert!(
                    body.height + 0.01 >= body.line_height,
                    "{width}x{height}, scale {font_scale}: body height {} < line {}",
                    body.height,
                    body.line_height
                );
                let name = p.texts.iter().find(|r| r.text == "Aya").unwrap();
                let key = TextEngine::key(name);
                let line = text.buffers[&key].layout_runs().next().unwrap();
                let name_rect = [
                    name.x,
                    name.y + line.line_top,
                    line.line_w,
                    line.line_height,
                ];
                let body_rect = [body.x, body.y, body.width, body.height];
                let scroll: Vec<_> = p
                    .semantics
                    .iter()
                    .filter(|n| {
                        matches!(
                            n.action,
                            UiAction::Scroll {
                                region: ScrollRegion::Dialogue,
                                ..
                            }
                        )
                    })
                    .collect();
                assert_eq!(scroll.len(), 2);
                for n in p
                    .semantics
                    .iter()
                    .filter(|n| toolbar(&n.action))
                    .chain(scroll.iter().copied())
                {
                    let [x, y, w, h] = n.rect;
                    assert!(
                        w >= 44.
                            && h >= 44.
                            && x >= 0.
                            && y >= 0.
                            && x + w <= width + 0.01
                            && y + h <= height + 0.01,
                        "{width}x{height}: {:?} {:?}",
                        n.action,
                        n.rect
                    );
                    assert!(
                        !overlaps(n.rect, name_rect),
                        "{width}x{height}, scale {font_scale}: {:?} {:?} covers speaker {:?}",
                        n.action,
                        n.rect,
                        name_rect
                    );
                    assert!(
                        !overlaps(n.rect, body_rect),
                        "{width}x{height}, scale {font_scale}: {:?} {:?} covers body {:?}",
                        n.action,
                        n.rect,
                        body_rect
                    );
                }
                assert!(!overlaps(scroll[0].rect, scroll[1].rect));
                assert!(text.buffers[&TextEngine::key(body)]
                    .layout_runs()
                    .any(|line| !line.glyphs.is_empty()
                        && line.line_top - body.scroll >= -0.01
                        && line.line_top - body.scroll + line.line_height <= body.height + 0.01));
                eprintln!("LAYOUT {width}x{height} scale={font_scale} speaker={name_rect:?} body={body_rect:?} scroll={:?}",scroll.iter().map(|n|n.rect).collect::<Vec<_>>());
            }
        }
    }
}

#[test]
fn compact_toolbar_labels_fit_and_keep_full_accessible_names() {
    let m = model();
    let messages = Messages::default();
    let p = project(&m, 240., 240., &messages);
    let mut text = TextEngine::default();
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/fonts/ABeeZee-Regular.ttf").to_vec(),
    )
    .unwrap();
    text.layout(&p);
    for n in p.semantics.iter().filter(|n| toolbar(&n.action)) {
        if n.action == UiAction::Menu {
            assert_eq!(n.label, "Menu");
        }
        if n.action == UiAction::History {
            assert_eq!(n.label, "History");
        }
        let r = p
            .texts
            .iter()
            .find(|r| {
                r.region.is_none()
                    && r.x >= n.rect[0]
                    && r.x < n.rect[0] + n.rect[2]
                    && r.y >= n.rect[1]
                    && r.y < n.rect[1] + n.rect[3]
            })
            .unwrap();
        let lines: Vec<_> = text.buffers[&TextEngine::key(r)].layout_runs().collect();
        assert_eq!(
            lines.len(),
            1,
            "{:?}: label {:?} wraps in {:?}",
            n.action,
            r.text,
            n.rect
        );
        assert!(lines[0].line_w <= r.width + 0.01);
    }
}
