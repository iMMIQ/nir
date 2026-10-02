use super::lsb::tests::{command, dialogue, literal, script, string, u32b};
use super::*;

pub(super) fn options(source: &Path, out: &Path) -> ImportOptions {
    ImportOptions {
        source: source.into(),
        out: out.into(),
        entry: Some("main.lsb".into()),
        line: 0,
        draft: false,
        // The neutral fixtures exercise both import paths; the approximate
        // LiveNovel rules are accepted here while dedicated tests assert the
        // gate itself.
        accept_approximate: APPROXIMATE_RULES.iter().map(|s| (*s).into()).collect(),
        game_id: "org.nir.test.import".into(),
        title: "Migration fixture".into(),
        locale: "ja".into(),
    }
}
/// Every rule the fixtures lower at approximate level; keep in sync with the
/// importers' mapping ledgers.
pub(super) const APPROXIMATE_RULES: &[&str] = &["livenovel.menu-hover", "livenovel.text.font"];
pub(super) fn sdk() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn exit(line: u32) -> Vec<u8> {
    let mut args = vec![];
    literal(&mut args, 1);
    command(6, line, &args)
}
fn jump(page: &str, target: u32, condition: u8) -> Vec<u8> {
    let mut args = vec![];
    string(&mut args, page);
    u32b(&mut args, target);
    literal(&mut args, condition);
    command(4, 5, &args)
}

#[test]
fn japanese_project_checks_and_runs_through_cross_page_call_and_jump() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let mut args = vec![];
    string(&mut args, "sub.lsb");
    u32b(&mut args, 90);
    string(&mut args, "");
    literal(&mut args, 1);
    u32b(&mut args, 0);
    // An unreachable unsupported command must not block import. A target LineNo
    // differs from its array index. The call returns to its original caller.
    fs::write(
        source.join("main.lsb"),
        script(&[
            command(5, 10, &args),
            jump("last.lsb", 0, 1),
            command(46, 30, &[]),
        ]),
    )
    .unwrap();
    let mut label = vec![];
    string(&mut label, "entry");
    fs::write(
        source.join("sub.lsb"),
        script(&[
            exit(20),
            command(3, 90, &label),
            command(20, 100, &dialogue("こんにちは。")),
            exit(110),
        ]),
    )
    .unwrap();
    fs::write(
        source.join("last.lsb"),
        script(&[command(20, 10, &dialogue("終わり。")), exit(20)]),
    )
    .unwrap();
    let out = temp.path().join("project");
    let report = convert(&options(&source, &out), &sdk()).unwrap();
    assert_eq!(report.errors, 0);
    assert_eq!(report.text_pages, 2);
    assert!(report.written);
    assert_eq!(report.format, 2);
    assert_eq!(report.status, "converted_with_adaptations");
    assert_eq!(report.approximate, 0);
    let by_rule = |rule: &str| {
        report
            .mappings
            .iter()
            .find(|m| m.rule == rule)
            .unwrap_or_else(|| panic!("missing mapping {rule}"))
    };
    assert_eq!(by_rule("lsb.control-flow").level, "exact");
    assert_eq!(by_rule("lsb.text").level, "adapted");
    assert!(by_rule("lsb.text").approximation.is_some());
    assert!(crate::text_status(&out).unwrap().ready);
    let p = crate::load_project(&out).unwrap();
    assert_eq!(p.program.default_locale, "ja");
    crate::compile(&p.program).unwrap();
    let mut manifest = p.manifest;
    manifest.inputs.scenarios.push("scenario.toml".into());
    fs::write(
        out.join("game.toml"),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(out.join("scenario.toml"), "format=1\nid='imported'\nentry='main'\ntext_locale='ja'\nsteps=[]\n[expect]\noutcome='completed'\n").unwrap();
    assert_eq!(crate::test_project(&out).unwrap(), vec!["imported"]);
    assert!(convert(&options(&source, &out), &sdk()).is_err());
}

#[test]
fn strict_refuses_and_draft_faults_at_unsupported_commands() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("main.lsb"), script(&[command(46, 12, &[])])).unwrap();
    let out = temp.path().join("project");
    let mut opts = options(&source, &out);
    let report = convert(&opts, &sdk()).unwrap();
    assert_eq!(report.errors, 1);
    assert!(!report.written);
    assert!(!out.exists());
    opts.draft = true;
    let report = convert(&opts, &sdk()).unwrap();
    assert!(report.written);
    assert_eq!(report.status, "blocked");
    assert!(report
        .mappings
        .iter()
        .any(|m| m.rule == "lsb.unsupported-commands" && m.level == "unsupported"));
    assert_eq!(report.diagnostics[0].line, 12);
    assert!(out.join("MIGRATION-INCOMPLETE.txt").exists());
    let p = crate::load_project(&out).unwrap();
    assert!(p.program.functions.values().flat_map(|f| f.blocks.values()).any(|b| matches!(&b.terminator, nir_format::Terminator::Fault { code, .. } if code == "E_IMPORT_UNSUPPORTED")));
}

#[test]
fn inventory_does_not_include_dialogue_and_counts_repeated_lines() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("story.lsb"),
        script(&[command(20, 1, &dialogue("秘密")), exit(1)]),
    )
    .unwrap();
    let report = inspect(temp.path()).unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(report.scripts[0].commands["TextIns"], 1);
    assert!(!serde_json::to_string(&report).unwrap().contains("秘密"));
}

#[test]
fn rejects_unsafe_paths_and_truncated_sources_without_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("main.lsb"),
        script(&[jump("../outside.lsb", 0, 1)]),
    )
    .unwrap();
    let out = temp.path().join("project");
    let report = convert(&options(&source, &out), &sdk()).unwrap();
    assert_eq!(report.errors, 1);
    assert!(!out.exists());
    assert!(report.diagnostics[0].message.contains("E_IMPORT_PATH"));
    assert!(convert(&options(&source, &source.join("output")), &sdk()).is_err());
    fs::write(source.join("main.lsb"), [116, 0]).unwrap();
    assert!(convert(&options(&source, &out), &sdk()).is_err());
    assert!(!out.exists());
}

#[test]
fn false_jump_does_not_resolve_missing_target_and_missing_label_is_not_guessed() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("main.lsb"),
        script(&[jump("absent.lsb", 1, 0), exit(80)]),
    )
    .unwrap();
    let mut lower = lower::Lowering::new(Source::new(temp.path()).unwrap());
    lower.run("main.lsb", 0).unwrap();
    assert_eq!(lower.report().errors, 0);
    assert!(lower.run("main.lsb", 80).is_err());
}

#[test]
fn japanese_text_does_not_enable_unsupported_japanese_ui() {
    let config: crate::LocaleManifest = toml::from_str(
        "format=1\ndefault_ui='en'\ndefault_text='ja'\n[ui]\nen=['font']\n[text]\nja=['font']",
    )
    .unwrap();
    assert!(config.resolve().is_ok());
    let config: crate::LocaleManifest = toml::from_str(
        "format=1\ndefault_ui='ja'\ndefault_text='ja'\n[ui]\nja=['font']\n[text]\nja=['font']",
    )
    .unwrap();
    assert!(config.resolve().is_err());
}

#[test]
#[ignore = "requires NIR_IMPORT_SOURCE and NIR_IMPORT_OUT"]
fn real_livenovel_conversion_and_all_routes() {
    use nir_core::{Core, CoreInput, CoreIntent, ValidatedProgram};
    let source = PathBuf::from(std::env::var_os("NIR_IMPORT_SOURCE").expect("NIR_IMPORT_SOURCE"));
    let out = PathBuf::from(std::env::var_os("NIR_IMPORT_OUT").expect("NIR_IMPORT_OUT"));
    if !out.exists() {
        let mut opts = options(&source, &out);
        opts.entry = None;
        let report = convert(&opts, &sdk()).unwrap();
        assert_eq!(report.errors, 0);
        assert!(report.written);
        eprintln!(
            "converted {} pages, {} functions",
            report.text_pages, report.functions
        );
    }
    let loaded = crate::load_project(&out).unwrap();
    let defaults = lsb::project_settings(&read_binary(&source.join("live.lpb")).unwrap()).unwrap();
    let lsb::Literal::Int(wait) = defaults["StatusAutoTextWait"] else {
        panic!()
    };
    assert_eq!(
        loaded.program.player.auto_delay_policy,
        nir_format::AutoDelayPolicy::Fixed
    );
    assert_eq!(loaded.program.player.auto_delay_us.0, wait as u64 * 1000);
    for (name, value) in [
        ("StatusBGMVolume", loaded.program.player.bgm_volume),
        ("StatusVoiceVolume", loaded.program.player.voice_volume),
        ("StatusSEVolume", loaded.program.player.sfx_volume),
    ] {
        let lsb::Literal::Int(expected) = defaults[name] else {
            panic!()
        };
        assert!((value - expected as f32 / 1000.).abs() < 0.0001);
    }
    let policies: Vec<_> = loaded
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
        .filter_map(|op| {
            if let nir_format::Operation::DialogueVoice { wait, .. } = op.operation {
                Some(wait)
            } else {
                None
            }
        })
        .collect();
    assert!(!policies.is_empty());
    assert!(policies
        .iter()
        .all(|p| *p == nir_format::VoiceWaitPolicy::SampledRemaining));
    let validated = ValidatedProgram::new(loaded.program.clone()).unwrap();
    let entries: Vec<_> = std::iter::once(loaded.program.entry.clone())
        .chain(loaded.program.theme.image_menus.values().flat_map(|menu| {
            menu.controls().filter_map(|(_, action, _)| match action {
                nir_format::ImageMenuAction::Entry { function } => Some(function.clone()),
                _ => None,
            })
        }))
        .collect();
    let expected_unlocks: std::collections::BTreeSet<_> = loaded
        .program
        .theme
        .image_menus
        .values()
        .flat_map(|menu| {
            menu.controls()
                .filter_map(|(_, _, requires)| requires.map(str::to_owned))
        })
        .filter(|key| key.starts_with("lm.replay."))
        .collect();
    assert!(
        entries.len() > 1,
        "source corpus must exercise replay routes"
    );
    assert!(
        !expected_unlocks.is_empty(),
        "source corpus must exercise unlock guards"
    );
    for entry in entries {
        let mut core =
            Core::new_at(validated.clone(), "corpus".into(), "ja".into(), &entry).unwrap();
        let mut restored = false;
        let mut unlocks = BTreeMap::new();
        for sequence in 1..100_000 {
            let input = if let Some(p) = &core.state().pending {
                CoreInput::Prepared { activation: p.id }
            } else if core.dialogue().is_some() {
                if !restored {
                    core = Core::restore(validated.clone(), core.snapshot(), "corpus").unwrap();
                    restored = true;
                }
                let d = core.dialogue().unwrap().1;
                if d.at_gate {
                    CoreInput::Time {
                        delta_us: 1_000_000,
                    }
                } else {
                    CoreInput::Advance {
                        interaction: d.interaction,
                        sequence,
                    }
                }
            } else {
                CoreInput::Time {
                    delta_us: 1_000_000,
                }
            };
            let step = core.step(input, 10_000);
            for intent in step.intents {
                if let CoreIntent::ProfileMerge { key } = intent {
                    *unlocks.entry(key).or_insert(0) += 1;
                }
            }
            assert!(
                core.state().fault.is_none(),
                "{entry}: {:?}",
                core.state().fault
            );
            if core.state().outcome.is_some() {
                break;
            }
        }
        assert!(core.state().outcome.is_some(), "{entry} did not finish");
        let replay_unlocks = unlocks
            .keys()
            .filter(|key| key.starts_with("lm.replay."))
            .count();
        if entry == loaded.program.entry {
            let actual_unlocks: std::collections::BTreeSet<_> = unlocks
                .keys()
                .filter(|key| key.starts_with("lm.replay."))
                .cloned()
                .collect();
            assert_eq!(actual_unlocks, expected_unlocks);
        }
        eprintln!("{entry}: completed, snapshot restored, {replay_unlocks} replay unlocks");
    }
}

#[test]
fn mapping_levels_carry_evidence_and_gate_approximate_acceptance() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    for (sites, fade_sites, menu_sounds) in [(0, 0, 0), (2, 0, 0), (2, 3, 0), (2, 3, 3)] {
        let ledger = livenovel::mapping_ledger(sites, fade_sites, menu_sounds, 128_000);
        assert!(ledger.len() >= 12);
        let mut rules = std::collections::BTreeSet::new();
        for m in &ledger {
            assert!(rules.insert(m.rule.clone()), "duplicate rule {}", m.rule);
            assert!(
                matches!(
                    m.level.as_str(),
                    "exact" | "adapted" | "approximate" | "unsupported"
                ),
                "{}",
                m.rule
            );
            assert!(
                matches!(m.evidence.as_str(), "decoded-source" | "documented"),
                "{}",
                m.rule
            );
            assert_eq!(m.approximate(), m.level == "approximate");
            if m.approximate() {
                assert!(
                    m.approximation.as_deref().is_some_and(|s| !s.is_empty()),
                    "{}",
                    m.rule
                );
            } else {
                // Exact and adapted rules carry no approximation statement;
                // open verification items stay in the fidelity warnings.
                assert_eq!(m.approximation, None, "{}", m.rule);
            }
            assert!(!m.behavior.contains(source.to_string_lossy().as_ref()), "{}", m.rule);
        }
        let choice = ledger.iter().find(|m| m.rule == "livenovel.story.choice");
        assert_eq!(choice.is_some(), sites > 0);
        if let Some(m) = choice {
            assert_eq!(m.level, "adapted");
            assert_eq!(m.evidence, "decoded-source");
            assert!(m
                .capabilities
                .iter()
                .any(|c| c == "story.typed-result.v1"));
        }
        let fade = ledger
            .iter()
            .find(|m| m.rule == "livenovel.textbox.fade")
            .unwrap();
        assert_eq!(fade.level, "adapted");
        assert_eq!(fade.approximation, None);
        assert_eq!(
            fade.capabilities.iter().any(|c| c == "text.window-transition.v1"),
            fade_sites > 0,
            "the window-transition capability follows actual fade sites"
        );
        let sfx = ledger
            .iter()
            .find(|m| m.rule == "livenovel.menu-sfx")
            .unwrap();
        assert_eq!(sfx.level, "adapted");
        assert_eq!(sfx.approximation, None);
        assert_eq!(
            sfx.capabilities.iter().any(|c| c == "ui.menu-effects.v1"),
            menu_sounds > 0,
            "the menu-effects capability follows actually mapped page effects"
        );
        let hover = ledger
            .iter()
            .find(|m| m.rule == "livenovel.menu-hover")
            .unwrap();
        assert_eq!(hover.level, "approximate");
        assert!(hover.approximation.as_deref().is_some_and(|s| !s.is_empty()));
        let font = ledger
            .iter()
            .find(|m| m.rule == "livenovel.text.font")
            .unwrap();
        assert_eq!(font.level, "approximate");
        assert!(font.approximation.as_deref().is_some_and(|s| !s.is_empty()));
        let reveal = ledger
            .iter()
            .find(|m| m.rule == "livenovel.text.reveal")
            .unwrap();
        assert_eq!(reveal.level, "adapted");
        assert_eq!(reveal.approximation, None);
        assert!(reveal.behavior.contains("128000 µs"));
        let approximate: Vec<&str> = ledger
            .iter()
            .filter(|m| m.approximate())
            .map(|m| m.rule.as_str())
            .collect();
        // Keep in sync with APPROXIMATE_RULES: acceptance names exactly these.
        assert_eq!(approximate, APPROXIMATE_RULES);
        assert_eq!(
            ImportReport::status_from_mappings(&ledger),
            "converted_with_approximations"
        );
        // The gate: unaccepted approximate rules block after publication; a
        // typo'd acceptance cannot pass because the real rule stays named.
        // The generic-path conversion above exercises the wiring with an
        // accepted ledger; the LiveNovel end of the gate runs against the
        // real corpus.
        let mut opts = options(&source, &temp.path().join("nowhere"));
        opts.accept_approximate.clear();
        let denied = enforce_acceptance(&ledger, &opts).unwrap_err();
        assert!(denied.to_string().contains("E_IMPORT_APPROXIMATE"), "{denied}");
        opts.accept_approximate = vec!["livenovel.menu-hover".into()];
        let denied = enforce_acceptance(&ledger, &opts).unwrap_err();
        assert!(
            denied.to_string().contains("livenovel.text.font"),
            "{denied}"
        );
        opts.accept_approximate = vec!["livenovel.text.font".into(), "typo".into()];
        let denied = enforce_acceptance(&ledger, &opts).unwrap_err();
        assert!(
            denied.to_string().contains("livenovel.menu-hover"),
            "{denied}"
        );
        opts.accept_approximate = APPROXIMATE_RULES.iter().map(|s| (*s).into()).collect();
        enforce_acceptance(&ledger, &opts).unwrap();
        opts.accept_approximate.clear();
        opts.draft = true;
        // Draft keeps its own incomplete contract and skips the gate.
        enforce_acceptance(&ledger, &opts).unwrap();
    }
    // A ledger without approximate or unsupported rules keeps the
    // converted_with_adaptations claim, and one without adaptations is exact.
    let exact_only = vec![ImportMapping {
        rule: "r".into(),
        level: "exact".into(),
        evidence: "documented".into(),
        source_version: "LSB116".into(),
        behavior: String::new(),
        capabilities: vec![],
        approximation: None,
    }];
    assert_eq!(
        ImportReport::status_from_mappings(&exact_only),
        "converted"
    );
    assert_eq!(
        ImportReport::status_from_mappings(&livenovel::mapping_ledger(0, 0, 0, 128_000)[..2]),
        "converted_with_adaptations"
    );
}
