use nir_format::*;
use nir_player::*;
use std::collections::BTreeMap;

fn bundled() -> (Program, BTreeMap<String, Vec<u8>>) {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.entry = "boot".into();
    p.functions.insert("boot".into(), serde_json::from_value(serde_json::json!({
        "entry":"start", "blocks":{
            "start":{"ops":[{"id":"boot.once","operation":{"type":"assign","target":"affection","value":{"type":"binary","op":"add","left":{"type":"var","name":"affection"},"right":{"type":"const","value":{"type":"i32","value":1}}}}}],
                "terminator":{"type":"call","function":"main","args":{},"next":"end"}},
            "end":{"ops":[],"terminator":{"type":"end","outcome":"done"}}
        }
    })).unwrap());
    let mut objects = BTreeMap::new();
    for (module, function) in [("boot", "boot"), ("story", "main")] {
        let f = p.functions[function].clone();
        let code = serde_json::to_vec(&ModuleCode {
            format: 1,
            module: module.into(),
            functions: BTreeMap::from([(function.into(), f.clone())]),
        })
        .unwrap();
        let hash = nir_content::digest(&code);
        objects.insert(hash.clone(), code);
        let mut index = ModuleIndex {
            functions: BTreeMap::from([(function.into(), FunctionSignature::from(&f))]),
            code: hash,
            ..Default::default()
        };
        if module == "story" {
            index.texts = p.texts.keys().cloned().collect();
            for (locale, texts) in &p.locales {
                let bytes = serde_json::to_vec(&ModuleTexts {
                    format: 1,
                    module: module.into(),
                    locale: locale.clone(),
                    texts: texts.clone(),
                })
                .unwrap();
                let hash = nir_content::digest(&bytes);
                objects.insert(hash.clone(), bytes);
                index.locales.insert(locale.clone(), hash);
            }
        }
        p.modules.insert(module.into(), index);
    }
    p.functions.clear();
    for texts in p.locales.values_mut() {
        texts.clear();
    }
    (p, objects)
}
fn action(p: &mut Player, action: UiAction) -> Vec<AppCommand> {
    p.pump(
        vec![AppEvent::Action {
            action,
            interaction: p.current_interaction(),
            sequence: p.core().state().last_input + 1,
            session: p.generation.session,
        }],
        1000,
    )
}
fn assets(p: &mut Player, commands: Vec<AppCommand>) -> Vec<AppCommand> {
    let mut q = commands;
    let mut rest = vec![];
    for _ in 0..30 {
        if q.is_empty() {
            return rest;
        }
        let mut next = vec![];
        for c in q {
            match c {
                AppCommand::GetAssets {
                    request, assets, ..
                } => {
                    for asset in assets {
                        next.extend(p.pump(vec![AppEvent::AssetReady { request, asset }], 1000));
                    }
                }
                AppCommand::PreparePresentation { request } => {
                    next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000))
                }
                AppCommand::PrepareLocale { request, .. } => {
                    next.extend(p.pump(vec![AppEvent::LocaleReady { request }], 1000))
                }
                c => rest.push(c),
            }
        }
        q = next;
    }
    panic!("preparation did not converge")
}
fn request(commands: &[AppCommand]) -> (u32, Vec<ContentRequest>) {
    commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((*request, objects.clone())),
            _ => None,
        })
        .expect("content request")
}
fn deliver(
    p: &mut Player,
    r: u32,
    needs: &[ContentRequest],
    objects: &BTreeMap<String, Vec<u8>>,
) -> Vec<AppCommand> {
    p.pump(
        vec![AppEvent::ContentReady {
            request: r,
            objects: needs.iter().map(|o| objects[&o.hash].clone()).collect(),
        }],
        1000,
    )
}
fn boot() -> (Player, BTreeMap<String, Vec<u8>>) {
    let (p, objects) = bundled();
    let mut p = Player::new(p, "release".into(), "Modules".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    assert!(!assets(&mut p, commands)
        .iter()
        .any(|c| matches!(c, AppCommand::GetContent { .. })));
    (p, objects)
}
#[test]
fn content_barrier_freezes_call_and_does_not_repeat_shared_assignment() {
    let (mut p, objects) = boot();
    let c = action(&mut p, UiAction::NewGame);
    let (r, needs) = request(&c);
    assert!(needs.iter().all(|n| n.module == "boot"));
    assert_eq!(p.core().state().variables["affection"], Value::I32(0));
    let c = deliver(&mut p, r, &needs, &objects);
    let (r, needs) = request(&c);
    assert!(needs.iter().all(|n| n.module == "story"));
    assert!(needs.iter().all(|n| n.locale.as_deref() != Some("en")));
    assert_eq!(p.core().state().variables["affection"], Value::I32(1));
    let tick = p.core().state().tick_us;
    p.pump(vec![AppEvent::Tick { delta_us: 100000 }], 1000);
    assert_eq!(p.core().state().tick_us, tick);
    let c = deliver(&mut p, r, &needs, &objects);
    assets(&mut p, c);
    assert_eq!(p.core().state().variables["affection"], Value::I32(1));
    assert!(p.core().program().locales["en"].is_empty());
    assert!(p.core().state().fault.is_none());
}
#[test]
fn failed_content_is_retryable_and_late_completion_cannot_enter_new_session() {
    let (mut p, objects) = boot();
    let c = action(&mut p, UiAction::NewGame);
    let (r, needs) = request(&c);
    p.pump(
        vec![AppEvent::ContentFailed {
            request: r,
            message: "offline".into(),
        }],
        1000,
    );
    assert!(p.error.is_some());
    deliver(&mut p, r, &needs, &objects);
    assert!(p.core().program().functions.is_empty());
    let c = action(&mut p, UiAction::Retry);
    let (new, needs) = request(&c);
    assert_ne!(r, new);
    let c = action(&mut p, UiAction::Title);
    assets(&mut p, c);
    deliver(&mut p, new, &needs, &objects);
    assert!(p.core().program().functions.is_empty());
    assert_eq!(p.core().state().variables["affection"], Value::I32(0));
}
#[test]
fn corrupt_batch_is_atomic_and_does_not_install_partial_code() {
    let (mut p, objects) = boot();
    let c = action(&mut p, UiAction::NewGame);
    let (r, needs) = request(&c);
    let c = deliver(&mut p, r, &needs, &objects);
    let (r, needs) = request(&c);
    let mut bytes: Vec<_> = needs.iter().map(|n| objects[&n.hash].clone()).collect();
    bytes.last_mut().unwrap().push(b' ');
    p.pump(
        vec![AppEvent::ContentReady {
            request: r,
            objects: bytes,
        }],
        1000,
    );
    assert!(p.error.is_some());
    assert!(!p.core().program().functions.contains_key("main"));
    assert!(p.core().program().locales["zh-Hans"].is_empty());
}

fn playing() -> (Player, BTreeMap<String, Vec<u8>>) {
    let (mut p, objects) = boot();
    let c = action(&mut p, UiAction::NewGame);
    let (r, needs) = request(&c);
    let c = deliver(&mut p, r, &needs, &objects);
    let (r, needs) = request(&c);
    let c = deliver(&mut p, r, &needs, &objects);
    assets(&mut p, c);
    assert!(p.core().dialogue().is_some());
    (p, objects)
}
#[test]
fn fresh_restore_prepares_stack_and_frozen_text_before_replacing_session() {
    let (source, _) = playing();
    let snapshot = source.core().snapshot();
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let (mut target, objects) = boot();
    let commands = target.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let (r, needs) = request(&commands);
    assert!(needs
        .iter()
        .any(|n| n.module == "story" && n.locale.as_deref() == Some("zh-Hans")));
    assert!(needs.iter().all(|n| n.locale.as_deref() != Some("en")));
    assert_eq!(target.core().state().variables["affection"], Value::I32(0));
    let commands = deliver(&mut target, r, &needs, &objects);
    assets(&mut target, commands);
    assert_eq!(target.core().state().variables["affection"], Value::I32(1));
    assert_eq!(
        target.core().dialogue().unwrap().1.text_id,
        source.core().dialogue().unwrap().1.text_id
    );
    assert!(target.paused());
}
#[test]
fn locale_bundle_failure_preserves_effective_locale_and_current_dialogue() {
    let (mut p, objects) = playing();
    let text = p.core().dialogue().unwrap().1.full_text();
    let commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let (r, needs) = request(&commands);
    assert_eq!(needs.len(), 1);
    assert_eq!(needs[0].locale.as_deref(), Some("en"));
    p.pump(
        vec![AppEvent::ContentFailed {
            request: r,
            message: "offline".into(),
        }],
        1000,
    );
    assert_eq!(p.effective_text_locale, "zh-Hans");
    assert_eq!(p.core().dialogue().unwrap().1.full_text(), text);
    let commands = action(&mut p, UiAction::LocaleRetry);
    let (r, needs) = request(&commands);
    let commands = deliver(&mut p, r, &needs, &objects);
    assets(&mut p, commands);
    assert_eq!(p.effective_text_locale, "en");
    assert_eq!(p.core().dialogue().unwrap().1.locale, "zh-Hans");
}

#[test]
fn superseded_remote_locale_cannot_install_or_change_effective_selection() {
    let (mut p, objects) = playing();
    let commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let (r, needs) = request(&commands);
    action(
        &mut p,
        UiAction::TextLocale {
            locale: "zh-Hans".into(),
        },
    );
    deliver(&mut p, r, &needs, &objects);
    assert_eq!(p.effective_text_locale, "zh-Hans");
    assert!(p.core().program().locales["en"].is_empty());
    assert!(!p.locale_pending());
}

#[test]
fn direct_core_time_input_stops_at_unloaded_content_without_duplicate_requests() {
    let (program, _) = bundled();
    let mut core = nir_core::Core::new(
        nir_core::ValidatedProgram::new(program).unwrap(),
        "release".into(),
        "zh-Hans".into(),
    )
    .unwrap();
    let output = core.step(
        nir_core::CoreInput::Time {
            delta_us: 1_000_000,
        },
        1000,
    );
    assert_eq!(core.state().tick_us.0, 0);
    assert!(output.waiting);
    assert_eq!(
        output
            .intents
            .iter()
            .filter(|i| matches!(i, nir_core::CoreIntent::PrepareContent { .. }))
            .count(),
        1
    );
}
