//! Bounded LiveMaker binary reader. The byte layout is documented by
//! https://pylivemaker.readthedocs.io/en/latest/livemaker.lsb.html .
//! This reader does not execute expressions, load DLLs, or launch game binaries.
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;

pub(super) const NAMES: [&str; 64] = [
    "If",
    "Elseif",
    "Else",
    "Label",
    "Jump",
    "Call",
    "Exit",
    "Wait",
    "BoxNew",
    "ImgNew",
    "MesNew",
    "Timer",
    "Movie",
    "Flip",
    "Calc",
    "VarNew",
    "VarDel",
    "GetProp",
    "SetProp",
    "ObjDel",
    "TextIns",
    "MovieStop",
    "ClrHist",
    "Cinema",
    "Caption",
    "Menu",
    "MenuClose",
    "Comment",
    "TextClr",
    "CallHist",
    "Button",
    "While",
    "WhileInit",
    "WhileLoop",
    "Break",
    "Continue",
    "ParticleNew",
    "FireNew",
    "GameSave",
    "GameLoad",
    "PCReset",
    "Reset",
    "Sound",
    "EditNew",
    "MemoNew",
    "Terminate",
    "DoEvent",
    "ClrRead",
    "MapImgNew",
    "WaveNew",
    "TileNew",
    "SliderNew",
    "ScrollbarNew",
    "GaugeNew",
    "CGCaption",
    "MediaPlay",
    "PrevMenuNew",
    "PropMotion",
    "FormatHist",
    "SaveCabinet",
    "LoadCabinet",
    "IFDEF",
    "IFNDEF",
    "ENDIF",
];

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .context("E_IMPORT_LIMIT: offset overflow")?;
        let bytes = self
            .data
            .get(self.at..end)
            .context("E_IMPORT_TRUNCATED: incomplete binary data")?;
        self.at = end;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(
            n <= max && n <= self.data.len().saturating_sub(self.at),
            "E_IMPORT_LIMIT: invalid length/count {n}"
        );
        Ok(n)
    }
    fn string(&mut self) -> Result<String> {
        let n = self.count(1024 * 1024)?;
        decode(self.bytes(n)?)
    }
    fn done(&self) -> Result<()> {
        ensure!(
            self.at == self.data.len(),
            "E_IMPORT_TRAILING: {} unparsed bytes",
            self.data.len() - self.at
        );
        Ok(())
    }
    fn expr(&mut self) -> Result<Expression> {
        let count = self.count(4096)?;
        let mut result = Expression::default();
        for _ in 0..count {
            let op = self.u8()?;
            ensure!(op <= 24, "E_IMPORT_EXPRESSION: unknown operator {op}");
            let name = self.string()?;
            let n = self.count(4096)?;
            if op == 11 {
                result.functions.insert(result.operations.len(), self.u8()?);
            }
            let mut literal = None;
            let mut operands = Vec::new();
            for _ in 0..n {
                let value = match self.u8()? {
                    0 => Literal::Variable(self.string()?),
                    1 => Literal::Int(self.i32()?),
                    2 => nir_format::Float80::from_le_bytes(self.bytes(10)?.try_into()?)
                        .map(Literal::Float)
                        .unwrap_or(Literal::Unsupported),
                    3 => Literal::Int(self.u8()?.into()),
                    4 => Literal::String(self.string()?),
                    ty => bail!("E_IMPORT_EXPRESSION: unknown operand type {ty}"),
                };
                literal = match &value {
                    Literal::Int(_) | Literal::Float(_) | Literal::String(_) => Some(value.clone()),
                    _ => None,
                };
                operands.push(value);
            }
            if count == 1 && n == 1 && matches!(op, 0 | 1) && (name.is_empty() || name == "____arg")
            {
                result.literal = literal;
            }
            result.operations.push((op, name, operands));
        }
        // Compiled string constants commonly use a temporary followed by
        // ____arg = temporary. Fold assignments only inside this temporary
        // namespace; never interpret source variables or function calls.
        let mut constants = BTreeMap::new();
        let mut safe = true;
        for (op, name, operands) in &result.operations {
            if !matches!(op, 0 | 1)
                || !(name.is_empty() || name.starts_with("____"))
                || operands.len() != 1
            {
                safe = false;
                break;
            }
            let value = match &operands[0] {
                Literal::Variable(name) => constants.get(name).cloned(),
                Literal::Int(_) | Literal::String(_) => Some(operands[0].clone()),
                _ => None,
            };
            if let Some(value) = value {
                constants.insert(name.clone(), value);
            } else {
                safe = false;
                break;
            }
        }
        if safe {
            if let Some((_, name, _)) = result.operations.last() {
                if name.is_empty() || name == "____arg" {
                    result.literal = constants.get(name).cloned();
                }
            }
        }
        Ok(result)
    }
    fn exprs(&mut self, n: usize) -> Result<Vec<Expression>> {
        (0..n).map(|_| self.expr()).collect()
    }
    fn expr_array(&mut self) -> Result<Vec<Expression>> {
        let n = self.count(4096)?;
        self.exprs(n)
    }
    fn reference(&mut self) -> Result<Reference> {
        Ok(Reference {
            page: self.string()?,
            line: self.u32()?,
        })
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<String> {
    let (text, errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(bytes);
    ensure!(!errors, "E_IMPORT_ENCODING: invalid CP932/Shift-JIS bytes");
    Ok(text.into_owned())
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(super) enum Literal {
    Int(i32),
    Float(nir_format::Float80),
    String(String),
    Variable(String),
    Unsupported,
}
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct Expression {
    pub literal: Option<Literal>,
    pub operations: Vec<(u8, String, Vec<Literal>)>,
    pub functions: BTreeMap<usize, u8>,
}
impl Expression {
    /// Recognize a bounded pure sum through compiler temporaries, without
    /// executing source code or accepting writes to source variables.
    pub fn is_sum_of_variables(&self, left: &str, right: &str) -> bool {
        use super::ui_expr::{normalize, Op, Term};
        if left == right {
            return false;
        }
        let Ok(Some(Term::Apply { op: Op::Add, args })) = normalize(self) else {
            return false;
        };
        matches!(args.as_slice(), [Term::Read { name: a }, Term::Read { name: b }]
            if (a == left && b == right) || (a == right && b == left))
    }

    pub fn flag(&self) -> Result<bool> {
        match &self.literal {
            Some(Literal::Int(n)) => Ok(*n != 0),
            _ => bail!("E_IMPORT_EXPRESSION: expected a literal integer/flag; dynamic expressions require adaptation"),
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Reference {
    pub page: String,
    pub line: u32,
}
#[derive(Clone, Debug)]
pub(super) enum Body {
    Other,
    Label(String),
    Delete(Expression),
    HistoryCall {
        parameters: BTreeMap<String, Expression>,
    },
    HistoryFormat {
        name: Expression,
        target: Expression,
    },
    Cabinet {
        properties: BTreeMap<u16, Expression>,
        act: Expression,
        targets: Vec<Expression>,
    },
    Flip {
        parameters: BTreeMap<String, Expression>,
        targets: Vec<Expression>,
    },
    Condition(Expression),
    LoopCondition {
        condition: Expression,
        target: u32,
    },
    Variable {
        name: String,
        value_type: u8,
        initial: Expression,
        scope: u8,
    },
    VariableDelete(String),
    Motion(Vec<Expression>),
    GetProperty {
        target: Expression,
        property: Expression,
        destination: String,
    },
    LoopUpdate {
        expression: Expression,
        target: Option<u32>,
    },
    Object(BTreeMap<u16, Expression>),
    SetProperty {
        target: Expression,
        property: Expression,
        value: Expression,
    },
    Jump(Reference, Expression),
    Call {
        target: Reference,
        condition: Expression,
        has_params: bool,
        params: Vec<Expression>,
    },
    Exit(Expression),
    Calc(Expression),
    Wait(Vec<Expression>),
    Text {
        text: Box<Novel>,
        target: Expression,
        history: Expression,
        wait: Expression,
        stop: Expression,
    },
}
#[derive(Clone, Debug)]
pub(super) struct Command {
    pub kind: u8,
    pub indent: u32,
    pub muted: bool,
    pub not_update: bool,
    pub line: u32,
    pub offset: usize,
    pub body: Body,
}
impl Command {
    pub fn name(&self) -> &'static str {
        NAMES[self.kind as usize]
    }
}
#[derive(Debug, Clone)]
pub(super) struct Script {
    pub source_sha256: String,
    pub version: u32,
    pub commands: Vec<Command>,
}

pub(super) fn parse(data: &[u8]) -> Result<Script> {
    let mut r = Reader::new(data);
    let version = r.u32()?;
    // Keep version gates explicit rather than guessing layouts of other releases.
    ensure!(
        matches!(version, 116 | 117),
        "E_IMPORT_VERSION: supported LSB versions are 116 and 117, found {version}"
    );
    r.u8()?; // script flags
    let types = r.count(64)?;
    let width = r.count(256)?;
    // Declaration masks are little-endian bit streams. Property identities
    // are one-based; SetProp's runtime property numbers are zero-based.
    let params = (0..types)
        .map(|_| {
            let mask = r.bytes(width)?;
            Ok(mask
                .iter()
                .enumerate()
                .flat_map(|(byte, bits)| {
                    (0..8).filter_map(move |bit| {
                        (bits & (1 << bit) != 0).then_some((byte * 8 + bit + 1) as u16)
                    })
                })
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>>>()?;
    let count = r.count(100_000)?;
    let mut commands = Vec::new();
    for index in 0..count {
        let offset = r.at;
        let command = (|| -> Result<Command> {
            let kind = r.u8()?;
            ensure!(
                (kind as usize) < types && kind < 64,
                "E_IMPORT_COMMAND: unknown command {kind}"
            );
            let indent = r.u32()?;
            let muted = r.u8()? != 0;
            let not_update = r.u8()? != 0;
            let line = r.u32()?;
            let mut body = Body::Other;
            match kind {
                0 | 1 => {
                    body = Body::Condition(r.expr()?);
                }
                19 => {
                    body = Body::Delete(r.expr()?);
                }
                26 | 28 | 39 | 55 => {
                    r.expr()?;
                }
                32 => {
                    body = Body::LoopUpdate {
                        expression: r.expr()?,
                        target: None,
                    };
                }
                14 => {
                    body = Body::Calc(r.expr()?);
                }
                2 | 22 | 45..=47 | 61..=63 => {}
                3 => {
                    body = Body::Label(r.string()?);
                }
                16 => {
                    body = Body::VariableDelete(r.string()?);
                }
                27 => {
                    r.string()?;
                }
                4 => {
                    body = Body::Jump(r.reference()?, r.expr()?);
                }
                5 => {
                    let target = r.reference()?;
                    let result = r.string()?;
                    let condition = r.expr()?;
                    let params = r.expr_array()?;
                    body = Body::Call {
                        target,
                        condition,
                        has_params: !result.is_empty() || !params.is_empty(),
                        params,
                    };
                }
                6 => {
                    body = Body::Exit(r.expr()?);
                }
                7 => {
                    body = Body::Wait(r.exprs(3)?);
                }
                18 => {
                    body = Body::SetProperty {
                        target: r.expr()?,
                        property: r.expr()?,
                        value: r.expr()?,
                    };
                }
                8..=12 | 23..=25 | 30 | 36 | 37 | 42..=44 | 48..=54 | 56 => {
                    let mut properties = BTreeMap::new();
                    for property in &params[kind as usize] {
                        properties.insert(*property, r.expr()?);
                    }
                    body = Body::Object(properties);
                }
                13 => {
                    let mut parameters = BTreeMap::new();
                    for name in ["wipe", "time", "reverse", "act"] {
                        parameters.insert(name.into(), r.expr()?);
                    }
                    let targets = r.expr_array()?;
                    // Version 116 has two fixed, unprefixed effect parameters,
                    // followed by Source and StopEvent; no DifferenceOnly.
                    for name in [
                        "delete",
                        "parameter_0",
                        "parameter_1",
                        "source",
                        "stop_event",
                    ] {
                        parameters.insert(name.into(), r.expr()?);
                    }
                    // LSB117 appends DifferenceOnly after StopEvent. Preserve
                    // it for semantic validation instead of losing alignment.
                    if version == 117 {
                        parameters.insert("difference_only".into(), r.expr()?);
                    }
                    body = Body::Flip {
                        parameters,
                        targets,
                    };
                }
                15 => {
                    body = Body::Variable {
                        name: r.string()?,
                        value_type: r.u8()?,
                        initial: r.expr()?,
                        scope: r.u8()?,
                    };
                }
                17 => {
                    body = Body::GetProperty {
                        target: r.expr()?,
                        property: r.expr()?,
                        destination: r.string()?,
                    };
                }
                20 => {
                    let n = r.count(16 * 1024 * 1024)?;
                    let text = novel(r.bytes(n)?)?;
                    body = Body::Text {
                        text: Box::new(text),
                        target: r.expr()?,
                        history: r.expr()?,
                        wait: r.expr()?,
                        stop: r.expr()?,
                    };
                }
                21 => {
                    r.exprs(4)?;
                }
                29 => {
                    let mut parameters = BTreeMap::new();
                    // Preserve source units: Index/Count are not NIR entry offsets.
                    for name in ["target", "index", "count", "cut_break", "format_name"] {
                        parameters.insert(name.into(), r.expr()?);
                    }
                    body = Body::HistoryCall { parameters };
                }
                31 => {
                    body = Body::LoopCondition {
                        condition: r.expr()?,
                        target: r.u32()?,
                    };
                }
                33 => {
                    body = Body::LoopUpdate {
                        expression: r.expr()?,
                        target: Some(r.u32()?),
                    };
                }
                34..=35 => {
                    r.expr()?;
                    r.u32()?;
                }
                38 => {
                    r.expr()?;
                    r.string()?;
                    r.u32()?;
                    r.expr()?;
                }
                40 | 41 => {
                    r.reference()?;
                    r.u8()?;
                }
                57 => {
                    body = Body::Motion(r.exprs(7)?);
                }
                58 => {
                    body = Body::HistoryFormat {
                        name: r.expr()?,
                        target: r.expr()?,
                    };
                }
                59 | 60 => {
                    let mut properties = BTreeMap::new();
                    for property in &params[kind as usize] {
                        properties.insert(*property, r.expr()?);
                    }
                    body = Body::Cabinet {
                        properties,
                        act: r.expr()?,
                        targets: r.expr_array()?,
                    };
                }
                _ => unreachable!(),
            }
            Ok(Command {
                kind,
                indent,
                muted,
                not_update,
                line,
                offset,
                body,
            })
        })()
        .with_context(|| format!("E_IMPORT_LSB: command index {index}, byte {offset}"))?;
        commands.push(command);
    }
    r.done()?;
    Ok(Script {
        source_sha256: nir_content::digest(data),
        version,
        commands,
    })
}

#[derive(Clone, Debug)]
pub(super) enum Glyph {
    Char(String),
    Variable(String),
    RubyChar {
        text: String,
        reading: String,
        style: i32,
    },
    InlineImage {
        source: String,
        align: u8,
        hover: String,
        margins: [i32; 4],
        down: String,
    },
    Break(u8),
    Event(Vec<String>),
    Unsupported,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Novel {
    pub glyphs: Vec<Glyph>,
    pub counts: BTreeMap<String, usize>,
    pub events: BTreeMap<String, usize>,
    pub has_conditions_or_links: bool,
    pub has_ruby: bool,
    pub conditions: Vec<(u32, String)>,
    pub links: Vec<(u32, String, String)>,
    pub glyph_conditions: Vec<i32>,
    pub glyph_links: Vec<i32>,
}
fn novel(data: &[u8]) -> Result<Novel> {
    let mut r = Reader::new(data);
    ensure!(
        r.bytes(6)? == b"TpWord",
        "E_IMPORT_TEXT: invalid TpWord signature"
    );
    let version = std::str::from_utf8(r.bytes(3)?)?.parse::<u32>()?;
    ensure!(
        (100..=105).contains(&version),
        "E_IMPORT_TEXT_VERSION: unsupported TpWord {version}"
    );
    let mut result = Novel::default();
    let mut legacy_link = false;
    let styles = r.count(100_000)?;
    let mut ruby_styles = Vec::with_capacity(styles);
    for _ in 0..styles {
        r.bytes(22)?;
        r.string()?;
        let ruby = r.string()?;
        ensure!(
            ruby.len() <= 4096,
            "E_IMPORT_TEXT: ruby annotation exceeds 4 KiB"
        );
        result.has_ruby |= !ruby.is_empty();
        ruby_styles.push(ruby);
        r.bytes(8)?;
    }
    if version >= 104 {
        let n = r.count(100_000)?;
        for _ in 0..n {
            result.conditions.push((r.u32()?, r.string()?));
        }
    }
    if version >= 105 {
        let n = r.count(100_000)?;
        for _ in 0..n {
            result.links.push((r.u32()?, r.string()?, r.string()?));
        }
    }
    let n = r.count(1_000_000)?;
    for _ in 0..n {
        let kind = r.u8()?;
        *result.counts.entry(format!("{kind:02x}")).or_default() += 1;
        if version >= 104 {
            let condition = r.i32()?;
            result.has_conditions_or_links |= condition != -1;
            result.glyph_conditions.push(condition);
        }
        if matches!(kind, 1 | 9) {
            if version >= 105 {
                let link = r.i32()?;
                result.has_conditions_or_links |= link != -1;
                result.glyph_links.push(link);
            } else {
                legacy_link |= !r.string()?.is_empty();
                result.has_conditions_or_links |= legacy_link;
            }
            r.u32()?; // source reveal speed is normalized by the importer
        }
        let glyph = match kind {
            1 => {
                let bytes = r.bytes(2)?;
                let ordered = [bytes[1], bytes[0]];
                let text = decode(if ordered[0] == 0 {
                    &ordered[1..]
                } else {
                    &ordered
                })?;
                let style = r.i32()?;
                ensure!(
                    style >= 0 && (style as usize) < ruby_styles.len(),
                    "E_IMPORT_TEXT: invalid style index"
                );
                let reading = &ruby_styles[style as usize];
                if reading.is_empty() {
                    Glyph::Char(text)
                } else {
                    Glyph::RubyChar {
                        text,
                        reading: reading.clone(),
                        style,
                    }
                }
            }
            2 => {
                r.u8()?;
                if version >= 105 {
                    r.bytes(9)?;
                }
                Glyph::Unsupported
            }
            3 => Glyph::Break(r.u8()?),
            4 | 5 => Glyph::Unsupported,
            6 => {
                let event = r.string()?;
                let name = event
                    .strip_prefix('\u{1}')
                    .map(|s| s.split("\r\n").next().unwrap_or("unknown"))
                    .unwrap_or("custom");
                *result.events.entry(name.into()).or_default() += 1;
                Glyph::Event(event.split("\r\n").map(str::to_owned).collect())
            }
            7 | 10 => {
                let style = r.i32()?;
                ensure!(
                    style >= 0 && (style as usize) < ruby_styles.len(),
                    "E_IMPORT_TEXT: invalid variable style index"
                );
                if version > 100 {
                    r.u32()?;
                }
                if (101..105).contains(&version) {
                    legacy_link |= !r.string()?.is_empty();
                }
                if version >= 105 {
                    result.glyph_links.push(r.i32()?);
                }
                if version < 102 {
                    r.expr()?;
                    Glyph::Unsupported
                } else {
                    let name = r.string()?;
                    if kind == 7 && ruby_styles[style as usize].is_empty() {
                        Glyph::Variable(name)
                    } else {
                        Glyph::Unsupported
                    }
                }
            }
            9 => {
                let source = r.string()?;
                let align = r.u8()?;
                let hover = if version >= 103 {
                    r.string()?
                } else {
                    String::new()
                };
                let margins = if version >= 105 {
                    [r.i32()?, r.i32()?, r.i32()?, r.i32()?]
                } else {
                    [0; 4]
                };
                let down = if version >= 105 {
                    r.string()?
                } else {
                    String::new()
                };
                Glyph::InlineImage {
                    source,
                    align,
                    hover,
                    margins,
                    down,
                }
            }
            _ => bail!("E_IMPORT_GLYPH: unknown glyph type {kind}"),
        };
        result.glyphs.push(glyph);
    }
    r.done()?;
    // Newer source compilers emit explicit empty default condition/link
    // rows. Resolve every index before certifying that they have no action;
    // nonempty expressions and link callbacks remain unsupported.
    let mut active = legacy_link;
    for index in &result.glyph_conditions {
        ensure!(*index >= -1, "E_IMPORT_TEXT: invalid condition index");
        if *index >= 0 {
            let (_, expression) = result
                .conditions
                .get(*index as usize)
                .context("E_IMPORT_TEXT: condition index outside table")?;
            active |= !expression.is_empty();
        }
    }
    for index in &result.glyph_links {
        ensure!(*index >= -1, "E_IMPORT_TEXT: invalid link index");
        if *index >= 0 {
            let (_, name, action) = result
                .links
                .get(*index as usize)
                .context("E_IMPORT_TEXT: link index outside table")?;
            active |= !name.is_empty() || !action.is_empty();
        }
    }
    result.has_conditions_or_links = active;
    Ok(result)
}

pub(super) fn startup(data: &[u8]) -> Result<String> {
    let mut r = Reader::new(data);
    let version = r.u32()?;
    ensure!(
        matches!(version, 116 | 117),
        "E_IMPORT_VERSION: supported LPB versions are 116 and 117, found {version}"
    );
    r.string()?; // title
    r.bytes(16)?;
    r.string()
}

/// Read the documented LPB116/117 settings prefix. Later project/editor sections are
/// intentionally not interpreted or exported by the importer.
pub(super) fn project_settings(data: &[u8]) -> Result<BTreeMap<String, Literal>> {
    let mut r = Reader::new(data);
    let version = r.u32()?;
    ensure!(
        matches!(version, 116 | 117),
        "E_IMPORT_VERSION: supported LPB versions are 116 and 117, found {version}"
    );
    r.string()?;
    r.bytes(16)?; // project title and reserved header
    r.string()?;
    r.string()?;
    r.string()?; // startup, exit, author project directory
    r.u32()?;
    r.bytes(2)?;
    r.string()?;
    r.bytes(3)?;
    r.string()?;
    r.string()?; // prompts
    let count = r.count(4096)?;
    let mut values = BTreeMap::new();
    for _ in 0..count {
        let kind = r.u8()?;
        let name = r.string()?;
        let value = match kind {
            1 => Literal::Int(r.i32()?),
            2 => nir_format::Float80::from_le_bytes(r.bytes(10)?.try_into()?)
                .map(Literal::Float)
                .unwrap_or(Literal::Unsupported),
            3 => Literal::Int(r.u8()?.into()),
            4 => Literal::String(r.string()?),
            _ => bail!("E_IMPORT_SETTINGS: unsupported setting type {kind}"),
        };
        ensure!(
            values.insert(name, value).is_none(),
            "E_IMPORT_SETTINGS: duplicate setting"
        );
    }
    Ok(values)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    #[test]
    fn version117_flip_reads_difference_only_and_preserves_next_command() {
        let mut args = vec![];
        for n in [3, 200, 0, 1] {
            args.extend(integer(n));
        }
        u32b(&mut args, 0); // targets
        for n in [0, 20, 1, 0, 0, 0] {
            args.extend(integer(n));
        }
        let mut label = vec![];
        string(&mut label, "after-flip");
        let mut bytes = script(&[command(13, 10, &args), command(3, 11, &label)]);
        bytes[..4].copy_from_slice(&117u32.to_le_bytes());
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.version, 117);
        let Body::Flip { parameters, .. } = &parsed.commands[0].body else {
            panic!("missing flip");
        };
        assert_eq!(parameters.len(), 10);
        assert!(matches!(
            parameters["difference_only"].literal,
            Some(Literal::Int(0))
        ));
        assert!(matches!(&parsed.commands[1].body, Body::Label(s) if s == "after-flip"));
        for end in 0..bytes.len() {
            assert!(parse(&bytes[..end]).is_err(), "truncated prefix {end}");
        }
        bytes[..4].copy_from_slice(&116u32.to_le_bytes());
        assert!(parse(&bytes).is_err(), "117 field cannot be read as116");
        bytes[..4].copy_from_slice(&118u32.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }
    #[test]
    fn version117_project_prefix_preserves_startup_and_settings() {
        let mut bytes = settings_file(&[("StatusBGMVolume", 400), ("StatusAutoTextWait", 1200)]);
        bytes[..4].copy_from_slice(&117u32.to_le_bytes());
        assert_eq!(startup(&bytes).unwrap(), "main.lsb");
        let values = project_settings(&bytes).unwrap();
        assert!(matches!(values["StatusBGMVolume"], Literal::Int(400)));
        assert!(matches!(values["StatusAutoTextWait"], Literal::Int(1200)));
        assert!(project_settings(&bytes[..bytes.len() - 1]).is_err());
        bytes[..4].copy_from_slice(&118u32.to_le_bytes());
        assert!(startup(&bytes).is_err());
        assert!(project_settings(&bytes).is_err());
    }
    #[test]
    fn layout_variable_property_read_and_loop_updates_preserve_data_flow() {
        let mut variable = vec![];
        string(&mut variable, "width");
        variable.push(1);
        variable.extend(integer(16));
        variable.push(2);
        let mut read = integer(7);
        read.extend(integer(5));
        string(&mut read, "width");
        let mut update = integer(1);
        u32b(&mut update, 42);
        let bytes = script(&[
            command(15, 10, &variable),
            command(17, 11, &read),
            command(32, 12, &integer(0)),
            command(33, 13, &update),
        ]);
        let parsed = parse(&bytes).unwrap();
        assert!(
            matches!(&parsed.commands[0].body, Body::Variable { name, value_type: 1, initial, scope: 2 }
            if name == "width" && matches!(initial.literal, Some(Literal::Int(16))))
        );
        assert!(
            matches!(&parsed.commands[1].body, Body::GetProperty { target, property, destination }
            if destination == "width" && matches!(target.literal, Some(Literal::Int(7))) && matches!(property.literal, Some(Literal::Int(5))))
        );
        assert!(matches!(
            &parsed.commands[2].body,
            Body::LoopUpdate { target: None, .. }
        ));
        assert!(matches!(
            &parsed.commands[3].body,
            Body::LoopUpdate {
                target: Some(42),
                ..
            }
        ));
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
    }
    #[test]
    fn conditional_and_loop_expressions_remain_available_to_ui_lowering() {
        let mut loop_args = integer(1);
        u32b(&mut loop_args, 77);
        let bytes = script(&[
            command(0, 10, &integer(0)),
            command(1, 11, &integer(1)),
            command(2, 12, &[]),
            command(31, 13, &loop_args),
        ]);
        let parsed = parse(&bytes).unwrap();
        assert!(matches!(&parsed.commands[0].body, Body::Condition(e) if !e.flag().unwrap()));
        assert!(matches!(&parsed.commands[1].body, Body::Condition(e) if e.flag().unwrap()));
        assert!(
            matches!(&parsed.commands[3].body, Body::LoopCondition { condition, target: 77 } if condition.flag().unwrap())
        );
    }
    #[test]
    fn extended_literals_preserve_all_ten_bytes_without_host_float_rounding() {
        let bits = (0x3fffu128 << 64) | (1u128 << 63) | 1;
        let mut bytes = vec![];
        u32b(&mut bytes, 1);
        bytes.push(1);
        string(&mut bytes, "____arg");
        u32b(&mut bytes, 1);
        bytes.push(2);
        bytes.extend_from_slice(&bits.to_le_bytes()[..10]);
        let expression = Reader::new(&bytes).expr().unwrap();
        assert!(matches!(expression.literal, Some(Literal::Float(v)) if v.bits() == bits));
        assert!(Reader::new(&bytes[..bytes.len() - 1]).expr().is_err());
    }

    #[test]
    fn string_temporary_is_folded_without_evaluating_source_variables() {
        let mut bytes = vec![];
        u32b(&mut bytes, 2);
        bytes.push(1);
        string(&mut bytes, "____0");
        u32b(&mut bytes, 1);
        bytes.push(4);
        string(&mut bytes, "Box");
        bytes.push(1);
        string(&mut bytes, "____arg");
        u32b(&mut bytes, 1);
        bytes.push(0);
        string(&mut bytes, "____0");
        let value = Reader::new(&bytes).expr().unwrap();
        assert!(matches!(value.literal,Some(Literal::String(s)) if s=="Box"));
        let mut bytes = vec![];
        u32b(&mut bytes, 1);
        bytes.push(1);
        string(&mut bytes, "____arg");
        u32b(&mut bytes, 1);
        bytes.push(0);
        string(&mut bytes, "external");
        assert!(Reader::new(&bytes).expr().unwrap().literal.is_none());
    }
    pub fn u32b(out: &mut Vec<u8>, n: u32) {
        out.extend(n.to_le_bytes());
    }
    pub fn string(out: &mut Vec<u8>, s: &str) {
        let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(s);
        assert!(!errors);
        u32b(out, bytes.len() as u32);
        out.extend_from_slice(&bytes);
    }
    pub fn literal(out: &mut Vec<u8>, n: u8) {
        u32b(out, 1);
        out.push(1);
        string(out, "____arg");
        u32b(out, 1);
        out.extend([3, n]);
    }
    pub fn command(kind: u8, line: u32, args: &[u8]) -> Vec<u8> {
        let mut b = vec![kind];
        u32b(&mut b, 0);
        b.extend([0, 0]);
        u32b(&mut b, line);
        b.extend(args);
        b
    }
    pub fn script(commands: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![];
        u32b(&mut b, 116);
        b.push(0);
        u32b(&mut b, 64);
        u32b(&mut b, 1);
        b.extend([0; 64]);
        u32b(&mut b, commands.len() as u32);
        for c in commands {
            b.extend(c);
        }
        b
    }
    pub fn integer(n: i32) -> Vec<u8> {
        let mut out = vec![];
        u32b(&mut out, 1);
        out.push(1);
        string(&mut out, "____arg");
        u32b(&mut out, 1);
        out.push(1);
        out.extend(n.to_le_bytes());
        out
    }
    #[test]
    fn labels_and_deletion_targets_preserve_alignment_and_reject_truncation() {
        let mut label = vec![];
        string(&mut label, "return-to-parent");
        let mut deletion = vec![];
        u32b(&mut deletion, 1);
        deletion.push(1);
        string(&mut deletion, "____arg");
        u32b(&mut deletion, 1);
        deletion.push(4);
        string(&mut deletion, "history-container");
        let parsed = parse(&script(&[
            command(3, 10, &label),
            command(19, 11, &deletion),
            command(6, 12, &integer(1)),
        ]))
        .unwrap();
        assert!(
            matches!(&parsed.commands[0].body, Body::Label(name) if name == "return-to-parent")
        );
        assert!(matches!(&parsed.commands[1].body, Body::Delete(e)
            if matches!(&e.literal, Some(Literal::String(name)) if name == "history-container")));
        assert!(matches!(&parsed.commands[2].body, Body::Exit(_)));
        for (kind, args) in [(3, label), (19, deletion)] {
            for length in 0..args.len() {
                assert!(parse(&script(&[command(kind, 1, &args[..length])])).is_err());
            }
        }
    }
    #[test]
    fn history_calls_preserve_parameter_order_and_empty_formatter() {
        let mut call = vec![];
        for n in [11, -23, 450, 0] {
            call.extend(integer(n));
        }
        u32b(&mut call, 0); // empty FormatName differs from numeric zero
        let mut format = integer(17);
        format.extend(integer(19));
        let bytes = script(&[
            command(29, 10, &call),
            command(58, 11, &format),
            command(6, 12, &integer(1)),
        ]);
        let parsed = parse(&bytes).unwrap();
        let Body::HistoryCall { parameters } = &parsed.commands[0].body else {
            panic!()
        };
        assert_eq!(parameters.len(), 5);
        for (key, expected) in [
            ("target", 11),
            ("index", -23),
            ("count", 450),
            ("cut_break", 0),
        ] {
            assert!(matches!(parameters[key].literal, Some(Literal::Int(n)) if n == expected));
        }
        assert!(parameters["format_name"].operations.is_empty());
        assert!(parameters["format_name"].literal.is_none());
        assert!(
            matches!(&parsed.commands[1].body, Body::HistoryFormat { name, target }
            if matches!(name.literal, Some(Literal::Int(17))) && matches!(target.literal, Some(Literal::Int(19))))
        );
        assert!(matches!(&parsed.commands[2].body, Body::Exit(_)));
        // Every truncated command body must fail, including a missing formatter.
        for kind_args in [(29, &call), (58, &format)] {
            for length in 0..kind_args.1.len() {
                assert!(
                    parse(&script(&[command(kind_args.0, 1, &kind_args.1[..length])])).is_err()
                );
            }
        }
    }
    #[test]
    fn cabinet_and_flip_keep_targets_flags_and_fixed_parameter_order() {
        let mut cabinet = integer(1);
        u32b(&mut cabinet, 2);
        cabinet.extend(integer(12));
        cabinet.extend(integer(34));
        let mut flip = vec![];
        for n in [3, 200, 1, 0] {
            flip.extend(integer(n));
        }
        u32b(&mut flip, 1);
        flip.extend(integer(99));
        for n in [1, 71, 72, 73, 74] {
            flip.extend(integer(n));
        }
        let bytes = script(&[
            command(59, 1, &cabinet),
            command(60, 2, &cabinet),
            command(13, 3, &flip),
        ]);
        let parsed = parse(&bytes).unwrap();
        for c in &parsed.commands[..2] {
            let Body::Cabinet {
                properties,
                act,
                targets,
            } = &c.body
            else {
                panic!()
            };
            assert!(properties.is_empty());
            assert!(matches!(act.literal, Some(Literal::Int(1))));
            assert_eq!(targets.len(), 2);
            assert!(matches!(targets[1].literal, Some(Literal::Int(34))));
        }
        let Body::Flip {
            parameters,
            targets,
        } = &parsed.commands[2].body
        else {
            panic!()
        };
        assert_eq!(parameters.len(), 9);
        assert_eq!(targets.len(), 1);
        for (name, value) in [
            ("time", 200),
            ("delete", 1),
            ("parameter_0", 71),
            ("parameter_1", 72),
            ("source", 73),
            ("stop_event", 74),
        ] {
            assert!(
                matches!(parameters[name].literal,Some(Literal::Int(n)) if n==value),
                "{name}"
            );
        }
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
        let mut sparse = object_script(60, &[(1, integer(4)), (43, integer(-128))]);
        sparse.extend(integer(0));
        u32b(&mut sparse, 0);
        let parsed = parse(&sparse).unwrap();
        assert!(
            matches!(&parsed.commands[0].body,Body::Cabinet {properties,targets,..} if properties.contains_key(&43)&&targets.is_empty())
        );
    }
    pub fn object_script(kind: u8, properties: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let width = 17;
        let mut out = vec![];
        u32b(&mut out, 116);
        out.push(0);
        u32b(&mut out, 64);
        u32b(&mut out, width);
        for command_kind in 0..64 {
            let mut mask = vec![0; width as usize];
            if command_kind == kind {
                for (id, _) in properties {
                    let bit = usize::from(*id) - 1;
                    mask[bit / 8] |= 1 << (bit % 8);
                }
            }
            out.extend(mask);
        }
        let args: Vec<_> = properties
            .iter()
            .flat_map(|(_, value)| value.clone())
            .collect();
        u32b(&mut out, 1);
        out.extend(command(kind, 17, &args));
        out
    }
    #[test]
    fn sparse_object_properties_preserve_identity_and_source_update_flags() {
        let mut bytes = object_script(
            51,
            &[
                (4, integer(37)),
                (124, integer(0)),
                (125, integer(1000)),
                (128, integer(10)),
            ],
        );
        let command_offset = 13 + 64 * 17 + 4;
        bytes[command_offset + 6] = 1;
        let parsed = parse(&bytes).unwrap();
        let command = &parsed.commands[0];
        assert!(command.not_update);
        assert_eq!(command.offset, command_offset);
        let Body::Object(properties) = &command.body else {
            panic!()
        };
        assert_eq!(
            properties.keys().copied().collect::<Vec<_>>(),
            vec![4, 124, 125, 128]
        );
        assert!(matches!(properties[&125].literal, Some(Literal::Int(1000))));
        assert!(matches!(properties[&128].literal, Some(Literal::Int(10))));
        assert!(!properties.contains_key(&3));
        for n in [13, command_offset, bytes.len() - 1] {
            assert!(parse(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn property_mutations_keep_runtime_zero_based_number_and_expressions() {
        let args = [integer(7), integer(3), integer(99)].concat();
        let parsed = parse(&script(&[command(18, 9, &args)])).unwrap();
        let Body::SetProperty {
            target,
            property,
            value,
        } = &parsed.commands[0].body
        else {
            panic!()
        };
        assert!(matches!(target.literal, Some(Literal::Int(7))));
        assert!(matches!(property.literal, Some(Literal::Int(3))));
        assert!(matches!(value.literal, Some(Literal::Int(99))));
    }
    #[test]
    fn expression_retains_function_identity_without_executing_it() {
        let mut bytes = vec![];
        u32b(&mut bytes, 1);
        bytes.push(11);
        string(&mut bytes, "____arg");
        u32b(&mut bytes, 0);
        bytes.push(37);
        let expression = Reader::new(&bytes).expr().unwrap();
        assert_eq!(expression.functions.get(&0), Some(&37));
        assert!(expression.literal.is_none());
    }
    #[test]
    fn pure_sum_recognizes_compiler_temporaries_but_rejects_side_effects_and_other_units() {
        let variable = |name: &str| Literal::Variable(name.into());
        let mut expression = Expression {
            literal: None,
            functions: BTreeMap::new(),
            operations: vec![
                (
                    2,
                    "____0".into(),
                    vec![variable("delay"), variable("remaining")],
                ),
                (1, "____arg".into(), vec![variable("____0")]),
            ],
        };
        assert!(expression.is_sum_of_variables("delay", "remaining"));
        expression.operations[0].0 = 4;
        assert!(!expression.is_sum_of_variables("delay", "remaining"));
        expression.operations[0].0 = 2;
        expression.operations[0].1 = "source_variable".into();
        assert!(!expression.is_sum_of_variables("delay", "remaining"));
        expression.operations[0].1 = "____source_variable".into();
        assert!(!expression.is_sum_of_variables("delay", "remaining"));
        expression.operations[0].1 = "____0".into();
        expression.functions.insert(0, 1);
        assert!(!expression.is_sum_of_variables("delay", "remaining"));
    }
    pub fn settings_file(settings: &[(&str, i32)]) -> Vec<u8> {
        let mut bytes = vec![];
        u32b(&mut bytes, 116);
        string(&mut bytes, "Fixture");
        bytes.extend([0; 16]);
        for value in ["main.lsb", "", "author-project-directory"] {
            string(&mut bytes, value);
        }
        u32b(&mut bytes, 0);
        bytes.extend([0; 2]);
        string(&mut bytes, ".wav");
        bytes.extend([0; 3]);
        string(&mut bytes, "");
        string(&mut bytes, "");
        u32b(&mut bytes, settings.len() as u32);
        for (name, value) in settings {
            bytes.push(1);
            string(&mut bytes, name);
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn lpb_settings_preserve_values_and_reject_duplicate_or_truncated_prefix() {
        let bytes = settings_file(&[("Delay", 3000), ("Volume", 1000)]);
        let values = project_settings(&bytes).unwrap();
        assert!(matches!(values["Delay"], Literal::Int(3000)));
        assert!(matches!(values["Volume"], Literal::Int(1000)));
        assert!(project_settings(&bytes[..bytes.len() - 1]).is_err());
        assert!(project_settings(&settings_file(&[("Delay", 1), ("Delay", 2)])).is_err());
        let mut extended = bytes;
        extended.extend([1, 2, 3]);
        assert_eq!(project_settings(&extended).unwrap().len(), 2);
    }
    pub fn dialogue(text: &str) -> Vec<u8> {
        let mut b = b"TpWord105".to_vec();
        u32b(&mut b, 1);
        b.extend([0; 22]);
        string(&mut b, "");
        string(&mut b, "");
        b.extend([0; 8]);
        for _ in 0..2 {
            u32b(&mut b, 0);
        }
        u32b(&mut b, text.chars().count() as u32);
        for ch in text.chars() {
            b.push(1);
            u32b(&mut b, u32::MAX);
            u32b(&mut b, u32::MAX);
            u32b(&mut b, 0);
            let character = ch.to_string();
            let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&character);
            let n = if bytes.len() == 1 {
                u16::from(bytes[0])
            } else {
                u16::from_be_bytes([bytes[0], bytes[1]])
            };
            b.extend(n.to_le_bytes());
            u32b(&mut b, 0);
        }
        let mut args = vec![];
        u32b(&mut args, b.len() as u32);
        args.extend(b);
        u32b(&mut args, 1);
        args.push(1);
        string(&mut args, "____arg");
        u32b(&mut args, 1);
        args.push(4);
        string(&mut args, "message");
        for _ in 0..3 {
            literal(&mut args, 1);
        }
        args
    }
    #[test]
    fn inline_images_keep_geometry_and_validate_explicit_default_tables() {
        let file = |condition: &str, index: u32| {
            let mut b = b"TpWord105".to_vec();
            u32b(&mut b, 0); // no font decorators on the inline image
            u32b(&mut b, 1);
            u32b(&mut b, 1);
            string(&mut b, condition);
            u32b(&mut b, 1);
            u32b(&mut b, 1);
            string(&mut b, "");
            string(&mut b, "");
            u32b(&mut b, 1);
            b.push(9);
            u32b(&mut b, index);
            u32b(&mut b, 0);
            u32b(&mut b, 0);
            string(&mut b, "icons\\heart.gal");
            b.push(3);
            string(&mut b, "hover.gal");
            for value in [1, 2, 3, 4] {
                u32b(&mut b, value);
            }
            string(&mut b, "down.gal");
            b
        };
        let bytes = file("", 0);
        let parsed = novel(&bytes).unwrap();
        assert!(!parsed.has_conditions_or_links);
        assert!(
            matches!(&parsed.glyphs[0], Glyph::InlineImage { source, align:3, hover, margins:[1,2,3,4], down }
            if source == "icons\\heart.gal" && hover == "hover.gal" && down == "down.gal")
        );
        assert!(novel(&file("enabled", 0)).unwrap().has_conditions_or_links);
        assert!(novel(&file("", 1)).is_err());
        assert!(novel(&bytes[..bytes.len() - 1]).is_err());
    }
    #[test]
    fn variable_glyphs_preserve_slot_names_and_validate_links_and_styles() {
        let file = |kind: u8, style: u32, link: u32| {
            let mut b = b"TpWord105".to_vec();
            u32b(&mut b, 1);
            b.extend([0; 22]);
            string(&mut b, "");
            string(&mut b, "");
            b.extend([0; 8]);
            u32b(&mut b, 0);
            u32b(&mut b, 1);
            u32b(&mut b, 1);
            string(&mut b, "open");
            string(&mut b, "callback");
            u32b(&mut b, 1);
            b.push(kind);
            u32b(&mut b, u32::MAX);
            u32b(&mut b, style);
            u32b(&mut b, 0);
            u32b(&mut b, link);
            string(&mut b, "Counter");
            b
        };
        let bytes = file(7, 0, u32::MAX);
        let parsed = novel(&bytes).unwrap();
        assert!(matches!(&parsed.glyphs[0], Glyph::Variable(name) if name == "Counter"));
        assert!(!parsed.has_conditions_or_links);
        assert!(novel(&file(7, 0, 0)).unwrap().has_conditions_or_links);
        assert!(novel(&file(7, 0, 1)).is_err());
        assert!(novel(&file(7, 1, u32::MAX)).is_err());
        assert!(matches!(
            novel(&file(10, 0, u32::MAX)).unwrap().glyphs[0],
            Glyph::Unsupported
        ));
        assert!(novel(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn reads_japanese_and_rejects_truncation() {
        let bytes = script(&[command(20, 15, &dialogue("こんにちは。"))]);
        let parsed = parse(&bytes).unwrap();
        let Body::Text { text, .. } = &parsed.commands[0].body else {
            panic!()
        };
        assert_eq!(text.glyphs.len(), 6);
        assert!(matches!(&text.glyphs[0], Glyph::Char(c) if c == "こ"));
        for end in 0..bytes.len() {
            assert!(parse(&bytes[..end]).is_err(), "prefix {end}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(parse(&extra).is_err());
    }
    #[test]
    fn rejects_unknown_versions_and_huge_counts() {
        assert!(parse(&[255; 64]).is_err());
        let mut bytes = script(&[]);
        bytes[77..81].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }
}
