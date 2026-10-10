//! Adapter for the stock LiveNovel 116/117 event/menu convention. This is deliberately
//! separate from generic LSB lowering: native system scripts are replaced explicitly.
use super::{
    lsb::{Body, Expression, Glyph, Literal, Reference, Script},
    media, read_binary, ImportDiagnostic, ImportMapping, ImportOptions, ImportReport, Source,
    SourceLocation,
};
use anyhow::{bail, ensure, Context, Result};
use nir_format::{
    AudioBus, ImageButton, ImageMenu, ImageMenuAction, MenuCondition, MenuContent, MenuEffects,
    MenuElement, MenuMusic, Node, Span,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
#[path = "livenovel_bitmap.rs"]
mod bitmap;
#[path = "livenovel_cinema.rs"]
mod cinema;
#[path = "livenovel_decoration.rs"]
mod decoration;
#[path = "livenovel_gallery.rs"]
mod gallery;
#[path = "livenovel_layout.rs"]
mod layout;
#[path = "livenovel_motion.rs"]
mod motion;
#[path = "livenovel_numeric.rs"]
mod numeric;
#[path = "livenovel_quake.rs"]
mod quake;
#[path = "livenovel_routes.rs"]
mod route_support;

/// The stock replay screen supplies an ordinal table through StringToArray's
/// default CRLF delimiter. Identify the destination and the complete write;
/// an unrelated numeric string must never become a gallery table.
fn replay_ids(ui: &Script, dispatch_count: usize) -> Result<Vec<u32>> {
    let mut table = None;
    for command in ui.commands.iter().filter(|c| !c.muted) {
        let Body::Calc(expression) = &command.body else {
            continue;
        };
        let operations = &expression.operations;
        let [(11, temporary, args), (1, discard, tail)] = operations.as_slice() else {
            continue;
        };
        let [Literal::String(text), Literal::Variable(destination)] = args.as_slice() else {
            continue;
        };
        if destination != "ids" {
            continue;
        }
        ensure!(
            expression.functions == BTreeMap::from([(0, 29)])
                && super::ui_expr::temporary(temporary)
                && (discard.is_empty() || discard == "____arg")
                && matches!(tail.as_slice(), [Literal::Variable(value)] if value == temporary),
            "E_IMPORT_LIVENOVEL: replay ID initializer changed"
        );
        let ids = text
            .split("\r\n")
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("E_IMPORT_LIVENOVEL: malformed replay IDs")?;
        ensure!(
            !ids.is_empty()
                && ids.len() <= dispatch_count
                && ids.len() <= 4096
                && ids.iter().collect::<BTreeSet<_>>().len() == ids.len(),
            "E_IMPORT_LIVENOVEL: replay ID count or duplicates"
        );
        ensure!(
            table.replace(ids).is_none(),
            "E_IMPORT_LIVENOVEL: ambiguous replay IDs"
        );
    }
    table.context("E_IMPORT_LIVENOVEL: replay IDs missing")
}

/// A stock dispatcher compares Int(@ParamStr[0]) with an authored ordinal.
/// Bind by that ordinal, including reordered Elseif branches, rather than
/// assuming the textual order of Call commands.
fn replay_dispatch(script: &Script) -> Result<BTreeMap<usize, Reference>> {
    let commands: Vec<_> = script
        .commands
        .iter()
        .filter(|c| !c.muted && c.kind != 27)
        .collect();
    let mut calls = BTreeMap::new();
    ensure!(
        commands.len() % 2 == 0,
        "E_IMPORT_REPLAY: incomplete dispatcher"
    );
    for (index, pair) in commands.chunks(2).enumerate() {
        let [guard, call] = pair else { unreachable!() };
        let Body::Condition(expression) = &guard.body else {
            bail!("E_IMPORT_REPLAY: missing ordinal guard")
        };
        let [(10, a, input), (1, b, copied), (11, c, converted), (12, d, compared), (1, output, discarded)] =
            expression.operations.as_slice()
        else {
            bail!("E_IMPORT_REPLAY: changed ordinal guard");
        };
        let [Literal::Variable(actual), Literal::Int(ordinal)] = compared.as_slice() else {
            bail!("E_IMPORT_REPLAY: dynamic ordinal")
        };
        ensure!(
            guard.indent == 0
                && guard.kind == if index == 0 { 0 } else { 1 }
                && expression.functions == BTreeMap::from([(2, 37)])
                && [a, b, c, d]
                    .into_iter()
                    .all(|s| super::ui_expr::temporary(s))
                && matches!(input.as_slice(), [Literal::Variable(name), Literal::Int(0)] if name == "@ParamStr")
                && matches!(copied.as_slice(), [Literal::Variable(name)] if name == a)
                && matches!(converted.as_slice(), [Literal::Variable(name)] if name == b)
                && actual == c
                && output == "____arg"
                && matches!(discarded.as_slice(), [Literal::Variable(name)] if name == d)
                && *ordinal >= 0
                && *ordinal < 4096,
            "E_IMPORT_REPLAY: unsupported dispatcher guard"
        );
        let Body::Call {
            target,
            condition,
            params,
            ..
        } = &call.body
        else {
            bail!("E_IMPORT_REPLAY: missing dispatcher call")
        };
        ensure!(
            call.indent == 1
                && condition.flag()?
                && params.is_empty()
                && calls.insert(*ordinal as usize, target.clone()).is_none(),
            "E_IMPORT_REPLAY: changed or duplicate dispatcher call"
        );
    }
    ensure!(
        calls.keys().copied().eq(0..calls.len()),
        "E_IMPORT_REPLAY: incomplete ordinal coverage"
    );
    Ok(calls)
}

fn verify_replay_reset(script: &Script) -> Result<()> {
    let commands: Vec<_> = script.commands.iter().filter(|c| !c.muted).collect();
    let calls = |page: &str| -> Vec<usize> {
        commands
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                matches!(&c.body, Body::Call{target,condition,..}
                if c.indent == 0 && target.page.replace('\\',"/")==page
                    && target.line==0 && condition.flag().ok()==Some(true))
                .then_some(index)
            })
            .collect()
    };
    let init = calls("変数初期化.lsb");
    let dispatch = calls("シーン回想.lsb");
    let ([init], [dispatch]) = (init.as_slice(), dispatch.as_slice()) else {
        bail!("E_IMPORT_REPLAY: missing or ambiguous unconditional reset/dispatch");
    };
    ensure!(
        matches!(&commands[*init].body, Body::Call{params,..} if params.is_empty()),
        "E_IMPORT_REPLAY: changed variable reset parameters"
    );
    ensure!(
        init < dispatch
            && commands[..*init].iter().any(|c| c.indent == 0
                && matches!(&c.body,
                Body::Flip{parameters,targets} if targets.is_empty()
                    && parameters.get("act").and_then(|e|literal_int(e).ok())==Some(1)
                    && parameters.get("delete").and_then(|e|literal_int(e).ok())==Some(1))),
        "E_IMPORT_REPLAY: replay reset order/clear differs"
    );
    Ok(())
}

pub(super) fn recognizes(source: &Source, entry: &str) -> bool {
    entry.replace('\\', "/") == "ノベルシステム/START.lsb"
        && source.path("メッセージボックス作成.lsb").is_ok()
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
    source_version: u32,
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
            source_version: u32::from_le_bytes(bytes[..4].try_into()?),
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
            && parameters.len() == if script.version == 117 { 10 } else { 9 }
            && targets.len() == 1
            && normalize(&targets[0])? == Some(text("メニュー背景")),
        "E_IMPORT_READING_MENU: unsupported reading menu exit target"
    );
    if script.version == 117 {
        ensure!(
            parameters
                .get("difference_only")
                .map(normalize)
                .transpose()?
                .flatten()
                == Some(Term::Int { value: 0 }),
            "E_IMPORT_READING_MENU: unsupported DifferenceOnly parameter"
        );
    }
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
    replay: bool,
    episodes: Vec<Episode>,
    blocks: BTreeMap<String, Value>,
    entries: BTreeMap<(String, usize), String>,
    queue: Vec<(String, usize, String, RoutePath)>,
    scripts: BTreeMap<String, std::sync::Arc<Script>>,
    variants: BTreeMap<(String, usize, String), String>,
    states: BTreeMap<String, RouteState>,
    choice_sites: usize,
}
#[derive(Debug, Clone, serde::Serialize)]
struct RouteState {
    nodes: BTreeMap<String, Node>,
    committed_images: BTreeSet<String>,
    centered: BTreeSet<String>,
    anchors: BTreeMap<String, (String, String)>,
    audio: BTreeMap<String, Value>,
    motion_tasks: BTreeMap<String, String>,
    movie_tasks: BTreeMap<String, String>,
    returns: Vec<ReturnSite>,
    pending_visibility: Option<bool>,
}
fn inheritable_image(node: &Node) -> bool {
    node.asset.is_some()
        && node.parent.is_none()
        && node.bitmap_text.is_none()
        && node.timeline_binding.is_none()
        && node.sprite_transform.is_none()
}
fn anchor_signature(value: &str, horizontal: bool) -> String {
    let value = if (horizontal && value == "L") || (!horizontal && value == "T") {
        "0"
    } else {
        value
    };
    match value.parse::<f32>() {
        Ok(number) if number.is_finite() => format!(
            "number:{:08x}",
            if number == 0. { 0 } else { number.to_bits() }
        ),
        _ => format!("anchor:{value}"),
    }
}
impl RouteState {
    #[cfg(test)]
    fn signature_before_delete(&self, deleted: &BTreeSet<String>) -> Result<String> {
        self.signature_for_route(
            deleted,
            &BTreeSet::from(["bgm".into(), "voice".into(), "sfx".into()]),
            &self.nodes.keys().cloned().collect(),
            None,
        )
    }
    fn signature_for_route(
        &self,
        deleted: &BTreeSet<String>,
        gain_reads: &BTreeSet<String>,
        geometry_reads: &BTreeSet<String>,
        anchor_reads: Option<&BTreeSet<String>>,
    ) -> Result<String> {
        // Audio cues already bind their concrete asset at runtime. Later
        // lowering uses channel presence, repeat mode and gain (CHGVOL),
        // never the old filename. Keep those facts while sharing the route
        // continuation across songs; the live task itself remains unchanged.
        let mut audio = self.audio.clone();
        for (bus, effect) in &mut audio {
            if effect["type"] == "audio" {
                effect.as_object_mut().unwrap().remove("asset");
                // Only voice repeat mode is observed by page binding and
                // dismissal. BGM/SFX waits address the live task, whose real
                // repeat mode remains in its original Audio cue.
                if bus != "voice" {
                    effect.as_object_mut().unwrap().remove("looped");
                }
                if !gain_reads.contains(bus) {
                    effect.as_object_mut().unwrap().remove("gain");
                }
            }
        }
        let mut nodes = self.nodes.clone();
        let mut discarded = deleted.clone();
        loop {
            let before = discarded.len();
            // Descendants can themselves own descendants.
            let parents = discarded.clone();
            discarded.extend(
                nodes
                    .values()
                    .filter(|n| n.parent.as_ref().is_some_and(|p| parents.contains(p)))
                    .map(|n| n.id.clone()),
            );
            if discarded.len() == before {
                break;
            }
        }
        nodes.retain(|id, _| !discarded.contains(id));
        let committed_images: BTreeSet<_> = self
            .committed_images
            .iter()
            .filter(|id| !discarded.contains(*id))
            .cloned()
            .collect();
        let centered: BTreeSet<_> = self
            .centered
            .iter()
            // An explicit axis pair always takes precedence over the legacy
            // center/bottom fallback during CHANGECG. Motion only clears it.
            .filter(|id| {
                !discarded.contains(*id)
                    && !self.anchors.contains_key(*id)
                    && anchor_reads.is_none_or(|reads| reads.contains(*id))
            })
            .cloned()
            .collect();
        let anchors: BTreeMap<_, _> = self
            .anchors
            .iter()
            .filter(|(id, _)| !discarded.contains(*id))
            .map(|(id, (x, y))| {
                (
                    id.clone(),
                    if anchor_reads.is_none_or(|reads| reads.contains(id)) {
                        (anchor_signature(x, true), anchor_signature(y, false))
                    } else {
                        ("unread".into(), "unread".into())
                    },
                )
            })
            .collect();
        for node in nodes
            .values_mut()
            .filter(|node| inheritable_image(node) && self.committed_images.contains(&node.id))
        {
            node.asset = Some("__nir_committed_image".into());
            if self.anchors.contains_key(&node.id) && !geometry_reads.contains(&node.id) {
                [node.x, node.y, node.width, node.height] = [0.; 4];
            }
        }
        Ok(nir_content::digest(&serde_json::to_vec(&(
            nodes,
            &committed_images,
            &centered,
            &anchors,
            audio,
            &self.motion_tasks,
            &self.movie_tasks,
            &self.returns,
            self.pending_visibility,
        ))?))
    }
}

/// The first scene writer can discard prior compile-time drawing state.
/// Its fade still captures the concrete old runtime scene. Stop-sound events
/// cannot inspect drawing state; all other prefixes remain conservatively
/// distinct. Live motion/audio task identities are never discarded here.
fn initial_scene_deletions(
    script: &Script,
    pc: usize,
    nodes: &BTreeMap<String, Node>,
) -> BTreeSet<String> {
    let Some(command) = script.commands.get(pc).filter(|c| !c.muted) else {
        return BTreeSet::new();
    };
    let Body::Text { text, .. } = &command.body else {
        return BTreeSet::new();
    };
    for glyph in &text.glyphs {
        let Glyph::Event(fields) = glyph else {
            return BTreeSet::new();
        };
        match fields.first().map(|s| s.trim_start_matches('\u{1}')) {
            Some("STOPSND") => continue,
            Some("DELETECG") => {
                return fields
                    .get(1)
                    .map(|s| s.split(',').map(str::to_owned).collect())
                    .unwrap_or_default()
            }
            Some("CREATECG")
                if fields.len() >= 10
                    && matches!(fields[3].as_str(), "NORMAL" | "WAIT")
                    && fields[4] == "#1" =>
            {
                let root = &fields[1];
                if nodes.get(root).is_none_or(|n| {
                    n.parent.is_none() && n.timeline_binding.is_none() && n.bitmap_text.is_none()
                }) && !nodes.values().any(|n| n.parent.as_ref() == Some(root))
                {
                    return BTreeSet::from([root.clone()]);
                }
                return BTreeSet::new();
            }
            _ => return BTreeSet::new(),
        }
    }
    BTreeSet::new()
}
#[derive(Debug, Clone, serde::Serialize)]
struct ReturnSite {
    page: String,
    pc: usize,
}
#[cfg(test)]
fn write_route_limit_probe(
    routes: &Routes,
    page: &str,
    pc: usize,
    candidate: &RouteState,
    adapter: &Adapter,
) -> Result<()> {
    let Ok(path) = std::env::var("NIR_LIVENOVEL_ROUTE_LIMIT_PROBE") else {
        return Ok(());
    };
    let mut positions: BTreeMap<(String, usize), Vec<&RouteState>> = BTreeMap::new();
    for ((page, pc, _), id) in &routes.variants {
        positions
            .entry((page.clone(), *pc))
            .or_default()
            .push(&routes.states[id]);
    }
    let mut ranked: Vec<_> = positions.iter().collect();
    ranked.sort_by_key(|(_, states)| std::cmp::Reverse(states.len()));
    let mut summaries = Vec::new();
    for ((page, pc), states) in ranked.into_iter().take(24) {
        let mut components: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut differing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for state in states {
            let value = serde_json::to_value(state)?;
            for (field, value) in value.as_object().unwrap() {
                components
                    .entry(field.clone())
                    .or_default()
                    .insert(nir_content::digest(&serde_json::to_vec(value)?));
            }
            for (id, node) in value["nodes"].as_object().unwrap() {
                for (property, value) in node.as_object().unwrap() {
                    differing
                        .entry(format!(
                            "{}.{}",
                            nir_content::digest(id.as_bytes()),
                            property
                        ))
                        .or_default()
                        .insert(nir_content::digest(&serde_json::to_vec(value)?));
                }
            }
        }
        summaries.push(json!({"page":page,"pc":pc,"variants":states.len(),
          "components":components.into_iter().map(|(k,v)|(k,v.len())).collect::<BTreeMap<_,_>>(),
          "differing_node_properties":differing.into_iter().filter(|(_,v)|v.len()>1).map(|(k,v)|(k,v.len())).collect::<BTreeMap<_,_>>() }));
    }
    let mut depths = BTreeMap::<usize, usize>::new();
    for state in routes.states.values() {
        *depths.entry(state.returns.len()).or_default() += 1;
    }
    fs::write(
        path,
        serde_json::to_vec(
            &json!({"variants":routes.variants.len(),"positions":positions.len(),"candidate":{"page":page,"pc":pc,"return_depth":candidate.returns.len()},"return_depths":depths,"top_positions":summaries,
                "gain_sensitive_buses":adapter.gain_reads,"emitted":{"texts":adapter.texts.len(),"functions":adapter.functions.len(),"scenes":adapter.scenes.len(),"cues":adapter.cues.len()}}),
        )?,
    )?;
    Ok(())
}

#[derive(Debug)]
struct GainReadRequired(String);
impl std::fmt::Display for GainReadRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source volume event requires {} gain specialization",
            self.0
        )
    }
}
impl std::error::Error for GainReadRequired {}

#[derive(Debug)]
struct GeometryReadRequired(String);
impl std::fmt::Display for GeometryReadRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source layout requires {} rectangle specialization",
            self.0
        )
    }
}
impl std::error::Error for GeometryReadRequired {}

#[derive(Debug)]
struct AnchorReadRequired(String);
impl std::fmt::Display for AnchorReadRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source image change requires {} anchor specialization",
            self.0
        )
    }
}
impl std::error::Error for AnchorReadRequired {}

struct Adapter {
    source: Source,
    defaults: Option<ImportedDefaults>,
    menu_items: Option<super::ui_items::MenuItems>,
    assets: BTreeMap<String, Asset>,
    texts: BTreeMap<String, crate::AuthorTextDoc>,
    functions: BTreeMap<String, Value>,
    share_episodes: bool,
    function_aliases: BTreeMap<String, String>,
    operation_sources: BTreeMap<String, String>,
    choice_declarations: BTreeMap<Vec<u8>, String>,
    cues: BTreeMap<String, Value>,
    scenes: BTreeMap<String, Vec<Node>>,
    image_template_assets: BTreeMap<String, String>,
    sprite_timelines: BTreeMap<String, nir_format::SpriteTimeline>,
    pending_movies: Vec<Value>,
    nodes: BTreeMap<String, Node>,
    committed_images: BTreeSet<String>,
    centered: BTreeSet<String>,
    anchors: BTreeMap<String, (String, String)>,
    audio: BTreeMap<String, Value>,
    gain_reads: BTreeSet<String>,
    missing_menu_sounds: BTreeSet<String>,
    geometry_reads: BTreeSet<String>,
    anchor_reads: Option<BTreeSet<String>>,
    motion_tasks: BTreeMap<String, String>,
    movie_tasks: BTreeMap<String, String>,
    returns: Vec<ReturnSite>,
    pending_visibility: Option<bool>,
    menus: BTreeMap<String, ImageMenu>,
    choices: BTreeMap<String, Value>,
    variables: BTreeMap<String, Value>,
    boolean_variables: BTreeSet<String>,
    status_flags: BTreeMap<String, String>,
    status_values: BTreeMap<String, String>,
    unsupported_status: BTreeSet<String>,
    bitmap_slots: BTreeSet<String>,
    implicit_integer_assignments: Vec<(Value, SourceLocation)>,
    choice_sites: usize,
    fade_sites: usize,
    menu_sounds: usize,
    menu_fades: Option<MenuFades>,
    source_map: BTreeMap<String, SourceLocation>,
    warnings: BTreeSet<String>,
    counter: usize,
    location: SourceLocation,
    page_ordinal: usize,
    textbox: String,
    textbox_theme: Option<Value>,
    dialogue_styles: BTreeMap<String, Value>,
    decoration_point: Option<[f32; 2]>,
    title_menu_version: &'static str,
    stage: [u32; 2],
    text_color: [f32; 4],
    title_route_entries: BTreeSet<usize>,
    cg_images: BTreeSet<String>,
    declared_cg_names: BTreeSet<String>,
    indexed_cg_scripts: BTreeSet<String>,
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
fn image_position(value: &str, stage: u32, size: u32, horizontal: bool) -> Result<f32> {
    let position = match value {
        "L" if horizontal => 0.,
        "T" if !horizontal => 0.,
        "C" => (stage as f32 - size as f32) / 2.,
        "R" if horizontal => stage as f32 - size as f32,
        "B" if !horizontal => stage as f32 - size as f32,
        _ => value
            .parse::<f32>()
            .context("E_IMPORT_EVENT: dynamic image coordinate")?,
    };
    ensure!(
        position.is_finite() && position.abs() <= 8192.,
        "E_IMPORT_EVENT: image coordinate out of range"
    );
    Ok(position)
}
fn sound_channel(value: &str) -> Result<(&'static str, &'static str)> {
    Ok(match value {
        "BGM" => ("bgm", "bgm"),
        "BGM2" => ("bgm2", "bgm"),
        "VOICE" => ("voice", "voice"),
        "VOICE2" => ("voice2", "voice"),
        "SE" => ("sfx", "sfx"),
        "SE2" => ("sfx2", "sfx"),
        _ => bail!("E_IMPORT_EVENT: unsupported sound channel {value}"),
    })
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
            if script.version == 117 {
                ensure!(
                    literal("difference_only")? == Some(0),
                    "E_IMPORT_MENU_TRANSITION: unsupported DifferenceOnly parameter"
                );
            }
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
type DispatchOption = (String, Reference);
type RoutePath = BTreeSet<(String, usize)>;
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
            "Single-frame GAL decodes to PNG preserving layering, palette, alpha and trailing rects. Bounded LiveCinema112 image tracks use shared sprite timelines; animated GAL and unmapped cinema records fail explicitly.",
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
fn choice_chain(script: &Script, pc: usize) -> Result<Option<(Vec<DispatchOption>, usize)>> {
    let mut options = vec![];
    let mut at = pc;
    while let Some(c) = script.commands.get(at) {
        let Body::Jump(target, condition) = &c.body else {
            break;
        };
        let Some(literal) = selection_dispatch(condition) else {
            break;
        };
        options.push((literal.to_owned(), target.clone()));
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
    let index = script
        .commands
        .iter()
        .position(|c| c.line == line)
        .context("E_IMPORT_LIVENOVEL: missing jump target")?;
    let c = &script.commands[index];
    let entry_wait = script.version == 117
        && matches!(&c.body, Body::Wait(args) if args.len() == 3 && args[0].flag().ok() == Some(true)
            && literal_int(&args[1]).ok() == Some(0) && literal_int(&args[2]).ok() == Some(0))
        && script.commands.get(index + 1).is_some_and(|c| c.kind == 3);
    // LSB117 can address a top-level jump directly, including a dispatch
    // condition. Execute that instruction and its ordinary fallthrough; never
    // enter a nested arm or skip evaluating a source condition.
    let entry_jump = script.version == 117
        && c.indent == 0
        && !c.muted
        && matches!(&c.body, Body::Jump(_, condition)
            if condition.flag().is_ok()
                || super::ui_expr::normalize(condition).is_ok_and(|value| value.is_some()));
    let after_label = script.version == 117
        && c.indent == 0
        && !c.muted
        && index > 0
        && script.commands.get(index - 1).is_some_and(|previous| {
            previous.kind == 3
                && previous.indent == 0
                && !previous.muted
                && previous.line.checked_add(1) == Some(c.line)
        });
    ensure!(
        c.kind == 3 || entry_wait || entry_jump || after_label,
        "E_IMPORT_LIVENOVEL: target is not a label, label-entry wait or bounded top-level jump"
    );
    Ok(index)
}

/// Title dispatch is local to the first root preview call, not every use of
/// the same result string in later map/chapter/CG menus.
fn title_profile(script: &Script) -> Result<Script> {
    let index = script.commands.iter().position(|c| !c.muted && matches!(&c.body,
        Body::Call{target,params,..} if target.page.replace('\\',"/") == "ノベルシステム/プレビューメニュー/■選択実行.lsb"
            && params.first().and_then(|e| e.literal.as_ref()).is_some_and(|v|matches!(v,Literal::String(s)if s.is_empty()))))
        .context("E_IMPORT_LIVENOVEL: missing title preview call")?;
    let start = script.commands[..index]
        .iter()
        .rposition(|c| c.kind == 3)
        .unwrap_or(0);
    let end = (index + 1..script.commands.len())
        .find(|i| script.commands[*i].kind == 3)
        .unwrap_or(script.commands.len());
    let mut title = script.clone();
    let mut indices: BTreeSet<_> = (start..end).collect();
    // Some templates create the title background in a separate block, then
    // jump backwards to the preview menu. Keep that incoming block as part
    // of the title, rather than searching unrelated chapter menus.
    let line = script.commands[start].line;
    for (i, command) in script.commands.iter().enumerate() {
        if !command.muted
            && matches!(&command.body, Body::Jump(target, condition)
            if condition.flag().ok() == Some(true) && target.line == line)
        {
            let from = script.commands[..i]
                .iter()
                .rposition(|c| c.kind == 3)
                .unwrap_or(0);
            indices.extend(from..=i);
        }
    }
    title.commands = indices
        .into_iter()
        .map(|i| script.commands[i].clone())
        .collect();
    Ok(title)
}

fn menu_dimensions(data: &[u8]) -> Result<[u32; 2]> {
    let dimensions = if data.starts_with(b"<?xml") {
        ensure!(data.len() <= 1024 * 1024, "E_IMPORT_LPM: XML exceeds 1 MiB");
        let text = super::lsb::decode(data)?;
        let doc = roxmltree::Document::parse_with_options(
            &text,
            roxmltree::ParsingOptions {
                allow_dtd: false,
                nodes_limit: 16_384,
            },
        )?;
        let root = doc.root_element();
        let number = |name| -> Result<u32> {
            let values: Vec<_> = root.children().filter(|n| n.has_tag_name(name)).collect();
            ensure!(
                values.len() == 1,
                "E_IMPORT_LPM: missing/duplicate stage dimension"
            );
            Ok(values[0]
                .text()
                .context("E_IMPORT_LPM: empty stage dimension")?
                .parse()?)
        };
        [number("Width")?, number("Height")?]
    } else {
        ensure!(
            data.starts_with(b"LivePrevMenu106") && data.len() >= 23,
            "E_IMPORT_LPM: stage header"
        );
        [
            u32::from_le_bytes(data[15..19].try_into()?),
            u32::from_le_bytes(data[19..23].try_into()?),
        ]
    };
    ensure!(
        dimensions.iter().all(|n| (1..=8192).contains(n)),
        "E_IMPORT_LPM: invalid stage dimensions"
    );
    Ok(dimensions)
}
fn local_target(script: &Script, page: &str, target: &super::lsb::Reference) -> Result<usize> {
    ensure!(
        target.page.is_empty() || target.page.replace('\\', "/") == page,
        "E_IMPORT_LIVENOVEL: unexpected external scenario jump"
    );
    label(script, target.line)
}
fn empty_subroutine(script: &Script, start: usize) -> bool {
    script.commands[start..]
        .iter()
        .find(|c| !c.muted && !matches!(c.kind, 3 | 27))
        .is_some_and(
            |c| matches!(&c.body, Body::Exit(e) if c.indent == 0 && e.flag().ok() == Some(true)),
        )
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
fn stock_title_menu(script: &Script) -> Result<String> {
    let mut menus = BTreeSet::new();
    for c in &script.commands {
        if c.muted {
            continue;
        }
        if let Body::Call {
            target,
            condition,
            params,
            ..
        } = &c.body
        {
            if target.page.replace('\\', "/") == "ノベルシステム/プレビューメニュー/■選択実行.lsb"
            {
                ensure!(
                    condition.flag()? && target.line == 0 && params.len() == 11,
                    "E_IMPORT_LIVENOVEL: unsupported preview menu call"
                );
                // Only the title profile has an empty object-name parameter.
                // In-story map menus require a separate control-flow lowering.
                if literal_string(&params[0])?.is_empty() {
                    menus.insert(literal_string(&params[1])?.replace('\\', "/"));
                }
            }
        }
    }
    ensure!(
        menus.len() == 1,
        "E_IMPORT_LIVENOVEL: ambiguous title preview menu"
    );
    Ok(menus.into_iter().next().unwrap())
}

/// Title filters may read only monotonic persisted flags. Convert the narrow
/// boolean predicate to a profile guard so title navigation needs no running VM.
fn title_profile_guard(adapter: &Adapter, value: &Value, expected: bool) -> Result<MenuCondition> {
    if value["type"] == "not" {
        return title_profile_guard(adapter, &value["value"], !expected);
    }
    if value["type"] == "binary"
        && value["op"] == "and"
        && value["left"] == json!({"type":"const","value":{"type":"bool","value":true}})
    {
        return title_profile_guard(adapter, &value["right"], expected);
    }
    if value["type"] == "binary" && matches!(value["op"].as_str(), Some("eq" | "ne")) {
        let (variable, constant) = if value["left"]["type"] == "var" {
            (&value["left"], &value["right"])
        } else {
            (&value["right"], &value["left"])
        };
        let name = variable["name"]
            .as_str()
            .context("E_IMPORT_TITLE: flag name")?;
        let key = adapter
            .status_flags
            .get(name)
            .context("E_IMPORT_TITLE: non-profile flag filter")?;
        ensure!(
            constant["type"] == "const"
                && constant["value"]["type"] == "i32"
                && matches!(constant["value"]["value"].as_i64(), Some(0 | 1)),
            "E_IMPORT_TITLE: unsupported flag comparison"
        );
        let present = (constant["value"]["value"] == 1) ^ (value["op"] == "ne") ^ !expected;
        return Ok(MenuCondition::Profile {
            key: key.clone(),
            present,
        });
    }
    bail!("E_IMPORT_TITLE: unsupported dynamic title filter")
}

fn title_filters(
    adapter: &mut Adapter,
    title: &Script,
) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
    let Some(call) = title.commands.iter().position(|c| {
        !c.muted
            && matches!(&c.body,
        Body::Call{target,..} if target.page.replace('\\',"/").ends_with("プレビューメニュー/■選択実行.lsb"))
    }) else {
        bail!("E_IMPORT_TITLE: missing preview call")
    };
    let Some(start) = title.commands[..call].iter().rposition(|c| {
        matches!(&c.body,
        Body::Variable{name,scope:2,..} if name == "_tmp3")
    }) else {
        return Ok(BTreeMap::new());
    };
    let filters = route_support::preview_option_filters(adapter, &title.commands[start + 1..call])?;
    if !filters.is_empty() {
        let (_, helper) = adapter
            .source
            .read("ノベルシステム/プレビューメニュー/■選択実行.lsb")?;
        ensure!(
            helper.version == 117
                && matches!(
                    helper.source_sha256.as_str(),
                    "796b52e73e9cdd8a59a12537865ad1dd41e63707f2be1f4cc777413a3f8720bd"
                        | "5795a8e97d0f0a8d2b94682d4e13f7059a9aaecf3f8c19df21dbd6a3587d8894"
                ),
            "E_IMPORT_TITLE: unverified filtered title helper"
        );
    }
    Ok(filters)
}
fn stock_title_background(script: &Script) -> Result<String> {
    let mut paths = Vec::new();
    let mut origins = BTreeSet::new();
    for c in &script.commands {
        if c.muted {
            continue;
        }
        if let Body::Text { text, .. } = &c.body {
            for glyph in &text.glyphs {
                let Glyph::Event(fields) = glyph else {
                    continue;
                };
                if fields.len() >= 3
                    && matches!(
                        fields[0].trim_start_matches('\u{1}'),
                        "CREATECG" | "CHANGECG"
                    )
                    && fields[1] == "menu"
                {
                    ensure!(
                        !text.has_conditions_or_links
                            && fields.get(3).is_some_and(|mode| mode == "NORMAL")
                            && fields[2].to_ascii_lowercase().ends_with(".gal"),
                        "E_IMPORT_LIVENOVEL: dynamic or animated title background"
                    );
                    paths.push(fields[2].replace('\\', "/"));
                    origins.insert(c.offset);
                }
            }
        }
    }
    ensure!(
        !paths.is_empty(),
        "E_IMPORT_LIVENOVEL: missing title background"
    );
    ensure!(
        paths.iter().collect::<BTreeSet<_>>().len() == 1 || origins.len() == 1,
        "E_IMPORT_LIVENOVEL: multiple title background branches"
    );
    // Stock startup is replaced by the title screen (see mapping ledger).
    // An unconditional startup slideshow settles on its final static image;
    // this does not flatten story animations or accept a cinema as an image.
    Ok(paths.pop().unwrap())
}
fn stock_replay_background(script: &Script) -> Result<String> {
    let mut paths = BTreeSet::new();
    for c in &script.commands {
        if c.muted {
            continue;
        }
        if let Body::Calc(e) = &c.body {
            // The stock page fills `files` with StringToArray's default
            // newline delimiter, then selects a tagged row. A one-row table
            // has a single background regardless of the selected tag.
            if matches!(e.operations.as_slice(), [(11, _, args), (1, _, _)]
                if args.get(1).is_some_and(|v| matches!(v,Literal::Variable(s) if s == "files")))
            {
                let [(11, result, args), (1, destination, tail)] = e.operations.as_slice() else {
                    unreachable!()
                };
                ensure!(
                    e.functions == [(0, 29)].into()
                        && args.len() == 2
                        && destination == "____arg"
                        && matches!(tail.as_slice(), [Literal::Variable(s)] if s == result),
                    "E_IMPORT_LIVENOVEL: unsupported replay background table"
                );
                let Literal::String(path) = &args[0] else {
                    bail!("E_IMPORT_LIVENOVEL: dynamic replay background table");
                };
                ensure!(
                    !path.contains(['\r', '\n']) && !path.is_empty(),
                    "E_IMPORT_LIVENOVEL: multiple/empty replay backgrounds"
                );
                paths.insert(path.replace('\\', "/"));
            }
        }
    }
    ensure!(
        paths.len() == 1,
        "E_IMPORT_LIVENOVEL: ambiguous replay background"
    );
    Ok(paths.into_iter().next().unwrap())
}
struct StockTextbox {
    path: String,
    padding: i32,
    font_size: i32,
    line_height: f32,
    opacity: f32,
}
fn stock_textbox(script: &Script) -> Result<StockTextbox> {
    use super::ui_expr::{normalize, Op, Term};
    let string = |value: &str| Term::String {
        value: value.into(),
    };
    let int = |value| Term::Int { value };
    let apply = |op, args| Term::Apply { op, args };
    let property = |number| {
        apply(
            Op::Property,
            vec![string("メッセージボックス土台"), int(number)],
        )
    };
    let index = script.commands.iter().position(|c| !c.muted && c.kind == 9 && matches!(&c.body, Body::Object(p) if p.get(&1).and_then(|e| e.literal.as_ref()).is_some_and(|v| matches!(v,Literal::String(s) if s == "メッセージボックス土台"))))
        .context("E_IMPORT_LIVENOVEL: missing stock textbox image")?;
    let image = &script.commands[index];
    ensure!(
        image.indent == 1,
        "E_IMPORT_LIVENOVEL: unsupported textbox scope"
    );
    let branch = script.commands[..index]
        .iter()
        .rev()
        .find(|c| !c.muted && c.indent == 0)
        .context("E_IMPORT_LIVENOVEL: missing textbox branch")?;
    let Body::Condition(condition) = &branch.body else {
        bail!("E_IMPORT_LIVENOVEL: missing standard textbox branch");
    };
    ensure!(
        normalize(condition)?
            == Some(apply(
                Op::Equal,
                vec![
                    apply(
                        Op::Index,
                        vec![
                            Term::Read {
                                name: "@ParamStr".into()
                            },
                            int(0)
                        ]
                    ),
                    string("(標準)")
                ]
            )),
        "E_IMPORT_LIVENOVEL: unsupported standard textbox branch"
    );
    let Body::Object(properties) = &image.body else {
        unreachable!()
    };
    let expr = |key| {
        properties
            .get(&key)
            .context("E_IMPORT_LIVENOVEL: missing textbox property")
    };
    ensure!(
        normalize(expr(4)?)?
            == Some(apply(
                Op::Divide,
                vec![
                    apply(
                        Op::Subtract,
                        vec![
                            Term::Read {
                                name: "@ScrWidth".into()
                            },
                            property(5)
                        ]
                    ),
                    int(2)
                ]
            ))
            && normalize(expr(5)?)?
                == Some(apply(
                    Op::Subtract,
                    vec![
                        apply(
                            Op::Subtract,
                            vec![
                                Term::Read {
                                    name: "@ScrHeight".into()
                                },
                                int(10)
                            ]
                        ),
                        property(6)
                    ]
                )),
        "E_IMPORT_LIVENOVEL: unsupported textbox placement"
    );
    let path = literal_string(expr(3)?)?.replace('\\', "/");
    let opacity = literal_int(expr(12)?)?;
    ensure!(
        (0..=255).contains(&opacity),
        "E_IMPORT_LIVENOVEL: textbox opacity"
    );
    let child = script
        .commands
        .get(index + 1)
        .context("E_IMPORT_LIVENOVEL: missing message object")?;
    let Body::Object(text) = &child.body else {
        bail!("E_IMPORT_LIVENOVEL: missing message object");
    };
    let text_expr = |key| {
        text.get(&key)
            .context("E_IMPORT_LIVENOVEL: missing message property")
    };
    ensure!(
        child.kind == 10
            && child.indent == 1
            && literal_string(text_expr(2)?)? == "メッセージボックス土台",
        "E_IMPORT_LIVENOVEL: unsupported message parent"
    );
    let padding = literal_int(text_expr(4)?)?;
    ensure!(
        padding == literal_int(text_expr(5)?)? && matches!(padding, 5 | 10),
        "E_IMPORT_LIVENOVEL: unsupported text insets"
    );
    let font_size = literal_int(text_expr(17)?)?;
    let leading = literal_int(text_expr(19)?)?;
    ensure!(
        font_size == 32 && leading == 8,
        "E_IMPORT_LIVENOVEL: unsupported font metrics"
    );
    Ok(StockTextbox {
        path,
        padding,
        font_size,
        line_height: (font_size + leading) as f32 / font_size as f32,
        opacity: opacity as f32 / 255.,
    })
}
/// Episodes built by op/effect/wait are linear, with explicit activation and
/// wait boundaries. Fold only goto NEXT operations into their successor;
/// retain every operation identity and every task boundary in the same frame.
/// Any explicit branch or block address disables this transformation.
fn compact_linear_episode(blocks: &mut Vec<Value>) {
    if !blocks.iter().all(|block| {
        let term = &block["terminator"];
        match term["type"].as_str() {
            Some("goto") => term["target"] == "NEXT",
            Some("activate") => term["next"] == "NEXT",
            Some("await") => {
                term["next"] == "NEXT"
                    && term["on_cancelled"] == "cancelled"
                    && term["on_failed"] == "failed"
            }
            _ => false,
        }
    }) {
        return;
    }
    let mut compact = Vec::with_capacity(blocks.len());
    let mut pending = Vec::new();
    for mut block in std::mem::take(blocks) {
        pending.append(
            block["ops"]
                .as_array_mut()
                .expect("authored episode operations"),
        );
        if block["terminator"]["type"] != "goto" {
            block["ops"] = json!(std::mem::take(&mut pending));
            compact.push(block);
        }
    }
    if !pending.is_empty() {
        compact.push(json!({"ops":pending,"terminator":{"type":"goto","target":"NEXT"}}));
    }
    *blocks = compact;
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
            share_episodes: false,
            function_aliases: BTreeMap::new(),
            operation_sources: BTreeMap::new(),
            choice_declarations: BTreeMap::new(),
            cues: BTreeMap::new(),
            scenes: BTreeMap::new(),
            image_template_assets: BTreeMap::new(),
            sprite_timelines: BTreeMap::new(),
            pending_movies: vec![],
            nodes: BTreeMap::new(),
            committed_images: BTreeSet::new(),
            centered: BTreeSet::new(),
            anchors: BTreeMap::new(),
            audio: BTreeMap::new(),
            gain_reads: BTreeSet::from(["bgm".into(), "voice".into(), "sfx".into()]),
            missing_menu_sounds: BTreeSet::new(),
            geometry_reads: BTreeSet::new(),
            anchor_reads: None,
            motion_tasks: BTreeMap::new(),
            movie_tasks: BTreeMap::new(),
            returns: vec![],
            pending_visibility: None,
            menus: BTreeMap::new(),
            choices: BTreeMap::new(),
            variables: BTreeMap::new(),
            boolean_variables: BTreeSet::new(),
            status_flags: BTreeMap::new(),
            status_values: BTreeMap::new(),
            unsupported_status: BTreeSet::new(),
            bitmap_slots: BTreeSet::new(),
            implicit_integer_assignments: vec![],
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
            page_ordinal: 0,
            textbox: String::new(),
            textbox_theme: None,
            dialogue_styles: BTreeMap::new(),
            decoration_point: None,
            title_menu_version: "LPM106",
            stage: [1024, 768],
            text_color: [1.; 4],
            title_route_entries: BTreeSet::new(),
            cg_images: BTreeSet::new(),
            declared_cg_names: BTreeSet::new(),
            indexed_cg_scripts: BTreeSet::new(),
        }
    }
    fn id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        let id = format!("{prefix}_{}", self.counter);
        self.source_map.insert(id.clone(), self.location.clone());
        id
    }
    fn intern_text(&mut self, role: &str, ordinal: usize, doc: crate::AuthorTextDoc) -> String {
        // Recompiling one source paragraph for another scene state must not
        // manufacture another read identity. Equal words in other commands
        // and repeated paragraphs on separate source pages remain distinct.
        let digest = nir_content::digest(
            &serde_json::to_vec(&(&self.location, role, ordinal, &doc))
                .expect("serializable source text"),
        );
        let id = format!("text_{digest}");
        self.texts.entry(id.clone()).or_insert(doc);
        self.source_map
            .entry(id.clone())
            .or_insert_with(|| self.location.clone());
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
            if path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("lcm") || s.eq_ignore_ascii_case("lmt"))
            {
                let film = super::cinema::parse(&read_binary(&path)?)
                    .with_context(|| format!("cinema {name}"))?;
                bail!("E_IMPORT_CINEMA_ANIMATION: {name}: LiveCinema{} ({}x{}, {} clips) requires timeline playback adaptation",film.version,film.size[0],film.size[1],film.clips.len());
            }
            let (w, h) = media::gal_file_size(&path).with_context(|| format!("image {name}"))?;
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
                self.stage,
                [
                    (color & 255) as f32 / 255.,
                    ((color >> 8) & 255) as f32 / 255.,
                    ((color >> 16) & 255) as f32 / 255.,
                    1.,
                ],
            ))
        } else {
            if Path::new(path)
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("lcm") || s.eq_ignore_ascii_case("lmt"))
            {
                let film = super::cinema::parse(&read_binary(&self.source.path(path)?)?)?;
                return Ok((None, film.size, [0.; 4]));
            }
            let (id, size) = self.image(path)?;
            Ok((Some(id), size, [1.; 4]))
        }
    }
    fn sound(&mut self, path: &str, gain: f32) -> Result<String> {
        let path = self
            .source
            .path(path)
            .with_context(|| format!("sound {path}"))?;
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
    fn menu_sound(&mut self, path: &str) -> Result<Option<String>> {
        match self.sound(path, 1.) {
            Ok(asset) => {
                self.menu_sounds += 1;
                Ok(Some(asset))
            }
            Err(error) if error.downcast_ref::<super::MissingSourcePath>().is_some() => {
                self.missing_menu_sounds.insert(path.replace('\\', "/"));
                self.warnings.insert(format!("Missing source menu click sound {path} is silent; explicit acceptance of livenovel.missing-menu-audio is required. Story media remain strict."));
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    fn op(&mut self, blocks: &mut Vec<Value>, operation: Value) {
        let id = self.id("op");
        blocks.push(json!({"ops":[{"id":id,"operation":operation}],"terminator":{"type":"goto","target":"NEXT"}}));
    }
    fn profile_reads(&mut self, reads: &BTreeSet<String>) -> Vec<Value> {
        let flags: Vec<_> = self
            .status_flags
            .iter()
            .filter(|(name, _)| reads.contains(*name))
            .map(|(name, key)| (name.clone(), key.clone(), "profile_read"))
            .chain(
                self.status_values
                    .iter()
                    .filter(|(name, _)| reads.contains(*name))
                    .map(|(name, key)| (name.clone(), key.clone(), "profile_value_read")),
            )
            .collect();
        flags
            .into_iter()
            .map(|(target, key, operation)| {
                let id = self.id("profile_read");
                json!({"id":id,"operation":{"type":operation,"target":target,"key":key}})
            })
            .collect()
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
    fn geometry_extent(&self, id: &str) -> Result<[f32; 2]> {
        let node = self
            .nodes
            .get(id)
            .context("E_IMPORT_BITMAP: missing parent")?;
        if inheritable_image(node)
            && self.committed_images.contains(id)
            && self.anchors.contains_key(id)
            && !self.geometry_reads.contains(id)
        {
            return Err(GeometryReadRequired(id.into()).into());
        }
        Ok([node.width, node.height])
    }
    fn scene(&mut self, blocks: &mut Vec<Value>, duration: u64) {
        let inherit_images: Vec<_> = self
            .nodes
            .values()
            .filter(|node| inheritable_image(node) && self.committed_images.contains(&node.id))
            .map(|node| node.id.clone())
            .collect();
        let geometry: Vec<_> = inherit_images
            .iter()
            .filter(|id| self.anchors.contains_key(*id) && !self.geometry_reads.contains(*id))
            .cloned()
            .collect();
        let mut nodes: Vec<_> = self.nodes.values().cloned().collect();
        for node in nodes.iter_mut().filter(|node| inheritable_image(node)) {
            let template = self
                .image_template_assets
                .entry(node.id.clone())
                .or_insert_with(|| node.asset.as_ref().unwrap().clone());
            if inherit_images.contains(&node.id) {
                // The concrete binding is captured by Core, so this unused
                // template reference can be shared across incoming pictures.
                node.asset = Some(template.clone());
            }
            if geometry.contains(&node.id) {
                // All four values are replaced atomically at commit. Source
                // geometry reads retain their actual rectangle instead.
                [node.x, node.y, node.width, node.height] = [0., 0., 1., 1.];
            }
        }
        let scene = format!(
            "scene_{}",
            nir_content::digest(&serde_json::to_vec(&nodes).expect("serializable scene recipe"))
        );
        self.scenes.entry(scene.clone()).or_insert(nodes);
        self.source_map
            .entry(scene.clone())
            .or_insert_with(|| self.location.clone());
        let mut effect =
            json!({"type":"stage_present","scene":scene,"duration_us":duration.to_string()});
        if !inherit_images.is_empty() {
            effect["inherit_images"] = json!(inherit_images);
            if !geometry.is_empty() {
                effect["inherit_image_geometry"] = json!(geometry);
            }
        }
        self.committed_images = self
            .nodes
            .values()
            .filter(|node| inheritable_image(node))
            .map(|node| node.id.clone())
            .collect();
        if let Some(visible) = self.pending_visibility.take() {
            effect["dialogue_visible"] = json!(visible);
        }
        // Prepare all image assets atomically, commit the scene first, then
        // start the movie at that same clock. Waiting for a dissolve consumes
        // movie time rather than delaying its start until the wipe ends.
        let mut effects = vec![json!({"id":"stage","scope":"session","effect":effect})];
        effects.append(&mut self.pending_movies);
        let cue = self.id("cue");
        self.cues.insert(cue.clone(), json!({"effects":effects}));
        blocks.push(json!({"ops":[],"terminator":{"type":"activate","cue":cue,"next":"NEXT"}}));
        if duration > 0 {
            self.wait(blocks, "stage", json!({"type":"finished"}));
        }
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
            // A trailing ! reverses the source wipe. Both directions use the
            // explicitly reported dissolve approximation.
            let value = arg(n)?.trim_end_matches('!');
            let n = if value.is_empty() {
                0
            } else {
                value
                    .parse::<u64>()
                    .with_context(|| format!("E_IMPORT_EVENT: {name} argument {n}"))?
            };
            ensure!(n <= 60_000, "E_IMPORT_EVENT: duration out of range");
            Ok(n)
        };
        match name {
            "FACE" | "NAMELABEL" => decoration::event(self, name, p, blocks)?,
            "MOTION" => motion::event(self, p, blocks)?,
            "QUAKE" | "QUAKESTOP" => quake::event(self, name, p, blocks)?,
            "CGCHARNEW" | "CGCHARCHG" => bitmap::event(self, name, p, blocks)?,
            "PAUSEMEDIA" | "RESUMEMEDIA" => {
                ensure!(p.len() == 1, "E_IMPORT_EVENT: audio pause argument count");
                let bus = match arg(0)? {
                    "BGM" => "bgm",
                    "VOICE" => "voice",
                    "SE" => "sfx",
                    _ => bail!("E_IMPORT_EVENT: unknown audio pause channel"),
                };
                self.op(
                    blocks,
                    json!({"type":"audio_pause","bus":bus,"paused":name == "PAUSEMEDIA"}),
                );
            }
            "MENUENABLED" | "MENUDISABLED" => {
                self.op(
                    blocks,
                    json!({"type":"menu_access","enabled":name == "MENUENABLED"}),
                );
            }
            "WAITPLAY" => {
                ensure!(
                    p.len() == 2 && matches!(arg(1)?, "NORMAL" | "CLICK"),
                    "E_IMPORT_EVENT: wait-play arguments"
                );
                if let Ok((channel, _)) = sound_channel(arg(0)?) {
                    if self.audio.contains_key(channel) {
                        self.wait(blocks, channel, json!({"type":"finished"}));
                        // Stock WAITPLAY CLICK exits its wait on LClick/Enter;
                        // it does not delete or stop the playing sound.
                        if arg(1)? == "CLICK" {
                            blocks.last_mut().unwrap()["terminator"]["on_advance"] = json!("NEXT");
                        }
                    }
                } else if let Some(task) = self.movie_tasks.get(arg(0)?).cloned() {
                    self.wait(blocks, &task, json!({"type":"finished"}));
                    if arg(1)? == "CLICK" {
                        blocks.last_mut().unwrap()["terminator"]["on_advance"] = json!("NEXT");
                    }
                } else if self.motion_tasks.contains_key(&motion::task_name(arg(0)?)) {
                    let task = motion::task_name(arg(0)?);
                    self.wait(blocks, &task, json!({"type":"finished"}));
                    if arg(1)? == "CLICK" {
                        blocks.last_mut().unwrap()["terminator"]["on_advance"] = json!("NEXT");
                    }
                } else {
                    // Static imported images never expose an active animation.
                    // The source Property(138) wait is therefore already done.
                    ensure!(
                        self.nodes.contains_key(arg(0)?),
                        "E_IMPORT_EVENT: unknown wait-play target"
                    );
                }
            }
            "" | "MESEND" => {}
            "MESON" | "MESOFF" => {
                if p.first().is_some_and(|value| value == "-1") {
                    self.pending_visibility = Some(name == "MESON");
                    return Ok(());
                }
                self.pending_visibility = None;
                // The stock handler initializes its integer `tm` to zero;
                // an omitted/blank duration keeps that immediate-toggle value.
                let fade_ms = if p.first().is_none_or(String::is_empty) {
                    0
                } else {
                    number(0)?
                };
                ensure!(
                    fade_ms <= 60_000,
                    "E_IMPORT_EVENT: message fade exceeds 60 seconds"
                );
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
                let fade_in = number(4)?;
                let (channel, bus) = sound_channel(arg(1)?)?;
                let looped = match arg(2)? {
                    "REPEAT" => true,
                    "NORMAL" | "WAIT" => false,
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
                self.play_sound(blocks, channel, effect);
                if fade_in > 0 {
                    let cue = blocks.last().unwrap()["terminator"]["cue"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let effects = self.cues.get_mut(&cue).unwrap()["effects"]
                        .as_array_mut()
                        .unwrap();
                    effects.push(json!({"id":format!("{channel}_fade_in"),"scope":"session","effect":{"type":"sequence","children":[
                        {"id":format!("{channel}_fade_zero"),"scope":"session","effect":{"type":"tween","target":{"type":"audio_instance","task":channel,"property":"gain"},"to":0.,"duration_us":"0"}},
                        {"id":format!("{channel}_fade_rise"),"scope":"session","effect":{"type":"tween","target":{"type":"audio_instance","task":channel,"property":"gain"},"to":1.,"duration_us":(fade_in*1000).to_string()}}
                    ]}}));
                }
                if arg(2)? == "WAIT" {
                    self.wait(blocks, channel, json!({"type":"finished"}));
                }
            }
            "REPLAYSND" => {
                ensure!(p.len() == 3, "E_IMPORT_EVENT: replay sound arguments");
                let event =
                    ["PLAYSND", arg(1)?, arg(0)?, "NORMAL", arg(2)?, "0"].map(str::to_owned);
                self.event(&event, blocks)?;
            }
            "CHGVOL" => {
                ensure!(p.len() == 4, "E_IMPORT_EVENT: volume change arguments");
                let (channel, _) = sound_channel(arg(0)?)?;
                if !self.gain_reads.contains(channel) {
                    return Err(GainReadRequired(channel.into()).into());
                }
                let gain = arg(1)?.parse::<f32>()? / 1000.;
                ensure!(
                    nir_format::valid_audio_gain(gain),
                    "E_IMPORT_EVENT: bounded volume required"
                );
                let duration = number(2)?;
                ensure!(
                    matches!(arg(3)?, "PASS" | "WAIT"),
                    "E_IMPORT_EVENT: volume change wait mode"
                );
                if !self.audio.contains_key(channel) {
                    let (_, events) = self
                        .source
                        .read("ノベルシステム/メッセージボックス/イベント.lsb")?;
                    let (_, functions) = self.source.read("ノベルシステム/■関数.lsb")?;
                    ensure!(events.version == 117 && functions.version == 117
                        && events.source_sha256 == "3e7df74bdfcbbca60e46863ab59f1b66e2f9c9ad3f99403cb3ab450ed39734fe"
                        && functions.source_sha256 == "ddff88a96d8f0bbe2c792ee0955dedc63bd16730ee3b33d84839afdef893137b",
                        "E_IMPORT_EVENT: unverified inactive volume target");
                    // Stock PropMotion returns finished when name lookup is
                    // absent (1435b0..1435c6). Its volume helper's private Per
                    // slot is overwritten by each admitted explicit PLAYSND;
                    // direct authored reads of those arrays remain rejected.
                    return Ok(());
                }
                let base = self
                    .audio
                    .get(channel)
                    .and_then(|effect| effect["gain"].as_f64())
                    .context("E_IMPORT_EVENT: volume change has no sound instance")?
                    as f32;
                let to = gain / base;
                ensure!(
                    base > 0. && to.is_finite() && (0. ..=1.).contains(&to),
                    "E_IMPORT_EVENT: volume increase requires gain adaptation"
                );
                ensure!(
                    matches!(arg(3)?, "PASS" | "WAIT"),
                    "E_IMPORT_EVENT: volume change wait mode"
                );
                let task = format!("{channel}_volume");
                self.effect(blocks, &task, "session", json!({"type":"tween","target":{"type":"audio_instance","task":channel,"property":"gain"},"to":to,"duration_us":(duration*1000).to_string()}), arg(3)? == "WAIT");
            }
            "STOPSND" => {
                let (channel, _) = sound_channel(arg(0)?)?;
                ensure!(
                    matches!(arg(2)?, "PASS" | "WAIT"),
                    "E_IMPORT_EVENT: unsupported sound stop"
                );
                let duration = number(1)?;
                ensure!(
                    duration <= 60_000,
                    "E_IMPORT_EVENT: sound fade exceeds 60 seconds"
                );
                let active = self.audio.contains_key(channel);
                self.fade_stop(blocks, channel, duration * 1000);
                if active && arg(2)? == "WAIT" {
                    self.wait(
                        blocks,
                        &format!("{channel}_stop"),
                        json!({"type":"finished"}),
                    );
                }
            }
            "CREATECG" | "CHANGECG" | "DELETECG" => {
                let mut unlocked_image = None;
                let (wipe, duration) = match name {
                    "CREATECG" => {
                        ensure!(
                            matches!(arg(2)?, "NORMAL" | "WAIT" | "SCRAP") && arg(3)? == "#1",
                            "E_IMPORT_EVENT: unsupported image placement"
                        );
                        let (asset, size, color) = self.visual(arg(1)?)?;
                        if let Some(asset) = &asset {
                            if self.cg_images.contains(asset) {
                                unlocked_image = Some(asset.clone());
                            }
                        }
                        let id = arg(0)?.to_owned();
                        self.committed_images.remove(&id);
                        let x = image_position(arg(4)?, self.stage[0], size[0], true)?;
                        let y = image_position(arg(5)?, self.stage[1], size[1], false)?;
                        self.anchors
                            .insert(id.clone(), (arg(4)?.into(), arg(5)?.into()));
                        if arg(4)? == "C" && arg(5)? == "B" {
                            self.centered.insert(id.clone());
                        } else {
                            self.centered.remove(&id);
                        }
                        self.nodes.insert(
                            id.clone(),
                            Node {
                                id,
                                parent: None,
                                asset,
                                x,
                                y,
                                width: size[0] as f32,
                                height: size[1] as f32,
                                scale: 1.,
                                opacity: 1.,
                                color,
                                order: arg(6)?.parse()?,
                                clip: None,
                                timeline_binding: None,
                                inherit_existence: false,
                                sprite_transform: None,
                                bitmap_text: None,
                                preserve_pose: vec![],
                                offset: [0.; 2],
                            },
                        );
                        (number(7)?, number(8)?)
                    }
                    "CHANGECG" => {
                        ensure!(
                            matches!(arg(2)?, "NORMAL" | "WAIT"),
                            "E_IMPORT_EVENT: unsupported image change mode"
                        );
                        let (asset, size, color) = self.visual(arg(1)?)?;
                        if let Some(asset) = &asset {
                            if self.cg_images.contains(asset) {
                                unlocked_image = Some(asset.clone());
                            }
                        }
                        let id = arg(0)?.to_owned();
                        self.committed_images.remove(&id);
                        let centered = self.centered.contains(&id);
                        if self
                            .anchor_reads
                            .as_ref()
                            .is_some_and(|reads| !reads.contains(&id))
                        {
                            return Err(AnchorReadRequired(id).into());
                        }
                        let anchor = self.anchors.get(&id).cloned();
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
                            timeline_binding: None,
                            inherit_existence: false,
                            sprite_transform: None,
                            bitmap_text: None,
                            preserve_pose: vec![],
                            offset: [0.; 2],
                        });
                        node.asset = asset;
                        node.bitmap_text = None;
                        node.timeline_binding = None;
                        node.inherit_existence = false;
                        node.clip = None;
                        node.color = color;
                        node.width = size[0] as f32;
                        node.height = size[1] as f32;
                        if let Some((x, y)) = anchor {
                            node.x = image_position(&x, self.stage[0], size[0], true)?;
                            node.y = image_position(&y, self.stage[1], size[1], false)?;
                        } else if centered {
                            node.x = (self.stage[0] as f32 - node.width) / 2.;
                            node.y = self.stage[1] as f32 - node.height;
                        }
                        (number(3)?, number(4)?)
                    }
                    _ => {
                        let mut deleted: BTreeSet<String> =
                            arg(0)?.split(',').map(str::to_owned).collect();
                        loop {
                            let children: Vec<_> = self
                                .nodes
                                .values()
                                .filter(|node| {
                                    node.parent.as_ref().is_some_and(|id| deleted.contains(id))
                                })
                                .map(|node| node.id.clone())
                                .collect();
                            let before = deleted.len();
                            deleted.extend(children);
                            if before == deleted.len() {
                                break;
                            }
                        }
                        for id in deleted {
                            let tasks: Vec<_> = self
                                .motion_tasks
                                .iter()
                                .filter(|(_, node)| *node == &id)
                                .map(|(task, _)| task.clone())
                                .collect();
                            for task in tasks {
                                self.op(
                                    blocks,
                                    json!({"type":"task_control","task":task,"action":"cancel"}),
                                );
                                self.motion_tasks.remove(&task);
                            }
                            self.nodes.remove(&id);
                            self.committed_images.remove(&id);
                            self.movie_tasks.remove(&id);
                            self.centered.remove(&id);
                            self.anchors.remove(&id);
                        }
                        (number(1)?, number(2)?)
                    }
                };
                if wipe != 0 && duration > 0 {
                    self.warnings.insert(format!("LiveMaker wipe {wipe} is represented by a dissolve with the original duration."));
                }
                if matches!(name, "CREATECG" | "CHANGECG") {
                    cinema::attach(self, arg(0)?, arg(1)?)?;
                    if arg(2)? == "SCRAP" {
                        let root = arg(0)?.to_owned();
                        let pending = self
                            .pending_movies
                            .last_mut()
                            .filter(|movie| movie["effect"]["root"].as_str() == Some(root.as_str()))
                            .context("E_IMPORT_CINEMA_SCRAP: finite motion required")?;
                        let path = self.source.path(arg(1)?)?;
                        ensure!(
                            crate::import::cinema::parse(&read_binary(&path)?)?.version == 111,
                            "E_IMPORT_CINEMA_SCRAP: only certified Motion111 completion clock"
                        );
                        pending["effect"]["delete_on_finish"] = json!(true);
                    }
                }
                self.scene(blocks, duration * 1000);
                if matches!(name, "CREATECG" | "CHANGECG") && arg(2)? == "SCRAP" {
                    self.nodes.get_mut(arg(0)?).unwrap().inherit_existence = true;
                }
                if matches!(name, "CREATECG" | "CHANGECG") && arg(2)? == "WAIT" {
                    if let Some(task) = self.movie_tasks.get(arg(0)?).cloned() {
                        self.wait(blocks, &task, json!({"type":"finished"}));
                    }
                }
                if let Some(image) = unlocked_image {
                    self.op(
                        blocks,
                        json!({"type":"profile_merge","key":format!("lm.cg.{image}")}),
                    );
                }
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
        let mut ruby_style = None;
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
            if !matches!(glyph, Glyph::RubyChar { .. }) {
                ruby_style = None;
            }
            match glyph {
                Glyph::InlineImage {
                    source,
                    align,
                    hover,
                    margins,
                    down,
                } => {
                    ensure!(
                        hover.is_empty() && down.is_empty(),
                        "E_IMPORT_GLYPH: interactive inline image requires a link adapter"
                    );
                    let align = match align {
                        3 => nir_format::InlineImageAlign::Center,
                        4 => nir_format::InlineImageAlign::Top,
                        5 => nir_format::InlineImageAlign::Bottom,
                        _ => bail!("E_IMPORT_GLYPH: invalid inline image alignment"),
                    };
                    let margins: Vec<u32> = margins
                        .iter()
                        .map(|n| u32::try_from(*n))
                        .collect::<std::result::Result<_, _>>()?;
                    let (asset, size) = self.image(source)?;
                    let image = nir_format::InlineImage {
                        asset,
                        width: size[0],
                        height: size[1],
                        align,
                        margins: margins.try_into().unwrap(),
                    };
                    ensure!(
                        image.valid(),
                        "E_IMPORT_GLYPH: inline image geometry exceeds bounds"
                    );
                    flush(&mut buffer, &mut spans);
                    spans.push(Span::Image {
                        id: format!("s{}", spans.len()),
                        image,
                    });
                }
                Glyph::Char(s) => buffer.push_str(s),
                Glyph::Variable(name) => {
                    // A source variable glyph is a slot read, frozen by the
                    // existing text-parameter contract when this line starts.
                    route_support::expression(
                        self,
                        &super::ui_expr::Term::Read { name: name.clone() },
                    )?;
                    flush(&mut buffer, &mut spans);
                    spans.push(Span::Param {
                        id: format!("s{}", spans.len()),
                        name: name.clone(),
                    });
                    if self.status_flags.contains_key(name) || self.status_values.contains_key(name)
                    {
                        for op in self.profile_reads(&BTreeSet::from([name.clone()])) {
                            blocks.push(
                                json!({"ops":[op],"terminator":{"type":"goto","target":"NEXT"}}),
                            );
                        }
                    }
                }
                Glyph::RubyChar {
                    text,
                    reading,
                    style,
                } => {
                    flush(&mut buffer, &mut spans);
                    if ruby_style == Some(*style) {
                        if let Some(Span::Ruby { text: previous, .. }) = spans.last_mut() {
                            previous.push_str(text);
                        }
                    } else {
                        spans.push(Span::Ruby {
                            id: format!("s{}", spans.len()),
                            text: text.clone(),
                            reading: reading.clone(),
                        });
                    }
                    ruby_style = Some(*style);
                }
                Glyph::Break(0) => {
                    flush(&mut buffer, &mut spans);
                    spans.push(Span::Break {
                        id: format!("s{}", spans.len()),
                    });
                }
                Glyph::Break(2) => {
                    flush(&mut buffer, &mut spans);
                    spans.push(Span::Pause {
                        id: format!("s{}", spans.len()),
                        timeout_us: None,
                    });
                }
                Glyph::Event(fields) => {
                    let name = fields
                        .first()
                        .map(|s| s.trim_start_matches('\u{1}'))
                        .unwrap_or("");
                    if name == "PAUSE"
                        || (name == "WAIT" && fields.get(2).is_some_and(|s| s == "CLICK"))
                    {
                        let timeout_us = if name == "WAIT" {
                            ensure!(
                                fields.get(3).is_some_and(|s| s == "SKIP"),
                                "E_IMPORT_EVENT: unsupported click wait mode"
                            );
                            let duration = fields
                                .get(1)
                                .context("E_IMPORT_EVENT: missing click wait duration")?
                                .parse::<u64>()?;
                            ensure!(
                                duration <= 60_000,
                                "E_IMPORT_EVENT: click wait exceeds 60 seconds"
                            );
                            Some(nir_format::Micros(duration * 1000))
                        } else {
                            ensure!(
                                fields.len() <= 2 && fields.get(1).is_none_or(String::is_empty),
                                "E_IMPORT_EVENT: unsupported pause arguments"
                            );
                            None
                        };
                        flush(&mut buffer, &mut spans);
                        spans.push(Span::Pause {
                            id: format!("s{}", spans.len()),
                            timeout_us,
                        });
                        continue;
                    }
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
        let tid = self.intern_text(
            "paragraph",
            self.page_ordinal,
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
    fn intern_choice(&mut self, definition: Value) -> String {
        let key =
            serde_json::to_vec(&(&self.location, &definition)).expect("serializable source choice");
        if self.share_episodes {
            if let Some(choice) = self.choice_declarations.get(&key) {
                return choice.clone();
            }
        }
        let choice = self.id("choice");
        self.choices.insert(choice.clone(), definition);
        if self.share_episodes {
            self.choice_declarations.insert(key, choice.clone());
        }
        choice
    }
    fn finish_function(&mut self, name: &str, mut blocks: Vec<Value>, end: Value) {
        let share = self.share_episodes
            && (name.starts_with("episode") || name.starts_with("replay_episode"));
        let mut cue_aliases = BTreeMap::<String, String>::new();
        if share {
            let full_source = nir_content::digest(
                &serde_json::to_vec(&self.location).expect("serializable source location"),
            );
            // Keep compact, stable operation identities without relying on
            // truncated digests being collision-free. A conflicting source
            // retains its complete digest and its own source-map entries.
            let short_source = full_source[..32].to_owned();
            let source = match self.operation_sources.get(&short_source) {
                Some(previous) if previous != &full_source => full_source,
                _ => {
                    self.operation_sources
                        .insert(short_source.clone(), full_source);
                    short_source
                }
            };
            for (index, block) in blocks.iter_mut().enumerate() {
                if let Some(ops) = block["ops"].as_array_mut() {
                    for (ordinal, op) in ops.iter_mut().enumerate() {
                        let id = format!("op_{source}_b{index}_o{ordinal}");
                        if let Some(old) = op["id"].as_str() {
                            self.source_map.remove(old);
                        }
                        self.source_map
                            .entry(id.clone())
                            .or_insert_with(|| self.location.clone());
                        op["id"] = json!(id);
                    }
                }
                if let Some(old) = block["terminator"]["cue"]
                    .as_str()
                    .filter(|s| s.starts_with("cue_"))
                    .map(str::to_owned)
                {
                    let shared = if let Some(shared) = cue_aliases.get(&old) {
                        shared.clone()
                    } else {
                        let cue = self.cues.remove(&old).expect("authored episode cue");
                        let shared = format!(
                            "shared_cue_{}",
                            nir_content::digest(
                                &serde_json::to_vec(&cue).expect("serializable cue")
                            )
                        );
                        self.cues.entry(shared.clone()).or_insert(cue);
                        if let Some(location) = self.source_map.remove(&old) {
                            self.source_map.entry(shared.clone()).or_insert(location);
                        }
                        cue_aliases.insert(old, shared.clone());
                        shared
                    };
                    block["terminator"]["cue"] = json!(shared);
                }
            }
            compact_linear_episode(&mut blocks);
        }
        let length = blocks.len();
        let mut table = BTreeMap::new();
        for (i, b) in blocks.iter_mut().enumerate() {
            let term = &mut b["terminator"];
            for key in ["target", "next", "on_advance"] {
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
        let mut function = json!({"entry":"b000000","blocks":table});
        if share {
            let digest = nir_content::digest(
                &serde_json::to_vec(&(&self.location, &function)).expect("serializable episode"),
            );
            let shared = format!("shared_episode_{digest}");
            // Source position alone cannot identify an operation globally:
            // two scene-state variants may share their first operations but
            // contain different later cues. Namespace the original instruction
            // coordinates by the complete canonical episode, after sharing.
            let short = digest[..32].to_owned();
            let namespace = match self.operation_sources.get(&short) {
                Some(previous) if previous != &digest => digest,
                _ => {
                    self.operation_sources.insert(short.clone(), digest);
                    short
                }
            };
            for block in function["blocks"].as_object_mut().unwrap().values_mut() {
                for op in block["ops"].as_array_mut().unwrap() {
                    let old = op["id"].as_str().unwrap().to_owned();
                    let (_, coordinate) = old.split_once("_b").unwrap();
                    let id = format!("op_{namespace}_b{coordinate}");
                    self.source_map.remove(&old);
                    self.source_map
                        .entry(id.clone())
                        .or_insert_with(|| self.location.clone());
                    op["id"] = json!(id);
                }
            }
            self.functions.entry(shared.clone()).or_insert(function);
            self.source_map
                .entry(shared.clone())
                .or_insert_with(|| self.location.clone());
            self.function_aliases.insert(name.into(), shared);
        } else {
            self.functions.insert(name.into(), function);
        }
    }
    /// Advances past commands the route walk ignores: labels, muted/system
    /// lines, the validated no-op scenario calc/wait, and the replay-index
    /// conditional jump that never fires on a fresh route.
    fn skip_forward(&self, script: &Script, mut pc: usize) -> Result<usize> {
        loop {
            let c = script.commands.get(pc).with_context(|| {
                format!(
                    "E_IMPORT_LIVENOVEL: unexpected route end at command {pc}; previous {}:{}",
                    self.location.source, self.location.line
                )
            })?;
            let skip = c.muted
                || c.kind == 3
                || c.kind == 27
                || match &c.body {
                    Body::Calc(e) => e.operations.iter().all(|(op, name, args)| {
                        *op == 1
                            && name == "__メッセージ終了"
                            && matches!(args.as_slice(), [Literal::Int(0)])
                    }),
                    Body::Wait(e) => {
                        e.len() == 3
                            && (references(&e[0], "__メッセージ終了")
                                || e[0].flag().ok() == Some(true))
                            && literal_int(&e[1]).ok() == Some(0)
                            && literal_int(&e[2]).ok() == Some(0)
                    }
                    Body::Call {
                        target,
                        condition,
                        params,
                        ..
                    } if target.page.replace('\\', "/")
                        == "ノベルシステム/メッセージボックス/終了待ち.lsb" =>
                    {
                        // Each imported text episode already awaits its final
                        // page. This stock helper dispatches system UI while
                        // waiting for the same asynchronous completion flag.
                        ensure!(
                            condition.flag()? && target.line == 0 && params.is_empty(),
                            "E_IMPORT_LIVENOVEL: unsupported message completion call"
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
            pc = route_support::successor(script, pc)?;
        }
    }
    /// The block that continues at a command index; allocates a fresh route
    /// block id on first visit.
    fn route_block(
        &mut self,
        script: &Script,
        page: &str,
        pc: usize,
        routes: &mut Routes,
    ) -> Result<String> {
        let pc = self.skip_forward(script, pc)?;
        let state = RouteState {
            nodes: self.nodes.clone(),
            committed_images: self.committed_images.clone(),
            centered: self.centered.clone(),
            anchors: self.anchors.clone(),
            audio: self.audio.clone(),
            motion_tasks: self.motion_tasks.clone(),
            movie_tasks: self.movie_tasks.clone(),
            returns: self.returns.clone(),
            pending_visibility: self.pending_visibility,
        };
        let key = (
            page.to_owned(),
            pc,
            state.signature_for_route(
                &initial_scene_deletions(script, pc, &state.nodes),
                &self.gain_reads,
                &self.geometry_reads,
                self.anchor_reads.as_ref(),
            )?,
        );
        if let Some(id) = routes.variants.get(&key) {
            return Ok(id.clone());
        }
        // Scene/text/cue/episode resources are interned separately. Keep the
        // offline specialization walk bounded while admitting large routes;
        // runtime function, text and block limits still validate the result.
        let variant_limit = 65_536;
        #[cfg(test)]
        let variant_limit = std::env::var("NIR_LIVENOVEL_ROUTE_VARIANT_LIMIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| (16_384..=65_536).contains(n))
            .unwrap_or(variant_limit);
        #[cfg(test)]
        if routes.variants.len() >= variant_limit {
            write_route_limit_probe(routes, page, pc, &state, self)?;
        }
        ensure!(
            routes.variants.len() < variant_limit,
            "E_IMPORT_LIMIT: route state variants exceed {variant_limit}"
        );
        let id = self.id("route");
        routes
            .entries
            .entry((page.to_owned(), pc))
            .or_insert(id.clone());
        routes.variants.insert(key, id.clone());
        routes.states.insert(id.clone(), state);
        Ok(id)
    }
    fn enqueue_route(
        &mut self,
        script: &Script,
        page: &str,
        pc: usize,
        path: RoutePath,
        routes: &mut Routes,
    ) -> Result<()> {
        let id = self.route_block(script, page, pc, routes)?;
        routes.queue.push((page.to_owned(), pc, id, path));
        Ok(())
    }
    fn route_target(
        &mut self,
        page: &str,
        target: &Reference,
        routes: &mut Routes,
    ) -> Result<(String, std::sync::Arc<Script>, usize)> {
        let destination = if target.page.is_empty() {
            page.to_owned()
        } else {
            target.page.replace('\\', "/")
        };
        let (destination, script) = if let Some(script) = routes.scripts.get(&destination) {
            (destination, script.clone())
        } else {
            let (name, script) = self.source.read(&destination)?;
            ensure!(
                matches!(script.version, 116 | 117),
                "E_IMPORT_VERSION: external route script"
            );
            quake::index_cg_names(self, &name, &script)?;
            routes.scripts.insert(name.clone(), script.clone());
            (name, script)
        };
        let pc = label(&script, target.line).with_context(|| {
            format!(
                "E_IMPORT_ROUTE_TARGET: {page} -> {destination}:{}",
                target.line
            )
        })?;
        Ok((destination, script, pc))
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
        quake::index_cg_names(self, page, script)?;
        let title_page = page.to_owned();
        // Each main/replay graph has its own compile-time call continuation.
        // Parameterized calls and unsupported local declarations remain errors.
        self.returns.clear();
        self.pending_visibility = None;
        routes
            .scripts
            .insert(page.to_owned(), std::sync::Arc::new(script.clone()));
        let entry = self.route_block(script, page, first, routes)?;
        routes
            .queue
            .push((page.to_owned(), first, entry.clone(), BTreeSet::new()));
        while !routes.queue.is_empty() {
            let (queued_page, start, id, path) = routes.queue.remove(0);
            let source_script = routes.scripts[&queued_page].clone();
            let script = source_script.as_ref();
            let page = queued_page.as_str();
            let state = routes.states[&id].clone();
            self.nodes = state.nodes;
            self.committed_images = state.committed_images;
            self.centered = state.centered;
            self.anchors = state.anchors;
            self.audio = state.audio;
            self.motion_tasks = state.motion_tasks;
            self.movie_tasks = state.movie_tasks;
            self.returns = state.returns;
            self.pending_visibility = state.pending_visibility;
            let pc = self.skip_forward(script, start)?;
            if routes.blocks.contains_key(&id) {
                if path.contains(&(page.to_owned(), pc)) {
                    ensure!(
                        path.iter()
                            .any(|(p, i)| match &routes.scripts[p].commands[*i].body {
                                Body::Text { .. } => true,
                                Body::Call { target, .. } =>
                                    target.page.replace('\\', "/") == CHOICE_EXECUTOR,
                                Body::Wait(e) =>
                                    e.first().is_some_and(|e| references(e, "選択実行中")),
                                _ => false,
                            }),
                        "E_IMPORT_LIVENOVEL: unexpected instantaneous route loop"
                    );
                }
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
            next_path.insert((page.to_owned(), pc));
            if route_support::inline_choice(self, script, page, pc, &id, &next_path, routes)? {
                continue;
            }
            if route_support::image_choice(self, script, page, pc, &id, &next_path, routes)? {
                continue;
            }
            match &c.body {
                Body::Condition(condition) => {
                    let term = super::ui_expr::normalize(condition)?
                        .context("E_IMPORT_GAME_EXPR: empty If condition")?;
                    let expression = route_support::condition(self, &term)?;
                    let (yes_pc, no_pc) = route_support::condition_targets(script, pc)?;
                    let yes = self.route_block(script, page, yes_pc, routes)?;
                    let no = self.route_block(script, page, no_pc, routes)?;
                    routes.blocks.insert(
                        id,
                        json!({"ops":[],"terminator":{
                            "type":"branch","condition":expression,"yes":yes,"no":no
                        }}),
                    );
                    self.enqueue_route(script, page, yes_pc, next_path.clone(), routes)?;
                    self.enqueue_route(script, page, no_pc, next_path, routes)?;
                }
                Body::VariableDelete(name) => {
                    route_support::dead_local_cleanup(self, script, pc, name)?;
                    let next = self.route_block(
                        script,
                        page,
                        route_support::successor(script, pc)?,
                        routes,
                    )?;
                    routes.blocks.insert(
                        id,
                        json!({"ops":[],"terminator":{"type":"goto","target":next}}),
                    );
                    self.enqueue_route(
                        script,
                        page,
                        route_support::successor(script, pc)?,
                        next_path,
                        routes,
                    )?;
                }
                Body::Calc(e) => {
                    let operation = route_support::assignment(self, e)
                        .with_context(|| format!("E_IMPORT_GAME_ASSIGN: {page}:{}", c.line))?;
                    let op = self.id("op");
                    let next = self.route_block(
                        script,
                        page,
                        route_support::successor(script, pc)?,
                        routes,
                    )?;
                    routes.blocks.insert(id,json!({"ops":[{"id":op,"operation":operation}],"terminator":{"type":"goto","target":next}}));
                    self.enqueue_route(
                        script,
                        page,
                        route_support::successor(script, pc)?,
                        next_path,
                        routes,
                    )?;
                }
                Body::Text {
                    text,
                    target,
                    history,
                    ..
                } => {
                    ensure!(
                        literal_string(target)? == "メッセージボックス"
                            && history.flag()?
                            && !text.has_conditions_or_links,
                        "E_IMPORT_LIVENOVEL: unsupported text properties"
                    );
                    let name = if routes.replay {
                        self.id("replay_episode")
                    } else if self.share_episodes {
                        // Main and authored title entries have independent
                        // route walks. Their temporary aliases must remain
                        // unique until the final shared-call rewrite.
                        self.id("episode")
                    } else {
                        format!("episode{}", routes.episodes.len() + 1)
                    };
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
                        self.page_ordinal = number;
                        let mut page_glyphs = page_glyphs.to_vec();
                        if number < waits
                            && !page_glyphs.iter().any(|g| {
                                matches!(
                                    g,
                                    Glyph::Char(_) | Glyph::RubyChar { .. } | Glyph::Break(0)
                                )
                            })
                        {
                            // An image-only page still has the original click wait.
                            page_glyphs.push(Glyph::Break(0));
                        }
                        self.page(&page_glyphs, &mut blocks).with_context(|| {
                            format!("{}:{} TextIns", self.location.source, self.location.line)
                        })?;
                    }
                    self.finish_function(&name, blocks, json!({"type":"return"}));
                    if routes.replay {
                        let end = self.id("replay_end");
                        routes.blocks.insert(end.clone(), json!({"ops":[],"terminator":{"type":"end","outcome":"replay_completed"}}));
                        routes.blocks.insert(id, json!({"ops":[],"terminator":{"type":"call","function":name,"next":end}}));
                    } else {
                        let next = self.route_block(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            routes,
                        )?;
                        routes.blocks.insert(id, json!({"ops":[],"terminator":{"type":"call","function":name,"next":next}}));
                        self.enqueue_route(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            next_path,
                            routes,
                        )?;
                    }
                }
                Body::Call {
                    target,
                    condition,
                    params,
                    ..
                } => {
                    let callee = target.page.replace('\\', "/");
                    if matches!(
                        callee.as_str(),
                        "ノベルシステム/■ロード.lsb"
                            | "ノベルシステム/■セーブ.lsb"
                            | "ノベルシステム/シーン回想/■開始.lsb"
                    ) {
                        let save_menu = callee == "ノベルシステム/■セーブ.lsb";
                        ensure!(
                            condition.flag()?
                                && target.line == 0
                                && (params.is_empty()
                                    || (save_menu
                                        && params.len() == 1
                                        && literal_string(&params[0])? == "メインメニュー")),
                            "E_IMPORT_STORY_MODAL: expected unconditional default menu call"
                        );
                        let (_, helper) = self.source.read(&callee)?;
                        let modal_target = if callee == "ノベルシステム/■ロード.lsb" {
                            ensure!(helper.version == 117 && helper.source_sha256 == "927717e83f44d5394fff99004dd239dc0a8870a48ba583d86d28082f865ca911",
                                "E_IMPORT_STORY_MODAL: unverified load helper");
                            json!({"type":"load_saves"})
                        } else if save_menu {
                            ensure!(helper.version == 117 && helper.source_sha256 == "65e82014f3a3518f449aa361694292c68d70ebbe6f21f27e00a68efa171b5d0d",
                                "E_IMPORT_STORY_MODAL: unverified save helper");
                            json!({"type":"save_saves"})
                        } else {
                            ensure!(helper.version == 117 && helper.source_sha256 == "a59350b620c7948945cd1ee453ffafe7485db0f7c74b8e4a6ddbde152822eda4",
                                "E_IMPORT_STORY_MODAL: unverified replay helper");
                            json!({"type":"image_menu","menu":"replay"})
                        };
                        let function = self.id("story_modal");
                        let task = self.id("story_modal_task");
                        let mut blocks = vec![];
                        self.effect(
                            &mut blocks,
                            &task,
                            "session",
                            json!({"type":"story_modal","target":modal_target}),
                            true,
                        );
                        self.finish_function(&function, blocks, json!({"type":"return"}));
                        let next_pc = route_support::successor(script, pc)?;
                        let next = self.route_block(script, page, next_pc, routes)?;
                        routes.blocks.insert(id, json!({"ops":[],"terminator":{"type":"call","function":function,"next":next}}));
                        self.enqueue_route(script, page, next_pc, next_path, routes)?;
                    } else if callee == "メッセージボックス作成.lsb" {
                        ensure!(
                            condition.flag()? && target.line == 0 && params.len() == 1,
                            "E_IMPORT_TEXT_LAYOUT: expected unconditional literal style call"
                        );
                        let (_, factory) = self.source.read(&callee)?;
                        let style = layout::style(self, &factory, literal_string(&params[0])?)?;
                        let cue = self.id("window_style");
                        let task = self.id("window_style_task");
                        self.cues.insert(cue.clone(), json!({"effects":[{"id":task,"scope":"session","effect":{"type":"dialogue_style","style":style}}]}));
                        let next = self.route_block(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            routes,
                        )?;
                        routes.blocks.insert(id, json!({"ops":[],"terminator":{"type":"activate","cue":cue,"next":next}}));
                        self.enqueue_route(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            next_path,
                            routes,
                        )?;
                    } else if callee == "ノベルシステム/シーン回想/■フラグON.lsb" {
                        ensure!(
                            condition.flag()? && params.len() == 1,
                            "E_IMPORT_LIVENOVEL: unexpected scenario call"
                        );
                        let key = format!("lm.replay.{}", literal_int(&params[0])?);
                        let op_id = self.id("op");
                        let next = self.route_block(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            routes,
                        )?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[{"id":op_id,"operation":{"type":"profile_merge","key":key}}],"terminator":{"type":"goto","target":next}}),
                        );
                        self.enqueue_route(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            next_path,
                            routes,
                        )?;
                    } else if callee == CHOICE_EXECUTOR {
                        self.lower_choice(script, page, pc, id, next_path, routes)?;
                    } else if condition.flag().ok() == Some(true)
                        && params.is_empty()
                        && (callee.is_empty() || callee == page)
                        && empty_subroutine(script, label(script, target.line)?)
                    {
                        let next = self.route_block(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            routes,
                        )?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"goto","target":next}}),
                        );
                        self.enqueue_route(
                            script,
                            page,
                            route_support::successor(script, pc)?,
                            next_path,
                            routes,
                        )?;
                    } else if condition.flag().ok() == Some(true)
                        && params.is_empty()
                        && !callee.starts_with("ノベルシステム/")
                    {
                        ensure!(
                            self.returns.len() < 64,
                            "E_IMPORT_LIMIT: authored call depth exceeds 64"
                        );
                        self.returns.push(ReturnSite {
                            page: page.to_owned(),
                            pc: route_support::successor(script, pc)?,
                        });
                        let (destination, target_script, target_pc) =
                            self.route_target(page, target, routes)?;
                        let next =
                            self.route_block(&target_script, &destination, target_pc, routes)?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"goto","target":next}}),
                        );
                        self.enqueue_route(
                            &target_script,
                            &destination,
                            target_pc,
                            next_path,
                            routes,
                        )?;
                    } else {
                        bail!(
                            "E_IMPORT_LIVENOVEL: unexpected scenario call {page}:{} -> {callee}:{}",
                            c.line,
                            target.line
                        );
                    }
                }
                Body::Jump(target, condition) => {
                    let (destination_page, destination_script, next) =
                        self.route_target(page, target, routes)?;
                    // A jump back to the initial dispatch restores the original title menu.
                    let next_pc = self.skip_forward(&destination_script, next)?;
                    let to_title = destination_page == title_page
                        && self.title_route_entries.iter().any(|entry| {
                            self.skip_forward(&destination_script, *entry).ok() == Some(next_pc)
                        });
                    if to_title && condition.flag().ok() == Some(true) {
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"end","outcome":"completed"}}),
                        );
                    } else {
                        let cont =
                            self.route_block(&destination_script, &destination_page, next, routes)?;
                        if condition.flag().ok() == Some(true) {
                            routes.blocks.insert(
                                id,
                                json!({"ops":[],"terminator":{"type":"goto","target":cont}}),
                            );
                        } else {
                            let term = super::ui_expr::normalize(condition)?
                                .context("E_IMPORT_GAME_EXPR: empty jump condition")?;
                            let expression = route_support::condition(self, &term)?;
                            let fallthrough = self.route_block(
                                script,
                                page,
                                route_support::successor(script, pc)?,
                                routes,
                            )?;
                            routes.blocks.insert(id,json!({"ops":[],"terminator":{"type":"branch","condition":expression,"yes":cont,"no":fallthrough}}));
                            self.enqueue_route(
                                script,
                                page,
                                route_support::successor(script, pc)?,
                                next_path.clone(),
                                routes,
                            )?;
                        }
                        self.enqueue_route(
                            &destination_script,
                            &destination_page,
                            next,
                            next_path,
                            routes,
                        )?;
                    }
                }
                Body::Exit(e) if e.flag()? => {
                    if let Some(site) = self.returns.pop() {
                        let caller = routes.scripts[&site.page].clone();
                        let next = self.route_block(&caller, &site.page, site.pc, routes)?;
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"goto","target":next}}),
                        );
                        self.enqueue_route(&caller, &site.page, site.pc, next_path, routes)?;
                    } else {
                        routes.blocks.insert(
                            id,
                            json!({"ops":[],"terminator":{"type":"end","outcome":"completed"}}),
                        );
                    }
                }
                Body::Motion(parameters) => bail!(
                    "E_IMPORT_LIVENOVEL: unsupported authored motion {}:{} ({} parameters)",
                    page,
                    c.line,
                    parameters.len()
                ),
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
        path: RoutePath,
        routes: &mut Routes,
    ) -> Result<()> {
        let (options, after) = choice_chain(script, pc + 1)?
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
            chain_path.insert((page.to_owned(), at));
        }
        for (index, (literal, target)) in options.into_iter().enumerate() {
            let option = format!("o{index}");
            let text = self.intern_text(
                "choice",
                index,
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
            let (destination_page, destination_script, target_pc) =
                self.route_target(page, &target, routes)?;
            let block =
                self.route_block(&destination_script, &destination_page, target_pc, routes)?;
            branches.insert(option, block);
            self.enqueue_route(
                &destination_script,
                &destination_page,
                target_pc,
                chain_path.clone(),
                routes,
            )?;
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
        // Gain belongs to the concrete runtime sound. It only affects later
        // lowering when CHGVOL reads the instance's original gain to derive
        // its relative envelope. Refine that bus and restart before publishing
        // any graph if a reachable volume event needs this fact. At most the
        // three admitted buses can cause a gain refinement. Source layout
        // reads refine individual image rectangles too; cached scripts are
        // shared across these bounded passes.
        let mut gain_reads = BTreeSet::new();
        let mut geometry_reads = BTreeSet::new();
        let mut anchor_reads = BTreeSet::new();
        loop {
            let mut pass = Self::new(self.source.clone());
            pass.share_episodes = true;
            pass.gain_reads = gain_reads.clone();
            pass.geometry_reads = geometry_reads.clone();
            pass.anchor_reads = Some(anchor_reads.clone());
            match pass.run_once(entry) {
                Ok(story) => {
                    *self = pass;
                    return Ok(story);
                }
                Err(error) => {
                    if let Some(required) = error.downcast_ref::<GainReadRequired>() {
                        ensure!(
                            gain_reads.insert(required.0.clone()) && gain_reads.len() <= 3,
                            "E_IMPORT_EVENT: invalid gain specialization"
                        );
                    } else if let Some(required) = error.downcast_ref::<GeometryReadRequired>() {
                        ensure!(
                            geometry_reads.insert(required.0.clone())
                                && geometry_reads.len() <= 256,
                            "E_IMPORT_LAYOUT: too many rectangle specializations"
                        );
                    } else if let Some(required) = error.downcast_ref::<AnchorReadRequired>() {
                        ensure!(
                            anchor_reads.insert(required.0.clone()) && anchor_reads.len() <= 256,
                            "E_IMPORT_LAYOUT: too many anchor specializations"
                        );
                    } else {
                        *self = pass;
                        return Err(error);
                    }
                    self.source = pass.source;
                }
            }
        }
    }
    fn run_once(&mut self, entry: &str) -> Result<Value> {
        for (name, value) in
            super::lsb::project_settings(&read_binary(&self.source.path("live.lpb")?)?)?
        {
            let value = match value {
                Literal::Int(value) => json!({"type":"i32","value":value}),
                Literal::Float(value) => json!({"type":"f80","value":value}),
                Literal::String(value) => json!({"type":"string","value":value}),
                _ => continue,
            };
            if !name.starts_with('@') {
                self.variables.insert(name, value);
            }
        }
        let (_, initializer) = self.source.read("変数初期化.lsb")?;
        for command in initializer.commands.iter().filter(|c| !c.muted) {
            let Body::Variable {
                name,
                initial,
                scope,
                value_type,
            } = &command.body
            else {
                ensure!(
                    matches!(command.kind, 3 | 27),
                    "E_IMPORT_GAME_INIT: unexpected initializer command"
                );
                continue;
            };
            if name.starts_with('@') {
                continue;
            }
            let value = match (*value_type, initial.literal.as_ref()) {
                (1 | 3, Some(Literal::Int(value))) => json!({"type":"i32","value":value}),
                (2, Some(Literal::Float(value))) => json!({"type":"f80","value":value}),
                (2, Some(Literal::Int(value))) => {
                    json!({"type":"f80","value":nir_format::Float80::from_i32(*value)})
                }
                (4, Some(Literal::String(value))) => json!({"type":"string","value":value}),
                _ => {
                    if *scope == 3 {
                        self.unsupported_status.insert(name.clone());
                    }
                    continue;
                }
            };
            if *value_type == 3 {
                self.boolean_variables.insert(name.clone());
            }
            if *scope == 3 {
                if *value_type == 3 && value["value"] == 0 {
                    self.status_flags.insert(
                        name.clone(),
                        format!("lm.status.{}", &nir_content::digest(name.as_bytes())[..24]),
                    );
                } else if matches!(*value_type, 1 | 2 | 4) {
                    self.status_values.insert(
                        name.clone(),
                        format!("lm.status.{}", &nir_content::digest(name.as_bytes())[..24]),
                    );
                } else {
                    self.unsupported_status.insert(name.clone());
                }
            }
            self.variables.insert(name.clone(), value);
        }
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
        let title = title_profile(&script)?;
        self.title_route_entries = title
            .commands
            .iter()
            .filter(|c| c.kind == 3)
            .map(|c| label(&script, c.line))
            .collect::<Result<_>>()?;
        let title_menu = stock_title_menu(&title)?;
        self.stage = menu_dimensions(&read_binary(&self.source.path(&title_menu)?)?)?;
        ensure!(
            matches!(script.version, 116 | 117),
            "E_IMPORT_LIVENOVEL: profile requires LSB116 or LSB117"
        );
        let mut first = None;
        for c in &title.commands {
            if let Body::Jump(target, condition) = &c.body {
                if matches!(selection_dispatch(condition), Some("はじめから" | "01")) {
                    ensure!(
                        first.is_none(),
                        "E_IMPORT_LIVENOVEL: ambiguous new-game route"
                    );
                    first = Some(local_target(&script, &page, target)?);
                }
            }
        }
        let first = first.context("E_IMPORT_LIVENOVEL: missing new-game route")?;
        let (_, box_script) = self.source.read("メッセージボックス作成.lsb")?;
        if script.version == 117 {
            self.textbox_theme = Some(layout::theme(self, &box_script)?);
        } else {
            let box_properties = stock_textbox(&box_script)?;
            let (textbox, box_size) = self.image(&box_properties.path)?;
            self.textbox = textbox;
            self.textbox_theme = Some(
                json!({"height":box_size[1],"padding":box_properties.padding,"font_size":box_properties.font_size,"line_height":box_properties.line_height,"opacity":box_properties.opacity,"background":self.textbox,"rect":[(1024. - box_size[0] as f32)/2.,768. - 10. - box_size[1] as f32,box_size[0],box_size[1]]}),
            );
        }
        self.warnings.insert("Stock LiveNovel startup, window/system scripts and asynchronous message handshake are replaced by NIR. This profile targets its episode/replay/choice convention, not arbitrary LSB expressions.".into());
        self.warnings.insert("Save/load, history and settings use NIR UI and save format; LiveMaker save files are not compatible. Title/replay select sounds and the replay BGM map to page effects; hover sounds and animated cursors are not reproduced.".into());
        self.warnings.insert("Text speed maps the persisted StatusTextSpeed (milliseconds per character, 0 meaning instant) to the reveal interval; text renders in the bundled NIR Japanese font because the source font-face setting names a system font that cannot be bundled.".into());
        self.warnings.insert("Source Auto uses a sampled remaining-voice timer plus fixed delay. The imported policy samples the bound voice duration/position and voice-volume preference once per Auto cycle; original device timing and unsupported simultaneous source voice channels remain outside certification.".into());
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
        gallery::build(self)?;
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
            let click = replay_click
                .map(|path| self.menu_sound(&path))
                .transpose()?
                .flatten();
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
            Some(MenuEffects {
                enter: None,
                close: None,
                click,
                music,
                elements: vec![],
            })
        };
        let list = read_binary(&self.source.path("グラフィック/シーン回想/シーン回想.TXT")?)?;
        let list = super::lsb::decode(&list)?;
        let thumbnails: Vec<(u32, String)> = list
            .lines()
            .filter(|s| !s.is_empty())
            .map(|line| {
                let (path, id) = line
                    .rsplit_once('/')
                    .context("E_IMPORT_LIVENOVEL: malformed replay list")?;
                Ok((id.parse::<u32>()?, path.to_owned()))
            })
            .collect::<Result<_>>()?;
        ensure!(thumbnails.len() <= 4096, "E_IMPORT_LIMIT: replay list");
        // The ids table belongs to the optional tag-filter branch. Normal
        // gallery entry has an empty target and copies the complete TXT list.
        let _tag_ids = replay_ids(&replay_ui, 4096)?;
        let calls = replay_dispatch(&replay)?;
        if !calls.is_empty() {
            let (_, bootstrap) = self.source.read("ノベルシステム/■シーン回想.lsb")?;
            verify_replay_reset(&bootstrap)?;
        }
        let empty_dispatcher = replay.commands.is_empty();
        ensure!(
            empty_dispatcher || thumbnails.len() == calls.len(),
            "E_IMPORT_LIVENOVEL: default replay list/dispatcher count differs"
        );
        if empty_dispatcher {
            self.warnings.insert("The source replay dispatcher is empty. Its default gallery comes from the authored TXT list; no missing entries or story content are invented.".into());
        }
        let replay_background = stock_replay_background(&replay_ui)?;
        let (background, _) = self.image(&replay_background)?;
        let mut buttons = vec![];
        for (index, (unlock, thumbnail)) in thumbnails.into_iter().enumerate() {
            let target = calls.get(&index);
            let function = format!("replay{}", index + 1);
            if let Some(target) = target {
                // The stock replay bootstrap resets variables and clears the
                // old drawing buffer before dispatch. Main-story variants are
                // not replay inputs. Stock CHANGECG creates absent images at
                // L,T with default priority; existing images keep their anchors.
                self.nodes.clear();
                self.committed_images.clear();
                self.centered.clear();
                self.anchors.clear();
                self.audio.clear();
                self.motion_tasks.clear();
                self.movie_tasks.clear();
                self.pending_movies.clear();
                let mut preamble = vec![];
                self.scene(&mut preamble, 0);
                for (bus, effect) in self.audio.clone() {
                    self.effect(&mut preamble, &bus, "session", effect, false);
                }
                let graph = self.id("replay_route");
                let mut replay_routes = Routes {
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
                let (replay_page, replay_script, first) =
                    self.route_target(&page, target, &mut replay_routes)?;
                let replay_entry =
                    self.walk_routes(&replay_script, &replay_page, first, &mut replay_routes)?;
                replay_routes.blocks.insert(
                    "cancelled".into(),
                    json!({"ops":[],"terminator":{"type":"end","outcome":"cancelled"}}),
                );
                replay_routes.blocks.insert("failed".into(), json!({"ops":[],"terminator":{"type":"fault","code":"E_IMPORT_TASK","message":"Imported replay failed"}}));
                self.functions.insert(
                    graph.clone(),
                    json!({"entry":replay_entry,"blocks":replay_routes.blocks}),
                );
                preamble.push(
                    json!({"ops":[],"terminator":{"type":"call","function":graph,"next":"NEXT"}}),
                );
                self.finish_function(
                    &function,
                    preamble,
                    json!({"type":"end","outcome":"replay_completed"}),
                );
            } else {
                self.finish_function(
                    &function,
                    vec![],
                    json!({"type":"end","outcome":"replay_completed"}),
                );
            }
            let (asset, size) = self.image(&thumbnail)?;
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
        let title_menu = stock_title_menu(&title)?;
        let title_background = stock_title_background(&title)?;
        self.warnings.insert("The NIR title starts at the stock startup's final static title image; source startup slideshow transitions are replaced by the title screen.".into());
        let (background, _) = self.image(&title_background)?;
        let lpm = read_binary(&self.source.path(&title_menu)?)?;
        self.title_menu_version = if lpm.starts_with(b"<?xml") {
            "LPM200"
        } else {
            "LPM106"
        };
        let title_directory = Path::new(&title_menu)
            .parent()
            .context("E_IMPORT_LIVENOVEL: title menu directory")?;
        let title_filters = title_filters(self, &title)?;
        let mut buttons = vec![];
        for (i, b) in menu(&lpm)?.into_iter().enumerate() {
            let action = match b.label.as_str() {
                "はじめから" | "01" => ImageMenuAction::NewGame,
                "つづきから" | "ロード" => ImageMenuAction::Saves,
                "CGモード" if self.menus.contains_key("cg-0") => ImageMenuAction::Menu {
                    menu: "cg-0".into(),
                },
                "回想" => ImageMenuAction::Menu {
                    menu: "replay".into(),
                },
                _ => {
                    let targets: Vec<_> = title
                        .commands
                        .iter()
                        .filter_map(|c| {
                            if c.muted {
                                return None;
                            }
                            match &c.body {
                                Body::Jump(target, condition)
                                    if selection_dispatch(condition) == Some(b.label.as_str()) =>
                                {
                                    Some(target)
                                }
                                _ => None,
                            }
                        })
                        .collect();
                    ensure!(
                        targets.len() == 1,
                        "E_IMPORT_TITLE: missing or ambiguous authored entry {}",
                        b.label
                    );
                    let first = local_target(&script, &page, targets[0])?;
                    self.nodes.clear();
                    self.committed_images.clear();
                    self.centered.clear();
                    self.anchors.clear();
                    self.audio.clear();
                    self.motion_tasks.clear();
                    self.movie_tasks.clear();
                    self.pending_movies.clear();
                    let mut bonus_routes = Routes {
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
                    let entry = self.walk_routes(&script, &page, first, &mut bonus_routes)?;
                    bonus_routes.blocks.insert(
                        "cancelled".into(),
                        json!({"ops":[],"terminator":{"type":"end","outcome":"cancelled"}}),
                    );
                    bonus_routes.blocks.insert("failed".into(), json!({"ops":[],"terminator":{"type":"fault","code":"E_IMPORT_TASK","message":"Imported title entry failed"}}));
                    self.choice_sites += bonus_routes.choice_sites;
                    let function = format!("title_entry_{i}");
                    self.functions.insert(
                        function.clone(),
                        json!({"entry":entry,"blocks":bonus_routes.blocks}),
                    );
                    ImageMenuAction::Entry { function }
                }
            };
            let (asset, size) = self.image(&title_directory.join(&b.source).to_string_lossy())?;
            let hover_asset = if b.selected.is_empty() {
                None
            } else {
                Some(
                    self.image(&title_directory.join(&b.selected).to_string_lossy())?
                        .0,
                )
            };
            let locked_asset = if b.disabled.is_empty() {
                None
            } else {
                let (asset, disabled_size) =
                    self.image(&title_directory.join(&b.disabled).to_string_lossy())?;
                ensure!(
                    disabled_size == size,
                    "E_IMPORT_LPM: disabled image dimensions differ"
                );
                Some(asset)
            };
            let label = b.label.clone();
            let mut element = menu_element(ImageButton {
                id: format!("title{i}"),
                label: b.label,
                asset,
                hover_asset,
                locked_asset,
                rect: [b.x as f32, b.y as f32, size[0] as f32, size[1] as f32],
                action,
                requires: None,
            });
            for (list, conditions) in [
                ("_tmp", &mut element.enabled_when),
                ("_tmp2", &mut element.visible_when),
            ] {
                if let Some(predicate) = title_filters.get(list).and_then(|list| list.get(&label)) {
                    conditions.push(title_profile_guard(self, predicate, false)?);
                }
            }
            buttons.push(element);
        }
        // The title menu's select sound arms the page's click effect. The
        // preview executor's hover parameter has no NIR counterpart and stays
        // in the accepted menu-hover approximation.
        let title_effects = match title_select_sound(&title)? {
            Some(path) => {
                let click = self.menu_sound(&path)?;
                if click.is_none() {
                    None
                } else {
                    Some(MenuEffects {
                        enter: None,
                        close: None,
                        click,
                        music: None,
                        elements: vec![],
                    })
                }
            }
            None => None,
        };
        self.menus.insert(
            "title".into(),
            ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: buttons,
                background,
                buttons: vec![],
                effects: title_effects,
            },
        );
        self.warnings.insert("Replay thumbnails retain original grid coordinates. Locked thumbnails preserve alpha with black RGB; a NIR return button and system-menu access remain available for touch/keyboard navigation.".into());
        self.scenes.insert("title".into(), vec![]);
        fn collect_reads(value: &Value, out: &mut BTreeSet<String>) {
            if value["type"] == "var" {
                if let Some(name) = value["name"].as_str() {
                    out.insert(name.into());
                }
            }
            match value {
                Value::Array(values) => {
                    for value in values {
                        collect_reads(value, out);
                    }
                }
                Value::Object(values) => {
                    for value in values.values() {
                        collect_reads(value, out);
                    }
                }
                _ => {}
            }
        }
        for name in self.functions.keys().cloned().collect::<Vec<_>>() {
            let mut function = self.functions.remove(&name).unwrap();
            for block in function["blocks"].as_object_mut().unwrap().values_mut() {
                let mut reads = BTreeSet::new();
                collect_reads(block, &mut reads);
                if let Some(choice) = block["terminator"]["choice"].as_str() {
                    if let Some(definition) = self.choices.get(choice) {
                        collect_reads(definition, &mut reads);
                    }
                }
                let mut ops = self.profile_reads(&reads);
                ops.extend(block["ops"].as_array().unwrap().iter().cloned());
                block["ops"] = json!(ops);
            }
            self.functions.insert(name, function);
        }
        fn rewrite_calls(value: &mut Value, aliases: &BTreeMap<String, String>) {
            match value {
                Value::Object(fields) => {
                    if let Some(function) = fields.get_mut("function") {
                        if let Some(shared) = function.as_str().and_then(|name| aliases.get(name)) {
                            *function = json!(shared);
                        }
                    }
                    for value in fields.values_mut() {
                        rewrite_calls(value, aliases);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        rewrite_calls(value, aliases);
                    }
                }
                _ => {}
            }
        }
        for function in self.functions.values_mut() {
            rewrite_calls(function, &self.function_aliases);
        }
        for menu in self.menus.values_mut() {
            let mut value = serde_json::to_value(&*menu)?;
            rewrite_calls(&mut value, &self.function_aliases);
            *menu = serde_json::from_value(value)?;
        }
        numeric::certify(self)?;
        if self.cues.values().any(|cue| {
            cue["effects"].as_array().is_some_and(|effects| {
                effects.iter().any(|def| {
                    def["effect"]["type"] == "story_modal"
                        && def["effect"]["target"]["menu"] == "replay"
                })
            })
        }) {
            certify_default_replay_target(self)?;
        }
        Ok(
            json!({"fragment_format":1,"variables":self.variables,"functions":self.functions,"cues":self.cues,"scenes":self.scenes,"choices":self.choices,"sprite_timelines":self.sprite_timelines}),
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
                total <= 8 * 1024 * 1024 * 1024,
                "E_IMPORT_LIMIT: converted media exceeds 8 GiB"
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
        manifest["stage"] = json!({"width":self.stage[0],"height":self.stage[1]});
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
        theme["dialogue_styles"] = serde_json::to_value(&self.dialogue_styles)?;
        theme["dialogue"] = self
            .textbox_theme
            .clone()
            .context("E_IMPORT_LIVENOVEL: missing textbox geometry")?;
        fs::write(path, toml::to_string_pretty(&theme)?)?;
        let tokens_path = root.join("theme/tokens.json");
        let mut tokens: Value = serde_json::from_slice(&fs::read(&tokens_path)?)?;
        tokens["text"] = json!(self.text_color);
        super::write_json(&tokens_path, &tokens)?;
        fs::write(root.join("README.md"),"LiveNovel migration with imported source media. Read import-report.json for fidelity limits. Original rights apply to all imported content.\n")?;
        Ok(())
    }
}

fn certify_default_replay_target(adapter: &Adapter) -> Result<()> {
    const TARGET: &str = "回想ターゲット";
    ensure!(
        !adapter.status_values.contains_key(TARGET) && !adapter.unsupported_status.contains(TARGET),
        "E_IMPORT_STORY_MODAL: persistent or unknown replay filter"
    );
    if let Some(value) = adapter.variables.get(TARGET) {
        ensure!(
            value == &json!({"type":"string","value":""}),
            "E_IMPORT_STORY_MODAL: non-default replay filter"
        );
    }
    for function in adapter.functions.values() {
        for block in function["blocks"]
            .as_object()
            .into_iter()
            .flat_map(|blocks| blocks.values())
        {
            for op in block["ops"].as_array().into_iter().flatten() {
                let operation = &op["operation"];
                if operation["target"] == TARGET {
                    ensure!(
                        operation["type"] == "assign"
                            && operation["value"]
                                == json!({"type":"const","value":{"type":"string","value":""}}),
                        "E_IMPORT_STORY_MODAL: authored replay filter is not provably empty"
                    );
                }
            }
        }
    }
    Ok(())
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
    // Reject malformed lowering before copying and decoding the game's media.
    crate::project::validate_generated_fragment(&story)?;
    let ui = super::ui::analyze_system_menu(&mut adapter.source)?;
    let route_shape = if adapter.choice_sites > 0 {
        format!(
            "Branching LiveNovel route with {} typed choice site(s),",
            adapter.choice_sites
        )
    } else {
        "Linear LiveNovel route,".into()
    };
    let mut mappings = mapping_ledger(
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
    if !adapter.missing_menu_sounds.is_empty() {
        mappings.push(ImportMapping {
            rule:"livenovel.missing-menu-audio".into(),
            level:"approximate".into(),evidence:"decoded-source".into(),source_version:"LSB116".into(),
            behavior:"Missing source click sounds in title/replay menus are silent. Ambiguous, unsafe or malformed resources remain errors; story images, voice, music and effects are never substituted or omitted.".into(),
            capabilities:vec![],approximation:Some("Explicit acceptance is required to ship the omission of these missing menu click sounds.".into()),
        });
    }
    if adapter.functions.values().any(|function| {
        function["blocks"].as_object().is_some_and(|blocks| {
            blocks.values().any(|block| {
                block["ops"]
                    .as_array()
                    .is_some_and(|ops| ops.iter().any(|op| op["operation"]["type"] == "random"))
            })
        })
    }) {
        mappings.push(ImportMapping {
            rule: "livenovel.random".into(),
            level: "adapted".into(),
            evidence: "decoded-source".into(),
            source_version: "LSB116".into(),
            behavior: "A single authored Random(N) assignment with positive literal integer N produces 0..N-1 through NIR's unbiased, saved RNG. Original engine seeds and draw sequences are not reproduced; nested, dynamic and persistent writes are rejected.".into(),
            capabilities: vec![],
            approximation: None,
        });
    }
    let lsb_versions = adapter
        .source
        .scripts
        .values()
        .map(|s| s.version)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|v| format!("LSB{v}"))
        .collect::<Vec<_>>()
        .join("/");
    let lpb_version = adapter
        .defaults
        .as_ref()
        .context("E_IMPORT_SETTINGS: missing defaults")?
        .source_version;
    for mapping in &mut mappings {
        if mapping.rule == "livenovel.title-menu" {
            mapping.source_version = adapter.title_menu_version.into();
        }
        if mapping.source_version == "LSB116" {
            mapping.source_version = lsb_versions.clone();
        }
        if mapping.source_version == "LPB116" {
            mapping.source_version = format!("LPB{lpb_version}");
        }
    }
    let mut report=ImportReport{format:2,engine:format!("livemaker-livenovel{lpb_version}"),status:ImportReport::status_from_mappings(&mappings).into(),written:false,errors:0,approximate:mappings.iter().filter(|m|m.approximate()).count(),text_pages:adapter.texts.len(),functions:adapter.functions.len(),coverage:format!("{} replay dispatch and title image menu. {} referenced media assets converted. Native system scripts are replaced; see fidelity warnings and per-rule mappings.",route_shape,adapter.assets.len()),mappings,diagnostics:adapter.warnings.iter().map(|message|ImportDiagnostic{severity:"warning".into(),source:entry.into(),index:0,line:0,byte:0,command:"LiveNovelProfile".into(),message:message.clone()}).collect(),source_map:adapter.source_map.clone()};
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
    disabled: String,
}
fn menu(data: &[u8]) -> Result<Vec<MenuButton>> {
    if data.starts_with(b"<?xml") {
        return xml_menu(data);
    }
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
            disabled: String::new(),
        });
    }
    ensure!(r.at == data.len(), "E_IMPORT_LPM: trailing bytes");
    Ok(buttons)
}
fn xml_menu(data: &[u8]) -> Result<Vec<MenuButton>> {
    ensure!(data.len() <= 1024 * 1024, "E_IMPORT_LPM: XML exceeds 1 MiB");
    let text = super::lsb::decode(data)?;
    let doc = roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 16_384,
        },
    )
    .context("E_IMPORT_LPM: invalid XML")?;
    let root = doc.root_element();
    ensure!(
        root.tag_name().name() == "PrevMenu",
        "E_IMPORT_LPM: invalid XML root"
    );
    ensure!(
        root.attributes().len() == 0
            && root.children().filter(|n| n.is_element()).all(|n| matches!(
                n.tag_name().name(),
                "Version" | "Width" | "Height" | "Button" | "BGFile"
            )),
        "E_IMPORT_LPM: unsupported XML root fields"
    );
    let field = |node: roxmltree::Node<'_, '_>, name: &str| -> Result<String> {
        let matches: Vec<_> = node
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == name)
            .collect();
        ensure!(matches.len() == 1, "E_IMPORT_LPM: missing/duplicate {name}");
        let node = matches[0];
        ensure!(
            !node.children().any(|n| n.is_element()) && node.attributes().len() == 0,
            "E_IMPORT_LPM: invalid {name}"
        );
        Ok(node.text().unwrap_or("").to_owned())
    };
    let optional_field = |node: roxmltree::Node<'_, '_>, name: &str| -> Result<Option<String>> {
        if node.children().any(|n| n.has_tag_name(name)) {
            field(node, name).map(Some)
        } else {
            Ok(None)
        }
    };
    ensure!(
        field(root, "Version")? == "200" && menu_dimensions(data).is_ok(),
        "E_IMPORT_LPM: unsupported XML version/stage"
    );
    let containers: Vec<_> = root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Button")
        .collect();
    ensure!(
        containers.len() == 1,
        "E_IMPORT_LPM: missing/duplicate Button"
    );
    let mut buttons = vec![];
    for node in containers[0].children().filter(|n| n.is_element()) {
        ensure!(
            node.tag_name().name() == "Item" && buttons.len() < 256,
            "E_IMPORT_LPM: invalid button count/item"
        );
        ensure!(
            node.attributes().len() == 0
                && node.children().filter(|n| n.is_element()).all(|n| matches!(
                    n.tag_name().name(),
                    "Left"
                        | "Top"
                        | "PrevLeft"
                        | "PrevTop"
                        | "Path"
                        | "CapMask"
                        | "CapMaskLevel"
                        | "Name"
                        | "Group"
                        | "InImagePath"
                        | "OutImagePath"
                        | "SettleImagePath"
                        | "DownOnPrevRepeat"
                        | "DownOffPrevRepeat"
                        | "DisImagePath"
                        | "DisInImagePath"
                )),
            "E_IMPORT_LPM: unsupported XML button fields"
        );
        for (name, expected) in [
            ("PrevLeft", "0"),
            ("PrevTop", "0"),
            ("CapMask", "0"),
            ("Group", "0"),
            ("DownOnPrevRepeat", "0"),
            ("DownOffPrevRepeat", "0"),
        ] {
            ensure!(
                field(node, name)? == expected,
                "E_IMPORT_LPM: unsupported {name}"
            );
        }
        let source = field(node, "Path")?;
        ensure!(
            optional_field(node, "OutImagePath")?.is_none_or(|path| path == source),
            "E_IMPORT_LPM: differing normal image"
        );
        let selected = optional_field(node, "InImagePath")?.unwrap_or_default();
        let disabled = optional_field(node, "DisImagePath")?.unwrap_or_default();
        ensure!(
            optional_field(node, "DisInImagePath")?
                .is_none_or(|path| path.is_empty() || path == disabled),
            "E_IMPORT_LPM: distinct disabled hover image"
        );
        ensure!(
            optional_field(node, "SettleImagePath")?.is_none_or(|path| path == selected),
            "E_IMPORT_LPM: distinct settled button image"
        );
        buttons.push(MenuButton {
            x: field(node, "Left")?.parse()?,
            y: field(node, "Top")?.parse()?,
            source,
            label: field(node, "Name")?,
            selected,
            disabled,
        });
    }
    ensure!(!buttons.is_empty(), "E_IMPORT_LPM: empty menu");
    Ok(buttons)
}

#[cfg(test)]
mod tests {
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
                    source_sha256:
                        "5795a8e97d0f0a8d2b94682d4e13f7059a9aaecf3f8c19df21dbd6a3587d8894".into(),
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
                serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(
                    adapter.variables
                ))
                .unwrap(),
            );
            p.scenes.extend(
                serde_json::from_value::<BTreeMap<String, Vec<Node>>>(json!(adapter.scenes))
                    .unwrap(),
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
                serde_json::from_value::<BTreeMap<String, nir_format::Choice>>(json!(
                    adapter.choices
                ))
                .unwrap(),
            );
            p.functions.insert("main".into(),serde_json::from_value(json!({"entry":routes.entries[&("00000001.lsb".into(),0)],"blocks":routes.blocks})).unwrap());
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
                serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(
                    adapter.variables
                ))
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
                crate::import::lsb::Reference {
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
            a.op(&mut blocks, json!({"type":"assign","target":"affection","value":{"type":"var","name":"affection"}}));
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
        crate::project::validate_generated_fragment(&story).unwrap();
        let mut broken = story.clone();
        let ids: Vec<_> = a.functions.keys().cloned().collect();
        broken["functions"][&ids[1]]["blocks"]["b000000"]["ops"][0]["id"] =
            broken["functions"][&ids[0]]["blocks"]["b000000"]["ops"][0]["id"].clone();
        assert!(crate::project::validate_generated_fragment(&broken)
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
            a.op(&mut blocks, json!({"type":"assign","target":"affection","value":{"type":"var","name":"affection"}}));
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
            nodes:BTreeMap::from([("actor".into(),serde_json::from_value(
                json!({"id":"actor","asset":"image.first","x":10,"y":20,"width":32,"height":24})
            ).unwrap())]),
            committed_images:BTreeSet::from(["actor".into()]),
            centered:BTreeSet::new(),
            anchors:BTreeMap::from([("actor".into(),("C".into(),"B".into()))]),
            audio:BTreeMap::new(),motion_tasks:BTreeMap::new(),movie_tasks:BTreeMap::new(),
            returns:vec![],pending_visibility:None,
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
                        page: ["ノベルシステム", "プレビューメニュー", "■選択実行.lsb"]
                            .join(separator),
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
