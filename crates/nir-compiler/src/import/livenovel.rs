//! Adapter for the stock LiveNovel 116 event/menu convention. This is deliberately
//! separate from generic LSB lowering: native system scripts are replaced explicitly.
use super::{
    lsb::{Body, Expression, Glyph, Literal, Script},
    media, read_binary, ImportDiagnostic, ImportOptions, ImportReport, Source, SourceLocation,
};
use anyhow::{bail, ensure, Context, Result};
use nir_format::{ImageButton, ImageMenu, ImageMenuAction, Node, Span};
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
struct Adapter {
    source: Source,
    assets: BTreeMap<String, Asset>,
    texts: BTreeMap<String, crate::AuthorTextDoc>,
    functions: BTreeMap<String, Value>,
    cues: BTreeMap<String, Value>,
    scenes: BTreeMap<String, Vec<Node>>,
    nodes: BTreeMap<String, Node>,
    centered: BTreeSet<String>,
    audio: BTreeMap<String, Value>,
    menus: BTreeMap<String, ImageMenu>,
    source_map: BTreeMap<String, SourceLocation>,
    warnings: BTreeSet<String>,
    counter: usize,
    location: SourceLocation,
    textbox: String,
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
            assets: BTreeMap::new(),
            texts: BTreeMap::new(),
            functions: BTreeMap::new(),
            cues: BTreeMap::new(),
            scenes: BTreeMap::new(),
            nodes: BTreeMap::new(),
            centered: BTreeSet::new(),
            audio: BTreeMap::new(),
            menus: BTreeMap::new(),
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
    fn stop(&mut self, blocks: &mut Vec<Value>, bus: &str) {
        if self.audio.remove(bus).is_some() {
            self.op(
                blocks,
                json!({"type":"task_control","task":bus,"action":"cancel"}),
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
                self.op(
                    blocks,
                    json!({"type":"dialogue_visibility","visible":name=="MESON"}),
                );
                if number(0)? > 0 {
                    self.warnings.insert("Message-box fade is represented by immediate visibility; text layout and original box image are preserved.".into());
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
                let asset = self.sound(
                    &format!("サウンド/{}", arg(0)?),
                    arg(3)?.parse::<f32>()? / 1000.,
                )?;
                self.stop(blocks, bus);
                let effect = json!({"type":"audio","asset":asset,"bus":bus,"looped":looped});
                self.effect(blocks, bus, "session", effect.clone(), false);
                self.audio.insert(bus.into(), effect);
            }
            "STOPSND" => {
                ensure!(
                    arg(0)? == "BGM" && arg(2)? == "PASS",
                    "E_IMPORT_EVENT: unsupported sound stop"
                );
                self.stop(blocks, "bgm");
                if number(1)? > 0 {
                    self.warnings.insert("Audio stop fades are currently immediate; original samples, loop flags and authored volume are retained.".into());
                }
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
        self.effect(
            blocks,
            "line",
            "interaction",
            json!({"type":"dialogue","text":tid,"speaker":"","reveal_us":"32000"}),
            false,
        );
        for (gate, events) in actions {
            self.wait(blocks, "line", json!({"type":"marker","id":gate}));
            for fields in events {
                self.event(&fields, blocks)?;
            }
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
            self.stop(blocks, "voice");
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
    fn run(&mut self, entry: &str) -> Result<Value> {
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
                if references(condition, "選択値")
                    && condition.operations.iter().any(|(op, _, _)| *op == 12)
                    && condition.operations.iter().any(|(_, _, args)| {
                        args.iter()
                            .any(|v| matches!(v,Literal::String(s) if s=="はじめから"))
                    })
                {
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
        self.warnings.insert("Stock LiveNovel startup, window/system scripts and asynchronous message handshake are replaced by NIR. This profile targets its linear episode/replay convention, not arbitrary LSB expressions.".into());
        self.warnings.insert("Save/load, history and settings use NIR UI and save format; LiveMaker save files are not compatible. Menu sound effects and animated cursors are not yet reproduced.".into());
        self.warnings.insert("Text uses the bundled NIR Japanese font and a 32 ms reveal interval; source font/style/reveal settings are not yet imported.".into());
        let mut pc = first;
        let mut seen = BTreeSet::new();
        let mut episodes: Vec<Episode> = vec![];
        let mut blocks = vec![];
        let mut main = vec![];
        loop {
            ensure!(
                pc < script.commands.len() && seen.insert(pc),
                "E_IMPORT_LIVENOVEL: unexpected route loop"
            );
            let c = &script.commands[pc];
            self.location = SourceLocation {
                source: page.clone(),
                index: pc,
                line: c.line,
                byte: c.offset,
                command: c.name().into(),
            };
            if c.muted || c.kind == 3 || c.kind == 27 {
                pc += 1;
                continue;
            }
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
                    let name = format!("episode{}", episodes.len() + 1);
                    episodes.push((
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
                    for (number, page) in text
                        .glyphs
                        .split(|g| matches!(g, Glyph::Break(1)))
                        .enumerate()
                    {
                        let mut page = page.to_vec();
                        if number < waits
                            && !page
                                .iter()
                                .any(|g| matches!(g, Glyph::Char(_) | Glyph::Break(0)))
                        {
                            // An image-only page still has the original click wait.
                            page.push(Glyph::Break(0));
                        }
                        self.page(&page, &mut blocks).with_context(|| {
                            format!("{}:{} TextIns", self.location.source, self.location.line)
                        })?;
                    }
                    self.finish_function(
                        &name,
                        std::mem::take(&mut blocks),
                        json!({"type":"return"}),
                    );
                    main.push(json!({"ops":[],"terminator":{"type":"call","function":name,"next":"NEXT"}}));
                    pc += 1;
                }
                Body::Calc(e) => {
                    ensure!(
                        e.operations.iter().all(|(op, name, args)| *op == 1
                            && name == "__メッセージ終了"
                            && matches!(args.as_slice(), [Literal::Int(0)])),
                        "E_IMPORT_LIVENOVEL: unexpected scenario assignment"
                    );
                    pc += 1;
                }
                Body::Wait(e) => {
                    ensure!(
                        e.len() == 3
                            && references(&e[0], "__メッセージ終了")
                            && literal_int(&e[1])? == 0
                            && literal_int(&e[2])? == 0,
                        "E_IMPORT_LIVENOVEL: unexpected scenario wait"
                    );
                    pc += 1;
                }
                Body::Jump(target, condition) => {
                    if references(condition, "回想番号") {
                        ensure!(
                            condition.operations.iter().any(|(op, _, _)| *op == 15),
                            "E_IMPORT_LIVENOVEL: unexpected replay condition"
                        );
                        pc += 1;
                        continue;
                    }
                    ensure!(
                        condition.flag()?,
                        "E_IMPORT_LIVENOVEL: conditional route requires adaptation"
                    );
                    let next = local_target(&script, &page, target)?;
                    // A jump back to the initial dispatch restores the original title menu.
                    if !episodes.is_empty() && next < first {
                        break;
                    }
                    pc = next;
                }
                Body::Call {
                    target,
                    condition,
                    params,
                    ..
                } => {
                    ensure!(
                        target.page.replace('\\', "/") == "ノベルシステム/シーン回想/■フラグON.lsb"
                            && condition.flag()?
                            && params.len() == 1,
                        "E_IMPORT_LIVENOVEL: unexpected scenario call"
                    );
                    let key = format!("lm.replay.{}", literal_int(&params[0])?);
                    self.op(&mut main, json!({"type":"profile_merge","key":key}));
                    pc += 1;
                }
                Body::Exit(e) if e.flag()? => break,
                _ => bail!(
                    "E_IMPORT_LIVENOVEL: unsupported route command {}:{} {}",
                    page,
                    c.line,
                    c.name()
                ),
            }
        }
        ensure!(!episodes.is_empty(), "E_IMPORT_LIVENOVEL: no episodes");
        self.finish_function("main", main, json!({"type":"end","outcome":"completed"}));
        // Build replay wrappers from the original dispatcher, preserving its order.
        let (_, replay) = self.source.read("シーン回想.lsb")?;
        let (_, replay_ui) = self.source.read("ノベルシステム/シーン回想/■開始.lsb")?;
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
                background,
                buttons,
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
        self.menus.insert(
            "title".into(),
            ImageMenu {
                background,
                buttons,
            },
        );
        self.warnings.insert("Replay thumbnails retain original grid coordinates. Locked thumbnails preserve alpha with black RGB; a NIR return button and system-menu access remain available for touch/keyboard navigation.".into());
        self.scenes.insert("title".into(), vec![]);
        Ok(
            json!({"fragment_format":1,"functions":self.functions,"cues":self.cues,"scenes":self.scenes}),
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
    let story = adapter.run(entry)?;
    let mut report=ImportReport{format:1,engine:"livemaker-livenovel116".into(),status:"converted_with_adaptations".into(),written:false,errors:0,text_pages:adapter.texts.len(),functions:adapter.functions.len(),coverage:format!("Linear LiveNovel route, replay dispatch and title image menu. {} referenced media assets converted. Native system scripts are replaced; see fidelity warnings.",adapter.assets.len()),diagnostics:adapter.warnings.iter().map(|message|ImportDiagnostic{severity:"warning".into(),source:entry.into(),index:0,line:0,byte:0,command:"LiveNovelProfile".into(),message:message.clone()}).collect(),source_map:adapter.source_map.clone()};
    let staging = tempfile::Builder::new()
        .prefix(".nir-import-")
        .tempdir_in(out.parent().unwrap())?;
    let project = staging.path().join("project");
    crate::init(&project, sdk, &options.game_id)?;
    super::export(&project, options, &adapter.texts, &story, &report)?;
    adapter.export_media(&project)?;
    let loaded = crate::load_project(&project)?;
    crate::compile(&loaded.program)?;
    report.written = true;
    super::write_json(&project.join("import-report.json"), &report)?;
    ensure!(
        !out.try_exists()?,
        "E_IMPORT_EXISTS: output appeared during import"
    );
    fs::rename(project, out)?;
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
    fn mid_page_event_waits_for_text_marker_then_resumes_same_dialogue() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
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
        assert_eq!(
            blocks[1]["terminator"]["conditions"][0]["milestone"],
            json!({"type":"marker","id":"s1"})
        );
        assert_eq!(blocks[2]["terminator"]["type"], "activate");
        assert_eq!(
            blocks[4]["ops"][0]["operation"]["type"],
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
    fn page_completion_cancels_non_looping_voice_only() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
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
        assert_eq!(blocks[1]["terminator"]["type"], "await");
        assert_eq!(
            blocks[2]["ops"][0]["operation"],
            json!({"type":"task_control","task":"voice","action":"cancel"})
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
        assert_eq!(blocks.len(), 2);
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
}
