use super::*;
fn model() -> UiModel {
    UiModel {
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
            full_text: "A long line of readable words. ".repeat(30),
            visible_text: "A long line of readable words. ".repeat(30),
            speaker: "Aya".into(),
            ready: true,
            gate: false,
            locale: "en".into(),
            font_plan_digest: "fixture".into(),
            font_assets: vec!["font.reader".into()],
            emphasis: vec![],
        }),
        hidden_dialogue: Default::default(),
        window_transition: Default::default(),
        menu_transition: Default::default(),
        menu_element_animations: Default::default(),
        interface_hidden: Default::default(),
        dialogue_appearance: Default::default(),
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
