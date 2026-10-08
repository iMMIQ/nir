//! Adapter for the stock LiveNovel 116 event/menu convention. This is deliberately
//! separate from generic LSB lowering: native system scripts are replaced explicitly.
use super::{
    lsb::{Body, Expression, Glyph, Literal, Script},
    media, read_binary, ImportDiagnostic, ImportMapping, ImportOptions, ImportReport, Source,
    SourceLocation,
};
use anyhow::{bail, ensure, Context, Result};
use nir_format::{
    AudioBus, ImageButton, ImageMenu, ImageMenuAction, MenuContent, MenuEffects, MenuElement,
    MenuMusic, Node, Span,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub(super) fn recognizes(source: &Source, entry: &str) -> bool {
    entry.replace('\\', "/") == "ノベルシステム/START.lsb"
        && source.path("グラフィック/menu/menu.lpm").is_ok()
        && source.path("シーン回想.lsb").is_ok()
}
type Episode = (usize, String, Vec<Node>, BTreeMap<String, Value>);
#[derive(Clone)]
struct Asset {
    source: String,
    gain: Option<f32>,
    size: [u32; 2],
    blackened: bool,
}
#[derive(serde::Serialize)]
struct ImportedDefaults {
    source_sha256: String,
    bgm_volume: f32,
    voice_volume: f32,
    sfx_volume: f32,
    auto_wait_ms: u32,
    // Milliseconds per character (0 = instant): certified by the stock slider
    // callback `StatusTextSpeed = @ParamStr[0] × 64` over the authored 0..10
    // slider, and by the documented per-character ms unit.
    text_speed_ms: i32,
}
impl ImportedDefaults {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let values = super::lsb::project_settings(bytes)?;
        let integer = |name: &str, max: i32| -> Result<i32> {
            let Some(Literal::Int(n)) = values.get(name) else {
                bail!("E_IMPORT_SETTINGS: missing or non-integer {name}");
            };
            ensure!(
                (0..=max).contains(n),
                "E_IMPORT_SETTINGS: out-of-range {name}"
            );
            Ok(*n)
        };
        Ok(Self {
            source_sha256: nir_content::digest(bytes),
            bgm_volume: integer("StatusBGMVolume", 1000)? as f32 / 1000.,
            voice_volume: integer("StatusVoiceVolume", 1000)? as f32 / 1000.,
            sfx_volume: integer("StatusSEVolume", 1000)? as f32 / 1000.,
            auto_wait_ms: integer("StatusAutoTextWait", 30_000)? as u32,
            text_speed_ms: integer("StatusTextSpeed", 640)?,
        })
    }
}
pub(super) fn verify_auto_timer(source: &mut Source) -> Result<()> {
    // The source slider callback establishes the unit used by the persisted
    // author setting. A timer-shaped expression alone does not prove this.
    let (_, callback) =
        source.read("ノベルシステム/システムメニュー/オプション待ち時間スライダー変化時.lsb")?;
    verify_auto_wait_callback(&callback)?;
    let (_, selection) = source.read("ノベルシステム/システムメニュー/選択時.lsb")?;
    super::ui_dispatch::verify(&selection)?;
    verify_auto_menu_branch(&selection)?;
    for page in [
        "ノベルシステム/メッセージボックス/終了.lsb",
        "ノベルシステム/メッセージボックス/イベント.lsb",
    ] {
        let (_, script) = source.read(page)?;
        let timers:Vec<_>=script.commands.iter().filter_map(|c| {
            if c.muted || c.kind!=11 {return None;}
            let Body::Object(properties)=&c.body else {return None;};
            matches!(properties.get(&1).and_then(|e|e.literal.as_ref()),Some(Literal::String(name)) if name=="自動送りタイマー").then_some(properties)
        }).collect();
        ensure!(
            timers.len() == 1,
            "E_IMPORT_AUTO_POLICY: expected one source auto timer in {page}"
        );
        let expression = timers[0]
            .get(&27)
            .context("E_IMPORT_AUTO_POLICY: missing timer delay")?;
        ensure!(
            expression.is_sum_of_variables("StatusAutoTextWait", "tm"),
            "E_IMPORT_AUTO_POLICY: unsupported timer delay in {page}"
        );
    }
    Ok(())
}
pub(super) fn verify_text_speed(source: &mut Source) -> Result<()> {
    // The stock slider callback certifies the unit of the persisted setting:
    // one slider step is 64 milliseconds per character, 0 meaning instant.
    let (_, callback) = source
        .read("ノベルシステム/システムメニュー/オプションテキスト速度スライダー変化時.lsb")?;
    verify_text_speed_callback(&callback)
}
// Certify only the stock Auto branch body. This does not prove the enclosing
// dispatch, source menu guards, cabinet contents, or equivalent UI fade timing.
fn verify_auto_menu_branch(script: &Script) -> Result<()> {
    verify_reading_menu_branch(script, false)
}
pub(super) fn verify_skip_menu_branch(script: &Script) -> Result<()> {
    verify_reading_menu_branch(script, true)
}
fn verify_reading_menu_branch(script: &Script, skip: bool) -> Result<()> {
    use super::ui_expr::{assignment, normalize, Op, Term};
    let text = |s: &str| Term::String { value: s.into() };
    let expected = Term::Apply {
        op: Op::Equal,
        args: vec![
            Term::Read { name: "val".into() },
            text(if skip {
                "読んだ文章を飛ばす"
            } else {
                "自動テキスト送り"
            }),
        ],
    };
    let starts: Vec<_> = script
        .commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let Body::Condition(e) = &c.body else {
                return None;
            };
            (normalize(e).ok().flatten().as_ref() == Some(&expected)).then_some(i)
        })
        .collect();
    ensure!(
        starts.len() == 1,
        "E_IMPORT_READING_MENU: expected one reading menu branch"
    );
    let start = starts[0];
    let header = &script.commands[start];
    ensure!(
        header.kind == 1 && header.indent == 0 && !header.muted && !header.not_update,
        "E_IMPORT_READING_MENU: unknown reading branch context"
    );
    let end = (start + 1..script.commands.len())
        .find(|i| script.commands[*i].indent == 0)
        .unwrap_or(script.commands.len());
    let body = &script.commands[start + 1..end];
    ensure!(
        body.len() == if skip { 7 } else { 4 }
            && body
                .iter()
                .all(|c| c.indent == 1 && !c.muted && !c.not_update),
        "E_IMPORT_READING_MENU: unknown reading menu effects"
    );
    let Body::Cabinet {
        properties,
        act,
        targets,
    } = &body[0].body
    else {
        bail!("E_IMPORT_READING_MENU: missing reading cabinet restore")
    };
    ensure!(
        body[0].kind == 60
            && properties.len() == 1
            && targets.is_empty()
            && properties.get(&1).map(normalize).transpose()?.flatten()
                == Some(text("キャビネット"))
            && normalize(act)? == Some(Term::Int { value: 1 }),
        "E_IMPORT_READING_MENU: unsupported reading cabinet restore"
    );
    let Body::Flip {
        parameters,
        targets,
    } = &body[1].body
    else {
        bail!("E_IMPORT_READING_MENU: missing reading menu exit")
    };
    ensure!(
        body[1].kind == 13
            && parameters.len() == 9
            && targets.len() == 1
            && normalize(&targets[0])? == Some(text("メニュー背景")),
        "E_IMPORT_READING_MENU: unsupported reading menu exit target"
    );
    for (name, value) in [
        ("wipe", Some(3)),
        ("time", Some(200)),
        ("reverse", Some(1)),
        ("act", Some(0)),
        ("delete", Some(1)),
        ("parameter_0", None),
        ("parameter_1", None),
        ("source", None),
        ("stop_event", Some(1)),
    ] {
        let e = parameters
            .get(name)
            .context("E_IMPORT_READING_MENU: missing reading exit parameter")?;
        ensure!(
            normalize(e)? == value.map(|value| Term::Int { value }),
            "E_IMPORT_READING_MENU: unsupported reading exit parameter {name}"
        );
    }
    let writes: &[(&str, i32)] = if skip {
        &[
            ("メッセージスキップ", 1),
            ("既読メッセージスキップ", 1),
            ("既読スキップ使用済み", 0),
        ]
    } else {
        &[("自動送り", 1), ("自動送りフラグ", 1)]
    };
    for (c, (name, value)) in body[2..].iter().zip(writes) {
        let Body::Calc(e) = &c.body else {
            bail!("E_IMPORT_READING_MENU: missing reading enable write")
        };
        ensure!(
            assignment(e)? == ((*name).into(), Term::Int { value: *value }),
            "E_IMPORT_READING_MENU: unknown reading enable write"
        );
    }
    if skip {
        for (command, (index, expected)) in body[5..].iter().zip([(82, -1), (26, 0)]) {
            let Body::SetProperty {
                target,
                property,
                value,
            } = &command.body
            else {
                bail!("E_IMPORT_SKIP_POLICY: missing textbox skip property");
            };
            ensure!(
                command.kind == 18
                    && normalize(target)?
                        == Some(Term::Read {
                            name: "メッセージボックス".into()
                        })
                    && normalize(property)? == Some(Term::Int { value: index })
                    && normalize(value)? == Some(Term::Int { value: expected }),
                "E_IMPORT_SKIP_POLICY: unknown textbox skip effect"
            );
        }
    }
    Ok(())
}
fn verify_auto_wait_callback(script: &Script) -> Result<()> {
    use super::ui_expr::{assignment, Op, Term};
    let commands: Vec<_> = script.commands.iter().filter(|c| !c.muted).collect();
    ensure!(
        commands.len() == 2 && commands.iter().all(|c| c.indent == 0 && !c.not_update),
        "E_IMPORT_AUTO_POLICY: unknown auto-wait callback control flow"
    );
    let Body::Calc(expression) = &commands[0].body else {
        bail!("E_IMPORT_AUTO_POLICY: missing auto-wait assignment");
    };
    let (target, value) =
        assignment(expression).context("E_IMPORT_AUTO_POLICY: invalid auto-wait assignment")?;
    let expected = Term::Apply {
        op: Op::Multiply,
        args: vec![
            Term::Apply {
                op: Op::Index,
                args: vec![
                    Term::Read {
                        name: "@ParamStr".into(),
                    },
                    Term::Int { value: 0 },
                ],
            },
            Term::Int { value: 1000 },
        ],
    };
    ensure!(
        target == "StatusAutoTextWait" && value == expected,
        "E_IMPORT_AUTO_POLICY: unsupported auto-wait setting units"
    );
    // Only the accompanying caption refresh is part of this recognized profile.
    let Body::SetProperty {
        target,
        property,
        value,
    } = &commands[1].body
    else {
        bail!("E_IMPORT_AUTO_POLICY: unknown auto-wait callback side effect");
    };
    ensure!(
        super::ui_expr::normalize(property)? == Some(Term::Int { value: 50 })
            && matches!(super::ui_expr::normalize(target)?, Some(Term::String { value }) if !value.is_empty()),
        "E_IMPORT_AUTO_POLICY: callback must only update caption text"
    );
    let expected = Term::Apply {
        op: Op::Concat,
        args: vec![
            Term::Apply {
                op: Op::Divide,
                args: vec![
                    Term::Read {
                        name: "StatusAutoTextWait".into(),
                    },
                    Term::Int { value: 1000 },
                ],
            },
            Term::String {
                value: "秒".into()
            },
        ],
    };
    ensure!(
        super::ui_expr::normalize(value)? == Some(expected),
        "E_IMPORT_AUTO_POLICY: unknown auto-wait caption units"
    );
    Ok(())
}
fn verify_text_speed_callback(script: &Script) -> Result<()> {
    use super::ui_expr::{assignment, Op, Term};
    let commands: Vec<_> = script.commands.iter().filter(|c| !c.muted).collect();
    ensure!(
        commands.len() == 1 && commands[0].indent == 0 && !commands[0].not_update,
        "E_IMPORT_TEXT_SPEED: unknown text-speed callback control flow"
    );
    let Body::Calc(expression) = &commands[0].body else {
        bail!("E_IMPORT_TEXT_SPEED: missing text-speed assignment");
    };
    let (target, value) =
        assignment(expression).context("E_IMPORT_TEXT_SPEED: invalid text-speed assignment")?;
    let expected = Term::Apply {
        op: Op::Multiply,
        args: vec![
            Term::Apply {
                op: Op::Index,
                args: vec![
                    Term::Read {
                        name: "@ParamStr".into(),
                    },
                    Term::Int { value: 0 },
                ],
            },
            Term::Int { value: 64 },
        ],
    };
    ensure!(
        target == "StatusTextSpeed" && value == expected,
        "E_IMPORT_TEXT_SPEED: unsupported text-speed setting units"
    );
    Ok(())
}

/// Route-walk state: a branchy route graph lowers into one `main`
/// function. `entries` maps a command index to the block that continues
/// there so converging routes merge instead of duplicating content;
/// each queue entry carries the reachability path for cycle detection.
#[derive(Debug)]
struct Routes {
    episodes: Vec<Episode>,
    blocks: BTreeMap<String, Value>,
    entries: BTreeMap<usize, String>,
    queue: Vec<(usize, BTreeSet<usize>)>,
    choice_sites: usize,
}
struct Adapter {
    source: Source,
    defaults: Option<ImportedDefaults>,
    menu_items: Option<super::ui_items::MenuItems>,
    assets: BTreeMap<String, Asset>,
    texts: BTreeMap<String, crate::AuthorTextDoc>,
    functions: BTreeMap<String, Value>,
    cues: BTreeMap<String, Value>,
    scenes: BTreeMap<String, Vec<Node>>,
    nodes: BTreeMap<String, Node>,
    centered: BTreeSet<String>,
    audio: BTreeMap<String, Value>,
    menus: BTreeMap<String, ImageMenu>,
    choices: BTreeMap<String, Value>,
    variables: BTreeMap<String, Value>,
    choice_sites: usize,
    fade_sites: usize,
    menu_sounds: usize,
    menu_fades: Option<MenuFades>,
    source_map: BTreeMap<String, SourceLocation>,
    warnings: BTreeSet<String>,
    counter: usize,
    location: SourceLocation,
    textbox: String,
}
// Source geometry and asset variants enter the same finite composition path as
// authored menus. Do not infer new actions from presentation labels here.
fn menu_element(button: ImageButton) -> MenuElement {
    MenuElement {
        id: button.id,
        parent: None,
        rect: button.rect,
        scale: 1.,
        opacity: 1.,
        clip: None,
        visible_when: vec![],
        enabled_when: vec![],
        text_local: None,
        text_preference: None,
        text_slot: None,
        content: MenuContent::Button {
            label: button.label,
            asset: button.asset,
            hover_asset: button.hover_asset,
            locked_asset: button.locked_asset,
            action: button.action,
            requires: button.requires,
        },
    }
}
fn literal_string(e: &Expression) -> Result<&str> {
    match &e.literal {
        Some(Literal::String(s)) => Ok(s),
        _ => bail!("E_IMPORT_LIVENOVEL: expected literal string"),
    }
}
fn literal_int(e: &Expression) -> Result<i32> {
    match &e.literal {
        Some(Literal::Int(n)) => Ok(*n),
        _ => bail!("E_IMPORT_LIVENOVEL: expected literal integer"),
    }
}
fn references(e: &Expression, name: &str) -> bool {
    e.operations.iter().any(|(_, _, args)| {
        args.iter()
            .any(|v| matches!(v,Literal::Variable(s) if s==name))
    })
}
/// A stock selection dispatch compares the 選択値 variable against exactly one
/// string literal: the 選択メニュー callback commits the selected option's
/// text into that variable (選択.lsb evidence), and callers branch on the
/// comparison (title dispatch evidence: はじめから/つづきから/回想). Returns
/// that literal.
fn selection_dispatch(e: &Expression) -> Option<&str> {
    if !references(e, CHOICE_RESULT) || !e.operations.iter().any(|(op, _, _)| *op == 12) {
        return None;
    }
    let mut literal = None;
    for (_, _, args) in &e.operations {
        for arg in args {
            if let Literal::String(s) = arg {
                if literal.is_some() {
                    return None;
                }
                literal = Some(s.as_str());
            }
        }
    }
    literal
}
/// The stock choice executor page: creates the 選択メニュー object from its
/// call parameters, parks until the player picks an option, and leaves the
/// result in the 選択値 variable.
const CHOICE_EXECUTOR: &str = "ノベルシステム/選択メニュー/■選択実行.lsb";
/// The preview-menu variant of the choice executor arms the LPM title menu;
/// its call parameters carry the menu's sound effects — hover at index 4,
/// select at index 5 (decoded-source evidence in both executor pages).
const PREVIEW_EXECUTOR: &str = "プレビューメニュー\\■選択実行.lsb";
/// The stock selection result variable (engine convention, like
/// __メッセージ終了).
const CHOICE_RESULT: &str = "選択値";
/// The title menu's select sound: the preview choice-executor call plays it
/// when a title button is accepted. Only the select parameter has a NIR
/// counterpart — the page's click effect; the hover parameter stays an
/// accepted approximation in the ledger.
fn title_select_sound(script: &Script) -> Result<Option<String>> {
    let mut found: Option<Option<String>> = None;
    for c in &script.commands {
        let Body::Call { target, params, .. } = &c.body else {
            continue;
        };
        if !target.page.ends_with(PREVIEW_EXECUTOR) {
            continue;
        }
        ensure!(
            found.is_none(),
            "E_IMPORT_LIVENOVEL: ambiguous title menu call"
        );
        let parameter = params
            .get(5)
            .context("E_IMPORT_LIVENOVEL: title menu select sound parameter missing")?;
        let Literal::String(file) = parameter
            .literal
            .as_ref()
            .context("E_IMPORT_LIVENOVEL: title menu select sound not a literal")?
        else {
            bail!("E_IMPORT_LIVENOVEL: title menu select sound not a string");
        };
        found = Some((!file.is_empty()).then(|| file.replace('\\', "/")));
    }
    Ok(found.flatten())
}
/// The replay grid's select sound: the thumbnail mouse handler creates one
/// "SE" object per labeled section — hover before any label, select under the
/// 選択 label — and the select sound is what plays when an entry is accepted.
fn replay_select_sound(script: &Script) -> Result<Option<String>> {
    let mut label = None;
    let mut select = None;
    for c in &script.commands {
        match &c.body {
            Body::Label(name) => label = Some(name.as_str()),
            Body::Object(fields) if c.kind == 42 => {
                let name = match fields.get(&1).and_then(|e| e.literal.as_ref()) {
                    Some(Literal::String(s)) => s.as_str(),
                    _ => continue,
                };
                if name != "SE" || label != Some("選択") {
                    continue;
                }
                match fields.get(&3).and_then(|e| e.literal.as_ref()) {
                    Some(Literal::String(file)) if !file.is_empty() => {
                        ensure!(
                            select.is_none(),
                            "E_IMPORT_LIVENOVEL: ambiguous replay select sound"
                        );
                        select = Some(file.replace('\\', "/"));
                    }
                    Some(Literal::String(_)) => {}
                    _ => bail!("E_IMPORT_LIVENOVEL: replay select sound not a literal"),
                }
            }
            _ => {}
        }
    }
    Ok(select)
}
/// The replay screen's looping menu music: ■開始 loads it through the stock
/// BGM再生 helper in ■関数.lsb with the sound file as the call's only
/// parameter. The helper section's line range keeps the match on convention
/// rather than on any call that happens to carry a string.
fn replay_bgm(open: &Script, functions: &Script) -> Result<Option<String>> {
    let mut labels = vec![];
    for c in &functions.commands {
        if let Body::Label(name) = &c.body {
            labels.push((name.clone(), c.line));
        }
    }
    let position = labels
        .iter()
        .position(|(name, _)| name == "BGM再生")
        .context("E_IMPORT_LIVENOVEL: BGM再生 helper missing")?;
    let start = labels[position].1;
    let end = labels.get(position + 1).map(|(_, line)| *line);
    let mut found = None;
    for c in &open.commands {
        let Body::Call { target, params, .. } = &c.body else {
            continue;
        };
        if !target.page.ends_with("■関数.lsb")
            || target.line < start
            || end.is_some_and(|end| target.line >= end)
        {
            continue;
        }
        ensure!(
            params.len() == 1,
            "E_IMPORT_LIVENOVEL: replay music helper call parameters"
        );
        match params[0].literal.as_ref() {
            Some(Literal::String(file)) if !file.is_empty() => {
                ensure!(
                    found.is_none(),
                    "E_IMPORT_LIVENOVEL: ambiguous replay menu music"
                );
                found = Some(file.replace('\\', "/"));
            }
            Some(Literal::String(_)) => {}
            _ => bail!("E_IMPORT_LIVENOVEL: replay menu music not a literal"),
        }
    }
    Ok(found)
}
/// Certified system-menu fade timings for the generated preview pages, in
/// microseconds: the initialization script's enter Flip and the right-click
/// close Flip, both lowered as whole-layer fades of the original timing.
pub(super) struct MenuFades {
    pub(super) enter: Option<u64>,
    pub(super) close: Option<u64>,
}
impl MenuFades {
    pub(super) fn is_empty(&self) -> bool {
        self.enter.is_none() && self.close.is_none()
    }
}
/// The stock system-menu fade convention: the initialization script's enter
/// Flip (act 1, no targets) and the close Flip the right-click handler runs on
/// the menu-background layer (act 0, delete 1, stop event) — both wipe 3 with
/// the literal parameters 20/1. Flips with another role signature (sub-page
/// selection enters, the reversed reading-exit close, save-screenshot and
/// game-exit fades) are not claimed by this mapping and pass through. A
/// package without any convention Flip lowers to no transition; a Flip whose
/// role is recognized but whose pinned parameters, target or timing differ
/// fails the import, and all enter-shaped (resp. close-shaped) Flips must
/// share one timing so a single menu fade represents them.
pub(super) fn system_menu_fades(init: &Script, right_click: &Script) -> Result<Option<MenuFades>> {
    use super::ui_expr::{normalize, Term};
    let text = |s: &str| Term::String { value: s.into() };
    let mut enter: Option<i64> = None;
    let mut close: Option<i64> = None;
    for script in [init, right_click] {
        for c in &script.commands {
            if c.muted || c.not_update || c.kind != 13 {
                continue;
            }
            let Body::Flip {
                parameters,
                targets,
            } = &c.body
            else {
                continue;
            };
            let param = |name: &str| -> Result<Term> {
                parameters
                    .get(name)
                    .map(normalize)
                    .transpose()?
                    .flatten()
                    .ok_or_else(|| {
                        anyhow::anyhow!("E_IMPORT_MENU_TRANSITION: dynamic {name} parameter")
                    })
            };
            let literal = |name: &str| -> Result<Option<i64>> {
                Ok(
                    match parameters.get(name).map(normalize).transpose()?.flatten() {
                        Some(Term::Int { value }) => Some(i64::from(value)),
                        _ => None,
                    },
                )
            };
            // The role signature is the part that identifies the flip as this
            // convention; anything else (including a dynamic signature) is
            // another effect we do not claim.
            let role = match (
                literal("act")?,
                literal("delete")?,
                literal("reverse")?,
                literal("stop_event")?,
                targets.len(),
            ) {
                (Some(1), Some(0), Some(0), Some(0), 0) => "enter",
                (Some(0), Some(1), Some(0), Some(1), 1)
                    if normalize(&targets[0])? == Some(text("メニュー背景")) =>
                {
                    "close"
                }
                _ => continue,
            };
            // With the role recognized the stock shape is pinned exactly.
            ensure!(
                param("wipe")? == Term::Int { value: 3 }
                    && param("parameter_0")? == Term::Int { value: 20 }
                    && param("parameter_1")? == Term::Int { value: 1 }
                    && param("source")? == text(""),
                "E_IMPORT_MENU_TRANSITION: unsupported {role} fade parameters"
            );
            let Term::Int { value } = param("time")? else {
                bail!("E_IMPORT_MENU_TRANSITION: nonconstant {role} fade time");
            };
            let time = i64::from(value);
            ensure!(
                (1..=2000).contains(&time),
                "E_IMPORT_MENU_TRANSITION: {role} fade time {time} ms outside the NIR menu fade bound"
            );
            let found = if role == "enter" {
                &mut enter
            } else {
                &mut close
            };
            ensure!(
                found.is_none() || *found == Some(time),
                "E_IMPORT_MENU_TRANSITION: ambiguous {role} fade timing"
            );
            *found = Some(time);
        }
    }
    Ok((enter.is_some() || close.is_some()).then(|| MenuFades {
        enter: enter.map(|ms| ms as u64 * 1000),
        close: close.map(|ms| ms as u64 * 1000),
    }))
}
/// One stock dispatch option: its literal text and the label index it jumps to.
type DispatchOption = (String, usize);
/// The compatibility ledger for this profile: one entry per mapping rule the
/// conversion actually applies, with the behavior level separated from the
/// evidence class behind it. Every entry below is grounded in decoded stock
/// scripts (this corpus) or the documented LSB/GAL/LPM formats; none claims
/// original-runtime or cross-backend verification, which stays an open item
/// recorded through the fidelity warnings.
pub(super) fn mapping_ledger(
    choice_sites: usize,
    fade_sites: usize,
    menu_sounds: usize,
    menu_transitions: usize,
    reveal_us: u64,
) -> Vec<ImportMapping> {
    let entry = |rule: &str,
                 level: &str,
                 evidence: &str,
                 source_version: &str,
                 behavior: &str,
                 capabilities: &[&str],
                 approximation: Option<&str>| ImportMapping {
        rule: rule.into(),
        level: level.into(),
        evidence: evidence.into(),
        source_version: source_version.into(),
        behavior: behavior.into(),
        capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
        approximation: approximation.map(str::to_owned),
    };
    let mut mappings = vec![
        entry(
            "livenovel.startup",
            "adapted",
            "decoded-source",
            "LSB116",
            "Stock startup, window and asynchronous message-handshake scripts are replaced by the NIR session bootstrap; the episode, replay and choice conventions they dispatch to are preserved.",
            &[],
            None,
        ),
        entry(
            "livenovel.system-services",
            "adapted",
            "decoded-source",
            "LSB116",
            "Save/load, history and settings use NIR UI and persisted formats; LiveMaker save files are not compatible.",
            &[],
            None,
        ),
        entry(
            "livenovel.menu-sfx",
            "adapted",
            "decoded-source",
            "LSB116",
            &format!(
                "Title and replay-grid select sounds map to the generated pages' click effect and the replay screen's BGM to looping page music ({menu_sounds} page effect(s)); volumes ride the sfx/bgm bus defaults decoded from live.lpb."
            ),
            if menu_sounds > 0 {
                &["ui.menu-effects.v1"]
            } else {
                &[] as &[&str]
            },
            None,
        ),
        entry(
            "livenovel.menu-transition",
            "adapted",
            "decoded-source",
            "LSB116",
            &format!(
                "The stock system-menu initialization enter Flip and right-click close Flip (wipe 3, literal parameters 20/1, menu-background layer) map to the generated system-menu preview's whole-layer enter/close fades of the original millisecond timing ({menu_transitions} mapped transition pair); the spatial wipe pattern is approximated by whole-layer fades and sub-page selection, save-screenshot and game-exit flips are not mapped."
            ),
            if menu_transitions > 0 {
                &["ui.menu-effects.v1"]
            } else {
                &[] as &[&str]
            },
            None,
        ),
        entry(
            "livenovel.menu-hover",
            "approximate",
            "decoded-source",
            "LSB116",
            "Menu hover sounds and animated cursors are not reproduced.",
            &[],
            Some("Excluded from the compatibility claim; NIR menus have no hover-driven audio or custom pointer cursors."),
        ),
        entry(
            "livenovel.title-menu",
            "adapted",
            "decoded-source",
            "LPM106",
            "Title background, normal/hover button images and LPM coordinates generate image-menu elements with pointer and keyboard semantics.",
            &["ui.menu-elements.v1"],
            None,
        ),
        entry(
            "livenovel.text.font",
            "approximate",
            "decoded-source",
            "LPB116",
            "Dialogue renders in the bundled NIR Japanese font at the preserved base size and line height; the source font-face setting is retained as evidence only.",
            &[],
            Some("Excluded from the compatibility claim; the persisted StatusFontName is a Windows system font outside the project, so it cannot be bundled or registered as NIR content."),
        ),
        entry(
            "livenovel.text.reveal",
            "adapted",
            "decoded-source",
            "LSB116",
            &format!(
                "Dialogue reveal interval maps the persisted StatusTextSpeed — milliseconds per character, certified by the stock slider callback `@ParamStr[0] × 64`, 0 meaning instant — to microseconds per grapheme cluster ({reveal_us} µs at the source default); source box image, position, opacity, 32 px base size and 40 px line height are preserved."
            ),
            &[],
            None,
        ),
        entry(
            "livenovel.textbox.fade",
            "adapted",
            "decoded-source",
            "LSB116",
            &format!(
                "MESON/MESOFF lower to dialogue-visibility operations; {fade_sites} fade duration(s) map to dissolve window reveals of the original millisecond timing, zero-duration toggles commit immediately."
            ),
            if fade_sites > 0 {
                &["text.window-transition.v1"]
            } else {
                &[] as &[&str]
            },
            None,
        ),
        entry(
            "livenovel.stage.wipe",
            "adapted",
            "decoded-source",
            "LSB116",
            "LiveMaker wipe numbers are represented by a dissolve of the original duration.",
            &[],
            None,
        ),
        entry(
            "livenovel.auto-policy",
            "adapted",
            "decoded-source",
            "LPB116",
            "Fixed Auto wait comes from StatusAutoTextWait with the remaining voice sampled and frozen at each cycle start; page-first and in-page voice bindings follow the decoded callbacks.",
            &["player.auto-delay-policy.v1", "text.voice-timer.v1"],
            None,
        ),
        entry(
            "livenovel.media.image",
            "adapted",
            "documented",
            "GAL105/106",
            "Single-frame 8/24/32-bit GAL decodes to PNG preserving layering, palette, alpha and trailing rects; animated GAL and LCM video are unsupported and excluded.",
            &[],
            None,
        ),
        entry(
            "livenovel.media.audio",
            "adapted",
            "documented",
            "LSB116",
            "WAV PCM16 and Ogg/Vorbis convert to player WAV, six-channel WAV downmixes to stereo, and event volume multiplies the player bus at play time.",
            &["audio.gain.v1"],
            None,
        ),
        entry(
            "livenovel.replay",
            "adapted",
            "decoded-source",
            "LSB116",
            "Replay thumbnails keep original grid coordinates, entries unlock through profile keys, and finishing returns through the replay-completed outcome; locked entries use blackened thumbnails and refuse execution.",
            &["ui.replay.v1"],
            None,
        ),
        entry(
            "livenovel.settings",
            "exact",
            "decoded-source",
            "LPB116",
            "BGM/voice/SE volume and Auto wait defaults are decoded from LPB116 with range checks and exact unit conversion; the source digest is retained.",
            &[],
            None,
        ),
    ];
    if choice_sites > 0 {
        mappings.push(entry(
            "livenovel.story.choice",
            "adapted",
            "decoded-source",
            "LSB116",
            &format!(
                "{choice_sites} 選択メニュー call site(s) with 選択値 dispatch chains lower to typed interactions whose option values are the committed texts; branches continue each route and converging targets merge."
            ),
            &["story.typed-result.v1"],
            None,
        ));
    }
    mappings
}
/// Collects the dispatch chain that must follow a ■選択実行 call: consecutive
/// conditional jumps, each comparing 選択値 with one string literal. Returns
/// the (option text, label index) pairs plus the index after the chain;
/// `None` when the command at `pc` is not such a jump.
fn choice_chain(
    script: &Script,
    page: &str,
    pc: usize,
) -> Result<Option<(Vec<DispatchOption>, usize)>> {
    let mut options = vec![];
    let mut at = pc;
    while let Some(c) = script.commands.get(at) {
        let Body::Jump(target, condition) = &c.body else {
            break;
        };
        let Some(literal) = selection_dispatch(condition) else {
            break;
        };
        options.push((literal.to_owned(), local_target(script, page, target)?));
        at += 1;
    }
    if options.is_empty() {
        return Ok(None);
    }
    Ok(Some((options, at)))
}
fn label(script: &Script, line: u32) -> Result<usize> {
    if line == 0 {
        return Ok(0);
    }
    script
        .commands
        .iter()
        .position(|c| c.kind == 3 && c.line == line)
        .context("E_IMPORT_LIVENOVEL: missing jump label")
}
fn local_target(script: &Script, page: &str, target: &super::lsb::Reference) -> Result<usize> {
    ensure!(
        target.page.is_empty() || target.page.replace('\\', "/") == page,
        "E_IMPORT_LIVENOVEL: unexpected external scenario jump"
    );
    label(script, target.line)
}
fn last_jump(script: &Script) -> Result<String> {
    match &script
        .commands
        .last()
        .context("E_IMPORT_LIVENOVEL: empty bootstrap")?
        .body
    {
        Body::Jump(r, e) if e.flag()? && r.line == 0 => Ok(r.page.clone()),
        _ => bail!("E_IMPORT_LIVENOVEL: unrecognized bootstrap"),
    }
}
impl Adapter {
    fn new(source: Source) -> Self {
        Self {
            source,
            defaults: None,
            menu_items: None,
            assets: BTreeMap::new(),
            texts: BTreeMap::new(),
            functions: BTreeMap::new(),
            cues: BTreeMap::new(),
            scenes: BTreeMap::new(),
            nodes: BTreeMap::new(),
            centered: BTreeSet::new(),
            audio: BTreeMap::new(),
            menus: BTreeMap::new(),
            choices: BTreeMap::new(),
            variables: BTreeMap::new(),
            choice_sites: 0,
            fade_sites: 0,
            menu_sounds: 0,
            menu_fades: None,
            source_map: BTreeMap::new(),
            warnings: BTreeSet::new(),
            counter: 0,
            location: SourceLocation {
                source: String::new(),
                index: 0,
                line: 0,
                byte: 0,
                command: String::new(),
            },
            textbox: String::new(),
        }
    }
    fn id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        let id = format!("{prefix}_{}", self.counter);
        self.source_map.insert(id.clone(), self.location.clone());
        id
    }
    fn image(&mut self, path: &str) -> Result<(String, [u32; 2])> {
        let path = self.source.path(path)?;
        let name = path
            .strip_prefix(&self.source.root)?
            .to_str()
            .context("E_IMPORT_PATH")?
            .to_owned();
        let id = format!("lm.image.{}", &nir_content::digest(name.as_bytes())[..20]);
        if !self.assets.contains_key(&id) {
            let bytes = read_binary(&path)?;
            let (w, h) = media::gal_size(&bytes).with_context(|| format!("image {name}"))?;
            self.assets.insert(
                id.clone(),
                Asset {
                    source: name,
                    gain: None,
                    size: [w, h],
                    blackened: false,
                },
            );
        }
        Ok((id.clone(), self.assets[&id].size))
    }
    fn locked_image(&mut self, image: &str) -> String {
        let id = format!("{image}.locked");
        let mut asset = self.assets[image].clone();
        asset.blackened = true;
        self.assets.insert(id.clone(), asset);
        id
    }
    fn visual(&mut self, path: &str) -> Result<(Option<String>, [u32; 2], [f32; 4])> {
        if let Some(hex) = path.strip_prefix('$') {
            ensure!(
                hex.len() == 6,
                "E_IMPORT_COLOR: expected six hexadecimal digits"
            );
            let color = u32::from_str_radix(hex, 16)?;
            Ok((
                None,
                [1024, 768],
                [
                    (color & 255) as f32 / 255.,
                    ((color >> 8) & 255) as f32 / 255.,
                    ((color >> 16) & 255) as f32 / 255.,
                    1.,
                ],
            ))
        } else {
            let (id, size) = self.image(path)?;
            Ok((Some(id), size, [1.; 4]))
        }
    }
    fn sound(&mut self, path: &str, gain: f32) -> Result<String> {
        let path = self.source.path(path)?;
        let name = path
            .strip_prefix(&self.source.root)?
            .to_str()
            .context("E_IMPORT_PATH")?
            .to_owned();
        let id = format!(
            "lm.audio.{}",
            &nir_content::digest(format!("{name}:{gain}").as_bytes())[..20]
        );
        self.assets.entry(id.clone()).or_insert(Asset {
            source: name,
            gain: Some(gain),
            size: [0, 0],
            blackened: false,
        });
        Ok(id)
    }
    fn op(&mut self, blocks: &mut Vec<Value>, operation: Value) {
        let id = self.id("op");
        blocks.push(json!({"ops":[{"id":id,"operation":operation}],"terminator":{"type":"goto","target":"NEXT"}}));
    }
    fn effect(
        &mut self,
        blocks: &mut Vec<Value>,
        task: &str,
        scope: &str,
        effect: Value,
        wait: bool,
    ) {
        let cue = self.id("cue");
        self.cues.insert(
            cue.clone(),
            json!({"effects":[{"id":task,"scope":scope,"effect":effect}]}),
        );
        blocks.push(json!({"ops":[],"terminator":{"type":"activate","cue":cue,"next":"NEXT"}}));
        if wait {
            self.wait(blocks, task, json!({"type":"finished"}));
        }
    }
    fn wait(&self, blocks: &mut Vec<Value>, task: &str, milestone: Value) {
        blocks.push(json!({"ops":[],"terminator":{"type":"await","conditions":[{"task":task,"milestone":milestone}],"next":"NEXT","on_cancelled":"cancelled","on_failed":"failed"}}));
    }
    fn scene(&mut self, blocks: &mut Vec<Value>, duration: u64) {
        let scene = self.id("scene");
        self.scenes
            .insert(scene.clone(), self.nodes.values().cloned().collect());
        self.effect(
            blocks,
            "stage",
            "session",
            json!({"type":"stage_present","scene":scene,"duration_us":duration.to_string()}),
            duration > 0,
        );
    }
    fn play_sound(&mut self, blocks: &mut Vec<Value>, bus: &str, effect: Value) {
        let mut effects = vec![];
        if self.audio.contains_key(bus) {
            // PLAYSND replaces the channel, rather than declaring an earlier
            // stop. Prepare the new media before committing either effect.
            // AudioStop captures the old concrete task before Audio rebinds
            // the channel handle, so the old source survives failed loading.
            effects.push(json!({"id":format!("{bus}_stop"),"scope":"session",
                "effect":{"type":"audio_stop","target":bus,"duration_us":"0"}}));
        }
        effects.push(json!({"id":bus,"scope":"session","effect":effect.clone()}));
        let cue = self.id("cue");
        self.cues.insert(cue.clone(), json!({"effects":effects}));
        blocks.push(json!({"ops":[],"terminator":{"type":"activate","cue":cue,"next":"NEXT"}}));
        self.audio.insert(bus.into(), effect);
    }
    fn fade_stop(&mut self, blocks: &mut Vec<Value>, bus: &str, duration_us: u64) {
        if self.audio.remove(bus).is_some() {
            let task = format!("{bus}_stop");
            self.effect(
                blocks,
                &task,
                "session",
                json!({"type":"audio_stop","target":bus,"duration_us":duration_us.to_string()}),
                false,
            );
        }
    }
    fn event(&mut self, fields: &[String], blocks: &mut Vec<Value>) -> Result<()> {
        let (name, p) = fields
            .split_first()
            .context("E_IMPORT_EVENT: empty event")?;
        let name = name.strip_prefix('\u{1}').unwrap_or(name);
        let arg = |n: usize| {
            p.get(n)
                .map(String::as_str)
                .context("E_IMPORT_EVENT: missing argument")
        };
        let number = |n: usize| -> Result<u64> {
            let n = arg(n)?.parse::<u64>()?;
            ensure!(n <= 60_000, "E_IMPORT_EVENT: duration out of range");
            Ok(n)
        };
        match name {
            "" | "MENUENABLED" | "MESEND" => {}
            "MESON" | "MESOFF" => {
                // The first argument is the box fade duration in ms; a zero
                // duration is the source's own immediate toggle.
                let fade_ms = number(0)?;
                if fade_ms > 0 {
                    self.fade_sites += 1;
                    self.op(
                        blocks,
                        json!({
                            "type":"dialogue_visibility","visible":name=="MESON",
                            "transition":{"type":"dissolve"},
                            "duration_us":(fade_ms*1000).to_string()
                        }),
                    );
                } else {
                    self.op(
                        blocks,
                        json!({"type":"dialogue_visibility","visible":name=="MESON"}),
                    );
                }
            }
            "WAIT" => {
                ensure!(
                    arg(1)? == "NORMAL" && arg(2)? == "SKIP",
                    "E_IMPORT_EVENT: unsupported WAIT mode"
                );
                self.effect(
                    blocks,
                    "delay",
                    "frame",
                    json!({"type":"delay","duration_us":(number(0)?*1000).to_string()}),
                    true,
                );
            }
            "PLAYSND" => {
                ensure!(
                    arg(4)? == "0",
                    "E_IMPORT_EVENT: sound fade-in requires adaptation"
                );
                let bus = match arg(1)? {
                    "BGM" => "bgm",
                    "VOICE" => "voice",
                    _ => bail!("E_IMPORT_EVENT: unsupported sound bus"),
                };
                let looped = match arg(2)? {
                    "REPEAT" => true,
                    "NORMAL" => false,
                    _ => bail!("E_IMPORT_EVENT: unsupported sound playback mode"),
                };
                let gain = arg(3)?.parse::<f32>()? / 1000.;
                ensure!(
                    nir_format::valid_audio_gain(gain),
                    "E_IMPORT_EVENT: invalid sound gain"
                );
                let asset = self.sound(&format!("サウンド/{}", arg(0)?), 1.)?;
                let effect =
                    json!({"type":"audio","asset":asset,"bus":bus,"looped":looped,"gain":gain});
                self.play_sound(blocks, bus, effect);
            }
            "STOPSND" => {
                ensure!(
                    arg(0)? == "BGM" && arg(2)? == "PASS",
                    "E_IMPORT_EVENT: unsupported sound stop"
                );
                let duration = number(1)?;
                ensure!(
                    duration <= 60_000,
                    "E_IMPORT_EVENT: sound fade exceeds 60 seconds"
                );
                self.fade_stop(blocks, "bgm", duration * 1000);
            }
            "CREATECG" | "CHANGECG" | "DELETECG" => {
                let (wipe, duration) = match name {
                    "CREATECG" => {
                        ensure!(
                            arg(2)? == "NORMAL"
                                && arg(3)? == "#1"
                                && arg(4)? == "C"
                                && arg(5)? == "B",
                            "E_IMPORT_EVENT: unsupported image placement"
                        );
                        let (asset, size, color) = self.visual(arg(1)?)?;
                        let id = arg(0)?.to_owned();
                        self.centered.insert(id.clone());
                        self.nodes.insert(
                            id.clone(),
                            Node {
                                id,
                                parent: None,
                                asset,
                                x: (1024. - size[0] as f32) / 2.,
                                y: 768. - size[1] as f32,
                                width: size[0] as f32,
                                height: size[1] as f32,
                                scale: 1.,
                                opacity: 1.,
                                color,
                                order: arg(6)?.parse()?,
                                clip: None,
                            },
                        );
                        (number(7)?, number(8)?)
                    }
                    "CHANGECG" => {
                        ensure!(
                            arg(2)? == "NORMAL",
                            "E_IMPORT_EVENT: unsupported image change mode"
                        );
                        let (asset, size, color) = self.visual(arg(1)?)?;
                        let id = arg(0)?.to_owned();
                        let centered = self.centered.contains(&id);
                        let node = self.nodes.entry(id.clone()).or_insert(Node {
                            id,
                            parent: None,
                            asset: None,
                            x: 0.,
                            y: 0.,
                            width: 0.,
                            height: 0.,
                            scale: 1.,
                            opacity: 1.,
                            color: [1.; 4],
                            order: 0,
                            clip: None,
                        });
                        node.asset = asset;
                        node.color = color;
                        node.width = size[0] as f32;
                        node.height = size[1] as f32;
                        if centered {
                            node.x = (1024. - node.width) / 2.;
                            node.y = 768. - node.height;
                        }
                        (number(3)?, number(4)?)
                    }
                    _ => {
                        for id in arg(0)?.split(',') {
                            self.nodes.remove(id);
                            self.centered.remove(id);
                        }
                        (number(1)?, number(2)?)
                    }
                };
                if wipe != 0 && duration > 0 {
                    self.warnings.insert(format!("LiveMaker wipe {wipe} is represented by a dissolve with the original duration."));
                }
                self.scene(blocks, duration * 1000);
            }
            _ => bail!("E_IMPORT_EVENT: unsupported event {name}"),
        }
        Ok(())
    }
    fn bind_page_voice(&mut self, blocks: &mut Vec<Value>) {
        let voice = self
            .audio
            .get("voice")
            .filter(|effect| effect["looped"] == false)
            .map(|_| "voice");
        self.op(
            blocks,
            json!({"type":"dialogue_voice","task":"line","voice":voice,"wait":"sampled_remaining"}),
        );
    }
    fn page(&mut self, glyphs: &[Glyph], blocks: &mut Vec<Value>) -> Result<()> {
        let mut spans = vec![];
        let mut buffer = String::new();
        let mut actions: Vec<(String, Vec<Vec<String>>)> = vec![];
        let mut prefix = vec![];
        let flush = |buffer: &mut String, spans: &mut Vec<Span>| {
            if !buffer.is_empty() {
                spans.push(Span::Text {
                    id: format!("s{}", spans.len()),
                    text: std::mem::take(buffer),
                    emphasis: false,
                });
            }
        };
        for glyph in glyphs {
            match glyph {
                Glyph::Char(s) => buffer.push_str(s),
                Glyph::Break(0) => {
                    flush(&mut buffer, &mut spans);
                    spans.push(Span::Break {
                        id: format!("s{}", spans.len()),
                    });
                }
                Glyph::Event(fields) => {
                    if buffer.is_empty() && spans.is_empty() {
                        prefix.push(fields.clone());
                    } else {
                        flush(&mut buffer, &mut spans);
                        let gate = format!("s{}", spans.len());
                        spans.push(Span::Gate { id: gate.clone() });
                        actions.push((gate, vec![fields.clone()]));
                    }
                }
                _ => bail!("E_IMPORT_GLYPH: unsupported LiveNovel page glyph"),
            }
        }
        flush(&mut buffer, &mut spans);
        for fields in prefix {
            self.event(&fields, blocks)?;
        }
        if spans.is_empty() {
            return Ok(());
        }
        let tid = self.id("text");
        self.texts.insert(
            tid.clone(),
            crate::AuthorTextDoc {
                source_revision: 1,
                contract_revision: 1,
                spans,
            },
        );
        // StatusTextSpeed is milliseconds per character; NIR wants microseconds
        // per grapheme cluster, and 0 keeps the instant-reveal meaning.
        let reveal_us = (self
            .defaults
            .as_ref()
            .context("E_IMPORT_SETTINGS: source defaults not loaded")?
            .text_speed_ms as u64)
            * 1000;
        self.effect(
            blocks,
            "line",
            "interaction",
            json!({"type":"dialogue","text":tid,"speaker":"","reveal_us":reveal_us.to_string()}),
            false,
        );
        self.bind_page_voice(blocks);
        for (gate, events) in actions {
            self.wait(blocks, "line", json!({"type":"marker","id":gate}));
            for fields in events {
                self.event(&fields, blocks)?;
            }
            self.bind_page_voice(blocks);
            self.op(blocks, json!({"type":"dialogue_continue","task":"line"}));
        }
        self.wait(blocks, "line", json!({"type":"finished"}));
        // LiveNovel stops non-repeating voice when the page is dismissed,
        // even if the following page has no PLAYSND event of its own.
        if self
            .audio
            .get("voice")
            .is_some_and(|effect| effect["looped"] == false)
        {
            self.fade_stop(blocks, "voice", 50_000);
        }
        Ok(())
    }
    fn finish_function(&mut self, name: &str, mut blocks: Vec<Value>, end: Value) {
        let length = blocks.len();
        let mut table = BTreeMap::new();
        for (i, b) in blocks.iter_mut().enumerate() {
            let term = &mut b["terminator"];
            for key in ["target", "next"] {
                if term.get(key) == Some(&json!("NEXT")) {
                    term[key] = json!(format!("b{:06}", i + 1));
                }
            }
            table.insert(format!("b{i:06}"), b.clone());
        }
        table.insert(format!("b{length:06}"), json!({"ops":[],"terminator":end}));
        table.insert(
            "cancelled".into(),
            json!({"ops":[],"terminator":{"type":"end","outcome":"cancelled"}}),
        );
        table.insert("failed".into(),json!({"ops":[],"terminator":{"type":"fault","code":"E_IMPORT_TASK","message":"Imported event failed"}}));
        self.functions
            .insert(name.into(), json!({"entry":"b000000","blocks":table}));
    }
    /// Advances past commands the route walk ignores: labels, muted/system
    /// lines, the validated no-op scenario calc/wait, and the replay-index
    /// conditional jump that never fires on a fresh route.
    fn skip_forward(&self, script: &Script, mut pc: usize) -> Result<usize> {
        loop {
            let c = script
                .commands
                .get(pc)
                .context("E_IMPORT_LIVENOVEL: unexpected route end")?;
            let skip = c.muted
                || c.kind == 3
                || c.kind == 27
                || match &c.body {
                    Body::Calc(e) => {
                        ensure!(
                            e.operations.iter().all(|(op, name, args)| *op == 1
                                && name == "__メッセージ終了"
                                && matches!(args.as_slice(), [Literal::Int(0)])),
                            "E_IMPORT_LIVENOVEL: unexpected scenario assignment"
                        );
                        true
                    }
                    Body::Wait(e) => {
                        ensure!(
                            e.len() == 3
                                && references(&e[0], "__メッセージ終了")
                                && literal_int(&e[1])? == 0
                                && literal_int(&e[2])? == 0,
                            "E_IMPORT_LIVENOVEL: unexpected scenario wait"
                        );
                        true
                    }
                    Body::Jump(_, condition) if references(condition, "回想番号") => {
                        ensure!(
                            condition.operations.iter().any(|(op, _, _)| *op == 15),
                            "E_IMPORT_LIVENOVEL: unexpected replay condition"
                        );
                        true
                    }
                    _ => false,
                };
            if !skip {
                return Ok(pc);
            }
            pc += 1;
        }
    }
    /// The block that continues at a command index; allocates a fresh route
    /// block id on first visit.
    fn route_block(&mut self, script: &Script, pc: usize, routes: &mut Routes) -> Result<String> {
        let pc = self.skip_forward(script, pc)?;
        if let Some(id) = routes.entries.get(&pc) {
            return Ok(id.clone());
        }
        let id = self.id("route");
        routes.entries.insert(pc, id.clone());
        Ok(id)
    }
    /// Lowers the route graph from `first` into `main` blocks and returns the
    /// entry block id. Choices lower to typed interactions whose branch
    /// targets continue the winning route; all other control flow keeps the
    /// strict linear-walk shape.
    fn walk_routes(
        &mut self,
        script: &Script,
        page: &str,
        first: usize,
        routes: &mut Routes,
    ) -> Result<String> {
        let entry = self.route_block(script, first, routes)?;
        routes.queue.push((first, BTreeSet::new()));
        while !routes.queue.is_empty() {
            let (start, path) = routes.queue.remove(0);
            let pc = self.skip_forward(script, start)?;
            ensure!(
                !path.contains(&pc),
                "E_IMPORT_LIVENOVEL: unexpected route loop"
            );
            let id = routes.entries[&pc].clone();
            if routes.blocks.contains_key(&id) {
                // A sibling route already lowered this position; the block
                // graph itself is the merge point.
                continue;
            }
            let c = &script.commands[pc];
            self.location = SourceLocation {
                source: page.to_owned(),
                index: pc,
                line: c.line,
                byte: c.offset,
                command: c.name().into(),
            };
            let mut next_path = path.clone();
            next_path.insert(pc);
            match &c.body {
                Body::Text {
                    text,
                    target,
                    history,
                    ..
                } => {
                    ensure!(
                        literal_string(target)? == "メッセージボックス"
                            && history.flag()?
                            && !text.has_conditions_or_links
                            && !text.has_ruby,
                        "E_IMPORT_LIVENOVEL: unsupported text properties"
                    );
                    let name = format!("episode{}", routes.episodes.len() + 1);
                    routes.episodes.push((
                        pc,
                        name.clone(),
                        self.nodes.values().cloned().collect(),
                        self.audio.clone(),
                    ));
                    let waits = text
                        .glyphs
                        .iter()
                        .filter(|g| matches!(g, Glyph::Break(1)))
                        .count();
                    let mut blocks = vec![];
                    for (number, page_glyphs) in text
                        .glyphs
                        .split(|g| matches!(g, Glyph::Break(1)))
                        .enumerate()
                    {
                        let mut page_glyphs = page_glyphs.to_vec();
                        if number < waits
                            && !page_glyphs
                                .iter()
                                .any(|g| matches!(g, Glyph::Char(_) | Glyph::Break(0)))
                        {
                            // An image-only page still has the original click wait.
                            page_glyphs.push(Glyph::Break(0));
                        }
                        self.page(&page_glyphs, &mut blocks).with_context(|| {
                            format!("{}:{} TextIns", self.location.source, self.location.line)
                        })?;
                    }
                    self.finish_function(&name, blocks, json!({"type":"return"}));
                    let next = self.route_block(script, pc + 1, routes)?;
                    routes.blocks.insert(
                        id,
                        json!({"ops":[],"terminator":{"type":"call","function":name,"next":next}}),
                    );
                    routes.queue.push((pc + 1, next_path));
                }
                Body::Call {
                    target,
                    condition,
                    params,
                    ..
                } => {
                    let callee = target.page.replace('\\', "/");
                    if callee == "ノベルシステム/シーン回想/■フラグON.lsb" {
                        ensure!(
                            condition.flag()? && params.len() == 1,
                            "E_IMPORT_LIVENOVEL: unexpected scenario call"
                        );
                        let key = format!("lm.replay.{}", literal_int(&params[0])?);
                        let op_id = self.id("op");
                        let next = self.route_block(script, pc + 1, routes)?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[{"id":op_id,"operation":{"type":"profile_merge","key":key}}],"terminator":{"type":"goto","target":next}}),
                        );
                        routes.queue.push((pc + 1, next_path));
                    } else if callee == CHOICE_EXECUTOR {
                        self.lower_choice(script, page, pc, id, next_path, routes)?;
                    } else {
                        bail!("E_IMPORT_LIVENOVEL: unexpected scenario call");
                    }
                }
                Body::Jump(target, condition) => {
                    ensure!(
                        condition.flag()?,
                        "E_IMPORT_LIVENOVEL: conditional route requires adaptation"
                    );
                    let next = local_target(script, page, target)?;
                    // A jump back to the initial dispatch restores the original title menu.
                    if !routes.episodes.is_empty() && next < first {
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"end","outcome":"completed"}}),
                        );
                    } else {
                        let cont = self.route_block(script, next, routes)?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"goto","target":cont}}),
                        );
                        routes.queue.push((next, next_path));
                    }
                }
                Body::Exit(e) if e.flag()? => {
                    routes.blocks.insert(
                        id,
                        json!({"ops":[],"terminator":{"type":"end","outcome":"completed"}}),
                    );
                }
                _ => bail!(
                    "E_IMPORT_LIVENOVEL: unsupported route command {}:{} {}",
                    page,
                    c.line,
                    c.name()
                ),
            }
        }
        Ok(entry)
    }
    /// Lowers one stock choice site: the ■選択実行 call plus its 選択値
    /// dispatch chain become a typed interaction whose branch targets
    /// continue the winning route at its label. The option's declared value
    /// is its own text, matching what the source callback commits.
    fn lower_choice(
        &mut self,
        script: &Script,
        page: &str,
        pc: usize,
        id: String,
        path: BTreeSet<usize>,
        routes: &mut Routes,
    ) -> Result<()> {
        let (options, after) = choice_chain(script, page, pc + 1)?
            .context("E_IMPORT_CHOICE: missing 選択値 dispatch after the choice call")?;
        ensure!(
            options.len() >= 2,
            "E_IMPORT_CHOICE: dispatch chain needs at least two options"
        );
        let mut literals = BTreeSet::new();
        for (literal, _) in &options {
            ensure!(
                literals.insert(literal.as_str()),
                "E_IMPORT_CHOICE: duplicate option text {literal}"
            );
        }
        ensure!(
            matches!(script.commands.get(after), Some(c) if matches!(&c.body, Body::Exit(e) if matches!(e.flag(), Ok(true)))),
            "E_IMPORT_CHOICE: the unreachable dispatch fallthrough must end with Exit"
        );
        let choice = self.id("choice");
        let mut definitions = vec![];
        let mut branches = BTreeMap::new();
        let mut chain_path = path.clone();
        for at in pc + 1..after {
            chain_path.insert(at);
        }
        for (index, (literal, target)) in options.into_iter().enumerate() {
            let option = format!("o{index}");
            let text = self.id("text");
            self.texts.insert(
                text.clone(),
                crate::AuthorTextDoc {
                    source_revision: 1,
                    contract_revision: 1,
                    spans: vec![Span::Text {
                        id: "s0".into(),
                        text: literal.clone(),
                        emphasis: false,
                    }],
                },
            );
            definitions.push(json!({
                "id": option,
                "text": text,
                "value": {"type":"string","value":literal},
            }));
            let block = self.route_block(script, target, routes)?;
            branches.insert(option, block);
            routes.queue.push((target, chain_path.clone()));
        }
        self.choices
            .insert(choice.clone(), json!({"options": definitions}));
        self.variables
            .entry(CHOICE_RESULT.to_string())
            .or_insert_with(|| json!({"type":"string","value":""}));
        routes.blocks.insert(
            id,
            json!({"ops":[],"terminator":{
                "type":"interact",
                "choice": choice,
                "branches": branches,
                "on_empty": "failed",
                "result": CHOICE_RESULT,
            }}),
        );
        routes.choice_sites += 1;
        Ok(())
    }
    fn run(&mut self, entry: &str) -> Result<Value> {
        self.defaults = Some(ImportedDefaults::parse(&read_binary(
            &self.source.path("live.lpb")?,
        )?)?);
        verify_auto_timer(&mut self.source)?;
        verify_text_speed(&mut self.source)?;
        self.menu_items = Some(super::ui_items::MenuItems::load(&mut self.source)?);
        // The stock system-menu fade convention (the initialization enter Flip
        // and the right-click close Flip) feeds the draft preview's page
        // effects below; the spatial wipe pattern itself is approximated by
        // whole-layer fades.
        let (_, menu_init) = self
            .source
            .read("ノベルシステム/システムメニュー/初期化.lsb")?;
        let (_, menu_close) = self
            .source
            .read("ノベルシステム/システムメニュー/右クリック時.lsb")?;
        self.menu_fades = system_menu_fades(&menu_init, &menu_close)?;
        let (_, startup) = self.source.read(entry)?;
        let (_, bootstrap) = self.source.read(&last_jump(&startup)?)?;
        let (page, script) = self.source.read(&last_jump(&bootstrap)?)?;
        ensure!(
            script.version == 116,
            "E_IMPORT_LIVENOVEL: profile requires LSB116"
        );
        let mut first = None;
        for c in &script.commands {
            if let Body::Jump(target, condition) = &c.body {
                if selection_dispatch(condition) == Some("はじめから") {
                    ensure!(
                        first.is_none(),
                        "E_IMPORT_LIVENOVEL: ambiguous new-game route"
                    );
                    first = Some(local_target(&script, &page, target)?);
                }
            }
        }
        let first = first.context("E_IMPORT_LIVENOVEL: missing new-game route")?;
        self.textbox = self.image("グラフィック/立ちポーズ/box.gal")?.0;
        self.warnings.insert("Stock LiveNovel startup, window/system scripts and asynchronous message handshake are replaced by NIR. This profile targets its episode/replay/choice convention, not arbitrary LSB expressions.".into());
        self.warnings.insert("Save/load, history and settings use NIR UI and save format; LiveMaker save files are not compatible. Title/replay select sounds and the replay BGM map to page effects; hover sounds and animated cursors are not reproduced.".into());
        self.warnings.insert("Text speed maps the persisted StatusTextSpeed (milliseconds per character, 0 meaning instant) to the reveal interval; text renders in the bundled NIR Japanese font because the source font-face setting names a system font that cannot be bundled.".into());
        self.warnings.insert("Source Auto uses a sampled remaining-voice timer plus fixed delay. The imported policy samples the bound voice duration/position and voice-volume preference once per Auto cycle; original device timing and unsupported simultaneous source voice channels remain outside certification.".into());
        let mut routes = Routes {
            episodes: vec![],
            blocks: BTreeMap::new(),
            entries: BTreeMap::new(),
            queue: vec![],
            choice_sites: 0,
        };
        let main_entry = self.walk_routes(&script, &page, first, &mut routes)?;
        ensure!(
            !routes.episodes.is_empty(),
            "E_IMPORT_LIVENOVEL: no episodes"
        );
        routes.blocks.insert(
            "cancelled".into(),
            json!({"ops":[],"terminator":{"type":"end","outcome":"cancelled"}}),
        );
        routes.blocks.insert("failed".into(),json!({"ops":[],"terminator":{"type":"fault","code":"E_IMPORT_TASK","message":"Imported event failed"}}));
        self.functions.insert(
            "main".into(),
            json!({"entry": main_entry, "blocks": routes.blocks}),
        );
        self.choice_sites = routes.choice_sites;
        let episodes = routes.episodes;
        // Build replay wrappers from the original dispatcher, preserving its order.
        let (_, replay) = self.source.read("シーン回想.lsb")?;
        let (_, replay_ui) = self.source.read("ノベルシステム/シーン回想/■開始.lsb")?;
        // The replay page's stock chrome carries its own sounds: the thumbnail
        // grid's select SE and the screen's looping BGM through the BGM再生
        // helper. Missing conventions lower to no effects; recognized-but-
        // malformed ones fail the import.
        let (_, mouse) = self
            .source
            .read("ノベルシステム/シーン回想/サムネイル・マウス処理.lsb")?;
        let (_, stock) = self.source.read("ノベルシステム/■関数.lsb")?;
        let replay_click = replay_select_sound(&mouse)?;
        let replay_music = replay_bgm(&replay_ui, &stock)?;
        let replay_effects = if replay_click.is_none() && replay_music.is_none() {
            None
        } else {
            let click = replay_click.map(|path| self.sound(&path, 1.)).transpose()?;
            let music = match replay_music {
                Some(path) => {
                    let asset = self.sound(&path, 1.)?;
                    self.menu_sounds += 1;
                    Some(MenuMusic {
                        asset,
                        loop_region: None,
                        bus: AudioBus::Bgm,
                        gain: 1.,
                    })
                }
                None => None,
            };
            self.menu_sounds += usize::from(click.is_some());
            Some(MenuEffects {
                enter: None,
                close: None,
                click,
                music,
                elements: vec![],
            })
        };
        let ids = replay_ui
            .commands
            .iter()
            .filter_map(|c| {
                if let Body::Calc(e) = &c.body {
                    Some(e)
                } else {
                    None
                }
            })
            .flat_map(|e| e.operations.iter())
            .flat_map(|(_, _, a)| a.iter())
            .filter_map(|v| {
                if let Literal::String(s) = v {
                    Some(s)
                } else {
                    None
                }
            })
            .find_map(|s| {
                let values = s
                    .lines()
                    .map(str::parse::<u32>)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .ok()?;
                (values.len() == episodes.len()).then_some(values)
            })
            .context("E_IMPORT_LIVENOVEL: replay IDs missing")?;
        let list = read_binary(&self.source.path("グラフィック/シーン回想/シーン回想.TXT")?)?;
        let list = super::lsb::decode(&list)?;
        let thumbnails: BTreeMap<_, _> = list
            .lines()
            .filter(|s| !s.is_empty())
            .map(|line| {
                let (path, id) = line
                    .rsplit_once('/')
                    .context("E_IMPORT_LIVENOVEL: malformed replay list")?;
                Ok((id.parse::<u32>()?, path.to_owned()))
            })
            .collect::<Result<_>>()?;
        let calls: Vec<_> = replay
            .commands
            .iter()
            .filter_map(|c| {
                if let Body::Call { target, .. } = &c.body {
                    Some(target)
                } else {
                    None
                }
            })
            .collect();
        ensure!(
            calls.len() == ids.len(),
            "E_IMPORT_LIVENOVEL: replay dispatcher mismatch"
        );
        let (background, _) = self.image("グラフィック/背景/kotei.gal")?;
        let mut buttons = vec![];
        for (index, (target, unlock)) in calls.iter().zip(ids).enumerate() {
            let mut at = local_target(&script, &page, target)?;
            let mut seen = BTreeSet::new();
            while !matches!(script.commands[at].body, Body::Text { .. }) {
                ensure!(seen.insert(at), "E_IMPORT_LIVENOVEL: replay route cycle");
                let c = &script.commands[at];
                at = match &c.body {
                    Body::Jump(r, e) if e.flag()? => local_target(&script, &page, r)?,
                    Body::Calc(_) if at + 1 < script.commands.len() => at + 1,
                    _ if c.kind == 3 && at + 1 < script.commands.len() => at + 1,
                    _ => bail!("E_IMPORT_LIVENOVEL: unsupported replay route"),
                };
            }
            let (_, episode, nodes, audio) = episodes
                .iter()
                .find(|(pc, _, _, _)| *pc == at)
                .context("E_IMPORT_LIVENOVEL: unknown replay episode")?;
            let function = format!("replay{}", index + 1);
            let mut replay_blocks = vec![];
            self.nodes = nodes.iter().map(|n| (n.id.clone(), n.clone())).collect();
            self.scene(&mut replay_blocks, 0);
            for (bus, effect) in audio {
                self.effect(&mut replay_blocks, bus, "session", effect.clone(), false);
            }
            replay_blocks.push(
                json!({"ops":[],"terminator":{"type":"call","function":episode,"next":"NEXT"}}),
            );
            self.finish_function(
                &function,
                replay_blocks,
                json!({"type":"end","outcome":"replay_completed"}),
            );
            let (asset, size) = self.image(
                thumbnails
                    .get(&unlock)
                    .context("E_IMPORT_LIVENOVEL: missing replay thumbnail")?,
            )?;
            let locked_asset = Some(self.locked_image(&asset));
            buttons.push(ImageButton {
                id: format!("replay{index}"),
                label: format!("回想 {}", index + 1),
                asset,
                hover_asset: None,
                locked_asset,
                rect: [
                    50. + (index % 3) as f32 * 250.,
                    20. + (index / 3) as f32 * 200.,
                    size[0] as f32,
                    size[1] as f32,
                ],
                action: ImageMenuAction::Entry { function },
                requires: Some(format!("lm.replay.{unlock}")),
            });
        }
        self.menus.insert(
            "replay".into(),
            ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: buttons.into_iter().map(menu_element).collect(),
                background,
                buttons: vec![],
                effects: replay_effects,
            },
        );
        let (background, _) = self.image("グラフィック/menu/menu2.gal")?;
        let lpm = read_binary(&self.source.path("グラフィック/menu/menu.lpm")?)?;
        let mut buttons = vec![];
        for (i, b) in menu(&lpm)?.into_iter().enumerate() {
            let action = match b.label.as_str() {
                "はじめから" => ImageMenuAction::NewGame,
                "つづきから" => ImageMenuAction::Saves,
                "回想" => ImageMenuAction::Menu {
                    menu: "replay".into(),
                },
                _ => bail!("E_IMPORT_LIVENOVEL: unknown title action"),
            };
            let (asset, size) = self.image(&format!("グラフィック/menu/{}", b.source))?;
            let hover_asset = if b.selected.is_empty() {
                None
            } else {
                Some(self.image(&format!("グラフィック/menu/{}", b.selected))?.0)
            };
            buttons.push(ImageButton {
                id: format!("title{i}"),
                label: b.label,
                asset,
                hover_asset,
                locked_asset: None,
                rect: [b.x as f32, b.y as f32, size[0] as f32, size[1] as f32],
                action,
                requires: None,
            });
        }
        // The title menu's select sound arms the page's click effect. The
        // preview executor's hover parameter has no NIR counterpart and stays
        // in the accepted menu-hover approximation.
        let title_effects = match title_select_sound(&script)? {
            Some(path) => {
                self.menu_sounds += 1;
                Some(MenuEffects {
                    enter: None,
                    close: None,
                    click: Some(self.sound(&path, 1.)?),
                    music: None,
                    elements: vec![],
                })
            }
            None => None,
        };
        self.menus.insert(
            "title".into(),
            ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: buttons.into_iter().map(menu_element).collect(),
                background,
                buttons: vec![],
                effects: title_effects,
            },
        );
        self.warnings.insert("Replay thumbnails retain original grid coordinates. Locked thumbnails preserve alpha with black RGB; a NIR return button and system-menu access remain available for touch/keyboard navigation.".into());
        self.scenes.insert("title".into(), vec![]);
        Ok(
            json!({"fragment_format":1,"variables":self.variables,"functions":self.functions,"cues":self.cues,"scenes":self.scenes,"choices":self.choices}),
        )
    }
    fn export_media(&self, root: &Path) -> Result<()> {
        let path = root.join("assets/catalog.toml");
        let mut catalog: Value = toml::from_str(&fs::read_to_string(&path)?)?;
        fs::create_dir(root.join("assets/imported"))?;
        let entries = catalog["assets"]
            .as_array_mut()
            .context("E_IMPORT_CATALOG")?;
        let mut total = 0usize;
        let mut media_map = BTreeMap::new();
        for (id, asset) in &self.assets {
            let bytes = read_binary(&self.source.path(&asset.source)?)?;
            let source_hash = nir_content::digest(&bytes);
            let (kind, extension, bytes) = if let Some(gain) = asset.gain {
                (
                    "audio",
                    "wav",
                    media::audio(&bytes, gain)
                        .with_context(|| format!("audio {}", asset.source))?,
                )
            } else {
                ("image", "png", {
                    let mut image =
                        media::gal(&bytes).with_context(|| format!("image {}", asset.source))?;
                    if asset.blackened {
                        media::blacken(&mut image);
                    }
                    media::png(&image)?
                })
            };
            total += bytes.len();
            ensure!(
                total <= 1024 * 1024 * 1024,
                "E_IMPORT_LIMIT: converted media exceeds 1 GiB"
            );
            let source = format!("imported/{id}.{extension}");
            media_map.insert(id.clone(),json!({"source":asset.source,"source_sha256":source_hash,"kind":kind,"output":source,"gain":asset.gain,"transform":if asset.blackened {Some("black_rgb_preserve_alpha")} else {None}}));
            fs::write(root.join("assets").join(&source), bytes)?;
            entries.push(json!({"id":id,"kind":kind,"source":source,"rights":"Imported source game asset; original rights retained."}));
        }
        fs::write(path, toml::to_string_pretty(&catalog)?)?;
        super::write_json(&root.join("import-media.json"), &media_map)?;
        let path = root.join("game.toml");
        let mut manifest: Value = toml::from_str(&fs::read_to_string(&path)?)?;
        manifest["game"]["title_scene"] = json!("title");
        manifest["stage"] = json!({"width":1024,"height":768});
        fs::write(path, toml::to_string_pretty(&manifest)?)?;
        let defaults = self
            .defaults
            .as_ref()
            .context("E_IMPORT_SETTINGS: source defaults not loaded")?;
        let path = root.join("config/player.toml");
        let mut config: Value = toml::from_str(&fs::read_to_string(&path)?)?;
        config["defaults"]["auto_delay_policy"] = json!("fixed");
        config["defaults"]["auto_delay_us"] =
            json!((u64::from(defaults.auto_wait_ms) * 1000).to_string());
        config["defaults"]["bgm_volume"] = json!(defaults.bgm_volume);
        config["defaults"]["voice_volume"] = json!(defaults.voice_volume);
        config["defaults"]["sfx_volume"] = json!(defaults.sfx_volume);
        fs::write(path, toml::to_string_pretty(&config)?)?;
        super::write_json(&root.join("import-defaults.json"), defaults)?;
        super::write_json(
            &root.join("import-menu-items.json"),
            self.menu_items
                .as_ref()
                .context("E_IMPORT_MENU_ITEMS: not loaded")?,
        )?;
        let path = root.join("theme/theme.toml");
        let mut theme: Value = toml::from_str(&fs::read_to_string(&path)?)?;
        theme["image_menus"] = serde_json::to_value(&self.menus)?;
        theme["return_to_title"] = json!(true);
        theme["dialogue"] = json!({"height":185.,"padding":5.,"font_size":32.,"line_height":1.25,"opacity":175./255.,"background":self.textbox,"rect":[12.,573.,1000.,185.]});
        fs::write(path, toml::to_string_pretty(&theme)?)?;
        let tokens_path = root.join("theme/tokens.json");
        let mut tokens: Value = serde_json::from_slice(&fs::read(&tokens_path)?)?;
        tokens["text"] = json!([1., 1., 1., 1.]);
        super::write_json(&tokens_path, &tokens)?;
        fs::write(root.join("README.md"),"LiveNovel migration with imported source media. Read import-report.json for fidelity limits. Original rights apply to all imported content.\n")?;
        Ok(())
    }
}

pub(super) fn convert(
    source: Source,
    entry: &str,
    options: &ImportOptions,
    sdk: &Path,
    out: &Path,
) -> Result<ImportReport> {
    let mut adapter = Adapter::new(source);
    let mut story = adapter.run(entry)?;
    if adapter.choice_sites > 0 {
        adapter.warnings.insert("Story choices reuse the NIR typed interaction: the selected option text is committed to the 選択値 variable and dispatches its branch. Stock 選択メニュー chrome (frame skins, hover/select sounds, countdown timers and alignment options) is not reproduced.".into());
    }
    if options.draft {
        super::ui_preview::prepare_story(&mut story)?;
    }
    let ui = super::ui::analyze_system_menu(&mut adapter.source)?;
    let route_shape = if adapter.choice_sites > 0 {
        format!(
            "Branching LiveNovel route with {} typed choice site(s),",
            adapter.choice_sites
        )
    } else {
        "Linear LiveNovel route,".into()
    };
    let mappings = mapping_ledger(
        adapter.choice_sites,
        adapter.fade_sites,
        adapter.menu_sounds,
        usize::from(options.draft && !adapter.menu_fades.as_ref().is_some_and(MenuFades::is_empty)),
        adapter
            .defaults
            .as_ref()
            .context("E_IMPORT_SETTINGS: source defaults not loaded")?
            .text_speed_ms as u64
            * 1000,
    );
    let mut report=ImportReport{format:2,engine:"livemaker-livenovel116".into(),status:ImportReport::status_from_mappings(&mappings).into(),written:false,errors:0,approximate:mappings.iter().filter(|m|m.approximate()).count(),text_pages:adapter.texts.len(),functions:adapter.functions.len(),coverage:format!("{} replay dispatch and title image menu. {} referenced media assets converted. Native system scripts are replaced; see fidelity warnings and per-rule mappings.",route_shape,adapter.assets.len()),mappings,diagnostics:adapter.warnings.iter().map(|message|ImportDiagnostic{severity:"warning".into(),source:entry.into(),index:0,line:0,byte:0,command:"LiveNovelProfile".into(),message:message.clone()}).collect(),source_map:adapter.source_map.clone()};
    report.diagnostics.extend(ui.diagnostics());
    let staging = tempfile::Builder::new()
        .prefix(".nir-import-")
        .tempdir_in(out.parent().unwrap())?;
    let project = staging.path().join("project");
    crate::init(&project, sdk, &options.game_id)?;
    super::export(&project, options, &adapter.texts, &story, &report)?;
    super::write_json(&project.join("import-ui.json"), &ui)?;
    adapter.export_media(&project)?;
    if options.draft {
        super::ui_preview::install(
            &project,
            &mut adapter.source,
            adapter.menu_fades.as_ref(),
            adapter
                .menu_items
                .as_ref()
                .context("E_IMPORT_MENU_ITEMS: not loaded")?,
            &mut report,
        )?;
    }
    let loaded = crate::load_project(&project)?;
    crate::compile(&loaded.program)?;
    report.written = true;
    super::write_json(&project.join("import-report.json"), &report)?;
    ensure!(
        !out.try_exists()?,
        "E_IMPORT_EXISTS: output appeared during import"
    );
    fs::rename(project, out)?;
    super::enforce_acceptance(&report.mappings, options)?;
    Ok(report)
}

struct MenuButton {
    x: u32,
    y: u32,
    source: String,
    label: String,
    selected: String,
}
fn menu(data: &[u8]) -> Result<Vec<MenuButton>> {
    struct Reader<'a> {
        data: &'a [u8],
        at: usize,
    }
    impl Reader<'_> {
        fn take(&mut self, n: usize) -> Result<&[u8]> {
            let end = self.at.checked_add(n).context("E_IMPORT_LPM: offset")?;
            let b = self
                .data
                .get(self.at..end)
                .context("E_IMPORT_LPM: truncated")?;
            self.at = end;
            Ok(b)
        }
        fn number(&mut self) -> Result<u32> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
        }
        fn string(&mut self) -> Result<String> {
            let n = self.number()? as usize;
            ensure!(n <= 1024 * 1024, "E_IMPORT_LPM: string too long");
            super::lsb::decode(self.take(n)?)
        }
    }
    let mut r = Reader { data, at: 0 };
    ensure!(
        r.take(15)? == b"LivePrevMenu106",
        "E_IMPORT_LPM: expected menu version 106"
    );
    r.take(8)?;
    let count = r.number()? as usize;
    ensure!(count <= 256, "E_IMPORT_LPM: too many buttons");
    let mut buttons = vec![];
    for _ in 0..count {
        let x = r.number()?;
        let y = r.number()?;
        let source = r.string()?;
        r.take(1)?;
        let label = r.string()?;
        let selected = r.string()?;
        for _ in 0..10 {
            r.string()?;
        }
        r.take(8)?;
        r.string()?;
        for _ in 0..5 {
            r.string()?;
        }
        r.take(4)?;
        r.string()?;
        r.string()?;
        buttons.push(MenuButton {
            x,
            y,
            source,
            label,
            selected,
        });
    }
    ensure!(r.at == data.len(), "E_IMPORT_LPM: trailing bytes");
    Ok(buttons)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                3 => {
                    script.commands[3].body = Body::Calc(expr(1, "自動送り", vec![Literal::Int(0)]))
                }
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
                    text: Novel {
                        glyphs: vec![Glyph::Char(s.into())],
                        counts: BTreeMap::new(),
                        events: BTreeMap::new(),
                        has_conditions_or_links: false,
                        has_ruby: false,
                    },
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
            let mut routes = Routes {
                episodes: vec![],
                blocks: BTreeMap::new(),
                entries: BTreeMap::new(),
                queue: vec![],
                choice_sites: 0,
            };
            adapter.walk_routes(script, "00000001.lsb", 0, &mut routes)?;
            Ok(routes)
        }
    }

    /// The dispatch predicate accepts exactly one literal compared with the
    /// 選択値 variable and rejects anything looser.
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
        let err = route::walk(&mut adapter, &script).unwrap_err().to_string();
        assert!(err.contains("route loop"), "{err}");
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
        let object = |name: &str, file: Expression| {
            Body::Object(BTreeMap::from([(1, text(name)), (3, file)]))
        };
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
            route::command(42, 37, object("BGM", text("サウンド\\BGM054mama.ogg"))),
            label("音量計算", 41),
        ]);
        let open = route::script(vec![route::command(
            5,
            100,
            call(
                "ノベルシステム\\■関数.lsb",
                37,
                vec![text("サウンド\\BGM054mama.ogg")],
            ),
        )]);
        assert_eq!(
            replay_bgm(&open, &functions).unwrap().as_deref(),
            Some("サウンド/BGM054mama.ogg")
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
        let close = |time: i32, indent: u32| {
            flip(stock(0, 1, 0, 1, time), vec![text("メニュー背景")], indent)
        };
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
}
