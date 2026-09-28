//! Explicitly incomplete, source-derived menu preview. Never used by normal import.
use super::{
    lsb::{Body, Expression, Script},
    ui_expr::{self, Term},
    ui_items::MenuItems,
    ImportDiagnostic, ImportReport, Source,
};
use anyhow::{ensure, Context, Result};
use nir_format::ImageMenu;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

const SOURCE: &str = "ノベルシステム/システムメニュー/初期化.lsb";
const ID: &str = "import.menu.preview";
const REPLAY_VARIABLE: &str = "import.replay_active";
const BACKDROP: &str = "import.preview.backdrop";
const LIMITS: &str = "Incomplete source menu preview: remaining source visibility predicates, submenu actions, screenshot/cabinet storage, arbitrary source UI components and 200 ms fades are not lowered. Font substitution, font-height/line-spacing mapping, fixed row widths and dimming are provisional. Unsupported rows are non-interactive text; Escape closes the preview using the player service.";

struct Style {
    x: i32,
    y: i32,
    size: i32,
    spacing: i32,
    font: String,
    color: [f32; 4],
    hover: [f32; 4],
}
fn term(p: &BTreeMap<u16, Expression>, key: u16) -> Result<Term> {
    ui_expr::normalize(
        p.get(&key)
            .context("E_IMPORT_UI_PREVIEW: missing property")?,
    )?
    .context("E_IMPORT_UI_PREVIEW: empty property")
}
fn integer(p: &BTreeMap<u16, Expression>, key: u16, min: i32, max: i32) -> Result<i32> {
    let Term::Int { value } = term(p, key)? else {
        anyhow::bail!("E_IMPORT_UI_PREVIEW: nonconstant integer property {key}")
    };
    ensure!(
        (min..=max).contains(&value),
        "E_IMPORT_UI_PREVIEW: property {key} outside bounds"
    );
    Ok(value)
}
fn color(p: &BTreeMap<u16, Expression>, key: u16) -> Result<[f32; 4]> {
    let v = integer(p, key, 0, 0xffffff)?;
    Ok([
        (v & 255) as f32 / 255.,
        ((v >> 8) & 255) as f32 / 255.,
        ((v >> 16) & 255) as f32 / 255.,
        1.,
    ])
}
fn style(script: &Script) -> Result<Style> {
    ensure!(
        script.version == 116,
        "E_IMPORT_UI_PREVIEW: unsupported version"
    );
    let mut candidates = vec![];
    for c in &script.commands {
        if c.muted || c.kind != 25 {
            continue;
        }
        let Body::Object(p) = &c.body else {
            continue;
        };
        if term(p, 1)?
            == (Term::String {
                value: "システムメニュー".into(),
            })
        {
            ensure!(
                c.indent == 0,
                "E_IMPORT_UI_PREVIEW: conditional menu declaration"
            );
            candidates.push(p);
        }
    }
    ensure!(
        candidates.len() == 1,
        "E_IMPORT_UI_PREVIEW: expected one system menu"
    );
    let p = candidates[0];
    let Term::String { value: callback } = term(p, 73)? else {
        anyhow::bail!("E_IMPORT_UI_PREVIEW: dynamic selection callback")
    };
    ensure!(
        callback.replace('\\', "/") == "ノベルシステム/システムメニュー/選択時.lsc",
        "E_IMPORT_UI_PREVIEW: unknown selection callback"
    );
    ensure!(
        term(p, 51)? == (Term::Read { name: "S".into() }),
        "E_IMPORT_UI_PREVIEW: unsupported menu text source"
    );
    integer(p, 18, 0, 0)?;
    integer(p, 24, 0, 0)?;
    let Term::String { value: font } = term(p, 16)? else {
        anyhow::bail!("E_IMPORT_UI_PREVIEW: dynamic font")
    };
    Ok(Style {
        x: integer(p, 4, 0, 1000)?,
        y: integer(p, 5, 40, 700)?,
        size: integer(p, 17, 8, 128)?,
        spacing: integer(p, 19, 0, 128)?,
        font,
        color: color(p, 20)?,
        hover: color(p, 23)?,
    })
}
// Check the source predicate that makes the stock read-skip item visible.
// The preview still reports that surrounding initialization/dataflow is partial.
fn verify_item_guard(
    script: &Script,
    action_name: &str,
    header_kind: u8,
    expected: Term,
) -> Result<()> {
    use super::ui_expr::{Op, Term};
    let read = |name: &str| Term::Read { name: name.into() };
    let text = |value: &str| Term::String {
        value: value.into(),
    };
    let apply = |op, args| Term::Apply { op, args };
    let action = apply(Op::Equal, vec![read("val"), text(action_name)]);
    let guards: Vec<_> = script
        .commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.body {
            Body::Condition(e) if ui_expr::normalize(e).ok().flatten() == Some(action.clone()) => {
                Some(i)
            }
            _ => None,
        })
        .collect();
    ensure!(
        guards.len() == 1,
        "E_IMPORT_MENU_GUARD: expected one source branch"
    );
    let index = guards[0];
    let branch = script
        .commands
        .get(index..index + 3)
        .context("E_IMPORT_MENU_GUARD: incomplete branch")?;
    for (c, (kind, indent)) in branch.iter().zip([(header_kind, 1), (0, 2), (14, 3)]) {
        ensure!(
            c.kind == kind && c.indent == indent && !c.muted && !c.not_update,
            "E_IMPORT_MENU_GUARD: unknown branch flow"
        );
    }
    ensure!(
        script
            .commands
            .get(index + 3)
            .is_some_and(|c| c.indent == 1),
        "E_IMPORT_MENU_GUARD: extra source effects"
    );
    let Body::Condition(e) = &branch[1].body else {
        anyhow::bail!("E_IMPORT_MENU_GUARD: missing condition")
    };
    ensure!(
        ui_expr::normalize(e)? == Some(expected),
        "E_IMPORT_MENU_GUARD: unsupported availability condition"
    );
    let Body::Calc(e) = &branch[2].body else {
        anyhow::bail!("E_IMPORT_MENU_GUARD: missing append")
    };
    ensure!(
        ui_expr::assignment(e)?
            == (
                "S".into(),
                apply(
                    Op::Concat,
                    vec![
                        apply(Op::AddDelimiter, vec![text("\r\n"), read("S")]),
                        apply(Op::Index, vec![read("システムメニュー項目名"), read("i")])
                    ]
                )
            ),
        "E_IMPORT_MENU_GUARD: altered menu label append"
    );
    Ok(())
}
fn verify_skip_guard(script: &Script) -> Result<()> {
    use super::ui_expr::{Op, Term};
    let read = |name: &str| Term::Read { name: name.into() };
    let text = |value: &str| Term::String {
        value: value.into(),
    };
    let apply = |op, args| Term::Apply { op, args };
    let not = |v| apply(Op::Not, vec![v]);
    let expected = apply(
        Op::SourceAnd,
        vec![
            apply(
                Op::SourceAnd,
                vec![
                    apply(
                        Op::SourceAnd,
                        vec![
                            apply(Op::SourceAnd, vec![read("mes"), not(read("選択実行中"))]),
                            not(apply(Op::ObjectExists, vec![text("選択メニュー")])),
                        ],
                    ),
                    apply(Op::Less, vec![read("回想番号"), Term::Int { value: 0 }]),
                ],
            ),
            read("既読"),
        ],
    );
    verify_item_guard(script, "読んだ文章を飛ばす", 1, expected)
}
fn verify_peek_guard(script: &Script) -> Result<()> {
    verify_item_guard(script, "文字を消す", 0, Term::Read { name: "mes".into() })?;
    let declarations: Vec<_> = script
        .commands
        .iter()
        .filter(|c| c.indent == 0 && matches!(&c.body,Body::Variable{name,..} if name=="mes"))
        .collect();
    ensure!(
        declarations.len() == 1,
        "E_IMPORT_PEEK: ambiguous message existence local"
    );
    let c = declarations[0];
    let Body::Variable {
        value_type,
        initial,
        scope,
        ..
    } = &c.body
    else {
        unreachable!()
    };
    ensure!(
        !c.muted
            && !c.not_update
            && *value_type == 3
            && *scope == 2
            && ui_expr::normalize(initial)?
                == Some(Term::Apply {
                    op: super::ui_expr::Op::ObjectExists,
                    args: vec![Term::Read {
                        name: "メッセージボックス".into()
                    }]
                }),
        "E_IMPORT_PEEK: unsupported message existence predicate"
    );
    Ok(())
}

fn verify_history_guard(script: &Script) -> Result<()> {
    verify_item_guard(
        script,
        "シナリオ回想",
        1,
        Term::Apply {
            op: super::ui_expr::Op::SourceAnd,
            args: vec![
                Term::Read { name: "mes".into() },
                Term::Apply {
                    op: super::ui_expr::Op::Greater,
                    args: vec![
                        Term::Read {
                            name: "@HistoryCount".into(),
                        },
                        Term::Int { value: 0 },
                    ],
                },
            ],
        },
    )
}

fn menu(style: &Style, items: &[super::ui_items::MenuItem]) -> Result<(ImageMenu, Vec<Value>)> {
    let rows: Vec<_> = items
        .iter()
        .filter(|i| !i.action.contains("オプション_") && !i.action.contains("ゲーム終了_"))
        .collect();
    ensure!(
        !rows.is_empty() && rows.len() <= 16,
        "E_IMPORT_UI_PREVIEW: root row count"
    );
    let height = style.size + style.spacing;
    ensure!(
        style.y + height * rows.len() as i32 <= 768,
        "E_IMPORT_UI_PREVIEW: rows exceed stage"
    );
    let mut elements = vec![
        json!({"id":"preview.notice","rect":[20,10,980,28],"content":{"type":"text","text":"Incomplete menu preview — Escape to close","size":20,"color":[1.,0.8,0.,1.]}}),
        json!({"id":"preview.rows","rect":[style.x,style.y,1024-style.x-20,768-style.y],"content":{"type":"stack","gap":0}}),
    ];
    let mut bindings = vec![];
    for row in rows {
        // Binding follows the source action ID after dispatch and reading-body
        // verification. A display label never selects a service.
        let mode = match row.action.as_str() {
            "自動テキスト送り" => Some("auto"),
            "文字を消す" => Some("peek_story"),
            "読んだ文章を飛ばす" => Some("skip_read"),
            _ => None,
        };
        let history = row.action == "シナリオ回想";
        let content = if history {
            json!({"type":"text_button","label":row.label,"size":style.size,"color":style.color,"hover_color":style.hover,"disabled_color":[0.5,0.5,0.5,1.],"action":{"type":"push_menu","menu":super::ui_history::ID}})
        } else if let Some(mode) = mode {
            json!({"type":"text_button","label":row.label,"size":style.size,"color":style.color,"hover_color":style.hover,"disabled_color":[0.5,0.5,0.5,1.],"action":{"type":"reading","mode":mode}})
        } else {
            json!({"type":"text","text":row.label,"size":style.size,"color":[0.5,0.5,0.5,1.]})
        };
        let mut element = json!({"id":format!("source.row.{}",row.index),"parent":"preview.rows","rect":[0,0,1024-style.x-20,height],"content":content});
        if history {
            element["visible_when"] = json!([
                {"type":"reading_available","mode":"peek_story","available":true},
                {"type":"history_available","available":true}
            ]);
        }
        if let Some(mode) = mode {
            element["enabled_when"] =
                json!([{"type":"reading_available","mode":mode,"available":true}]);
            if mode == "peek_story" {
                element["visible_when"] =
                    json!([{"type":"reading_available","mode":"peek_story","available":true}]);
            }
            if mode == "skip_read" {
                element["visible_when"] = json!([
                    {"type":"story","name":"replay","equals":false},
                    {"type":"reading_available","mode":"skip_read","available":true}
                ]);
            }
        }
        elements.push(element);
        bindings.push(json!({"source_index":row.index,"label":row.label,"source_action":row.action,"binding":if history {"history.source_page".into()}else{mode.map(|mode|format!("reading.{mode}")).unwrap_or_else(||"unbound_noninteractive".into())}}));
    }
    Ok((
        serde_json::from_value(
            json!({"background":BACKDROP,"buttons":[],"story_exports":{"replay":REPLAY_VARIABLE},"elements":elements}),
        )?,
        bindings,
    ))
}

/// Mark converter-created replay entry wrappers in the existing story state.
/// These flags inherit the ordinary story snapshot contract; they are not UI locals.
pub(super) fn prepare_story(story: &mut Value) -> Result<()> {
    // Story nodes are visible during PeekStory. Source #-prefixed components
    // belong to its hidden UI group and need an explicit projection first.
    let scenes = story
        .get("scenes")
        .and_then(Value::as_object)
        .context("E_IMPORT_UI_PREVIEW: scenes")?;
    for nodes in scenes.values() {
        for node in nodes
            .as_array()
            .context("E_IMPORT_UI_PREVIEW: scene nodes")?
        {
            ensure!(
                !node["id"]
                    .as_str()
                    .context("E_IMPORT_UI_PREVIEW: node identity")?
                    .starts_with('#'),
                "E_IMPORT_PEEK: source UI component represented as a story node"
            );
        }
    }
    if story.get("variables").is_none() {
        story["variables"] = json!({});
    }
    let variables = story["variables"]
        .as_object_mut()
        .context("E_IMPORT_UI_PREVIEW: variables")?;
    ensure!(
        !variables.contains_key(REPLAY_VARIABLE),
        "E_IMPORT_UI_PREVIEW: context variable collision"
    );
    variables.insert(REPLAY_VARIABLE.into(), json!({"type":"bool","value":false}));
    let functions = story["functions"]
        .as_object_mut()
        .context("E_IMPORT_UI_PREVIEW: functions")?;
    ensure!(
        functions.contains_key("main"),
        "E_IMPORT_UI_PREVIEW: missing main entry"
    );
    for (name, function) in functions {
        let replay = name
            .strip_prefix("replay")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()));
        if name != "main" && !replay {
            continue;
        }
        let entry = function["entry"]
            .as_str()
            .context("E_IMPORT_UI_PREVIEW: entry block")?
            .to_owned();
        let ops = function["blocks"][entry]["ops"]
            .as_array_mut()
            .context("E_IMPORT_UI_PREVIEW: entry operations")?;
        ops.insert(0,json!({"id":format!("{name}.menu-context"),"operation":{"type":"assign","target":REPLAY_VARIABLE,"value":{"type":"const","value":{"type":"bool","value":replay}}}}));
    }
    Ok(())
}

pub(super) fn install(
    root: &Path,
    source: &mut Source,
    items: &MenuItems,
    report: &mut ImportReport,
) -> Result<()> {
    // Keep this prerequisite local so a future caller cannot bypass certification.
    super::livenovel::verify_auto_timer(source)?;
    let (_, selection) = source.read("ノベルシステム/システムメニュー/選択時.lsb")?;
    super::livenovel::verify_skip_menu_branch(&selection)?;
    super::ui_peek::verify(&selection)?;
    let (_, script) = source.read(SOURCE)?;
    verify_skip_guard(&script)?;
    verify_peek_guard(&script)?;
    verify_history_guard(&script)?;
    super::ui_history::verify_selection(&selection)?;
    let history = super::ui_history::HistoryPreview::load(source)?;
    let style = style(&script)?;
    let (menu, bindings) = menu(&style, &items.items)?;
    let path = root.join("theme/theme.toml");
    let mut theme: Value = toml::from_str(&fs::read_to_string(&path)?)?;
    theme["image_menus"][ID] = serde_json::to_value(menu)?;
    theme["image_menus"][super::ui_history::ID] = serde_json::to_value(&history.menu)?;
    theme["menu_overlay"] = json!(ID);
    fs::write(path, toml::to_string_pretty(&theme)?)?;
    let path = root.join("assets/catalog.toml");
    let mut catalog: Value = toml::from_str(&fs::read_to_string(&path)?)?;
    catalog["assets"].as_array_mut().context("E_IMPORT_CATALOG")?.push(json!({"id":BACKDROP,"kind":"image","source":"imported/draft-menu-backdrop.png","rights":"Generated preview overlay"}));
    fs::write(path, toml::to_string_pretty(&catalog)?)?;
    fs::write(
        root.join("assets/imported/draft-menu-backdrop.png"),
        super::media::png(&image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 0, 0, 128]),
        ))?,
    )?;
    history.install(root)?;
    super::ui_history::mark_draft_diagnostics(report);
    super::write_json(
        &root.join("import-menu-preview.json"),
        &json!({"format":2,"status":"incomplete","menu":ID,"source":SOURCE,"source_sha256":script.source_sha256,"source_version":script.version,"source_font":style.font,"bindings":bindings,"limitations":LIMITS,"history_preview":"import-history-preview.json","history_limitations":super::ui_history::LIMITS}),
    )?;
    let mut messages = vec![LIMITS.to_owned(), super::ui_history::LIMITS.to_owned()];
    messages.extend(
        bindings
            .iter()
            .filter(|b| b["binding"] == "unbound_noninteractive")
            .map(|b| format!("Unbound source menu action: {}", b["source_action"])),
    );
    report.errors += messages.len();
    report.status = "incomplete_ui_preview".into();
    report.coverage.push_str(
        " Includes incomplete source-derived system and history pages; see import-menu-preview.json and import-history-preview.json.",
    );
    report
        .diagnostics
        .extend(messages.into_iter().map(|message| ImportDiagnostic {
            severity: "error".into(),
            source: SOURCE.into(),
            index: 0,
            line: 0,
            byte: 0,
            command: "MenuPreview".into(),
            message,
        }));
    fs::write(
        root.join("MIGRATION-INCOMPLETE.txt"),
        format!("{LIMITS}\n{}\nSee import-report.json, import-menu-preview.json and import-history-preview.json.\n",super::ui_history::LIMITS),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{
        lsb::{Command, Literal},
        ui_items::MenuItem,
    };
    use super::*;
    fn script() -> Script {
        let props = [
            (1, Literal::String("システムメニュー".into())),
            (4, Literal::Int(177)),
            (5, Literal::Int(73)),
            (16, Literal::String("Test Font".into())),
            (17, Literal::Int(26)),
            (18, Literal::Int(0)),
            (19, Literal::Int(18)),
            (20, Literal::Int(0xffffff)),
            (23, Literal::Int(0xffff80)),
            (24, Literal::Int(0)),
            (51, Literal::Variable("S".into())),
            (
                73,
                Literal::String("ノベルシステム/システムメニュー/選択時.lsc".into()),
            ),
        ];
        Script {
            version: 116,
            source_sha256: "a".repeat(64),
            commands: vec![Command {
                kind: 25,
                indent: 0,
                muted: false,
                not_update: true,
                line: 1,
                offset: 0,
                body: Body::Object(
                    props
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k,
                                Expression {
                                    literal: Some(Literal::Int(-1)),
                                    operations: vec![(1, "____arg".into(), vec![v])],
                                    functions: BTreeMap::new(),
                                },
                            )
                        })
                        .collect(),
                ),
            }],
        }
    }
    #[test]
    #[ignore = "requires NIR_IMPORT_SOURCE"]
    fn source_preview_reading_contract() {
        let path = std::path::PathBuf::from(
            std::env::var_os("NIR_IMPORT_SOURCE").expect("NIR_IMPORT_SOURCE"),
        );
        let mut source = Source::new(&path).unwrap();
        super::super::livenovel::verify_auto_timer(&mut source).unwrap();
        let (_, selection) = source
            .read("ノベルシステム/システムメニュー/選択時.lsb")
            .unwrap();
        super::super::livenovel::verify_skip_menu_branch(&selection).unwrap();
        super::super::ui_peek::verify(&selection).unwrap();
        super::super::ui_history::verify_selection(&selection).unwrap();
        for case in 0..6 {
            let mut changed = (*selection).clone();
            match case {
                0 => changed.commands[6].muted = true,
                1 => {
                    if let Body::LoopCondition { target, .. } = &mut changed.commands[29].body {
                        *target += 1;
                    }
                }
                2 => {
                    if let Body::GetProperty { destination, .. } = &mut changed.commands[15].body {
                        *destination = "different".into();
                    }
                }
                3 => {
                    if let Body::SetProperty { value, .. } = &mut changed.commands[57].body {
                        *value = super::super::lsb::Expression {
                            literal: None,
                            operations: vec![(1, "____arg".into(), vec![Literal::Int(0)])],
                            functions: BTreeMap::new(),
                        };
                    }
                }
                4 => changed.commands.insert(57, changed.commands[56].clone()),
                _ => {
                    if let Body::LoopCondition { condition, .. } = &mut changed.commands[37].body {
                        for (_, _, args) in &mut condition.operations {
                            for a in args {
                                if matches!(a, Literal::Int(27)) {
                                    *a = Literal::Int(28);
                                }
                            }
                        }
                    }
                }
            }
            assert!(
                super::super::ui_peek::verify(&changed).is_err(),
                "peek mutation {case}"
            );
        }

        let (_, init) = source.read(SOURCE).unwrap();
        style(&init).unwrap();
        verify_skip_guard(&init).unwrap();
        verify_peek_guard(&init).unwrap();
        verify_history_guard(&init).unwrap();
        let mut wrong_history = (*init).clone();
        for c in &mut wrong_history.commands {
            if let Body::Condition(e) = &mut c.body {
                for (_, _, args) in &mut e.operations {
                    for arg in args {
                        if matches!(arg,Literal::Variable(name) if name=="@HistoryCount") {
                            *arg = Literal::Int(1);
                        }
                    }
                }
            }
        }
        assert!(verify_history_guard(&wrong_history).is_err());
        let mut changed = (*init).clone();
        for c in &mut changed.commands {
            if let Body::Condition(e) = &mut c.body {
                for (_, _, args) in &mut e.operations {
                    for arg in args {
                        if matches!(arg,Literal::Variable(name) if name=="既読") {
                            *arg = Literal::Int(1);
                        }
                    }
                }
            }
        }
        assert!(verify_skip_guard(&changed).is_err());
    }
    #[test]
    fn generated_entry_context_preserves_variables_and_marks_only_replay_wrappers() {
        let function = json!({"entry":"start","blocks":{"start":{"ops":[]}}});
        let mut story = json!({"variables":{"score":{"type":"i32","value":7}},"scenes":{},"functions":{"main":function,"replay1":function,"chapter":function}});
        prepare_story(&mut story).unwrap();
        assert_eq!(story["variables"]["score"]["value"], 7);
        for (name, expected) in [("main", false), ("replay1", true)] {
            assert_eq!(
                story["functions"][name]["blocks"]["start"]["ops"][0]["operation"]["value"]
                    ["value"]["value"],
                expected
            );
        }
        assert!(story["functions"]["chapter"]["blocks"]["start"]["ops"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(prepare_story(&mut story).is_err());
    }
    #[test]
    fn source_style_and_ids_produce_only_certified_controls() {
        let style = style(&script()).unwrap();
        assert_eq!(style.hover, [128. / 255., 1., 1., 1.]);
        assert_eq!(
            (style.x, style.y, style.size, style.spacing),
            (177, 73, 26, 18)
        );
        let items = vec![
            MenuItem {
                index: 0,
                label: "Save".into(),
                action: "自動テキスト送り".into(),
            },
            MenuItem {
                index: 1,
                label: "Auto".into(),
                action: "セーブ".into(),
            },
            MenuItem {
                index: 2,
                label: "Nested".into(),
                action: "prefixオプション_test".into(),
            },
        ];
        let (menu, bindings) = menu(&style, &items).unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(menu.controls().count(), 1);
        assert_eq!(menu.controls().next().unwrap().0, "source.row.0");
        assert!(
            matches!(&menu.elements[2].content, nir_format::MenuContent::TextButton { label, .. } if label == "Save")
        );
        assert_eq!(
            menu.element_state(
                "source.row.0",
                &BTreeMap::new(),
                &Default::default(),
                &Default::default(),
                &Default::default(),
                false
            ),
            (true, false)
        );
        assert!(matches!(
            menu.elements[3].content,
            nir_format::MenuContent::Text { .. }
        ));
    }
    #[test]
    fn refuses_ambiguous_dynamic_and_unbounded_style() {
        for case in 0..6 {
            let mut s = script();
            match case {
                0 => s.commands.push(s.commands[0].clone()),
                1 => s.commands[0].indent = 1,
                4 | 5 => {
                    let Body::Object(p) = &mut s.commands[0].body else {
                        panic!()
                    };
                    p.get_mut(&(if case == 4 { 73 } else { 51 }))
                        .unwrap()
                        .operations[0]
                        .2[0] = Literal::String("unknown".into());
                }
                n => {
                    let Body::Object(p) = &mut s.commands[0].body else {
                        panic!()
                    };
                    p.get_mut(&17).unwrap().operations[0].2[0] = if n == 2 {
                        Literal::Variable("dynamic".into())
                    } else {
                        Literal::Int(1000)
                    };
                }
            }
            assert!(style(&s).is_err());
        }
    }
    #[test]
    fn history_binding_uses_source_action_and_both_availability_facts() {
        let items = vec![
            MenuItem {
                index: 0,
                label: "History".into(),
                action: "セーブ".into(),
            },
            MenuItem {
                index: 1,
                label: "Renamed".into(),
                action: "シナリオ回想".into(),
            },
        ];
        let (menu, bindings) = menu(&style(&script()).unwrap(), &items).unwrap();
        assert_eq!(menu.controls().count(), 1);
        assert_eq!(bindings[1]["binding"], "history.source_page");
        assert!(
            matches!(menu.controls().next().unwrap().1, nir_format::ImageMenuAction::PushMenu{menu} if menu == super::super::ui_history::ID)
        );
        for (reading, history, expected) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let modes = if reading {
                std::collections::BTreeSet::from([nir_format::MenuReadingMode::PeekStory])
            } else {
                Default::default()
            };
            assert_eq!(
                menu.element_state(
                    "source.row.1",
                    &BTreeMap::new(),
                    &Default::default(),
                    &modes,
                    &Default::default(),
                    history
                ),
                (expected, true)
            );
        }
    }
}
