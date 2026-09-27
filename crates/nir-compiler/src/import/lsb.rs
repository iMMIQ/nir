//! Bounded LiveMaker binary reader. The byte layout is documented by
//! https://pylivemaker.readthedocs.io/en/latest/livemaker.lsb.html .
//! This reader does not execute expressions, load DLLs, or launch game binaries.
use anyhow::{bail, ensure, Context, Result};
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
                self.u8()?;
            }
            let mut literal = None;
            let mut operands = Vec::new();
            for _ in 0..n {
                let value = match self.u8()? {
                    0 => Literal::Variable(self.string()?),
                    1 => Literal::Int(self.i32()?),
                    2 => {
                        self.bytes(10)?;
                        Literal::Unsupported
                    }
                    3 => Literal::Int(self.u8()?.into()),
                    4 => Literal::String(self.string()?),
                    ty => bail!("E_IMPORT_EXPRESSION: unknown operand type {ty}"),
                };
                literal = match &value {
                    Literal::Int(_) | Literal::String(_) => Some(value.clone()),
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

#[derive(Clone, Debug)]
pub(super) enum Literal {
    Int(i32),
    String(String),
    Variable(String),
    Unsupported,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Expression {
    pub literal: Option<Literal>,
    pub operations: Vec<(u8, String, Vec<Literal>)>,
}
impl Expression {
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
        text: Novel,
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
    pub line: u32,
    pub offset: usize,
    pub body: Body,
}
impl Command {
    pub fn name(&self) -> &'static str {
        NAMES[self.kind as usize]
    }
}
#[derive(Debug)]
pub(super) struct Script {
    pub version: u32,
    pub commands: Vec<Command>,
}

pub(super) fn parse(data: &[u8]) -> Result<Script> {
    let mut r = Reader::new(data);
    let version = r.u32()?;
    // Keep version gates explicit rather than guessing layouts of other releases.
    ensure!(
        version == 116,
        "E_IMPORT_VERSION: supported LSB version is 116, found {version}"
    );
    r.u8()?; // script flags
    let types = r.count(64)?;
    let width = r.count(256)?;
    let params = (0..types)
        .map(|_| {
            Ok(r.bytes(width)?
                .iter()
                .map(|b| b.count_ones() as usize)
                .sum::<usize>())
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
            r.u8()?; // NotUpdate
            let line = r.u32()?;
            let mut body = Body::Other;
            match kind {
                0 | 1 | 19 | 26 | 28 | 32 | 39 | 55 => {
                    r.expr()?;
                }
                14 => {
                    body = Body::Calc(r.expr()?);
                }
                2 | 22 | 45..=47 | 61..=63 => {}
                3 | 16 | 27 => {
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
                    r.exprs(3)?;
                }
                8..=12 | 23..=25 | 30 | 36 | 37 | 42..=44 | 48..=54 | 56 => {
                    r.exprs(params[kind as usize])?;
                }
                13 => {
                    r.exprs(4)?;
                    r.expr_array()?;
                    r.exprs(5)?;
                }
                15 => {
                    r.string()?;
                    r.u8()?;
                    r.expr()?;
                    r.u8()?;
                }
                17 => {
                    r.exprs(2)?;
                    r.string()?;
                }
                20 => {
                    let n = r.count(16 * 1024 * 1024)?;
                    let text = novel(r.bytes(n)?)?;
                    body = Body::Text {
                        text,
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
                    r.exprs(5)?;
                }
                31 | 33..=35 => {
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
                    r.exprs(7)?;
                }
                58 => {
                    r.exprs(2)?;
                }
                59 | 60 => {
                    r.exprs(params[kind as usize])?;
                    r.expr()?;
                    r.expr_array()?;
                }
                _ => unreachable!(),
            }
            Ok(Command {
                kind,
                indent,
                muted,
                line,
                offset,
                body,
            })
        })()
        .with_context(|| format!("E_IMPORT_LSB: command index {index}, byte {offset}"))?;
        commands.push(command);
    }
    r.done()?;
    Ok(Script { version, commands })
}

#[derive(Clone, Debug)]
pub(super) enum Glyph {
    Char(String),
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
    let styles = r.count(100_000)?;
    for _ in 0..styles {
        r.bytes(22)?;
        r.string()?;
        result.has_ruby |= !r.string()?.is_empty();
        r.bytes(8)?;
    }
    if version >= 104 {
        let n = r.count(100_000)?;
        for _ in 0..n {
            r.u32()?;
            r.string()?;
        }
    }
    if version >= 105 {
        let n = r.count(100_000)?;
        for _ in 0..n {
            r.u32()?;
            r.string()?;
            r.string()?;
        }
    }
    let n = r.count(1_000_000)?;
    for _ in 0..n {
        let kind = r.u8()?;
        *result.counts.entry(format!("{kind:02x}")).or_default() += 1;
        if version >= 104 {
            result.has_conditions_or_links |= r.i32()? != -1;
        }
        if matches!(kind, 1 | 9) {
            if version >= 105 {
                result.has_conditions_or_links |= r.i32()? != -1;
            } else {
                result.has_conditions_or_links |= !r.string()?.is_empty();
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
                r.i32()?;
                Glyph::Char(text)
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
                r.i32()?;
                if version > 100 {
                    r.u32()?;
                }
                if (101..105).contains(&version) {
                    r.string()?;
                }
                if version >= 105 {
                    r.i32()?;
                }
                if version < 102 {
                    r.expr()?;
                } else {
                    r.string()?;
                }
                Glyph::Unsupported
            }
            9 => {
                r.string()?;
                r.u8()?;
                if version >= 103 {
                    r.string()?;
                }
                if version >= 105 {
                    r.bytes(16)?;
                    r.string()?;
                }
                Glyph::Unsupported
            }
            _ => bail!("E_IMPORT_GLYPH: unknown glyph type {kind}"),
        };
        result.glyphs.push(glyph);
    }
    r.done()?;
    Ok(result)
}

pub(super) fn startup(data: &[u8]) -> Result<String> {
    let mut r = Reader::new(data);
    ensure!(
        r.u32()? == 116,
        "E_IMPORT_VERSION: supported LPB version is 116"
    );
    r.string()?; // title
    r.bytes(16)?;
    r.string()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
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
    pub fn dialogue(text: &str) -> Vec<u8> {
        let mut b = b"TpWord105".to_vec();
        for _ in 0..3 {
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
