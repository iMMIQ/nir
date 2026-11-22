use super::*;
const LIMIT: u64 = super::MEMORY_LEDGER_LIMIT;

fn title_music_program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("ui.menu-effects.v1".into());
    p.theme.image_menus.insert(
        "title".into(),
        serde_json::from_value(serde_json::json!({
            "background":"bg.station", "buttons":[], "effects":{"music":{"asset":"audio.bgm"}}
        }))
        .unwrap(),
    );
    p
}
#[test]
fn physical_surface_admission_tracks_extent_without_touching_audio_or_failed_state() {
    let mut player = Player::new(title_music_program(), "release".into(), "title".into()).unwrap();
    let original_cost = render_surface_cost(&player.validated.program().stage, 1280, 720);
    let before = player.memory_used();
    let snapshot = player.core.state().clone();
    let command_count = player.commands.len();
    player.reserve_render_surfaces(640, 360).unwrap();
    let small_cost = render_surface_cost(&player.validated.program().stage, 640, 360);
    assert_eq!(player.memory_used(), before - original_cost + small_cost);
    player.reserve_render_surfaces(3840, 2160).unwrap();
    let large_cost = render_surface_cost(&player.validated.program().stage, 3840, 2160);
    assert_eq!(player.memory_used(), before - original_cost + large_cost);
    let large_used = player.memory_used();
    assert_eq!(
        player
            .reserve_render_surfaces(16384, 16384)
            .unwrap_err()
            .code,
        "E_BUDGET"
    );
    assert_eq!(player.memory_used(), large_used);
    assert_eq!(
        player.reserve_render_surfaces(0, 720).unwrap_err().code,
        "E_VIEWPORT"
    );
    player.reserve_render_surfaces(1280, 720).unwrap();
    assert_eq!(player.memory_used(), before);
    assert_eq!(
        serde_json::to_value(player.core.state()).unwrap(),
        serde_json::to_value(snapshot).unwrap()
    );
    assert_eq!(player.commands.len(), command_count);
}

#[test]
fn boot_music_reserves_context_pcm_before_any_host_decode_without_changing_authored_data() {
    let p = title_music_program();
    let source = p.assets["audio.bgm"].clone();
    let native = Player::new(p.clone(), "release".into(), "title".into()).unwrap();
    let web = Player::from_validated(
        ValidatedProgram::new(p).unwrap(),
        "release".into(),
        "title".into(),
        None,
        Some(48_000),
    )
    .unwrap();
    assert_eq!(web.audio_decode_sample_rate(), Some(48_000));
    assert_eq!(native.audio_decode_sample_rate(), None);
    assert_eq!(
        web.memory_used(),
        native.memory_used() + source.resampled_pcm_budget(48_000) - source.decoded_bytes
    );
    assert_eq!(
        web.validated.program().assets["audio.bgm"].decoded_bytes,
        source.decoded_bytes
    );
    assert!(web.prepare.as_ref().unwrap().assets.contains("audio.bgm"));
}
#[test]
fn web_boot_rejects_unadmittable_resampled_music_before_fetch_while_native_contract_remains_valid()
{
    let mut p = title_music_program();
    let audio = p.assets.get_mut("audio.bgm").unwrap();
    audio.duration_us = Micros(600_000_000);
    audio.decoded_bytes = 600 * 24_000 * 4;
    assert!(Player::new(p.clone(), "release".into(), "title".into()).is_ok());
    let error = Player::from_validated(
        ValidatedProgram::new(p).unwrap(),
        "release".into(),
        "title".into(),
        None,
        Some(48_000),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "E_BUDGET");
}
#[test]
fn zero_decoder_sample_rate_is_not_an_unbounded_policy() {
    let p = title_music_program();
    let error = Player::from_validated(
        ValidatedProgram::new(p).unwrap(),
        "release".into(),
        "title".into(),
        None,
        Some(0),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "E_AUDIO_RATE");
}

#[test]
fn imported_japanese_story_settings_only_offer_configured_languages() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let text = program.locales.remove("en").unwrap();
    program.locales.clear();
    program.locales.insert("ja".into(), text);
    let plan = program.locale_config.text.remove("en").unwrap();
    program.locale_config.text.clear();
    program.locale_config.text.insert("ja".into(), plan);
    program.locale_config.ui.remove("zh-Hans");
    program.locale_config.default_ui = "en".into();
    program.locale_config.default_text = "ja".into();
    program.default_locale = "ja".into();
    let player = Player::new(program, "release".into(), "Test".into()).unwrap();
    let mut model = player.model();
    model.screen = Screen::Settings;
    assert_eq!(model.available_ui_locales, ["en"]);
    assert_eq!(model.available_text_locales, ["ja"]);
    let packet =
        nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default());
    let language_actions: Vec<_> = packet
        .semantics
        .iter()
        .filter_map(|node| match &node.action {
            UiAction::TextLocale { locale } => Some(locale.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(language_actions, ["ja"]);
}

#[test]
fn image_menu_scales_hover_and_enforces_replay_unlocks() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.functions.insert("replay".into(), serde_json::from_value(serde_json::json!({"entry":"start","blocks":{"start":{"ops":[],"terminator":{"type":"call","function":"main","next":"end"}},"end":{"ops":[],"terminator":{"type":"end","outcome":"replay"}}}})).unwrap());
    let button = nir_format::ImageButton {
        id: "replay-button".into(),
        label: "Replay".into(),
        asset: "bg.station".into(),
        hover_asset: Some("bg.river".into()),
        locked_asset: None,
        rect: [100., 100., 200., 60.],
        action: nir_format::ImageMenuAction::Entry {
            function: "replay".into(),
        },
        requires: Some("seen".into()),
    };
    program.theme.image_menus.insert(
        "title".into(),
        nir_format::ImageMenu {
            builtin_navigation: true,
            story_exports: BTreeMap::new(),
            locals: BTreeMap::new(),
            elements: vec![],
            background: "bg.station".into(),
            buttons: vec![button],
            effects: None,
        },
    );
    let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
    player.cancel_preparation();
    player.pauses.remove("prepare");
    let entry = UiAction::ImageMenuEntry {
        function: "replay".into(),
    };
    player.action(entry.clone(), 0, 1, &mut 100).unwrap();
    assert_eq!(player.screen, Screen::Title);
    let packet = nir_presentation::project(
        &player.model(),
        320.,
        240.,
        &nir_presentation::Messages::default(),
    );
    assert!(packet.hit(30., 60.).is_none());
    player.profile.insert("seen".into());
    player
        .action(
            UiAction::HoverImage {
                id: Some("replay-button".into()),
            },
            0,
            2,
            &mut 100,
        )
        .unwrap();
    let packet = nir_presentation::project(
        &player.model(),
        320.,
        240.,
        &nir_presentation::Messages::default(),
    );
    assert_eq!(packet.hit(30., 60.), Some(entry.clone()));
    assert!(packet
        .quads
        .iter()
        .any(|q| q.asset.as_deref() == Some("bg.river") && q.rect == [25., 55., 50., 15.]));
    assert!(player.title_assets().contains("bg.river"));
    player
        .action(
            UiAction::ImageMenuEntry {
                function: "not-configured".into(),
            },
            0,
            3,
            &mut 100,
        )
        .unwrap();
    assert_eq!(player.screen, Screen::Title);
    player.action(entry, 0, 4, &mut 100).unwrap();
    assert_eq!(player.screen, Screen::Story);
    assert_eq!(player.core.state().frames[0].function, "replay");
}

#[test]
fn story_releases_menu_images_but_keeps_dialogue_background() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.theme.image_menus.insert(
        "title".into(),
        nir_format::ImageMenu {
            builtin_navigation: true,
            story_exports: BTreeMap::new(),
            locals: BTreeMap::new(),
            elements: vec![],
            background: "bg.river".into(),
            buttons: vec![],
            effects: None,
        },
    );
    program.theme.dialogue.background = Some("bg.station".into());
    let player = at_intro_wait(program);
    let retained = player.retained_assets();
    assert!(retained.contains("bg.station"));
    assert!(!retained.contains("bg.river"));
    assert!(player.title_assets().contains("bg.river"));
}

#[test]
fn next_activation_releases_obsolete_active_reservations() {
    let mut player = player();
    player.cancel_preparation();
    player.pauses.retain(|_| false);
    player.screen = Screen::Story;
    player.return_screen = Screen::Story;
    player.active = Some(
        player
            .ledger
            .reserve(&BTreeMap::from([(
                "retired-audio".into(),
                LIMIT - player.ledger.used() - 1,
            )]))
            .unwrap(),
    );
    player
        .begin_prepare(Purpose::Activation, 7, BTreeSet::from(["bg.river".into()]))
        .unwrap();
    assert!(player.prepare.is_some());
    assert!(player.memory_used() < LIMIT / 2);
}

#[test]
fn admission_failure_pauses_and_retries_same_activation() {
    let mut player = player();
    player.cancel_preparation();
    player.pauses.retain(|_| false);
    player.commands.clear();
    player.screen = Screen::Story;
    player.return_screen = Screen::Story;
    let occupied = player
        .ledger
        .reserve(&BTreeMap::from([(
            "test-pressure".into(),
            LIMIT - player.ledger.used() - 1,
        )]))
        .unwrap();
    let error = player
        .action(UiAction::NewGame, 0, 0, &mut 100)
        .unwrap_err();
    let activation = player.core.state().pending.as_ref().unwrap().id;
    assert_eq!(error.code, "E_BUDGET");
    assert!(error
        .details
        .as_ref()
        .unwrap()
        .recovery
        .contains(&Recovery::Retry));
    player.report(error, true);
    assert!(player.paused());
    assert!(!player.needs_clock());
    assert!(player.failed_admission.is_some());
    assert!(player.prepare.is_none());
    drop(occupied);
    player.action(UiAction::Retry, 0, 0, &mut 100).unwrap();
    assert!(player.failed_admission.is_none());
    assert_eq!(player.prepare.as_ref().unwrap().job.activation, activation);
    assert!(player
        .commands
        .iter()
        .any(|c| matches!(c, AppCommand::GetAssets { .. })));
    player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
    assert!(player.failed_admission.is_none());
}

#[test]
fn image_menu_rejects_unknown_assets_and_entry_functions() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.theme.image_menus.insert(
        "title".into(),
        nir_format::ImageMenu {
            builtin_navigation: true,
            story_exports: BTreeMap::new(),
            locals: BTreeMap::new(),
            elements: vec![],
            background: "missing".into(),
            buttons: vec![],
            effects: None,
        },
    );
    assert!(Player::new(program, "release".into(), "Test".into()).is_err());
}

fn player() -> Player {
    let program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    Player::new(program, "release".into(), "Test".into()).unwrap()
}

#[test]
fn title_menus_project_only_the_retained_title_scene() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.title_scene = Some("station".into());
    let mut player = at_intro_wait(program);
    player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
    for action in [
        UiAction::Menu,
        UiAction::Settings,
        UiAction::Saves,
        UiAction::History,
    ] {
        player.action(action, 0, 0, &mut 100).unwrap();
        let model = player.model();
        assert_eq!(
            serde_json::to_value(&model.nodes).unwrap(),
            serde_json::to_value(player.title_nodes()).unwrap()
        );
        assert!(model.transition.is_none());
        assert!(model
            .nodes
            .iter()
            .filter_map(|n| n.asset.as_ref())
            .all(|id| player.retained_assets().contains(id)));
    }
}

fn speculative(player: &mut Player, request: u32, id: &str) {
    let assets = BTreeSet::from([id.to_owned()]);
    let job = PrepareJob::new(
        0,
        player.generation,
        player.costs(&assets).unwrap(),
        &player.ledger,
    )
    .unwrap();
    player.media_lookahead = Some(MediaLookahead {
        request,
        fingerprint: "wait".into(),
        assets,
        job,
    });
    player.request = request;
}

fn at_intro_wait(mut program: Program) -> Player {
    program.player.prefetch_media = true;
    let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
    player.cancel_preparation();
    player.pauses.remove("prepare");
    player.commands.clear();
    for _ in 0..20 {
        let input = player
            .core
            .state()
            .pending
            .as_ref()
            .map(|pending| CoreInput::Prepared {
                activation: pending.id,
            })
            .unwrap_or(CoreInput::None);
        player.core.step(input, 10_000);
        assert!(player.core.state().fault.is_none());
        if player.core.state().waiting.is_some() {
            break;
        }
    }
    assert_eq!(player.core.predict_next_cue().as_deref(), Some("enter"));
    player.screen = Screen::Story;
    player.return_screen = Screen::Story;
    player
}

#[test]
fn cancelled_speculation_keeps_ledger_charge_until_host_settles() {
    let mut player = player();
    let before = player.memory_used();
    let assets = BTreeSet::from(["speculative-only".to_owned()]);
    let job = PrepareJob::new(
        0,
        player.generation,
        BTreeMap::from([("speculative-only".into(), 4096)]),
        &player.ledger,
    )
    .unwrap();
    player.media_lookahead = Some(MediaLookahead {
        request: 91,
        fingerprint: "wait".into(),
        assets,
        job,
    });
    assert_eq!(player.memory_used(), before + 4096);
    player.cancel_media_lookahead();
    assert_eq!(player.memory_used(), before + 4096);
    player
        .event(AppEvent::AssetsCancelled { request: 91 }, &mut 100)
        .unwrap();
    assert_eq!(player.memory_used(), before);
}

#[test]
fn promoted_ready_media_is_not_requested_again() {
    let mut player = player();
    player.cancel_preparation();
    player.commands.clear();
    player.request = 41;
    let id = "bg.river".to_owned();
    let mut job = PrepareJob::new(
        0,
        player.generation,
        player.costs(&BTreeSet::from([id.clone()])).unwrap(),
        &player.ledger,
    )
    .unwrap();
    assert!(job.ready(&id, player.generation));
    player.media_lookahead = Some(MediaLookahead {
        request: 41,
        fingerprint: "wait".into(),
        assets: BTreeSet::from([id.clone()]),
        job,
    });
    player
        .begin_prepare(Purpose::Activation, 7, BTreeSet::from([id.clone()]))
        .unwrap();
    assert!(player
        .commands
        .iter()
        .any(|c| matches!(c, AppCommand::PromoteAssets { request: 41, .. })));
    assert!(player.commands.iter().all(|c| match c {
        AppCommand::GetAssets { assets, .. } => !assets.contains(&id),
        _ => true,
    }));
    assert!(!player.prepare.as_ref().unwrap().job.missing.contains(&id));
    assert!(player.retained_assets().contains(&id));
}

#[test]
fn promoted_inflight_media_ready_arrives_on_original_request() {
    let mut player = player();
    player.cancel_preparation();
    player.commands.clear();
    speculative(&mut player, 41, "bg.river");
    player
        .begin_prepare(Purpose::Activation, 7, BTreeSet::from(["bg.river".into()]))
        .unwrap();
    let required_request = player.prepare.as_ref().unwrap().request;
    assert_ne!(required_request, 41);
    assert_eq!(player.prepare.as_ref().unwrap().promoted_from, Some(41));
    assert!(player.accepts_resource(41));
    player
        .event(
            AppEvent::AssetReady {
                request: 41,
                asset: "bg.river".into(),
            },
            &mut 100,
        )
        .unwrap();
    assert!(!player
        .prepare
        .as_ref()
        .unwrap()
        .job
        .missing
        .contains("bg.river"));
    assert!(player.retained_assets().contains("bg.river"));
}

#[test]
fn required_prepare_waits_for_cancel_ack_under_budget_pressure() {
    let mut player = player();
    player.cancel_preparation();
    player.commands.clear();
    let mut required = player.title_assets();
    required.insert("bg.station".into());
    let required_cost: u64 = player.costs(&required).unwrap().values().sum();
    let fill = LIMIT - player.memory_used() - required_cost;
    let _fill = player
        .ledger
        .reserve(&BTreeMap::from([("@test-fill".into(), fill)]))
        .unwrap();
    speculative(&mut player, 41, "audio.bell");
    player
        .begin_prepare(Purpose::Activation, 7, required)
        .unwrap();
    assert!(player.prepare.is_none());
    assert!(player.deferred_prepare.is_some());
    assert!(player.media_retired.contains_key(&41));
    assert!(player
        .commands
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { request: 41 })));
    player.commands.clear();
    player
        .event(AppEvent::AssetsCancelled { request: 40 }, &mut 100)
        .unwrap();
    assert!(player.prepare.is_none(), "stale ack cannot release budget");
    player
        .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
        .unwrap();
    assert!(player.prepare.is_some());
    assert!(player.deferred_prepare.is_none());
    assert!(player.commands.iter().any(|c| matches!(
        c,
        AppCommand::GetAssets {
            priority: PreparePriority::Required,
            ..
        }
    )));
}

#[test]
fn title_supersedes_deferred_activation_before_ack() {
    let mut player = player();
    player.cancel_preparation();
    let mut required = player.title_assets();
    required.insert("bg.station".into());
    let cost: u64 = player.costs(&required).unwrap().values().sum();
    let _fill = player
        .ledger
        .reserve(&BTreeMap::from([(
            "@test-fill".into(),
            LIMIT - player.memory_used() - cost,
        )]))
        .unwrap();
    speculative(&mut player, 41, "audio.bell");
    player
        .begin_prepare(Purpose::Activation, 7, required)
        .unwrap();
    assert!(player.deferred_prepare.is_some());
    let old_session = player.generation.session;
    player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
    assert!(player.generation.session > old_session);
    assert!(!player
        .deferred_prepare
        .as_ref()
        .is_some_and(
            |(_, purpose, activation, _)| matches!(purpose, Purpose::Activation)
                && *activation == 7
        ));
    player
        .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
        .unwrap();
    assert!(!player
        .prepare
        .as_ref()
        .is_some_and(|p| matches!(p.purpose, Purpose::Activation)));
}

#[test]
fn locale_font_admission_retries_after_speculative_cancel() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let mut font = program.assets["font.reader"].clone();
    font.object = "alt-font".into();
    program.assets.insert("font.alt".into(), font);
    let media: BTreeMap<_, _> = program
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    for plan in [
        program.locale_config.ui.get_mut("en").unwrap(),
        program.locale_config.text.get_mut("en").unwrap(),
    ] {
        plan.fonts = vec!["font.alt".into()];
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
    }
    let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
    player.cancel_preparation();
    speculative(&mut player, 41, "bg.river");
    let font_cost = player.costs(&BTreeSet::from(["font.alt".into()])).unwrap()["font.alt"];
    let _fill = player
        .ledger
        .reserve(&BTreeMap::from([(
            "@test-fill".into(),
            LIMIT - player.memory_used() - font_cost + 1,
        )]))
        .unwrap();
    player.preferences.ui_locale = "en".into();
    player.preferences.text_locale = "en".into();
    player.start_locale_switch().unwrap();
    assert!(player.deferred_locale.is_some());
    assert!(player.media_retired.contains_key(&41));
    assert!(player.locale_error.is_none());
    player
        .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
        .unwrap();
    assert!(player.deferred_locale.is_none());
    assert!(player.locale_job.is_some());
    assert!(player.locale_error.is_none());
}

#[test]
fn oversized_next_cue_is_skipped_once_for_its_wait() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let actor = program.assets.get_mut("actor.aki").unwrap();
    actor.width = 2048;
    actor.height = 2048;
    let mut player = at_intro_wait(program);
    let before = player.request;
    player.maybe_prefetch_media();
    let fingerprint = player.media_attempted.clone();
    assert!(fingerprint.is_some());
    assert!(player.media_lookahead.is_none());
    assert_eq!(player.request, before);
    player.pauses.insert("hidden".into());
    player.maybe_prefetch_media();
    player.pauses.remove("hidden");
    player.maybe_prefetch_media();
    assert_eq!(player.media_attempted, fingerprint);
    assert_eq!(player.request, before);
}

#[test]
fn failed_next_cue_is_not_retried_at_the_same_wait() {
    let program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let mut player = at_intro_wait(program);
    player.maybe_prefetch_media();
    let request = player.media_lookahead.as_ref().unwrap().request;
    assert!(player.commands.iter().any(|c| matches!(c, AppCommand::GetAssets { request: r, priority: PreparePriority::Near, .. } if *r == request)));
    player
        .event(
            AppEvent::AssetFailed {
                request,
                message: "decode failed".into(),
            },
            &mut 100,
        )
        .unwrap();
    assert!(player.media_lookahead.is_none());
    assert!(player.media_retired.contains_key(&request));
    player
        .event(AppEvent::AssetsCancelled { request }, &mut 100)
        .unwrap();
    let before = player.request;
    player.commands.clear();
    player.maybe_prefetch_media();
    assert_eq!(player.request, before);
    assert!(!player.commands.iter().any(|c| matches!(
        c,
        AppCommand::GetAssets {
            priority: PreparePriority::Near,
            ..
        }
    )));
}

#[test]
fn device_loss_retires_inflight_media_and_discards_late_ready() {
    let program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let mut player = at_intro_wait(program);
    let baseline = player.memory_used();
    player.maybe_prefetch_media();
    let request = player.media_lookahead.as_ref().unwrap().request;
    assert!(player.memory_used() > baseline);
    player.pump(vec![AppEvent::DeviceLost], 1000);
    assert!(player.media_lookahead.is_none());
    assert!(player.media_retired.contains_key(&request));
    assert!(player.memory_used() > baseline);
    player.pump(
        vec![AppEvent::AssetReady {
            request,
            asset: "actor.aki".into(),
        }],
        1000,
    );
    player.pump(
        vec![AppEvent::AssetFault {
            request,
            diagnostic: Box::new(Diagnostic::new("E_DECODE", "media", "late decode")),
        }],
        1000,
    );
    assert!(player.error.is_none());
    assert!(player.media_retired.contains_key(&request));
    player.pump(vec![AppEvent::AssetsCancelled { request }], 1000);
    assert_eq!(player.memory_used(), baseline);
}
