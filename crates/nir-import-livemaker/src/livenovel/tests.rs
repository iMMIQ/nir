use super::*;
#[test]
fn inline_choice_does_not_capture_later_choices_across_numeric_input_jumps() {
    let (_dir, mut adapter) = route::adapter();
    let source = route::script(vec![
        route::command(
            7,
            0,
            Body::Wait(vec![
                route::expr(1, "____arg", vec![Literal::Variable("選択実行中".into())]),
                route::int(0),
                route::int(0),
            ]),
        ),
        route::command(
            5,
            1,
            Body::Call {
                target: Reference {
                    page: "ノベルシステム/メッセージボックス/文字列入力.lsb".into(),
                    line: 0,
                },
                condition: route::int(1),
                has_params: true,
                params: vec![],
            },
        ),
        route::command(
            4,
            2,
            Body::Jump(
                Reference {
                    page: "main.lsb".into(),
                    line: 3,
                },
                route::int(1),
            ),
        ),
        route::command(
            5,
            3,
            Body::Call {
                target: Reference {
                    page: CHOICE_EXECUTOR.into(),
                    line: 0,
                },
                condition: route::int(1),
                has_params: true,
                params: vec![route::int(0); 8],
            },
        ),
        route::command(16, 4, Body::VariableDelete("_tmpid".into())),
    ]);
    let mut routes = Routes {
        replay: false,
        episodes: vec![],
        blocks: BTreeMap::new(),
        entries: BTreeMap::new(),
        queue: vec![],
        scripts: BTreeMap::new(),
        variants: BTreeMap::new(),
        states: BTreeMap::new(),
        choice_sites: 0,
    };
    assert!(!route_support::inline_choice(
        &mut adapter,
        &source,
        "main.lsb",
        0,
        "start",
        &BTreeSet::new(),
        &mut routes
    )
    .unwrap());
    assert!(routes.blocks.is_empty());
    assert!(adapter.choices.is_empty());
}
#[test]
fn preview_cancel_and_retained_settle_images_execute_through_restore() {
    use route::{command, expr, script};
    let dir = tempfile::tempdir().unwrap();
    let xml = r#"<?xml version="1.0"?><PrevMenu><Version>200</Version><Width>1024</Width><Height>768</Height><Button><Item><Left>120</Left><Top>100</Top><PrevLeft>0</PrevLeft><PrevTop>0</PrevTop><Path>normal.gal</Path><CapMask>0</CapMask><CapMaskLevel>128</CapMaskLevel><Name>Option</Name><Group>0</Group><InImagePath>hover.gal</InImagePath><SettleImagePath>hover.gal</SettleImagePath><OutImagePath>normal.gal</OutImagePath><DownOnPrevRepeat>0</DownOnPrevRepeat><DownOffPrevRepeat>0</DownOffPrevRepeat></Item></Button></PrevMenu>"#;
    fs::write(dir.path().join("menu.lpm"), xml).unwrap();
    let helper = dir
        .path()
        .join("ノベルシステム/プレビューメニュー/■選択実行.lsb");
    fs::create_dir_all(helper.parent().unwrap()).unwrap();
    fs::write(helper, []).unwrap();
    let mut header = vec![0; 47];
    header[..7].copy_from_slice(b"Gale106");
    header[15..19].copy_from_slice(&32u32.to_le_bytes());
    header[19..23].copy_from_slice(&24u32.to_le_bytes());
    for name in ["normal.gal", "hover.gal"] {
        fs::write(dir.path().join(name), &header).unwrap();
    }
    let wait = Expression {
        literal: None,
        operations: vec![
            (
                11,
                "____0".into(),
                vec![
                    Literal::String("メッセージボックス".into()),
                    Literal::Int(138),
                ],
            ),
            (
                11,
                "____1".into(),
                vec![Literal::Variable("選択実行中".into())],
            ),
            (
                8,
                "____2".into(),
                vec![
                    Literal::Variable("____0".into()),
                    Literal::Variable("____1".into()),
                ],
            ),
            (1, "____arg".into(), vec![Literal::Variable("____2".into())]),
        ],
        functions: BTreeMap::from([(0, 2), (1, 20)]),
    };
    let string = |s: &str| Expression {
        literal: Some(Literal::String(s.into())),
        operations: vec![(1, "____arg".into(), vec![Literal::String(s.into())])],
        functions: BTreeMap::new(),
    };
    let read = |s: &str| expr(1, "____arg", vec![Literal::Variable(s.into())]);
    for (close, cancel_input) in [(-1, false), (-1, true), (0, false)] {
        let mut adapter = Adapter::new(Source::new(dir.path()).unwrap());
        adapter.source.scripts.insert(
            "ノベルシステム/プレビューメニュー/■選択実行.lsb".into(),
            std::sync::Arc::new(Script {
                version: 117,
                source_sha256: "5795a8e97d0f0a8d2b94682d4e13f7059a9aaecf3f8c19df21dbd6a3587d8894"
                    .into(),
                commands: vec![],
            }),
        );
        let local = |line, name: &str| {
            command(
                15,
                line,
                Body::Variable {
                    name: name.into(),
                    value_type: 4,
                    scope: 2,
                    initial: Expression::default(),
                },
            )
        };
        let mut source = script(vec![
            command(
                7,
                0,
                Body::Wait(vec![wait.clone(), route::int(0), route::int(0)]),
            ),
            command(
                14,
                1,
                Body::Calc(expr(1, "選択実行中", vec![Literal::Int(1)])),
            ),
            local(2, "_tmp"),
            local(3, "_tmp2"),
            local(4, "_tmp3"),
            command(
                5,
                5,
                Body::Call {
                    target: Reference {
                        page: "ノベルシステム\\プレビューメニュー\\■選択実行.lsb".into(),
                        line: 0,
                    },
                    condition: route::int(1),
                    has_params: true,
                    params: vec![
                        string(""),
                        string("menu.lpm"),
                        route::int(0),
                        route::int(close),
                        string(""),
                        string(""),
                        route::int(1),
                        read("_tmp"),
                        read("_tmp2"),
                        route::int(0),
                        read("_tmp3"),
                    ],
                },
            ),
            command(
                14,
                6,
                Body::Calc(expr(1, "選択実行中", vec![Literal::Int(0)])),
            ),
            command(19, 7, Body::Delete(string("VOICE"))),
            command(16, 8, Body::VariableDelete("_tmp".into())),
            command(16, 9, Body::VariableDelete("_tmp2".into())),
            command(16, 10, Body::VariableDelete("_tmp3".into())),
            route::exit(11),
        ]);
        source.version = 117;
        for (i, c) in source.commands.iter_mut().enumerate().take(11) {
            c.not_update = !matches!(i, 0 | 5);
        }
        let mut routes = route::walk(&mut adapter, &source).unwrap();
        routes.blocks.insert(
            "failed".into(),
            json!({"ops":[],"terminator":{"type":"fault","code":"E_FIXTURE","message":"test"}}),
        );
        let mut p: nir_format::Program =
            serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
        p.requires = nir_format::CAPABILITIES
            .iter()
            .map(|s| s.to_string())
            .collect();
        p.variables.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(adapter.variables))
                .unwrap(),
        );
        p.scenes.extend(
            serde_json::from_value::<BTreeMap<String, Vec<Node>>>(json!(adapter.scenes)).unwrap(),
        );
        p.cues.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Cue>>(json!(adapter.cues))
                .unwrap(),
        );
        p.functions.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Function>>(json!(
                adapter.functions
            ))
            .unwrap(),
        );
        p.choices.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Choice>>(json!(adapter.choices))
                .unwrap(),
        );
        p.functions.insert(
            "main".into(),
            serde_json::from_value(
                json!({"entry":routes.entries[&("00000001.lsb".into(),0)],"blocks":routes.blocks}),
            )
            .unwrap(),
        );
        for (id, asset) in &adapter.assets {
            p.assets.insert(id.clone(),serde_json::from_value(json!({"kind":"image","object":"neutral","bytes":1,"width":asset.size[0],"height":asset.size[1]})).unwrap());
        }
        let contract = p.texts["intro"].clone();
        for id in adapter.texts.keys() {
            p.texts.insert(id.clone(), contract.clone());
            for locale in p.locales.values_mut() {
                locale.insert(id.clone(), locale["intro"].clone());
            }
        }
        let validated = nir_core::ValidatedProgram::new(p).unwrap();
        let mut core =
            nir_core::Core::new(validated.clone(), "preview".into(), "en".into()).unwrap();
        core.step(nir_core::CoreInput::None, 1000);
        while let Some(pending) = core.state().pending.clone() {
            core.step(
                nir_core::CoreInput::Prepared {
                    activation: pending.id,
                },
                1000,
            );
        }
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        let old = core.state().choice.as_ref().unwrap().interaction;
        let mut core = nir_core::Core::restore(validated, core.snapshot(), "preview").unwrap();
        let current = core.state().choice.as_ref().unwrap().interaction;
        assert_ne!(current, old);
        core.step(
            nir_core::CoreInput::CancelChoice {
                interaction: old,
                sequence: 1,
            },
            1000,
        );
        assert_eq!(core.state().choice.as_ref().unwrap().interaction, current);
        let input = if cancel_input {
            nir_core::CoreInput::CancelChoice {
                interaction: current,
                sequence: 1,
            }
        } else {
            nir_core::CoreInput::Choose {
                interaction: current,
                sequence: 1,
                option: "o0".into(),
            }
        };
        core.step(input, 1000);
        while let Some(pending) = core.state().pending.clone() {
            core.step(
                nir_core::CoreInput::Prepared {
                    activation: pending.id,
                },
                1000,
            );
        }
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        let expected = if cancel_input { "" } else { "Option" };
        assert_eq!(
            core.state().variables["最終選択値"],
            nir_format::Value::String(expected.into())
        );
        assert_eq!(
            core.state().variables["選択番号"],
            nir_format::Value::I32(if cancel_input { -1 } else { 0 })
        );
        let retained = core
            .state()
            .scene
            .iter()
            .any(|n| n.id == "__nir_lm_preview_root");
        assert_eq!(retained, close == -1 && !cancel_input);
        if retained {
            let hover = adapter
                .assets
                .iter()
                .find(|(_, a)| a.source == "hover.gal")
                .unwrap()
                .0;
            assert!(core
                .state()
                .scene
                .iter()
                .any(|n| n.asset.as_ref() == Some(hover) && n.opacity == 1.));
        }
    }
}
fn execute_route_fixture(adapter: &mut Adapter, source: &Script) -> nir_core::Core {
    let routes = route::walk(adapter, source).unwrap();
    let entry = routes.entries[&(
        "00000001.lsb".into(),
        adapter.skip_forward(source, 0).unwrap(),
    )]
        .clone();
    let mut p: nir_format::Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    p.variables.extend(
        serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(adapter.variables))
            .unwrap(),
    );
    p.functions.insert(
        "main".into(),
        serde_json::from_value(json!({"entry":entry,"blocks":routes.blocks})).unwrap(),
    );
    let mut core = nir_core::Core::new(
        nir_core::ValidatedProgram::new(p).unwrap(),
        "branches".into(),
        "en".into(),
    )
    .unwrap();
    core.step(nir_core::CoreInput::None, 1000);
    assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
    core
}

#[test]
fn cg_index_recognizes_later_branch_declarations_without_creating_images() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Adapter::new(Source::new(dir.path()).unwrap());
    let mut command = route::text(1, "fixture");
    let Body::Text { text, .. } = &mut command.body else {
        unreachable!()
    };
    text.glyphs = ["later", "?dynamic", "@Sender"]
        .into_iter()
        .map(|name| {
            Glyph::Event(
                ["CREATECG", name, "actor.gal", "NORMAL", "#1", "C", "B", "0"]
                    .map(str::to_owned)
                    .to_vec(),
            )
        })
        .collect();
    let source = route::script(vec![command]);
    quake::index_cg_names(&mut a, "neutral.lsb", &source).unwrap();
    assert_eq!(a.declared_cg_names, BTreeSet::from(["later".into()]));
    assert!(a.nodes.is_empty());
    assert!(a.assets.is_empty());
    // An absent, known source image has no visual motion, but retains
    // the callback's finite lifetime; it must not manufacture a sprite.
    let mut blocks = vec![];
    // lower is tested within quake.rs; this index does not bypass the
    // certified event/helper check for production input.
    assert!(quake::event(
        &mut a,
        "QUAKE",
        &["later", "BOUND", "1", "0", "30", "500", "25"].map(str::to_owned),
        &mut blocks
    )
    .is_err());
    assert!(blocks.is_empty());
}
#[test]
fn conditional_empty_arms_and_subroutine_returns_skip_sibling_arms() {
    use route::{command, expr, script};
    let mut write = command(14, 3, Body::Calc(expr(1, "result", vec![Literal::Int(22)])));
    write.indent = 1;
    let mut call = command(
        5,
        1,
        Body::Call {
            target: Reference {
                page: String::new(),
                line: 50,
            },
            condition: route::int(1),
            has_params: false,
            params: vec![],
        },
    );
    call.indent = 1;
    let source = script(vec![
        command(
            0,
            0,
            Body::Condition(expr(1, "____arg", vec![Literal::Variable("a".into())])),
        ),
        call,
        command(2, 2, Body::Other),
        write,
        route::exit(4),
        route::label(50),
        command(
            14,
            51,
            Body::Calc(expr(1, "result", vec![Literal::Int(99)])),
        ),
        route::exit(52),
    ]);
    for value in [0, 1] {
        let (_temp, mut adapter) = route::adapter();
        adapter
            .variables
            .insert("a".into(), json!({"type":"i32","value":value}));
        let core = execute_route_fixture(&mut adapter, &source);
        assert_eq!(
            core.state().variables["result"],
            nir_format::Value::I32(if value == 0 { 22 } else { 99 })
        );
    }
    // With an empty first arm, taking it must skip the Else entirely.
    let mut empty = source.clone();
    empty.commands.remove(1);
    for value in [0, 1] {
        let (_temp, mut adapter) = route::adapter();
        adapter
            .variables
            .insert("a".into(), json!({"type":"i32","value":value}));
        adapter
            .variables
            .insert("result".into(), json!({"type":"i32","value":0}));
        let core = execute_route_fixture(&mut adapter, &empty);
        assert_eq!(
            core.state().variables["result"],
            nir_format::Value::I32(if value == 0 { 22 } else { 0 })
        );
    }
    empty.commands[0].kind = 1;
    assert!(route_support::condition_targets(&empty, 0).is_err());
    empty.commands[0].kind = 2;
    assert!(route_support::condition_targets(&empty, 0).is_err());
}
#[test]
fn structured_conditions_execute_only_the_selected_nested_arm_and_join() {
    use route::{command, expr, script};
    let conditional = |kind, line, depth, name: &str| {
        let mut c = command(
            kind,
            line,
            Body::Condition(expr(1, "____arg", vec![Literal::Variable(name.into())])),
        );
        c.indent = depth;
        c
    };
    let write = |line, depth, target: &str, value| {
        let mut c = command(
            14,
            line,
            Body::Calc(expr(1, target, vec![Literal::Int(value)])),
        );
        c.indent = depth;
        c
    };
    let otherwise = |line, depth| {
        let mut c = command(2, line, Body::Other);
        c.indent = depth;
        c
    };
    let source = script(vec![
        conditional(0, 0, 0, "a"),
        conditional(0, 1, 1, "b"),
        write(2, 2, "result", 11),
        conditional(1, 3, 1, "c"),
        write(4, 2, "result", 22),
        otherwise(5, 1),
        write(6, 2, "result", 33),
        conditional(1, 7, 0, "d"),
        write(8, 1, "result", 44),
        otherwise(9, 0),
        write(10, 1, "result", 55),
        write(11, 0, "joined", 1),
        route::exit(12),
    ]);
    for mask in 0..16 {
        let (_temp, mut adapter) = route::adapter();
        for (bit, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
            adapter
                .variables
                .insert(name.into(), json!({"type":"i32","value":(mask >> bit) & 1}));
        }
        let routes = route::walk(&mut adapter, &source).unwrap();
        let entry = routes.entries[&("00000001.lsb".into(), 0)].clone();
        let mut p: nir_format::Program =
            serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
        p.variables.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(adapter.variables))
                .unwrap(),
        );
        p.functions.insert(
            "main".into(),
            serde_json::from_value(json!({"entry":entry,"blocks":routes.blocks})).unwrap(),
        );
        let mut core = nir_core::Core::new(
            nir_core::ValidatedProgram::new(p).unwrap(),
            "branches".into(),
            "en".into(),
        )
        .unwrap();
        core.step(nir_core::CoreInput::None, 1000);
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        let expected = if mask & 1 != 0 {
            if mask & 2 != 0 {
                11
            } else if mask & 4 != 0 {
                22
            } else {
                33
            }
        } else if mask & 8 != 0 {
            44
        } else {
            55
        };
        assert_eq!(
            core.state().variables["result"],
            nir_format::Value::I32(expected),
            "mask {mask}"
        );
        assert_eq!(core.state().variables["joined"], nir_format::Value::I32(1));
        assert_eq!(core.state().outcome.as_deref(), Some("completed"));
    }
}
#[test]
fn direct_conditional_jump_entry_executes_both_dispatch_and_fallthrough() {
    use route::{command, expr};
    let jump = |line, target, condition| {
        command(
            4,
            line,
            Body::Jump(
                Reference {
                    page: String::new(),
                    line: target,
                },
                condition,
            ),
        )
    };
    let write = |line, value| {
        command(
            14,
            line,
            Body::Calc(expr(1, "result", vec![Literal::Int(value)])),
        )
    };
    let mut source = route::script(vec![
        jump(0, 12, route::int(1)),
        write(1, 99),
        route::exit(2),
        jump(
            12,
            20,
            expr(
                12,
                "____arg",
                vec![
                    Literal::Variable("selected".into()),
                    Literal::String("yes".into()),
                ],
            ),
        ),
        write(13, 22),
        route::exit(14),
        route::label(20),
        write(21, 11),
        route::exit(22),
    ]);
    source.version = 117;
    for (selected, expected) in [("yes", 11), ("no", 22)] {
        let (_temp, mut adapter) = route::adapter();
        adapter
            .variables
            .insert("selected".into(), json!({"type":"string","value":selected}));
        let core = execute_route_fixture(&mut adapter, &source);
        assert_eq!(
            core.state().variables["result"],
            nir_format::Value::I32(expected)
        );
        assert_eq!(core.state().outcome.as_deref(), Some("completed"));
    }
}
#[test]
fn direct_jump_entries_require_top_level_pure_117_commands() {
    let jump = route::command(
        4,
        12,
        Body::Jump(
            crate::lsb::Reference {
                page: "next.lsb".into(),
                line: 0,
            },
            route::int(1),
        ),
    );
    let mut source = route::script(vec![jump]);
    assert!(label(&source, 12).is_err());
    source.version = 117;
    assert_eq!(label(&source, 12).unwrap(), 0);
    source.commands[0].indent = 1;
    assert!(label(&source, 12).is_err());
    source.commands[0].indent = 0;
    source.commands[0].muted = true;
    assert!(label(&source, 12).is_err());
    source.commands[0].muted = false;
    if let Body::Jump(_, condition) = &mut source.commands[0].body {
        *condition = route::int(0);
    }
    assert_eq!(label(&source, 12).unwrap(), 0);
    if let Body::Jump(_, condition) = &mut source.commands[0].body {
        condition.literal = None;
        condition.operations = vec![(11, "____arg".into(), vec![Literal::Int(10)])];
        condition.functions.insert(0, 26); // Random is not a pure dispatch.
    }
    assert!(label(&source, 12).is_err());
}
#[test]
fn click_wait_releases_audio_wait_without_stopping_or_restarting_sound() {
    let (_temp, mut adapter) = route::adapter();
    adapter
        .audio
        .insert("sfx".into(), json!({"type":"audio","bus":"sfx"}));
    let mut blocks = vec![];
    adapter
        .event(
            &["WAITPLAY".into(), "SE".into(), "CLICK".into()],
            &mut blocks,
        )
        .unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["terminator"]["conditions"][0]["task"], "sfx");
    assert_eq!(blocks[0]["terminator"]["on_advance"], "NEXT");
    assert_eq!(blocks[0]["ops"], json!([]));
    assert!(adapter.cues.is_empty());
    adapter
        .event(
            &["WAITPLAY".into(), "SE".into(), "NORMAL".into()],
            &mut blocks,
        )
        .unwrap();
    assert!(blocks[1]["terminator"].get("on_advance").is_none());
    assert!(adapter
        .event(
            &["WAITPLAY".into(), "SE".into(), "OTHER".into()],
            &mut blocks
        )
        .is_err());
}
#[test]
fn post_label_entries_preserve_the_first_statement_and_reject_unanchored_targets() {
    let mut source = route::script(vec![
        route::label(11),
        route::command(14, 12, Body::Calc(Expression::default())),
    ]);
    assert!(label(&source, 12).is_err());
    source.version = 117;
    assert_eq!(label(&source, 12).unwrap(), 1);
    source.commands[0].muted = true;
    assert!(label(&source, 12).is_err());
    source.commands[0].muted = false;
    source.commands[1].indent = 1;
    assert!(label(&source, 12).is_err());
    source.commands[1].indent = 0;
    source.commands[1].line = 13;
    assert!(label(&source, 13).is_err());
}
#[test]
fn route_anchor_keys_share_only_equivalent_position_rules() {
    let a = RouteState {
        nodes: BTreeMap::new(),
        committed_images: BTreeSet::new(),
        centered: BTreeSet::from(["image".into()]),
        anchors: BTreeMap::from([("image".into(), ("C".into(), "B".into()))]),
        audio: BTreeMap::new(),
        motion_tasks: BTreeMap::new(),
        movie_tasks: BTreeMap::new(),
        returns: vec![],
        pending_visibility: None,
    };
    let none = BTreeSet::new();
    let key = |s: &RouteState| s.signature_before_delete(&none).unwrap();
    let mut b = a.clone();
    b.centered.clear();
    assert_eq!(key(&a), key(&b));
    b.anchors.clear();
    let mut c = b.clone();
    c.centered.insert("image".into());
    assert_ne!(key(&b), key(&c));
    b.anchors.insert("image".into(), ("L".into(), "T".into()));
    c = b.clone();
    c.anchors
        .insert("image".into(), ("-0.0".into(), "0e0".into()));
    assert_eq!(key(&b), key(&c));
    assert_ne!(key(&a), key(&b));
    // Center/bottom must remain symbolic: changing the image's size
    // later changes these coordinates, whereas numeric zero stays zero.
    assert_ne!(
        image_position("C", 1024, 32, true).unwrap(),
        image_position("C", 1024, 64, true).unwrap()
    );
    assert_ne!(
        image_position("B", 768, 32, false).unwrap(),
        image_position("B", 768, 64, false).unwrap()
    );
}
#[test]
fn cleanup_shares_only_discarded_drawing_state_and_keeps_live_owners_distinct() {
    let node = |id, parent, color| {
        serde_json::from_value::<Node>(
            json!({"id":id,"parent":parent,"x":0,"y":0,"width":32,"height":24,"color":color,"scale":1,"opacity":1,"order":0}),
        )
        .unwrap()
    };
    let mut a = RouteState {
        nodes: BTreeMap::from([
            ("old".into(), node("old", None::<String>, [1.; 4])),
            ("child".into(), node("child", Some("old".into()), [1.; 4])),
            ("keep".into(), node("keep", None::<String>, [1.; 4])),
        ]),
        committed_images: BTreeSet::from(["old".into()]),
        centered: BTreeSet::from(["old".into()]),
        anchors: BTreeMap::from([("old".into(), ("C".into(), "B".into()))]),
        audio: BTreeMap::new(),
        motion_tasks: BTreeMap::new(),
        movie_tasks: BTreeMap::new(),
        returns: vec![],
        pending_visibility: None,
    };
    let mut b = a.clone();
    b.nodes.get_mut("old").unwrap().color = [0., 0., 0., 1.];
    b.nodes.remove("child");
    b.anchors.clear();
    b.centered.clear();
    let none = BTreeSet::new();
    let deleted = BTreeSet::from(["old".into()]);
    assert_ne!(
        a.signature_before_delete(&none).unwrap(),
        b.signature_before_delete(&none).unwrap()
    );
    assert_eq!(
        a.signature_before_delete(&deleted).unwrap(),
        b.signature_before_delete(&deleted).unwrap()
    );
    assert!(a.nodes.contains_key("old") && a.nodes.contains_key("child"));
    b.nodes.get_mut("keep").unwrap().color = [0., 0., 0., 1.];
    assert_ne!(
        a.signature_before_delete(&deleted).unwrap(),
        b.signature_before_delete(&deleted).unwrap()
    );
    b.nodes.get_mut("keep").unwrap().color = [1.; 4];
    a.movie_tasks.insert("old".into(), "movie-a".into());
    assert_ne!(
        a.signature_before_delete(&deleted).unwrap(),
        b.signature_before_delete(&deleted).unwrap()
    );
    a.movie_tasks.clear();
    a.audio
        .insert("bgm".into(), json!({"type":"audio","gain":1,"looped":true}));
    assert_ne!(
        a.signature_before_delete(&deleted).unwrap(),
        b.signature_before_delete(&deleted).unwrap()
    );
    let mut command = route::text(0, "");
    let Body::Text { text, .. } = &mut command.body else {
        unreachable!()
    };
    text.glyphs = vec![
        Glyph::Event(vec!["STOPSND".into(), "BGM".into()]),
        Glyph::Event(vec!["DELETECG".into(), "old".into()]),
    ];
    let mut source = route::script(vec![command]);
    assert_eq!(initial_scene_deletions(&source, 0, &a.nodes), deleted);
    let Body::Text { text, .. } = &mut source.commands[0].body else {
        unreachable!()
    };
    text.glyphs.insert(0, Glyph::Char("x".into()));
    assert!(initial_scene_deletions(&source, 0, &a.nodes).is_empty());
    let Body::Text { text, .. } = &mut source.commands[0].body else {
        unreachable!()
    };
    text.glyphs = vec![Glyph::Event(
        [
            "CREATECG",
            "old",
            "image.gal",
            "NORMAL",
            "#1",
            "C",
            "B",
            "500",
            "1",
            "1000",
        ]
        .map(str::to_owned)
        .into(),
    )];
    assert!(
        initial_scene_deletions(&source, 0, &a.nodes).is_empty(),
        "owned subtrees cannot be discarded by this rule"
    );
    a.nodes.remove("child");
    assert_eq!(initial_scene_deletions(&source, 0, &a.nodes), deleted);
}
#[test]
fn story_replay_modal_requires_a_provably_default_filter() {
    let (_temp, mut a) = route::adapter();
    assert!(certify_default_replay_target(&a).is_ok());
    a.variables.insert(
        "回想ターゲット".into(),
        json!({"type":"string","value":"custom"}),
    );
    assert!(certify_default_replay_target(&a).is_err());
    a.variables
        .insert("回想ターゲット".into(), json!({"type":"string","value":""}));
    a.functions.insert("test".into(),json!({"blocks":{"entry":{"ops":[{"operation":{"type":"assign","target":"回想ターゲット","value":{"type":"var","name":"dynamic"}}}]}}}));
    assert!(certify_default_replay_target(&a).is_err());
}
#[test]
fn replay_ids_use_the_authored_table_and_reject_ambiguous_writes() {
    let table = |name: &str, text: &str| {
        route::command(
            14,
            0,
            Body::Calc(Expression {
                literal: None,
                operations: vec![
                    (
                        11,
                        "____0".into(),
                        vec![Literal::String(text.into()), Literal::Variable(name.into())],
                    ),
                    (1, "____arg".into(), vec![Literal::Variable("____0".into())]),
                ],
                functions: BTreeMap::from([(0, 29)]),
            }),
        )
    };
    let script = route::script(vec![table("tags", "98\r\n99"), table("ids", "23\r\n24")]);
    assert_eq!(replay_ids(&script, 3).unwrap(), vec![23, 24]);
    assert!(replay_ids(&script, 1).is_err());
    for value in ["", "23\r\n23", "23\n24", "23\r\n", "-1"] {
        assert!(replay_ids(&route::script(vec![table("ids", value)]), 3).is_err());
    }
    assert!(replay_ids(&route::script(vec![table("tags", "23")]), 3).is_err());
    assert!(replay_ids(
        &route::script(vec![table("ids", "23"), table("ids", "24")]),
        3
    )
    .is_err());
    let mut altered = table("ids", "23");
    if let Body::Calc(e) = &mut altered.body {
        e.operations
            .push((1, "status".into(), vec![Literal::Int(1)]));
    }
    assert!(replay_ids(&route::script(vec![altered]), 3).is_err());
}
#[test]
fn replay_background_accepts_only_one_literal_table_row() {
    let table = |path: Literal| Expression {
        literal: None,
        operations: vec![
            (
                11,
                "____0".into(),
                vec![path, Literal::Variable("files".into())],
            ),
            (1, "____arg".into(), vec![Literal::Variable("____0".into())]),
        ],
        functions: [(0, 29)].into(),
    };
    let source = |e| route::script(vec![route::command(14, 0, Body::Calc(e))]);
    assert_eq!(
        stock_replay_background(&source(table(Literal::String("images\\back.gal".into()))))
            .unwrap(),
        "images/back.gal"
    );
    for value in [
        Literal::String("a.gal\r\nb.gal".into()),
        Literal::String(String::new()),
        Literal::Variable("dynamic".into()),
    ] {
        assert!(stock_replay_background(&source(table(value))).is_err());
    }
    let mut changed = table(Literal::String("back.gal".into()));
    changed.functions.insert(0, 72);
    assert!(stock_replay_background(&source(changed)).is_err());
}
#[test]
fn stock_textbox_keeps_both_source_insets_and_refuses_changed_geometry() {
    use route::{command, int, script};
    let variable = |name: &str| Literal::Variable(name.into());
    let string = |name: &str| Expression {
        literal: Some(Literal::String(name.into())),
        ..Expression::default()
    };
    let expr = |operations, functions| Expression {
        literal: None,
        operations,
        functions,
    };
    let branch = expr(
        vec![
            (
                10,
                "____0".into(),
                vec![variable("@ParamStr"), Literal::Int(0)],
            ),
            (
                12,
                "____1".into(),
                vec![variable("____0"), Literal::String("(標準)".into())],
            ),
            (1, "____arg".into(), vec![variable("____1")]),
        ],
        BTreeMap::new(),
    );
    let property = |number| {
        vec![
            Literal::String("メッセージボックス土台".into()),
            Literal::Int(number),
        ]
    };
    let x = expr(
        vec![
            (11, "____0".into(), property(5)),
            (
                3,
                "____1".into(),
                vec![variable("@ScrWidth"), variable("____0")],
            ),
            (5, "____2".into(), vec![variable("____1"), Literal::Int(2)]),
            (1, "____arg".into(), vec![variable("____2")]),
        ],
        [(0, 2)].into(),
    );
    let y = expr(
        vec![
            (11, "____0".into(), property(6)),
            (
                3,
                "____1".into(),
                vec![variable("@ScrHeight"), Literal::Int(10)],
            ),
            (
                3,
                "____2".into(),
                vec![variable("____1"), variable("____0")],
            ),
            (1, "____arg".into(), vec![variable("____2")]),
        ],
        [(0, 2)].into(),
    );
    for padding in [5, 10] {
        let mut image = command(
            9,
            1,
            Body::Object(
                [
                    (1, string("メッセージボックス土台")),
                    (3, string("images/box.gal")),
                    (4, x.clone()),
                    (5, y.clone()),
                    (12, int(175)),
                ]
                .into(),
            ),
        );
        image.indent = 1;
        let mut text = command(
            10,
            2,
            Body::Object(
                [
                    (2, string("メッセージボックス土台")),
                    (4, int(padding)),
                    (5, int(padding)),
                    (17, int(32)),
                    (19, int(8)),
                ]
                .into(),
            ),
        );
        text.indent = 1;
        let mut source = script(vec![
            command(0, 0, Body::Condition(branch.clone())),
            image,
            text,
        ]);
        let actual = stock_textbox(&source).unwrap();
        assert_eq!(actual.padding, padding);
        assert_eq!(actual.path, "images/box.gal");
        assert_eq!(actual.line_height, 1.25);
        let Body::Object(p) = &mut source.commands[1].body else {
            unreachable!()
        };
        p.insert(4, int(0));
        assert!(stock_textbox(&source).is_err());
    }
}
#[test]
fn menu200_preserves_entities_and_rejects_ambiguous_or_effectful_fields() {
    let xml = r#"<?xml version="1.0" encoding="Shift_JIS"?><PrevMenu><Version>200</Version><Width>1024</Width><Height>768</Height><Button><Item><Left>418</Left><Top>295</Top><PrevLeft>0</PrevLeft><PrevTop>0</PrevTop><Path>a1.gal</Path><CapMask>0</CapMask><CapMaskLevel>128</CapMaskLevel><Name>A&amp;B</Name><Group>0</Group><InImagePath>a2.gal</InImagePath><OutImagePath>a1.gal</OutImagePath><DownOnPrevRepeat>0</DownOnPrevRepeat><DownOffPrevRepeat>0</DownOffPrevRepeat></Item></Button></PrevMenu>"#;
    let buttons = menu(xml.as_bytes()).unwrap();
    assert_eq!(buttons.len(), 1);
    assert_eq!((buttons[0].x, buttons[0].y), (418, 295));
    assert_eq!(buttons[0].label, "A&B");
    assert_eq!(buttons[0].selected, "a2.gal");
    for (from, to) in [
        ("<Version>200</Version>", "<Version>201</Version>"),
        ("<Width>1024</Width>", "<Width>0</Width>"),
        ("<Group>0</Group>", "<Group>1</Group>"),
        ("<CapMask>0</CapMask>", "<CapMask>1</CapMask>"),
        (
            "<DownOnPrevRepeat>0</DownOnPrevRepeat>",
            "<DownOnPrevRepeat>1</DownOnPrevRepeat>",
        ),
        ("<Name>A&amp;B</Name>", "<Name>A</Name><Name>B</Name>"),
        (
            "<OutImagePath>a1.gal</OutImagePath>",
            "<OutImagePath>other.gal</OutImagePath>",
        ),
        ("</PrevMenu>", "</PrevMenu><extra/>"),
    ] {
        assert!(menu(xml.replace(from, to).as_bytes()).is_err(), "{to}");
    }
    let dtd = xml.replace(
        "?><PrevMenu>",
        "?><!DOCTYPE PrevMenu [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><PrevMenu>",
    );
    assert!(menu(dtd.as_bytes()).is_err());
    for n in 0..xml.len() {
        assert!(menu(&xml.as_bytes()[..n]).is_err());
    }
}
#[test]
fn auto_menu_profile_rejects_changed_lifecycle_parameters_and_extra_effects() {
    use super::super::lsb::Command;
    let expr = |op, name: &str, args| Expression {
        literal: None,
        operations: vec![(op, name.into(), args)],
        functions: BTreeMap::new(),
    };
    let int = |n| expr(1, "____arg", vec![Literal::Int(n)]);
    let text = |s: &str| expr(1, "____arg", vec![Literal::String(s.into())]);
    let command = |kind, indent, body| Command {
        kind,
        indent,
        body,
        muted: false,
        not_update: false,
        line: 0,
        offset: 0,
    };
    let mut parameters: BTreeMap<_, _> = [
        ("wipe", 3),
        ("time", 200),
        ("reverse", 1),
        ("act", 0),
        ("delete", 1),
        ("stop_event", 1),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), int(v)))
    .collect();
    for name in ["parameter_0", "parameter_1", "source"] {
        parameters.insert(name.into(), Expression::default());
    }
    let mut script = Script {
        version: 116,
        source_sha256: "a".repeat(64),
        commands: vec![
            command(
                1,
                0,
                Body::Condition(expr(
                    12,
                    "____arg",
                    vec![
                        Literal::Variable("val".into()),
                        Literal::String("自動テキスト送り".into()),
                    ],
                )),
            ),
            command(
                60,
                1,
                Body::Cabinet {
                    properties: BTreeMap::from([(1, text("キャビネット"))]),
                    act: int(1),
                    targets: vec![],
                },
            ),
            command(
                13,
                1,
                Body::Flip {
                    parameters,
                    targets: vec![text("メニュー背景")],
                },
            ),
            command(
                14,
                1,
                Body::Calc(expr(1, "自動送り", vec![Literal::Int(1)])),
            ),
            command(
                14,
                1,
                Body::Calc(expr(1, "自動送りフラグ", vec![Literal::Int(1)])),
            ),
        ],
    };
    verify_auto_menu_branch(&script).unwrap();
    let mut skip = script.clone();
    skip.commands[0].body = Body::Condition(expr(
        12,
        "____arg",
        vec![
            Literal::Variable("val".into()),
            Literal::String("読んだ文章を飛ばす".into()),
        ],
    ));
    skip.commands.truncate(3);
    for (name, value) in [
        ("メッセージスキップ", 1),
        ("既読メッセージスキップ", 1),
        ("既読スキップ使用済み", 0),
    ] {
        skip.commands.push(command(
            14,
            1,
            Body::Calc(expr(1, name, vec![Literal::Int(value)])),
        ));
    }
    for (property, value) in [(82, -1), (26, 0)] {
        skip.commands.push(command(
            18,
            1,
            Body::SetProperty {
                target: expr(
                    1,
                    "____arg",
                    vec![Literal::Variable("メッセージボックス".into())],
                ),
                property: int(property),
                value: int(value),
            },
        ));
    }
    verify_skip_menu_branch(&skip).unwrap();
    for case in 0..4 {
        let mut bad = skip.clone();
        match case {
            0 => {
                if let Body::SetProperty { value, .. } = &mut bad.commands[6].body {
                    *value = int(0);
                }
            }
            1 => bad.commands[6].not_update = true,
            2 => bad.commands[7].indent = 2,
            _ => bad.commands.push(command(
                14,
                1,
                Body::Calc(expr(1, "extra", vec![Literal::Int(1)])),
            )),
        }
        assert!(verify_skip_menu_branch(&bad).is_err(), "skip {case}");
    }
    for case in 0..7 {
        let original = script.commands.clone();
        match case {
            0 => {
                if let Body::Cabinet { act, .. } = &mut script.commands[1].body {
                    *act = int(0);
                }
            }
            1 => {
                if let Body::Flip { parameters, .. } = &mut script.commands[2].body {
                    parameters.insert("time".into(), int(201));
                }
            }
            2 => {
                if let Body::Flip { targets, .. } = &mut script.commands[2].body {
                    targets[0] = text("story");
                }
            }
            3 => script.commands[3].body = Body::Calc(expr(1, "自動送り", vec![Literal::Int(0)])),
            4 => script.commands[2].not_update = true,
            5 => script.commands.push(command(
                14,
                1,
                Body::Calc(expr(1, "extra", vec![Literal::Int(1)])),
            )),
            _ => script.commands.push(script.commands[0].clone()),
        }
        assert!(verify_auto_menu_branch(&script).is_err(), "{case}");
        script.commands = original;
    }
}
#[test]
fn auto_wait_profile_checks_both_write_and_caption_units() {
    use super::super::lsb::Command;
    let var = |name: &str| Literal::Variable(name.into());
    let expression = |ops| Expression {
        literal: None,
        operations: ops,
        functions: BTreeMap::new(),
    };
    let write = expression(vec![
        (
            10,
            "____d_0".into(),
            vec![var("@ParamStr"), Literal::Int(0)],
        ),
        (4, "____1".into(), vec![var("____d_0"), Literal::Int(1000)]),
        (1, "StatusAutoTextWait".into(), vec![var("____1")]),
    ]);
    let caption = expression(vec![
        (
            5,
            "____0".into(),
            vec![var("StatusAutoTextWait"), Literal::Int(1000)],
        ),
        (
            19,
            "____1".into(),
            vec![var("____0"), Literal::String("秒".into())],
        ),
        (1, "____arg".into(), vec![var("____1")]),
    ]);
    let command = |kind, body| Command {
        kind,
        body,
        indent: 0,
        muted: false,
        not_update: false,
        line: 0,
        offset: 0,
    };
    let mut script = Script {
        source_sha256: String::new(),
        version: 116,
        commands: vec![
            command(14, Body::Calc(write)),
            command(
                18,
                Body::SetProperty {
                    target: expression(vec![(
                        1,
                        "____arg".into(),
                        vec![Literal::String("caption".into())],
                    )]),
                    property: expression(vec![(1, "____arg".into(), vec![Literal::Int(50)])]),
                    value: caption,
                },
            ),
        ],
    };
    verify_auto_wait_callback(&script).unwrap();
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[1].2[1] = Literal::Int(100);
    }
    assert!(verify_auto_wait_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("units"));
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[1].2[1] = Literal::Int(1000);
    }
    if let Body::SetProperty { value, .. } = &mut script.commands[1].body {
        value.operations[0].2[1] = Literal::Int(100);
    }
    assert!(verify_auto_wait_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("caption units"));
    script
        .commands
        .push(command(14, Body::Calc(Expression::default())));
    assert!(verify_auto_wait_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("control flow"));
}

#[test]
fn text_speed_profile_checks_slider_write_units() {
    use super::super::lsb::Command;
    let var = |name: &str| Literal::Variable(name.into());
    let expression = |ops| Expression {
        literal: None,
        operations: ops,
        functions: BTreeMap::new(),
    };
    let write = expression(vec![
        (
            10,
            "____d_0".into(),
            vec![var("@ParamStr"), Literal::Int(0)],
        ),
        (4, "____1".into(), vec![var("____d_0"), Literal::Int(64)]),
        (1, "StatusTextSpeed".into(), vec![var("____1")]),
    ]);
    let command = |kind, body| Command {
        kind,
        body,
        indent: 0,
        muted: false,
        not_update: false,
        line: 0,
        offset: 0,
    };
    let mut script = Script {
        source_sha256: String::new(),
        version: 116,
        commands: vec![command(14, Body::Calc(write))],
    };
    verify_text_speed_callback(&script).unwrap();
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[1].2[1] = Literal::Int(100);
    }
    assert!(verify_text_speed_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("units"));
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[1] = (5, "____1".into(), vec![var("____d_0"), Literal::Int(64)]);
    }
    assert!(verify_text_speed_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("units"));
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[1] = (4, "____1".into(), vec![var("____d_0"), Literal::Int(64)]);
        e.operations[2] = (1, "StatusAutoTextWait".into(), vec![var("____1")]);
    }
    assert!(verify_text_speed_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("units"));
    if let Body::Calc(e) = &mut script.commands[0].body {
        e.operations[2] = (1, "StatusTextSpeed".into(), vec![var("____1")]);
    }
    script
        .commands
        .push(command(14, Body::Calc(Expression::default())));
    assert!(verify_text_speed_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("control flow"));
    script.commands.pop();
    script.commands[0].not_update = true;
    assert!(verify_text_speed_callback(&script)
        .unwrap_err()
        .to_string()
        .contains("control flow"));
}

fn stock_defaults() -> ImportedDefaults {
    ImportedDefaults {
        source_sha256: String::new(),
        source_version: 116,
        bgm_volume: 1.,
        voice_volume: 1.,
        sfx_volume: 1.,
        auto_wait_ms: 3000,
        text_speed_ms: 128,
    }
}

#[test]
fn mid_page_event_waits_for_text_marker_then_resumes_same_dialogue() {
    let temp = tempfile::tempdir().unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    adapter.defaults = Some(stock_defaults());
    let mut blocks = vec![];
    adapter
        .page(
            &[
                Glyph::Char("A".into()),
                Glyph::Event(vec![
                    "\u{1}WAIT".into(),
                    "40".into(),
                    "NORMAL".into(),
                    "SKIP".into(),
                ]),
                Glyph::Char("B".into()),
            ],
            &mut blocks,
        )
        .unwrap();
    let doc = adapter.texts.values().next().unwrap();
    assert!(matches!(&doc.spans[1],Span::Gate{id} if id=="s1"));
    assert!(adapter
        .cues
        .values()
        .any(|cue| cue["effects"][0]["effect"]["reveal_us"] == "128000"));
    assert_eq!(
        blocks[2]["terminator"]["conditions"][0]["milestone"],
        json!({"type":"marker","id":"s1"})
    );
    assert_eq!(blocks[3]["terminator"]["type"], "activate");
    assert_eq!(
        blocks[6]["ops"][0]["operation"]["type"],
        "dialogue_continue"
    );
    assert_eq!(adapter.texts.len(), 1);
    assert!(adapter
        .cues
        .values()
        .any(|cue| cue["effects"][0]["effect"]["duration_us"] == "40000"));
    assert!(adapter
        .page(&[Glyph::Event(vec!["UNKNOWN".into()])], &mut blocks)
        .is_err());
}
#[test]
fn voice_events_bind_the_source_page_before_and_after_its_gate() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("サウンド")).unwrap();
    std::fs::write(temp.path().join("サウンド/tone.wav"), b"fixture").unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    adapter.defaults = Some(stock_defaults());
    let event = || {
        Glyph::Event(
            ["PLAYSND", "tone.wav", "VOICE", "NORMAL", "1000", "0"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        )
    };
    let mut blocks = vec![];
    adapter
        .page(
            &[
                event(),
                Glyph::Char("First".into()),
                event(),
                Glyph::Char("Second".into()),
            ],
            &mut blocks,
        )
        .unwrap();
    let bindings: Vec<_> = blocks
        .iter()
        .flat_map(|b| b["ops"].as_array().into_iter().flatten())
        .map(|op| &op["operation"])
        .filter(|op| op["type"] == "dialogue_voice")
        .collect();
    assert_eq!(bindings.len(), 2);
    for binding in bindings {
        assert_eq!(binding["task"], "line");
        assert_eq!(binding["voice"], "voice");
        assert_eq!(binding["wait"], "sampled_remaining");
    }
    assert_eq!(adapter.texts.len(), 1);
    assert_eq!(
        adapter
            .texts
            .values()
            .next()
            .unwrap()
            .spans
            .iter()
            .filter(|s| matches!(s, Span::Gate { .. }))
            .count(),
        1
    );
    assert!(!adapter.audio.contains_key("voice"));
}

#[test]
fn event_gain_does_not_change_or_duplicate_media_recipe() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("サウンド")).unwrap();
    std::fs::write(temp.path().join("サウンド/tone.wav"), b"fixture").unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    let mut blocks = vec![];
    for volume in ["1500", "800"] {
        let event = ["PLAYSND", "tone.wav", "VOICE", "NORMAL", volume, "0"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        adapter.event(&event, &mut blocks).unwrap();
        assert_eq!(
            adapter.audio["voice"]["gain"],
            volume.parse::<f32>().unwrap() / 1000.
        );
    }
    assert_eq!(adapter.assets.len(), 1);
    assert_eq!(adapter.assets.values().next().unwrap().gain, Some(1.));
}
#[test]
fn playsnd_replacement_stops_only_when_the_new_media_cue_commits() {
    for (bus, mode) in [("BGM", "REPEAT"), ("VOICE", "NORMAL")] {
        for next in ["first.mp3", "second.mp3"] {
            let temp = tempfile::tempdir().unwrap();
            std::fs::create_dir(temp.path().join("サウンド")).unwrap();
            for file in ["first.mp3", "second.mp3"] {
                std::fs::write(temp.path().join("サウンド").join(file), b"fixture").unwrap();
            }
            let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
            let mut blocks = vec![];
            for file in ["first.mp3", next] {
                adapter
                    .event(
                        &["PLAYSND", file, bus, mode, "800", "0"].map(str::to_owned),
                        &mut blocks,
                    )
                    .unwrap();
            }
            // No cancellation operation runs before the replacement's
            // preparation barrier, even for an explicit same-track play.
            assert_eq!(blocks.len(), 2);
            assert!(blocks
                .iter()
                .all(|b| b["ops"].as_array().unwrap().is_empty()));
            let first = blocks[0]["terminator"]["cue"].as_str().unwrap();
            let replacement = blocks[1]["terminator"]["cue"].as_str().unwrap();
            assert_eq!(adapter.cues[first]["effects"].as_array().unwrap().len(), 1);
            let effects = adapter.cues[replacement]["effects"].as_array().unwrap();
            assert_eq!(effects.len(), 2);
            let task = bus.to_lowercase();
            assert_eq!(
                effects[0]["effect"],
                json!({"type":"audio_stop",
                "target":task,"duration_us":"0"})
            );
            assert_eq!(effects[1]["id"], task);
            assert_eq!(effects[1]["effect"]["type"], "audio");
            assert_eq!(effects[1]["effect"]["looped"], mode == "REPEAT");
            assert_eq!(effects[1]["effect"]["gain"], json!(0.8_f32));
        }
    }
}
#[test]
fn bgm_stop_preserves_duration_without_blocking_pass_event() {
    let temp = tempfile::tempdir().unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    adapter
        .audio
        .insert("bgm".into(), json!({"type":"audio","looped":true}));
    let mut blocks = vec![];
    adapter
        .event(
            &["STOPSND", "BGM", "500", "PASS"].map(str::to_owned),
            &mut blocks,
        )
        .unwrap();
    assert_eq!(blocks.len(), 1);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        adapter.cues[cue]["effects"][0]["effect"],
        json!({"type":"audio_stop","target":"bgm","duration_us":"500000"})
    );
    assert!(!adapter.audio.contains_key("bgm"));
}
#[test]
fn page_completion_cancels_non_looping_voice_only() {
    let temp = tempfile::tempdir().unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    adapter.defaults = Some(stock_defaults());
    adapter
        .audio
        .insert("bgm".into(), json!({"type":"audio","looped":true}));
    adapter
        .audio
        .insert("voice".into(), json!({"type":"audio","looped":false}));
    let mut blocks = vec![];
    adapter
        .page(&[Glyph::Char("Test".into())], &mut blocks)
        .unwrap();
    assert_eq!(blocks[2]["terminator"]["type"], "await");
    assert_eq!(blocks[3]["terminator"]["type"], "activate");
    let cue = blocks[3]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        adapter.cues[cue]["effects"][0]["effect"],
        json!({"type":"audio_stop","target":"voice","duration_us":"50000"})
    );
    assert!(!adapter.audio.contains_key("voice"));
    assert!(adapter.audio.contains_key("bgm"));
    adapter
        .audio
        .insert("voice".into(), json!({"type":"audio","looped":true}));
    blocks.clear();
    adapter
        .page(&[Glyph::Char("Next".into())], &mut blocks)
        .unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[1]["ops"][0]["operation"]["voice"], Value::Null);
    assert!(adapter.audio.contains_key("voice"));
}

#[test]
fn volume_reads_refine_gain_keys_before_any_relative_envelope_is_emitted() {
    let (_temp, mut adapter) = route::adapter();
    adapter.gain_reads.clear();
    let script = route::script(vec![route::exit(0)]);
    let mut routes = Routes {
        replay: false,
        episodes: vec![],
        blocks: BTreeMap::new(),
        entries: BTreeMap::new(),
        queue: vec![],
        scripts: BTreeMap::new(),
        variants: BTreeMap::new(),
        states: BTreeMap::new(),
        choice_sites: 0,
    };
    let mut enter = |adapter: &mut Adapter, gain: f32| {
        adapter.audio.insert(
            "bgm".into(),
            json!({"type":"audio", "bus":"bgm", "asset":"same", "gain":gain,"looped":true}),
        );
        adapter
            .route_block(&script, "00000001.lsb", 0, &mut routes)
            .unwrap()
    };
    assert_eq!(enter(&mut adapter, 1.), enter(&mut adapter, 0.5));
    let change = ["CHGVOL", "BGM", "250", "0", "WAIT"].map(str::to_owned);
    let mut blocks = vec![];
    let error = adapter.event(&change, &mut blocks).unwrap_err();
    assert_eq!(error.downcast_ref::<GainReadRequired>().unwrap().0, "bgm");
    assert!(blocks.is_empty());
    adapter.gain_reads.insert("bgm".into());
    assert_ne!(enter(&mut adapter, 1.), enter(&mut adapter, 0.5));
    for base in [1., 0.5] {
        adapter.audio.get_mut("bgm").unwrap()["gain"] = json!(base);
        let mut blocks = vec![];
        adapter.event(&change, &mut blocks).unwrap();
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        let envelope = adapter.cues[cue]["effects"][0]["effect"]["to"]
            .as_f64()
            .unwrap();
        assert_eq!(envelope * base, 0.25);
    }
}

#[test]
fn route_audio_merges_runtime_facts_but_keeps_read_gain_voice_repeat_and_channels() {
    let (_temp, mut adapter) = route::adapter();
    let script = route::script(vec![route::exit(0)]);
    let mut routes = Routes {
        replay: false,
        episodes: vec![],
        blocks: BTreeMap::new(),
        entries: BTreeMap::new(),
        queue: vec![],
        scripts: BTreeMap::new(),
        variants: BTreeMap::new(),
        states: BTreeMap::new(),
        choice_sites: 0,
    };
    let mut prefixes = Vec::new();
    let mut enter =
        |adapter: &mut Adapter, routes: &mut Routes, asset: &str, gain: f32, looped: bool| {
            let mut blocks = Vec::new();
            adapter.play_sound(
                &mut blocks,
                "bgm",
                json!({"type":"audio","asset":asset,"bus":"bgm","gain":gain,"looped":looped}),
            );
            prefixes.push(
                blocks.last().unwrap()["terminator"]["cue"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            adapter
                .route_block(&script, "00000001.lsb", 0, routes)
                .unwrap()
        };
    let first = enter(&mut adapter, &mut routes, "music.first", 1., true);
    let second = enter(&mut adapter, &mut routes, "music.second", 1., true);
    assert_eq!(
        first, second,
        "song identity is already held by the live runtime task"
    );
    assert_ne!(
        first,
        enter(&mut adapter, &mut routes, "music.second", 0.5, true)
    );
    assert_eq!(
        first,
        enter(&mut adapter, &mut routes, "music.second", 1., false)
    );
    let voice = json!({"type":"audio","asset":"voice","bus":"voice","gain":1.,"looped":true});
    adapter.audio.insert("voice".into(), voice);
    let repeated_voice = adapter
        .route_block(&script, "00000001.lsb", 0, &mut routes)
        .unwrap();
    adapter.audio.get_mut("voice").unwrap()["looped"] = json!(false);
    assert_ne!(
        repeated_voice,
        adapter
            .route_block(&script, "00000001.lsb", 0, &mut routes)
            .unwrap()
    );
    adapter.audio.clear();
    assert_ne!(
        first,
        adapter
            .route_block(&script, "00000001.lsb", 0, &mut routes)
            .unwrap()
    );
    assert_eq!(
        adapter.cues[&prefixes[0]]["effects"][0]["effect"]["asset"],
        "music.first"
    );
    assert_eq!(
        adapter.cues[&prefixes[1]]["effects"][1]["effect"]["asset"],
        "music.second"
    );
    // A merged stop addresses the live channel handle, never the
    // representative state's song. This preserves either incoming route.
    adapter.audio = routes.states[&first].audio.clone();
    let mut blocks = Vec::new();
    adapter.fade_stop(&mut blocks, "bgm", 50000);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        adapter.cues[cue]["effects"][0]["effect"],
        json!({"type":"audio_stop","target":"bgm","duration_us":"50000"})
    );
}

#[test]
fn missing_menu_clicks_are_reported_without_omitting_story_or_ambiguous_paths() {
    let (temp, mut a) = route::adapter();
    assert!(a.menu_sound("missing.wav").unwrap().is_none());
    assert_eq!(
        a.missing_menu_sounds,
        BTreeSet::from(["missing.wav".into()])
    );
    assert!(a.sound("missing.wav", 1.).is_err());
    fs::write(temp.path().join("AuD.wav"), b"invalid audio").unwrap();
    fs::write(temp.path().join("aUD.wav"), b"invalid audio").unwrap();
    assert!(a.menu_sound("aud.wav").is_err());
    assert!(a.menu_sound("../outside.wav").is_err());
    assert!(a.menu_sound("AuD.wav").unwrap().is_some());
    // A malformed existing file is registered for the strict decoder;
    // only a genuinely absent menu path can enter the omission ledger.
    assert_eq!(a.missing_menu_sounds.len(), 1);
}
#[test]
fn choice_sharing_keeps_source_read_identity_and_distinct_predicates() {
    let (_temp, mut a) = route::adapter();
    a.share_episodes = true;
    let mut choice = json!({"options":[{"id":"o0","text":"neutral",
        "visible":{"type":"const","value":{"type":"bool","value":true}}}]});
    let first = a.intern_choice(choice.clone());
    assert_eq!(a.intern_choice(choice.clone()), first);
    a.location.index += 1;
    assert_ne!(a.intern_choice(choice.clone()), first);
    a.location.index -= 1;
    choice["options"][0]["visible"]["value"]["value"] = json!(false);
    assert_ne!(a.intern_choice(choice), first);
    assert_eq!(a.choices.len(), 3);
}
#[test]
fn compact_operation_ids_keep_colliding_sources_distinct() {
    let (_temp, mut a) = route::adapter();
    a.share_episodes = true;
    let mut blocks = vec![];
    a.op(
        &mut blocks,
        json!({"type":"random","target":"affection","min":0,"max":10}),
    );
    let repeated = blocks.clone();
    a.finish_function("episode1", blocks, json!({"type":"return"}));
    let shared = a.function_aliases["episode1"].clone();
    let digest = shared.strip_prefix("shared_episode_").unwrap().to_owned();
    // Simulate a real truncated identity collision without relying on
    // finding one cryptographically. The final namespace keeps 256 bits.
    a.functions.clear();
    a.operation_sources
        .insert(digest[..32].into(), "different episode digest".into());
    a.finish_function("episode1", repeated, json!({"type":"return"}));
    let function = &a.functions[&a.function_aliases["episode1"]];
    let operation = &function["blocks"]["b000000"]["ops"][0];
    assert_eq!(operation["id"], json!(format!("op_{digest}_b0_o0")));
    assert!(a.source_map.contains_key(operation["id"].as_str().unwrap()));
}
#[test]
fn source_variants_keep_globally_unique_operations_and_execute_both_bodies() {
    use nir_core::{Core, CoreInput, ValidatedProgram};
    let (_temp, mut a) = route::adapter();
    a.share_episodes = true;
    for (name, add) in [("episode1", 1), ("episode2", 2)] {
        let mut blocks = vec![];
        a.op(
            &mut blocks,
            json!({"type":"assign","target":"affection","value":{"type":"var","name":"affection"}}),
        );
        a.op(&mut blocks, json!({"type":"assign","target":"affection","value":{"type":"binary","op":"add","left":{"type":"var","name":"affection"},"right":{"type":"const","value":{"type":"i32","value":add}}}}));
        a.finish_function(name, blocks, json!({"type":"return"}));
    }
    assert_eq!(a.functions.len(), 2);
    let mut p: nir_format::Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    p.functions = a
        .functions
        .iter()
        .map(|(id, f)| (id.clone(), serde_json::from_value(f.clone()).unwrap()))
        .collect();
    p.functions.insert("main".into(), serde_json::from_value(json!({"entry":"first","blocks":{
        "first":{"terminator":{"type":"call","function":a.function_aliases["episode1"],"next":"second"}},
        "second":{"terminator":{"type":"call","function":a.function_aliases["episode2"],"next":"done"}},
        "done":{"terminator":{"type":"end","outcome":"completed"}}
    }})).unwrap());
    let story = json!({"fragment_format":1,"functions":p.functions});
    nir_compiler::validate_generated_fragment(&story).unwrap();
    let mut broken = story.clone();
    let ids: Vec<_> = a.functions.keys().cloned().collect();
    broken["functions"][&ids[1]]["blocks"]["b000000"]["ops"][0]["id"] =
        broken["functions"][&ids[0]]["blocks"]["b000000"]["ops"][0]["id"].clone();
    assert!(nir_compiler::validate_generated_fragment(&broken)
        .unwrap_err()
        .to_string()
        .contains("E_DUPLICATE"));
    let mut core = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "variants".into(),
        "en".into(),
    )
    .unwrap();
    core.step(CoreInput::None, 100);
    assert!(core.state().fault.is_none());
    assert_eq!(core.state().outcome.as_deref(), Some("completed"));
    assert_eq!(
        core.state().variables["affection"],
        nir_format::Value::I32(3)
    );
}
#[test]
fn compact_episode_preserves_audio_delays_reading_gates_and_cold_restore() {
    use nir_core::{Core, CoreInput, CoreIntent, TaskState, ValidatedProgram};
    let build = |compact: bool| {
        let (_temp, mut a) = route::adapter();
        a.share_episodes = compact;
        let mut blocks = vec![];
        a.op(
            &mut blocks,
            json!({"type":"random","target":"affection","min":0,"max":1000}),
        );
        a.op(
            &mut blocks,
            json!({"type":"assign","target":"affection","value":{"type":"var","name":"affection"}}),
        );
        a.effect(
            &mut blocks,
            "music",
            "session",
            json!({"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}),
            false,
        );
        a.effect(
            &mut blocks,
            "delay",
            "frame",
            json!({"type":"delay","duration_us":"30000"}),
            true,
        );
        a.effect(
            &mut blocks,
            "line",
            "interaction",
            json!({"type":"dialogue","text":"letter","speaker":"","reveal_us":"0"}),
            false,
        );
        a.wait(&mut blocks, "line", json!({"type":"marker","id":"bell"}));
        a.op(
            &mut blocks,
            json!({"type":"random","target":"affection","min":0,"max":1000}),
        );
        a.op(
            &mut blocks,
            json!({"type":"dialogue_continue","task":"line"}),
        );
        a.wait(&mut blocks, "line", json!({"type":"finished"}));
        a.effect(
            &mut blocks,
            "delay",
            "frame",
            json!({"type":"delay","duration_us":"30000"}),
            true,
        );
        let before = blocks.len();
        a.finish_function("episode1", blocks, json!({"type":"return"}));
        let id = a
            .function_aliases
            .get("episode1")
            .cloned()
            .unwrap_or_else(|| "episode1".into());
        if compact {
            assert!(a.functions[&id]["blocks"].as_object().unwrap().len() < before + 3);
        }
        let mut p: nir_format::Program =
            serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
        p.functions = serde_json::from_value(json!(a.functions)).unwrap();
        p.cues = serde_json::from_value(json!(a.cues)).unwrap();
        p.functions.insert(
            "main".into(),
            serde_json::from_value(json!({"entry":"call","blocks":{
                "call":{"terminator":{"type":"call","function":id,"next":"done"}},
                "done":{"terminator":{"type":"end","outcome":"completed"}}
            }}))
            .unwrap(),
        );
        ValidatedProgram::new(p).unwrap()
    };
    let run = |program: ValidatedProgram, restore: bool| {
        let mut core = Core::new(program.clone(), "compact".into(), "en".into()).unwrap();
        let mut starts = 0;
        let mut restored = false;
        let mut input = CoreInput::None;
        for sequence in 1..500 {
            let step = core.step(input, 1000);
            starts += step
                .intents
                .iter()
                .filter(|intent| matches!(intent, CoreIntent::AudioStart { .. }))
                .count();
            assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
            if core.state().outcome.is_some() {
                break;
            }
            if restore
                && !restored
                && core.state().tasks.values().any(|task| {
                    task.scope == nir_format::Scope::Frame && task.state == TaskState::Running
                })
            {
                core = Core::restore(program.clone(), core.snapshot(), "compact").unwrap();
                restored = true;
            }
            input = if let Some(pending) = &core.state().pending {
                CoreInput::Prepared {
                    activation: pending.id,
                }
            } else if let Some((_, dialogue)) = core.dialogue() {
                CoreInput::Advance {
                    interaction: dialogue.interaction,
                    sequence,
                }
            } else {
                CoreInput::Time { delta_us: 10_000 }
            };
        }
        assert_eq!(core.state().outcome.as_deref(), Some("completed"));
        assert_eq!(
            starts, 1,
            "music must not restart at a wait or compacted boundary"
        );
        if restore {
            assert!(restored);
        }
        (
            core.state().variables.clone(),
            serde_json::to_value(&core.state().rng).unwrap(),
            core.state().tick_us,
        )
    };
    let reference = run(build(false), false);
    assert_eq!(run(build(true), false), reference);
    assert_eq!(run(build(true), true), reference);
    let mut branched = vec![json!({"ops":[],"terminator":{"type":"goto","target":"explicit"}})];
    let original = branched.clone();
    compact_linear_episode(&mut branched);
    assert_eq!(branched, original);
}
#[test]
fn separate_main_and_title_walks_keep_their_shared_episode_calls() {
    use nir_core::{Core, CoreInput, ValidatedProgram};
    let (_temp, mut a) = route::adapter();
    a.share_episodes = true;
    // The same source position can have different state effects in two
    // independently specialized graphs from the same source tree.
    let make_source = |visible| {
        let mut command = route::text(0, "");
        let Body::Text { text, .. } = &mut command.body else {
            unreachable!()
        };
        text.glyphs = vec![Glyph::Event(vec![
            if visible { "MESON" } else { "MESOFF" }.into(),
            "0".into(),
        ])];
        route::script(vec![command, route::exit(1)])
    };
    let main = route::walk(&mut a, &make_source(false)).unwrap();
    let first_name = main.episodes[0].1.clone();
    let first_shared = a.function_aliases[&first_name].clone();
    let bonus = route::walk(&mut a, &make_source(true)).unwrap();
    let second_name = bonus.episodes[0].1.clone();
    assert_ne!(first_name, second_name);
    assert_eq!(a.function_aliases[&first_name], first_shared);
    assert_ne!(first_shared, a.function_aliases[&second_name]);
    let mut p: nir_format::Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    p.functions = serde_json::from_value(json!(a.functions)).unwrap();
    p.cues = serde_json::from_value(json!(a.cues)).unwrap();
    for (name, mut graph) in [("main", main), ("bonus", bonus)] {
        for block in graph.blocks.values_mut() {
            if let Some(function) = block["terminator"]["function"].as_str() {
                block["terminator"]["function"] = json!(a.function_aliases[function]);
            }
        }
        let entry = &graph.entries[&("00000001.lsb".into(), 0)];
        p.functions.insert(
            name.into(),
            serde_json::from_value(json!({"entry":entry,"blocks":graph.blocks})).unwrap(),
        );
    }
    let validated = ValidatedProgram::new(p).unwrap();
    for (entry, expected) in [("main", false), ("bonus", true)] {
        let mut core = Core::new_at(
            validated.clone(),
            "episode-walks".into(),
            "en".into(),
            entry,
        )
        .unwrap();
        core.step(CoreInput::None, 1000);
        assert!(core.state().fault.is_none());
        assert_eq!(core.state().outcome.as_deref(), Some("completed"));
        assert_eq!(!core.state().dialogue_hidden, expected);
    }
}
#[test]
fn shared_episode_calls_keep_independent_random_draws_and_restore_the_live_frame() {
    use nir_core::{Core, CoreInput, ValidatedProgram};
    let (_temp, mut a) = route::adapter();
    a.share_episodes = true;
    for name in ["episode1", "episode2"] {
        let mut blocks = vec![];
        a.op(
            &mut blocks,
            json!({"type":"random","target":"affection","min":0,"max":1000}),
        );
        a.finish_function(name, blocks, json!({"type":"return"}));
    }
    assert_eq!(a.functions.len(), 1);
    let shared = a.function_aliases["episode1"].clone();
    assert_eq!(shared, a.function_aliases["episode2"]);
    let mut p: nir_format::Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    p.functions.insert(
        shared.clone(),
        serde_json::from_value(a.functions[&shared].clone()).unwrap(),
    );
    p.functions.insert(
        "main".into(),
        serde_json::from_value(json!({"entry":"first","blocks":{
            "first":{"terminator":{"type":"call","function":shared,"next":"second"}},
            "second":{"terminator":{"type":"call","function":shared,"next":"done"}},
            "done":{"terminator":{"type":"end","outcome":"completed"}}
        }}))
        .unwrap(),
    );
    let mut separate = p.clone();
    let function = separate.functions.remove(&shared).unwrap();
    for name in ["original.first", "original.second"] {
        let mut copy = function.clone();
        for block in copy.blocks.values_mut() {
            for op in &mut block.ops {
                op.id.push_str(name);
            }
        }
        separate.functions.insert(name.into(), copy);
    }
    for (block, name) in [("first", "original.first"), ("second", "original.second")] {
        let nir_format::Terminator::Call { function, .. } = &mut separate
            .functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut(block)
            .unwrap()
            .terminator
        else {
            panic!()
        };
        *function = name.into();
    }
    let validated = ValidatedProgram::new(p).unwrap();
    let mut live = Core::new(validated.clone(), "intern".into(), "en".into()).unwrap();
    live.step(CoreInput::None, 3);
    let mut restored = Core::restore(validated, live.snapshot(), "intern").unwrap();
    live.step(CoreInput::None, 100);
    restored.step(CoreInput::None, 100);
    let mut reference = Core::new(
        ValidatedProgram::new(separate).unwrap(),
        "intern".into(),
        "en".into(),
    )
    .unwrap();
    reference.step(CoreInput::None, 100);
    for core in [&live, &restored, &reference] {
        assert!(core.state().fault.is_none());
        assert_eq!(core.state().outcome.as_deref(), Some("completed"));
    }
    assert_eq!(live.state().variables, reference.state().variables);
    assert_eq!(restored.state().variables, reference.state().variables);
    assert_eq!(
        serde_json::to_value(&live.state().rng).unwrap(),
        serde_json::to_value(&reference.state().rng).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&restored.state().rng).unwrap(),
        serde_json::to_value(&reference.state().rng).unwrap()
    );
}
#[test]
fn source_text_identity_survives_recompilation_but_distinguishes_pages_and_commands() {
    let (_temp, mut a) = route::adapter();
    a.defaults = Some(stock_defaults());
    let glyphs = [Glyph::Char("Neutral.".into())];
    let mut blocks = vec![];
    a.page(&glyphs, &mut blocks).unwrap();
    let first = a.texts.keys().next().unwrap().clone();
    a.page(&glyphs, &mut blocks).unwrap();
    assert_eq!(a.texts.len(), 1);
    a.page_ordinal = 1;
    a.page(&glyphs, &mut blocks).unwrap();
    assert_eq!(a.texts.len(), 2);
    a.page_ordinal = 0;
    a.location.index += 1;
    a.location.line += 1;
    a.page(&glyphs, &mut blocks).unwrap();
    assert_eq!(a.texts.len(), 3);
    assert_ne!(a.source_map[&first].index, a.location.index);
    // Scene content sharing does not remove any authored commit: every
    // activation still obtains its own runtime generation and task.
    let mut scenes = vec![];
    a.scene(&mut scenes, 0);
    a.scene(&mut scenes, 0);
    assert_eq!(a.scenes.len(), 1);
    assert_eq!(scenes.len(), 2);
}
#[test]
fn unread_anchors_share_until_an_image_change_requires_their_position_rule() {
    let (_temp, mut a) = route::adapter();
    let mut first = RouteState {
        nodes: BTreeMap::from([(
            "actor".into(),
            serde_json::from_value(
                json!({"id":"actor","asset":"image.first","x":10,"y":20,"width":32,"height":24}),
            )
            .unwrap(),
        )]),
        committed_images: BTreeSet::from(["actor".into()]),
        centered: BTreeSet::new(),
        anchors: BTreeMap::from([("actor".into(), ("C".into(), "B".into()))]),
        audio: BTreeMap::new(),
        motion_tasks: BTreeMap::new(),
        movie_tasks: BTreeMap::new(),
        returns: vec![],
        pending_visibility: None,
    };
    let mut second = first.clone();
    second
        .anchors
        .insert("actor".into(), ("10".into(), "20".into()));
    let none = BTreeSet::new();
    let key = |s: &RouteState, reads| {
        s.signature_for_route(&none, &none, &none, Some(reads))
            .unwrap()
    };
    assert_eq!(key(&first, &none), key(&second, &none));
    a.nodes = first.nodes.clone();
    a.anchors = first.anchors.clone();
    a.committed_images = first.committed_images.clone();
    a.anchor_reads = Some(none.clone());
    let args = ["CHANGECG", "actor", "$000000", "NORMAL", "0", "0"].map(str::to_owned);
    let mut blocks = vec![];
    let error = a.event(&args, &mut blocks).unwrap_err();
    assert_eq!(
        error.downcast_ref::<AnchorReadRequired>().unwrap().0,
        "actor"
    );
    assert!(blocks.is_empty());
    let reads = BTreeSet::from(["actor".into()]);
    assert_ne!(key(&first, &reads), key(&second, &reads));
    a.anchor_reads = Some(reads);
    a.event(&args, &mut blocks).unwrap();
    let centered = [a.nodes["actor"].x, a.nodes["actor"].y];
    first.anchors = second.anchors;
    a.anchors = first.anchors;
    a.event(&args, &mut blocks).unwrap();
    assert_eq!([a.nodes["actor"].x, a.nodes["actor"].y], [10., 20.]);
    assert_ne!(centered, [10., 20.]);
}
#[test]
fn route_images_keep_live_rectangles_but_refine_any_source_layout_read() {
    let (_temp, mut a) = route::adapter();
    a.nodes.insert(
        "actor".into(),
        serde_json::from_value(
            json!({"id":"actor","asset":"image.first","x":10,"y":20,"width":32,"height":24}),
        )
        .unwrap(),
    );
    a.anchors.insert("actor".into(), ("C".into(), "B".into()));
    let mut blocks = vec![];
    a.scene(&mut blocks, 0);
    let state = |a: &Adapter| RouteState {
        nodes: a.nodes.clone(),
        committed_images: a.committed_images.clone(),
        centered: a.centered.clone(),
        anchors: a.anchors.clone(),
        audio: a.audio.clone(),
        motion_tasks: a.motion_tasks.clone(),
        movie_tasks: a.movie_tasks.clone(),
        returns: a.returns.clone(),
        pending_visibility: a.pending_visibility,
    };
    let before = state(&a);
    let node = a.nodes.get_mut("actor").unwrap();
    node.asset = Some("image.second".into());
    [node.x, node.y, node.width, node.height] = [100., 200., 64., 48.];
    let after = state(&a);
    let none = BTreeSet::new();
    let key = |s: &RouteState, reads| s.signature_for_route(&none, &none, reads, None).unwrap();
    assert_eq!(key(&before, &none), key(&after, &none));
    let error = a.geometry_extent("actor").unwrap_err();
    assert_eq!(
        error.downcast_ref::<GeometryReadRequired>().unwrap().0,
        "actor"
    );
    a.geometry_reads.insert("actor".into());
    assert_ne!(
        key(&before, &a.geometry_reads),
        key(&after, &a.geometry_reads)
    );
    assert_eq!(a.geometry_extent("actor").unwrap(), [64., 48.]);
    a.geometry_reads.clear();
    blocks.clear();
    a.scene(&mut blocks, 0);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        a.cues[cue]["effects"][0]["effect"]["inherit_image_geometry"],
        json!(["actor"])
    );
    a.committed_images.remove("actor");
    blocks.clear();
    a.scene(&mut blocks, 0);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert!(a.cues[cue]["effects"][0]["effect"]
        .get("inherit_image_geometry")
        .is_none());
}
#[test]
fn route_images_merge_committed_assets_and_keep_new_assignments_explicit() {
    let (_temp, mut adapter) = route::adapter();
    adapter.nodes.insert(
        "photo".into(),
        serde_json::from_value(
            json!({"id":"photo","asset":"image.first","x":10,"y":20,"width":32,"height":24}),
        )
        .unwrap(),
    );
    let script = route::script(vec![route::exit(0)]);
    let mut routes = Routes {
        replay: false,
        episodes: vec![],
        blocks: BTreeMap::new(),
        entries: BTreeMap::new(),
        queue: vec![],
        scripts: BTreeMap::new(),
        variants: BTreeMap::new(),
        states: BTreeMap::new(),
        choice_sites: 0,
    };
    let mut blocks = Vec::new();
    adapter.scene(&mut blocks, 0);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert!(adapter.cues[cue]["effects"][0]["effect"]
        .get("inherit_images")
        .is_none());
    let first = adapter
        .route_block(&script, "00000001.lsb", 0, &mut routes)
        .unwrap();
    adapter.nodes.get_mut("photo").unwrap().asset = Some("image.second".into());
    assert_eq!(
        first,
        adapter
            .route_block(&script, "00000001.lsb", 0, &mut routes)
            .unwrap()
    );
    adapter.committed_images.remove("photo");
    assert_ne!(
        first,
        adapter
            .route_block(&script, "00000001.lsb", 0, &mut routes)
            .unwrap()
    );
    blocks.clear();
    adapter.scene(&mut blocks, 0);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    let effect = &adapter.cues[cue]["effects"][0]["effect"];
    assert!(effect.get("inherit_images").is_none());
    assert_eq!(
        adapter.scenes[effect["scene"].as_str().unwrap()][0]
            .asset
            .as_deref(),
        Some("image.second")
    );
    blocks.clear();
    adapter.scene(&mut blocks, 500000);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        adapter.cues[cue]["effects"][0]["effect"]["inherit_images"],
        json!(["photo"])
    );
}

#[test]
fn locked_thumbnail_is_a_distinct_black_variant() {
    let temp = tempfile::tempdir().unwrap();
    let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
    adapter.assets.insert(
        "thumbnail".into(),
        Asset {
            source: "fixture.gal".into(),
            gain: None,
            size: [2, 1],
            blackened: false,
        },
    );
    let locked = adapter.locked_image("thumbnail");
    assert_ne!(locked, "thumbnail");
    assert!(adapter.assets[&locked].blackened);
    assert!(!adapter.assets["thumbnail"].blackened);
    let mut image =
        image::RgbaImage::from_raw(2, 1, vec![90, 120, 255, 255, 50, 60, 70, 34]).unwrap();
    media::blacken(&mut image);
    assert_eq!(image.as_raw(), &[0, 0, 0, 255, 0, 0, 0, 34]);
}

#[test]
fn source_defaults_keep_units_and_only_export_selected_author_settings() {
    use super::super::lsb::tests::settings_file;
    let settings = [
        ("StatusAutoTextWait", 3000),
        ("StatusBGMVolume", 750),
        ("StatusVoiceVolume", 1000),
        ("StatusSEVolume", 0),
        ("StatusTextSpeed", 128),
    ];
    let bytes = settings_file(&settings);
    let defaults = ImportedDefaults::parse(&bytes).unwrap();
    assert_eq!(defaults.auto_wait_ms, 3000);
    assert_eq!(defaults.bgm_volume, 0.75);
    assert_eq!(defaults.voice_volume, 1.);
    assert_eq!(defaults.sfx_volume, 0.);
    assert_eq!(defaults.text_speed_ms, 128);
    assert_eq!(defaults.source_sha256, nir_content::digest(&bytes));
    assert!(!serde_json::to_string(&defaults)
        .unwrap()
        .contains("author-project-directory"));
    for (index, value) in [(0, -1), (0, 30_001), (1, 1001), (2, -1), (4, -1), (4, 641)] {
        let mut broken = settings;
        broken[index].1 = value;
        assert!(ImportedDefaults::parse(&settings_file(&broken)).is_err());
    }
    assert!(ImportedDefaults::parse(&settings_file(&settings[..4])).is_err());
}
#[test]
fn imported_menu_composition_preserves_geometry_variants_and_guards() {
    let element = menu_element(ImageButton {
        id: "begin".into(),
        label: "Begin".into(),
        asset: "normal".into(),
        hover_asset: Some("hover".into()),
        locked_asset: Some("locked".into()),
        rect: [17., 29., 101., 43.],
        action: ImageMenuAction::NewGame,
        requires: Some("seen".into()),
    });
    let encoded = serde_json::to_value(&element).unwrap();
    let toml = toml::to_string(&encoded).unwrap();
    let decoded: MenuElement = toml::from_str(&toml).unwrap();
    assert_eq!(decoded.rect, element.rect);
    assert_eq!(element.rect, [17., 29., 101., 43.]);
    assert_eq!(element.scale, 1.);
    let MenuContent::Button {
        asset,
        hover_asset,
        locked_asset,
        requires,
        action,
        ..
    } = element.content
    else {
        panic!()
    };
    assert_eq!(asset, "normal");
    assert_eq!(hover_asset.as_deref(), Some("hover"));
    assert_eq!(locked_asset.as_deref(), Some("locked"));
    assert_eq!(requires.as_deref(), Some("seen"));
    assert!(matches!(action, ImageMenuAction::NewGame));
}
#[test]
fn menu106_reads_coordinates_hover_and_rejects_truncation() {
    use super::super::lsb::tests::{string, u32b};
    let mut data = b"LivePrevMenu106".to_vec();
    data.extend([0; 8]);
    u32b(&mut data, 1);
    u32b(&mut data, 100);
    u32b(&mut data, 200);
    string(&mut data, "button.gal");
    data.push(0);
    string(&mut data, "Begin");
    string(&mut data, "hover.gal");
    for _ in 0..10 {
        string(&mut data, "");
    }
    data.extend([0; 8]);
    string(&mut data, "");
    for _ in 0..5 {
        string(&mut data, "");
    }
    data.extend([0; 4]);
    string(&mut data, "");
    string(&mut data, "");
    let parsed = menu(&data).unwrap();
    assert_eq!((parsed[0].x, parsed[0].y), (100, 200));
    assert_eq!(parsed[0].selected, "hover.gal");
    for n in [0, 14, 25, data.len() - 1] {
        assert!(menu(&data[..n]).is_err());
    }
}
/// Shared builder for route-walk fixtures: labels, unconditional calls,
/// 選択値 dispatch jumps, exits and plain text commands assembled in
/// memory (no source tree needed for the walk itself).
mod route {
    use super::super::super::lsb::{Command, Novel, Reference};
    use super::*;
    pub(super) fn expr(op: u8, name: &str, args: Vec<Literal>) -> Expression {
        Expression {
            literal: None,
            operations: vec![(op, name.into(), args)],
            functions: BTreeMap::new(),
        }
    }
    pub(super) fn int(n: i32) -> Expression {
        Expression {
            literal: Some(Literal::Int(n)),
            operations: vec![],
            functions: BTreeMap::new(),
        }
    }
    pub(super) fn flag() -> Expression {
        int(1)
    }
    /// The exact stock dispatch shape: one literal is assigned, compared
    /// with the 選択値 variable, and the result is returned.
    pub(super) fn selection(literal: &str) -> Expression {
        Expression {
            literal: None,
            operations: vec![
                (1, "____0".into(), vec![Literal::String(literal.into())]),
                (
                    12,
                    "____2".into(),
                    vec![
                        Literal::Variable("選択値".into()),
                        Literal::Variable("____0".into()),
                    ],
                ),
                (1, "____arg".into(), vec![Literal::Variable("____2".into())]),
            ],
            functions: BTreeMap::new(),
        }
    }
    pub(super) fn command(kind: u8, line: u32, body: Body) -> Command {
        Command {
            kind,
            indent: 0,
            muted: false,
            not_update: false,
            line,
            offset: 0,
            body,
        }
    }
    pub(super) fn label(line: u32) -> Command {
        command(3, line, Body::Label(String::new()))
    }
    pub(super) fn text(line: u32, s: &str) -> Command {
        command(
            20,
            line,
            Body::Text {
                text: Box::new(Novel {
                    glyphs: vec![Glyph::Char(s.into())],
                    counts: BTreeMap::new(),
                    events: BTreeMap::new(),
                    has_conditions_or_links: false,
                    has_ruby: false,
                    ..Novel::default()
                }),
                target: Expression {
                    literal: Some(Literal::String("メッセージボックス".into())),
                    operations: vec![],
                    functions: BTreeMap::new(),
                },
                history: flag(),
                wait: flag(),
                stop: int(0),
            },
        )
    }
    pub(super) fn exit(line: u32) -> Command {
        command(6, line, Body::Exit(flag()))
    }
    pub(super) fn dispatch(line: u32, literal: &str, target_line: u32) -> Command {
        command(
            4,
            line,
            Body::Jump(
                Reference {
                    page: String::new(),
                    line: target_line,
                },
                selection(literal),
            ),
        )
    }
    pub(super) fn choice_call(line: u32) -> Command {
        command(
            5,
            line,
            Body::Call {
                target: Reference {
                    page: "ノベルシステム\\選択メニュー\\■選択実行.lsb".into(),
                    line: 0,
                },
                condition: flag(),
                has_params: true,
                params: vec![],
            },
        )
    }
    pub(super) fn script(commands: Vec<Command>) -> Script {
        Script {
            version: 116,
            source_sha256: "0".repeat(64),
            commands,
        }
    }
    pub(super) fn adapter() -> (tempfile::TempDir, Adapter) {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        adapter.defaults = Some(super::stock_defaults());
        (temp, adapter)
    }
    pub(super) fn walk(adapter: &mut Adapter, script: &Script) -> Result<Routes> {
        walk_cached(adapter, script, BTreeMap::new())
    }
    pub(super) fn walk_cached(
        adapter: &mut Adapter,
        script: &Script,
        scripts: BTreeMap<String, std::sync::Arc<Script>>,
    ) -> Result<Routes> {
        let mut routes = Routes {
            replay: false,
            episodes: vec![],
            blocks: BTreeMap::new(),
            entries: BTreeMap::new(),
            queue: vec![],
            scripts,
            variants: BTreeMap::new(),
            states: BTreeMap::new(),
            choice_sites: 0,
        };
        adapter.walk_routes(script, "00000001.lsb", 0, &mut routes)?;
        Ok(routes)
    }
}

#[test]
fn window_reservation_is_consumed_by_next_scene_and_direct_flips_replace_it() {
    let (_dir, mut a) = route::adapter();
    let mut blocks = vec![];
    a.event(&["MESOFF".into(), "-1".into()], &mut blocks)
        .unwrap();
    assert!(blocks.is_empty());
    assert_eq!(a.pending_visibility, Some(false));
    a.scene(&mut blocks, 200_000);
    let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
    assert_eq!(
        a.cues[cue]["effects"][0]["effect"]["dialogue_visible"],
        false
    );
    assert_eq!(a.pending_visibility, None);
    a.event(&["MESON".into(), "-1".into()], &mut blocks)
        .unwrap();
    a.event(&["MESOFF".into(), "0".into()], &mut blocks)
        .unwrap();
    assert_eq!(a.pending_visibility, None);
    for field in [vec!["MESON".into()], vec!["MESON".into(), "".into()]] {
        a.event(&field, &mut blocks).unwrap();
    }
    assert!(a
        .event(&["MESON".into(), "-2".into()], &mut blocks)
        .is_err());
    assert!(a
        .event(&["MESON".into(), "60001".into()], &mut blocks)
        .is_err());
}
#[test]
fn authored_calls_keep_nested_and_repeated_return_sites_separate() {
    use route::{adapter, command, exit, flag, script, walk_cached};
    let call = |line, page: &str| {
        command(
            5,
            line,
            Body::Call {
                target: Reference {
                    page: page.into(),
                    line: 0,
                },
                condition: flag(),
                has_params: false,
                params: vec![],
            },
        )
    };
    let assign = |line, n| {
        command(
            14,
            line,
            Body::Calc(Expression {
                literal: None,
                functions: BTreeMap::new(),
                operations: vec![(1, "counter".into(), vec![Literal::Int(n)])],
            }),
        )
    };
    let main = script(vec![
        call(0, "first.lsb"),
        assign(1, 2),
        call(2, "first.lsb"),
        assign(3, 3),
        exit(4),
    ]);
    let first = script(vec![call(0, "second.lsb"), exit(1)]);
    let second = script(vec![assign(0, 1), exit(1)]);
    let (_dir, mut a) = adapter();
    let routes = walk_cached(
        &mut a,
        &main,
        BTreeMap::from([
            ("first.lsb".into(), std::sync::Arc::new(first)),
            ("second.lsb".into(), std::sync::Arc::new(second)),
        ]),
    )
    .unwrap();
    let mut id = routes.entries[&("00000001.lsb".into(), 0)].clone();
    let mut writes = vec![];
    for _ in 0..32 {
        let block = &routes.blocks[&id];
        for op in block["ops"].as_array().unwrap() {
            if op["operation"]["target"] == "counter" {
                writes.push(op["operation"]["value"]["value"]["value"].as_i64().unwrap());
            }
        }
        let term = &block["terminator"];
        if term["type"] == "end" {
            assert_eq!(term["outcome"], "completed");
            break;
        }
        assert_eq!(term["type"], "goto");
        id = term["target"].as_str().unwrap().into();
    }
    assert_eq!(writes, [1, 2, 1, 3]);
    assert!(a.returns.is_empty());
    assert_eq!(
        routes
            .variants
            .keys()
            .filter(|(p, pc, _)| p == "second.lsb" && *pc == 0)
            .count(),
        2
    );
}
#[test]
fn authored_call_recursion_is_bounded_and_system_calls_are_not_admitted() {
    use route::{adapter, command, exit, flag, script, walk};
    let call = |page: &str| {
        command(
            5,
            0,
            Body::Call {
                target: Reference {
                    page: page.into(),
                    line: 0,
                },
                condition: flag(),
                has_params: false,
                params: vec![],
            },
        )
    };
    let (_dir, mut a) = adapter();
    let recursive = script(vec![call(""), exit(1)]);
    assert!(walk(&mut a, &recursive)
        .unwrap_err()
        .to_string()
        .contains("call depth exceeds 64"));
    let unauthorized_system = script(vec![call("ノベルシステム/未対応.lsb"), exit(1)]);
    assert!(walk(&mut a, &unauthorized_system)
        .unwrap_err()
        .to_string()
        .contains("unexpected scenario call"));
}

/// The dispatch predicate accepts exactly one literal compared with the
/// 選択値 variable and rejects anything looser.
#[test]
fn replay_bootstrap_requires_one_unconditional_reset_after_clear() {
    use route::{command, flag, int, script};
    let call = |page: &str| {
        command(
            5,
            0,
            Body::Call {
                target: Reference {
                    page: page.into(),
                    line: 0,
                },
                condition: flag(),
                has_params: true,
                params: vec![],
            },
        )
    };
    let good = script(vec![
        command(
            13,
            0,
            Body::Flip {
                parameters: [("act".into(), int(1)), ("delete".into(), int(1))].into(),
                targets: vec![],
            },
        ),
        call("変数初期化.lsb"),
        call("シーン回想.lsb"),
    ]);
    verify_replay_reset(&good).unwrap();
    let mut nested = good.clone();
    nested.commands[1].indent = 1;
    assert!(verify_replay_reset(&nested).is_err());
    let mut conditional_clear = good.clone();
    conditional_clear.commands[0].indent = 1;
    assert!(verify_replay_reset(&conditional_clear).is_err());
    let mut reversed = good.clone();
    reversed.commands.swap(1, 2);
    assert!(verify_replay_reset(&reversed).is_err());
    let mut duplicated = good.clone();
    duplicated.commands.push(call("変数初期化.lsb"));
    assert!(verify_replay_reset(&duplicated).is_err());
}

#[test]
fn replay_dispatch_uses_authored_ordinals_and_rejects_gaps() {
    use route::{command, flag, script};
    let pair = |ordinal: i32, first: bool| {
        let var = |s: &str| Literal::Variable(s.into());
        let guard = command(
            if first { 0 } else { 1 },
            0,
            Body::Condition(Expression {
                literal: None,
                operations: vec![
                    (10, "____0".into(), vec![var("@ParamStr"), Literal::Int(0)]),
                    (1, "____1".into(), vec![var("____0")]),
                    (11, "____2".into(), vec![var("____1")]),
                    (
                        12,
                        "____3".into(),
                        vec![var("____2"), Literal::Int(ordinal)],
                    ),
                    (1, "____arg".into(), vec![var("____3")]),
                ],
                functions: [(2, 37)].into(),
            }),
        );
        let mut call = command(
            5,
            0,
            Body::Call {
                target: Reference {
                    page: "story.lsb".into(),
                    line: ordinal as u32 + 10,
                },
                condition: flag(),
                has_params: true,
                params: vec![],
            },
        );
        call.indent = 1;
        vec![guard, call]
    };
    let good = script([pair(1, true), pair(0, false)].concat());
    let map = replay_dispatch(&good).unwrap();
    assert_eq!(map[&0].line, 10);
    assert_eq!(map[&1].line, 11);
    assert!(replay_dispatch(&script(pair(1, true))).is_err());
    assert!(replay_dispatch(&script([pair(0, true), pair(0, false)].concat())).is_err());
    assert!(replay_dispatch(&script(vec![])).unwrap().is_empty());
    let mut nested = good;
    nested.commands[0].indent = 1;
    assert!(replay_dispatch(&nested).is_err());
}

#[test]
fn title_unlock_filters_use_persisted_flags_and_refuse_ordinary_variables() {
    let (_temp, mut adapter) = route::adapter();
    for separator in ["/", "\\"] {
        let title = route::script(vec![route::command(
            5,
            0,
            Body::Call {
                target: Reference {
                    page: ["ノベルシステム", "プレビューメニュー", "■選択実行.lsb"].join(separator),
                    line: 0,
                },
                condition: route::flag(),
                has_params: false,
                params: vec![],
            },
        )]);
        assert!(title_filters(&mut adapter, &title).unwrap().is_empty());
    }
    adapter
        .status_flags
        .insert("second_run".into(), "fixture.second_run".into());
    let eq = json!({"type":"binary","op":"eq","left":{"type":"var","name":"second_run"},
        "right":{"type":"const","value":{"type":"i32","value":1}}});
    let hidden = json!({"type":"binary","op":"and","left":{"type":"const","value":{"type":"bool","value":true}},
        "right":{"type":"not","value":eq}});
    let guard = title_profile_guard(&adapter, &hidden, false).unwrap();
    let mut element = menu_element(ImageButton {
        id: "bonus".into(),
        label: "Bonus".into(),
        asset: "fixture.button".into(),
        hover_asset: None,
        locked_asset: None,
        rect: [0., 0., 100., 40.],
        action: ImageMenuAction::Entry {
            function: "fixture.bonus".into(),
        },
        requires: None,
    });
    element.visible_when.push(guard);
    let menu = ImageMenu {
        builtin_navigation: true,
        story_exports: BTreeMap::new(),
        locals: BTreeMap::new(),
        elements: vec![element],
        background: "fixture.background".into(),
        buttons: vec![],
        effects: None,
    };
    assert!(
        !menu
            .element_state(
                "bonus",
                &BTreeMap::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeMap::new(),
                false
            )
            .0
    );
    assert!(
        menu.element_state(
            "bonus",
            &BTreeMap::new(),
            &BTreeSet::from(["fixture.second_run".into()]),
            &BTreeSet::new(),
            &BTreeMap::new(),
            false
        )
        .0
    );
    adapter.status_flags.clear();
    assert!(title_profile_guard(&adapter, &hidden, false).is_err());
}

#[test]
fn replay_entry_resolves_external_labels_without_replaying_the_whole_chart() {
    use route::{adapter, exit, label, script, text};
    let (temp, mut adapter) = adapter();
    std::fs::write(temp.path().join("second.lsb"), []).unwrap();
    let chart = script(vec![
        text(1, "Before replay entry"),
        label(38),
        text(39, "Selected replay"),
        text(40, "Following episode"),
        exit(41),
    ]);
    adapter
        .source
        .scripts
        .insert("second.lsb".into(), std::sync::Arc::new(chart));
    let mut routes = Routes {
        replay: true,
        episodes: vec![],
        blocks: BTreeMap::new(),
        entries: BTreeMap::new(),
        queue: vec![],
        scripts: BTreeMap::new(),
        variants: BTreeMap::new(),
        states: BTreeMap::new(),
        choice_sites: 0,
    };
    let target = Reference {
        page: "second.lsb".into(),
        line: 38,
    };
    let (page, chart, pc) = adapter
        .route_target("00000001.lsb", &target, &mut routes)
        .unwrap();
    assert_eq!(page, "second.lsb");
    adapter.walk_routes(&chart, &page, pc, &mut routes).unwrap();
    assert_eq!(routes.episodes.len(), 1);
    assert_eq!(adapter.texts.len(), 1);
    assert!(routes
        .blocks
        .values()
        .any(|block| { block["terminator"]["outcome"] == "replay_completed" }));
    assert!(adapter
        .route_target(
            "00000001.lsb",
            &Reference { line: 99, ..target },
            &mut routes,
        )
        .is_err());
}

#[test]
fn cross_chart_jumps_do_not_merge_equal_command_indices() {
    use route::{adapter, command, exit, flag, label, script, text};
    let (temp, mut adapter) = adapter();
    std::fs::write(temp.path().join("second.lsb"), []).unwrap();
    let a = script(vec![
        label(10),
        text(11, "First chart"),
        command(
            4,
            12,
            Body::Jump(
                Reference {
                    page: "second.lsb".into(),
                    line: 10,
                },
                flag(),
            ),
        ),
    ]);
    let b = script(vec![label(10), text(11, "Second chart"), exit(12)]);
    adapter
        .source
        .scripts
        .insert("second.lsb".into(), std::sync::Arc::new(b));
    let routes = route::walk(&mut adapter, &a).unwrap();
    assert_eq!(routes.episodes.len(), 2);
    assert_eq!(routes.scripts.len(), 2);
    let calls: BTreeSet<_> = routes
        .blocks
        .values()
        .filter_map(|b| b["terminator"]["function"].as_str())
        .collect();
    assert_eq!(calls, BTreeSet::from(["episode1", "episode2"]));
}

#[test]
fn exhaustive_inline_choices_skip_only_the_impossible_dispatch_fallthrough() {
    use route::{adapter, command, exit, flag, label, script, text};
    let empty = Expression {
        literal: None,
        operations: vec![],
        functions: BTreeMap::new(),
    };
    let wait = Expression {
        literal: None,
        operations: vec![
            (
                11,
                "____0".into(),
                vec![Literal::Variable("選択実行中".into())],
            ),
            (
                11,
                "____1".into(),
                vec![
                    Literal::String("メッセージボックス".into()),
                    Literal::Int(138),
                ],
            ),
            (
                8,
                "____2".into(),
                vec![
                    Literal::Variable("____1".into()),
                    Literal::Variable("____0".into()),
                ],
            ),
            (1, "____arg".into(), vec![Literal::Variable("____2".into())]),
        ],
        functions: BTreeMap::from([(0, 20), (1, 2)]),
    };
    let mut commands = vec![command(7, 0, Body::Wait(vec![wait, flag(), flag()]))];
    // The stock timer values are zero, independently of the wait predicate.
    if let Body::Wait(args) = &mut commands[0].body {
        args[1] = Expression {
            literal: Some(Literal::Int(0)),
            operations: vec![],
            functions: BTreeMap::new(),
        };
        args[2] = args[1].clone();
    }
    for (name, ty) in [("_tmp", 4), ("_tmpno", 1), ("_tmpid", 4)] {
        commands.push(command(
            15,
            commands.len() as u32,
            Body::Variable {
                name: name.into(),
                value_type: ty,
                initial: empty.clone(),
                scope: 2,
            },
        ));
    }
    for option in ["Alpha", "Beta"] {
        commands.push(command(
            14,
            commands.len() as u32,
            Body::Calc(Expression {
                literal: None,
                operations: vec![
                    (
                        11,
                        "____0".into(),
                        vec![
                            Literal::Variable("_tmp".into()),
                            Literal::String(option.into()),
                        ],
                    ),
                    (1, "____arg".into(), vec![Literal::Variable("____0".into())]),
                ],
                functions: BTreeMap::from([(0, 66)]),
            }),
        ));
    }
    let zero = Expression {
        literal: Some(Literal::Int(0)),
        operations: vec![],
        functions: BTreeMap::new(),
    };
    commands.push(command(
        5,
        commands.len() as u32,
        Body::Call {
            target: Reference {
                page: CHOICE_EXECUTOR.into(),
                line: 0,
            },
            condition: flag(),
            has_params: true,
            params: vec![zero; 8],
        },
    ));
    for name in ["_tmp", "_tmpno", "_tmpid"] {
        commands.push(command(
            16,
            commands.len() as u32,
            Body::VariableDelete(name.into()),
        ));
    }
    let dispatch_index = commands.len();
    commands.push(route::dispatch(10, "Alpha", 100));
    commands.push(route::dispatch(11, "Beta", 200));
    commands.push(command(45, 12, Body::Other));
    commands.extend([
        label(100),
        text(101, "Alpha route"),
        exit(102),
        label(200),
        text(201, "Beta route"),
        exit(202),
    ]);
    let source = script(commands);
    let (_temp, mut a) = adapter();
    let routes = route::walk(&mut a, &source).unwrap();
    assert_eq!(routes.episodes.len(), 2);
    assert!(!routes
        .blocks
        .values()
        .any(|b| b["terminator"]["type"] == "branch"));
    assert!(routes
        .blocks
        .values()
        .any(|b| b["terminator"]["code"] == "E_IMPORT_CHOICE_EMPTY"));
    let mut incomplete = source;
    incomplete.commands[dispatch_index] = route::dispatch(10, "Unmatched", 100);
    let (_temp, mut a) = adapter();
    assert!(route::walk(&mut a, &incomplete).is_err());
}

#[test]
fn authored_title_return_does_not_require_a_prior_dialogue() {
    let (_temp, mut adapter) = route::adapter();
    adapter.title_route_entries.insert(1);
    let script = route::script(vec![
        route::command(
            4,
            0,
            Body::Jump(
                Reference {
                    page: String::new(),
                    line: 10,
                },
                route::flag(),
            ),
        ),
        route::label(10),
        // The native title dispatcher is replaced by NIR and must not be
        // executed merely because this story entry has read no paragraph.
        route::command(45, 11, Body::Other),
    ]);
    let routes = route::walk(&mut adapter, &script).unwrap();
    assert!(routes.episodes.is_empty());
    assert!(routes
        .blocks
        .values()
        .any(|block| block["terminator"]["outcome"] == "completed"));
}

#[test]
fn external_chart_can_return_to_the_initial_title_dispatch() {
    use route::{adapter, command, flag, label, script, text};
    let (temp, mut adapter) = adapter();
    std::fs::write(temp.path().join("second.lsb"), []).unwrap();
    adapter.title_route_entries.insert(0);
    let jump = |page: &str| {
        command(
            4,
            12,
            Body::Jump(
                Reference {
                    page: page.into(),
                    line: 10,
                },
                flag(),
            ),
        )
    };
    let a = script(vec![label(10), text(11, "First chart"), jump("second.lsb")]);
    let b = script(vec![
        label(10),
        text(11, "Second chart"),
        jump("00000001.lsb"),
    ]);
    adapter
        .source
        .scripts
        .insert("second.lsb".into(), std::sync::Arc::new(b));
    let routes = route::walk(&mut adapter, &a).unwrap();
    assert_eq!(routes.episodes.len(), 2);
    assert!(routes.blocks.values().any(|b| {
        b["terminator"]["type"] == "end" && b["terminator"]["outcome"] == "completed"
    }));
}

#[test]
fn dialogue_variables_use_typed_slots_without_substituting_import_defaults() {
    let (_temp, mut adapter) = route::adapter();
    adapter
        .variables
        .insert("Counter".into(), json!({"type":"i32","value":17}));
    let mut blocks = vec![];
    adapter
        .page(
            &[
                Glyph::Char("Total: ".into()),
                Glyph::Variable("Counter".into()),
            ],
            &mut blocks,
        )
        .unwrap();
    let doc = adapter.texts.values().next().unwrap();
    assert!(matches!(&doc.spans[1], Span::Param { name, .. } if name == "Counter"));
    assert!(adapter
        .page(&[Glyph::Variable("Undeclared".into())], &mut vec![])
        .is_err());
    adapter
        .status_values
        .insert("Counter".into(), "counter".into());
    let mut persistent = vec![];
    adapter
        .page(&[Glyph::Variable("Counter".into())], &mut persistent)
        .unwrap();
    assert!(persistent
        .iter()
        .any(|b| b["ops"][0]["operation"]["type"] == "profile_value_read"));
}

#[test]
fn empty_subroutine_never_swallows_an_assignment_or_nested_exit() {
    use route::{command, exit, int, label, script};
    let empty = script(vec![label(1), exit(2)]);
    assert!(empty_subroutine(&empty, 0));
    let mut nested = empty.clone();
    nested.commands[1].indent = 1;
    assert!(!empty_subroutine(&nested, 0));
    let writes = script(vec![label(1), command(14, 2, Body::Calc(int(1))), exit(3)]);
    assert!(!empty_subroutine(&writes, 0));
}

#[test]
fn selection_dispatch_matches_one_string_literal_only() {
    use route::{expr, selection};
    assert_eq!(selection_dispatch(&selection("甲")), Some("甲"));
    assert_eq!(
        selection_dispatch(&expr(
            11,
            "____arg",
            vec![
                Literal::Variable("選択値".into()),
                Literal::String("甲".into())
            ]
        )),
        None,
        "op 11 is not the stock equality"
    );
    assert_eq!(
        selection_dispatch(&expr(
            12,
            "____arg",
            vec![
                Literal::Variable("別の変数".into()),
                Literal::String("甲".into())
            ]
        )),
        None,
        "comparisons on other variables do not dispatch choices"
    );
    let mut two = selection("甲");
    two.operations[0].2.push(Literal::String("乙".into()));
    assert_eq!(selection_dispatch(&two), None, "ambiguous literal");
}

/// A stock choice site lowers onto the typed interaction core: one
/// Interact whose branch targets continue the winning route, the option
/// value is the option text the source callback commits, and the result
/// variable is declared for the VM-owned typed write.
#[test]
fn choice_site_lowers_to_typed_interaction_with_route_branches() {
    let script = route::script(vec![
        route::label(10),
        route::text(11, "共通の導入。"),
        route::choice_call(12),
        route::dispatch(13, "synthetic-alpha", 20),
        route::dispatch(14, "synthetic-beta", 30),
        route::exit(15),
        route::label(20),
        route::text(21, "甲ルート。"),
        route::exit(22),
        route::label(30),
        route::text(31, "乙ルート。"),
        route::exit(32),
    ]);
    let (_temp, mut adapter) = route::adapter();
    let routes = route::walk(&mut adapter, &script).unwrap();
    assert_eq!(routes.choice_sites, 1);
    assert_eq!(routes.episodes.len(), 3);
    let interact = routes
        .blocks
        .values()
        .find(|b| b["terminator"]["type"] == "interact")
        .unwrap()["terminator"]
        .clone();
    let choice_id = interact["choice"].as_str().unwrap().to_owned();
    let definition = &adapter.choices[&choice_id];
    let options = definition["options"].as_array().unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["id"], "o0");
    assert_eq!(
        options[0]["value"],
        json!({"type":"string","value":"synthetic-alpha"})
    );
    assert_eq!(
        options[1]["value"],
        json!({"type":"string","value":"synthetic-beta"})
    );
    for option in options {
        let doc = &adapter.texts[option["text"].as_str().unwrap()];
        assert_eq!(doc.spans.len(), 1);
    }
    assert_eq!(
        adapter.variables["選択値"],
        json!({"type":"string","value":""})
    );
    assert_eq!(interact["result"], "選択値");
    assert_eq!(interact["on_empty"], "failed");
    // The interaction itself validates against the runtime schema.
    let terminator: nir_format::Terminator = serde_json::from_value(interact.clone()).unwrap();
    assert!(
        matches!(terminator, nir_format::Terminator::Interact { result, .. } if result.as_deref() == Some("選択値"))
    );
    let def: nir_format::Choice = serde_json::from_value(definition.clone()).unwrap();
    assert!(def.options.iter().all(|o| o.value.is_some()));
    // Branch targets continue at each route's episode call, and every
    // route reaches its own end.
    let branch_a = routes.blocks[interact["branches"]["o0"].as_str().unwrap()].clone();
    let branch_b = routes.blocks[interact["branches"]["o1"].as_str().unwrap()].clone();
    assert_eq!(branch_a["terminator"]["function"], "episode2");
    assert_eq!(branch_b["terminator"]["function"], "episode3");
    let ends: Vec<_> = routes
        .blocks
        .values()
        .filter(|b| b["terminator"]["type"] == "end")
        .collect();
    assert_eq!(ends.len(), 2);
    assert!(ends
        .iter()
        .all(|b| b["terminator"]["outcome"] == "completed"));
    // The unreachable dispatch fallthrough lowers to nothing.
    assert!(!routes
        .blocks
        .values()
        .any(|b| b["terminator"]["type"] == "fault"));
}

/// Converging branch targets merge into one continuation instead of
/// duplicating the shared route.
#[test]
fn converging_choice_branches_merge_into_one_continuation() {
    let script = route::script(vec![
        route::label(10),
        route::text(11, "共通。"),
        route::choice_call(12),
        route::dispatch(13, "synthetic-alpha", 20),
        route::dispatch(14, "synthetic-beta", 20),
        route::exit(15),
        route::label(20),
        route::text(21, "合流ルート。"),
        route::exit(22),
    ]);
    let (_temp, mut adapter) = route::adapter();
    let routes = route::walk(&mut adapter, &script).unwrap();
    assert_eq!(routes.episodes.len(), 2);
    let interact = routes
        .blocks
        .values()
        .find(|b| b["terminator"]["type"] == "interact")
        .unwrap()["terminator"]
        .clone();
    assert_eq!(
        interact["branches"]["o0"], interact["branches"]["o1"],
        "both options continue at the merged route"
    );
}

/// A branch that jumps back onto its own reachability path is a route
/// loop, and malformed choice sites are refused with import errors.
#[test]
fn choice_sites_reject_loops_and_malformed_dispatch() {
    let (_temp, mut adapter) = route::adapter();
    let script = route::script(vec![
        route::label(10),
        route::text(11, "共通。"),
        route::choice_call(12),
        route::dispatch(13, "synthetic-alpha", 10),
        route::dispatch(14, "synthetic-beta", 20),
        route::exit(15),
        route::label(20),
        route::text(21, "乙。"),
        route::exit(22),
    ]);
    let routes = route::walk(&mut adapter, &script).unwrap();
    assert_eq!(routes.choice_sites, 1, "input-gated choice loops are valid");
    let (_temp, mut adapter) = route::adapter();
    let script = route::script(vec![
        route::label(10),
        route::choice_call(12),
        route::exit(13),
    ]);
    let err = route::walk(&mut adapter, &script).unwrap_err().to_string();
    assert!(err.contains("E_IMPORT_CHOICE"), "{err}");
    let (_temp, mut adapter) = route::adapter();
    let script = route::script(vec![
        route::label(10),
        route::choice_call(12),
        route::dispatch(13, "synthetic-alpha", 20),
        route::exit(14),
        route::label(20),
        route::exit(21),
    ]);
    let err = route::walk(&mut adapter, &script).unwrap_err().to_string();
    assert!(err.contains("at least two options"), "{err}");
    let (_temp, mut adapter) = route::adapter();
    let script = route::script(vec![
        route::label(10),
        route::choice_call(12),
        route::dispatch(13, "synthetic-alpha", 20),
        route::dispatch(14, "synthetic-beta", 30),
        route::text(15, "生き残る後続。"),
        route::label(20),
        route::exit(21),
        route::label(30),
        route::exit(31),
    ]);
    let err = route::walk(&mut adapter, &script).unwrap_err().to_string();
    assert!(err.contains("must end with Exit"), "{err}");
}

/// The three stock menu-sound extractions: the title select sound from the
/// preview executor's sixth parameter, the replay select sound from the
/// 選択-labeled SE object, and the replay BGM from a call inside the
/// BGM再生 helper's line range. Absent conventions stay absent;
/// recognized-but-malformed ones fail instead of guessing.
#[test]
fn menu_sound_helpers_read_stock_conventions_and_refuse_malformed() {
    use super::super::lsb::Reference;
    let text = |v: &str| Expression {
        literal: Some(Literal::String(v.into())),
        operations: vec![],
        functions: BTreeMap::new(),
    };
    let bare = || Expression {
        literal: None,
        operations: vec![],
        functions: BTreeMap::new(),
    };
    let call = |page: &str, line: u32, params: Vec<Expression>| Body::Call {
        target: Reference {
            page: page.into(),
            line,
        },
        condition: route::flag(),
        has_params: true,
        params,
    };
    let object =
        |name: &str, file: Expression| Body::Object(BTreeMap::from([(1, text(name)), (3, file)]));
    let label = |name: &str, line: u32| route::command(3, line, Body::Label(name.into()));
    let preview = "ノベルシステム\\プレビューメニュー\\■選択実行.lsb";
    // Title: the sixth parameter is the select sound; paths normalize to '/'.
    let title = route::script(vec![route::command(
        5,
        143,
        call(
            preview,
            0,
            vec![
                text("はじめから"),
                text("skin"),
                text("0"),
                text("0"),
                text("サウンド\\tm2_switch001.wav"),
                text("サウンド\\tm2_switch002.wav"),
            ],
        ),
    )]);
    assert_eq!(
        title_select_sound(&title).unwrap().as_deref(),
        Some("サウンド/tm2_switch002.wav")
    );
    // An absent call or an empty parameter means no sound, not a guess.
    assert_eq!(
        title_select_sound(&route::script(vec![route::exit(1)])).unwrap(),
        None
    );
    assert_eq!(
        title_select_sound(&route::script(vec![route::command(
            5,
            143,
            call(preview, 0, vec![text(""); 6]),
        )]))
        .unwrap(),
        None
    );
    // The story executor is a different page and never arms the title.
    assert_eq!(
        title_select_sound(&route::script(vec![route::choice_call(143)])).unwrap(),
        None
    );
    for bad in [
        // A preview call without the select parameter is malformed.
        route::script(vec![route::command(5, 143, call(preview, 0, vec![]))]),
        // So are non-string and non-literal parameters, and a second call.
        route::script(vec![route::command(
            5,
            143,
            call(preview, 0, vec![bare(); 6]),
        )]),
        route::script(vec![
            route::command(5, 143, call(preview, 0, vec![text(""); 6])),
            route::command(
                5,
                150,
                call("プレビューメニュー\\■選択実行.lsb", 0, vec![text(""); 6]),
            ),
        ]),
    ] {
        assert!(title_select_sound(&bad).is_err());
    }
    // Replay select: the SE object under the 選択 label, not the unlabeled
    // hover object before it.
    let mouse = route::script(vec![
        route::command(42, 10, object("SE", text("サウンド\\tm2_switch001.wav"))),
        label("選択", 11),
        route::command(42, 12, object("SE", text("サウンド\\tm2_switch002.wav"))),
    ]);
    assert_eq!(
        replay_select_sound(&mouse).unwrap().as_deref(),
        Some("サウンド/tm2_switch002.wav")
    );
    assert_eq!(
        replay_select_sound(&route::script(vec![route::command(
            42,
            10,
            object("SE", text("サウンド\\tm2_switch001.wav")),
        )]))
        .unwrap(),
        None,
        "hover-only handlers carry no select sound"
    );
    assert_eq!(
        replay_select_sound(&route::script(vec![
            route::command(42, 9, object("BGM", text("サウンド\\x.ogg"))),
            label("選択", 11),
            route::command(42, 12, object("BGM", text("サウンド\\y.ogg"))),
        ]))
        .unwrap(),
        None,
        "only SE objects are the convention"
    );
    assert!(replay_select_sound(&route::script(vec![
        label("選択", 11),
        route::command(42, 12, object("SE", bare())),
    ]))
    .is_err());
    // Replay BGM: the call into the BGM再生 section of ■関数.lsb.
    let functions = route::script(vec![
        label("BGM再生", 33),
        route::command(42, 37, object("BGM", text("サウンド\\fixture_music.ogg"))),
        label("音量計算", 41),
    ]);
    let open = route::script(vec![route::command(
        5,
        100,
        call(
            "ノベルシステム\\■関数.lsb",
            37,
            vec![text("サウンド\\fixture_music.ogg")],
        ),
    )]);
    assert_eq!(
        replay_bgm(&open, &functions).unwrap().as_deref(),
        Some("サウンド/fixture_music.ogg")
    );
    assert_eq!(
        replay_bgm(
            &route::script(vec![route::command(
                5,
                100,
                call("ノベルシステム\\■関数.lsb", 41, vec![text("x")]),
            )]),
            &functions,
        )
        .unwrap(),
        None,
        "calls outside the helper section are other helpers"
    );
    assert!(replay_bgm(&open, &route::script(vec![route::exit(1)])).is_err());
    assert!(replay_bgm(
        &route::script(vec![route::command(
            5,
            100,
            call("ノベルシステム\\■関数.lsb", 37, vec![text("a"), text("b")]),
        )]),
        &functions,
    )
    .is_err());
    assert!(replay_bgm(
        &route::script(vec![
            route::command(
                5,
                100,
                call("ノベルシステム\\■関数.lsb", 37, vec![text("a")]),
            ),
            route::command(
                5,
                101,
                call("ノベルシステム\\■関数.lsb", 39, vec![text("b")]),
            ),
        ]),
        &functions,
    )
    .is_err());
}

#[test]
fn system_menu_fades_match_the_stock_pair_skip_other_roles_and_refuse_malformed() {
    use super::super::lsb::Command;
    let expr = |op, name: &str, args| Expression {
        literal: None,
        operations: vec![(op, name.into(), args)],
        functions: BTreeMap::new(),
    };
    let int = |n| expr(1, "____arg", vec![Literal::Int(n)]);
    let text = |s: &str| expr(1, "____arg", vec![Literal::String(s.into())]);
    // The exact stock Flip shape: an enter/close role signature with the
    // pinned wipe/parameter/source conventions and a literal millisecond
    // time (the corpus pair uses 200 ms in both directions).
    let stock = |act: i32,
                 delete: i32,
                 reverse: i32,
                 stop_event: i32,
                 time: i32|
     -> BTreeMap<String, Expression> {
        [
            ("act", int(act)),
            ("delete", int(delete)),
            ("reverse", int(reverse)),
            ("stop_event", int(stop_event)),
            ("wipe", int(3)),
            ("time", int(time)),
            ("parameter_0", int(20)),
            ("parameter_1", int(1)),
            ("source", text("")),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect()
    };
    let flip = |parameters: BTreeMap<String, Expression>,
                targets: Vec<Expression>,
                indent: u32|
     -> Command {
        Command {
            kind: 13,
            indent,
            body: Body::Flip {
                parameters,
                targets,
            },
            muted: false,
            not_update: false,
            line: 0,
            offset: 0,
        }
    };
    let enter = |time: i32| flip(stock(1, 0, 0, 0, time), vec![], 0);
    let close =
        |time: i32, indent: u32| flip(stock(0, 1, 0, 1, time), vec![text("メニュー背景")], indent);
    let empty = || route::script(vec![route::exit(1)]);
    // The stock pair: an enter Flip and nested close Flip in the
    // initialization script plus the right-click close Flip.
    let init = route::script(vec![enter(200), close(200, 1)]);
    let right_click = route::script(vec![close(200, 0)]);
    let fades = system_menu_fades(&init, &right_click)
        .unwrap()
        .expect("stock pair maps");
    assert_eq!((fades.enter, fades.close), (Some(200_000), Some(200_000)));
    // No convention at all stays silent.
    assert!(system_menu_fades(&empty(), &empty()).unwrap().is_none());
    // A lone direction maps only its side.
    let fades = system_menu_fades(&route::script(vec![enter(200)]), &empty())
        .unwrap()
        .expect("lone enter maps");
    assert_eq!((fades.enter, fades.close), (Some(200_000), None));
    // Other role signatures are not claimed: the sub-page selection enter
    // (stop_event 1, parameter_0 8, empty parameter_1), the reversed
    // reading-exit close, a dynamic-act Flip, a muted stock Flip and a
    // close aimed at another layer all pass through without a claim.
    let mut sub_page = stock(1, 0, 0, 1, 200);
    sub_page.insert("parameter_0".into(), int(8));
    sub_page.insert("parameter_1".into(), Expression::default());
    let mut reversed = stock(0, 1, 1, 1, 200);
    reversed.insert("parameter_0".into(), Expression::default());
    reversed.insert("parameter_1".into(), Expression::default());
    reversed.insert("source".into(), Expression::default());
    let mut dynamic_act = stock(1, 0, 0, 0, 200);
    dynamic_act.insert("act".into(), Expression::default());
    let mut muted = enter(200);
    muted.muted = true;
    let other_layer = flip(stock(0, 1, 0, 1, 200), vec![text("物語")], 0);
    // A dynamic close target is as uncertifiable as a dynamic signature.
    let dynamic_target = flip(stock(0, 1, 0, 1, 200), vec![Expression::default()], 0);
    let mixed = route::script(vec![
        flip(sub_page, vec![], 0),
        flip(reversed, vec![text("メニュー背景")], 0),
        flip(dynamic_act, vec![], 0),
        muted,
        other_layer,
        dynamic_target,
    ]);
    assert!(system_menu_fades(&mixed, &empty()).unwrap().is_none());
    // A recognized role with malformed pinned shape or timing is refused.
    for (name, parameters, targets) in [
        (
            "wipe",
            {
                let mut p = stock(1, 0, 0, 0, 200);
                p.insert("wipe".into(), int(4));
                p
            },
            vec![] as Vec<Expression>,
        ),
        (
            "parameter_0",
            {
                let mut p = stock(1, 0, 0, 0, 200);
                p.insert("parameter_0".into(), int(8));
                p
            },
            vec![],
        ),
        (
            "dynamic parameter_1",
            {
                let mut p = stock(1, 0, 0, 0, 200);
                p.insert("parameter_1".into(), Expression::default());
                p
            },
            vec![],
        ),
        (
            "nonempty source",
            {
                let mut p = stock(1, 0, 0, 0, 200);
                p.insert("source".into(), text("風"));
                p
            },
            vec![],
        ),
        ("zero time", stock(1, 0, 0, 0, 0), vec![]),
        ("time beyond the NIR bound", stock(1, 0, 0, 0, 2500), vec![]),
        (
            "dynamic time",
            {
                let mut p = stock(1, 0, 0, 0, 200);
                p.insert("time".into(), Expression::default());
                p
            },
            vec![],
        ),
    ] {
        let script = route::script(vec![flip(parameters, targets, 0)]);
        assert!(
            system_menu_fades(&script, &empty()).is_err(),
            "{name} must be refused"
        );
    }
    // All same-role Flips must agree on one timing; a matching repeat is
    // accepted and carries no extra claim.
    let both = route::script(vec![enter(200), enter(200)]);
    let fades = system_menu_fades(&both, &empty())
        .unwrap()
        .expect("repeat timing maps");
    assert_eq!(fades.enter, Some(200_000));
    let ambiguous = route::script(vec![enter(200), enter(300)]);
    assert!(system_menu_fades(&ambiguous, &empty()).is_err());
}
