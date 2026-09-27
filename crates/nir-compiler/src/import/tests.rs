use super::lsb::tests::{command, dialogue, literal, script, string, u32b};
use super::*;

fn options(source: &Path, out: &Path) -> ImportOptions {
    ImportOptions {
        source: source.into(),
        out: out.into(),
        entry: Some("main.lsb".into()),
        line: 0,
        draft: false,
        game_id: "org.nir.test.import".into(),
        title: "Migration fixture".into(),
        locale: "ja".into(),
    }
}
fn sdk() -> PathBuf {
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

/// Opt-in local corpus test: no proprietary content is stored in the repository.
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
    let validated = ValidatedProgram::new(loaded.program.clone()).unwrap();
    let entries: Vec<_> = std::iter::once(loaded.program.entry.clone())
        .chain(loaded.program.theme.image_menus.values().flat_map(|menu| {
            menu.buttons.iter().filter_map(|b| match &b.action {
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
            menu.buttons
                .iter()
                .filter_map(|button| button.requires.clone())
        })
        .filter(|key| key.starts_with("lm.replay."))
        .collect();
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
