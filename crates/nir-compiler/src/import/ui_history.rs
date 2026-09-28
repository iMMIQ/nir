//! Checked, source-derived history-page draft. Only offline declarations and
//! the stock callback shape are lowered; source scripts are never shipped.
use super::{
    lsb::{Body, Command, Expression, Literal, Script},
    media, read_binary,
    ui_expr::{self, Op, Term},
    ImportReport, Source,
};
use anyhow::{ensure, Context, Result};
use image::{Rgba, RgbaImage};
use nir_format::ImageMenu;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

pub(super) const ID: &str = "import.history.preview";
const SOURCE: &str = "ノベルシステム/シナリオ回想/初期化.lsb";
const GLOBALS: &str = "ノベルシステム/■初期化.lsb";
const MES: &str = "履歴用メッセージボックス";
const BAR: &str = "履歴用スクロールバー";
const PANEL: &str = "履歴用シネマ";
const BOX_W: &str = "履歴用ボックス幅";
const BOX_H: &str = "履歴用ボックス高さ";
const SIZE: &str = "履歴用フォントサイズ";
const SPACE: &str = "履歴用行間";
pub(super) const LIMITS: &str = "History draft: original state strips and tiled frame are converted automatically, with checked pixel-range callbacks and parent return. Bundled font shaping replaces the source formatter; source scenario-page gaps, formatter retention by pages, font shadow/outline, dynamic source styles, arrow hold-repeat and exact source track stretching are not yet reproduced. NIR retains bounded history records instead of the source's ten formatted pages. No original executable comparison has been performed.";

/// Called only after the checked page has been emitted. The raw UI inventory
/// remains an analysis report; only this page's conversion diagnostics change.
pub(super) fn mark_draft_diagnostics(report: &mut ImportReport) {
    for diagnostic in &mut report.diagnostics {
        if diagnostic.source == SOURCE
            && (diagnostic.message.starts_with("E_IMPORT_UI_UNMAPPED:")
                || diagnostic
                    .message
                    .starts_with("E_IMPORT_UI_HISTORY_UNMAPPED:"))
        {
            diagnostic.message = format!("E_IMPORT_UI_DRAFT_ADAPTED: {LIMITS}");
        }
    }
}

fn int(value: i32) -> Term {
    Term::Int { value }
}
fn text(value: &str) -> Term {
    Term::String {
        value: value.into(),
    }
}
fn read(name: &str) -> Term {
    Term::Read { name: name.into() }
}
fn op(op: Op, a: Term, b: Term) -> Term {
    Term::Apply {
        op,
        args: vec![a, b],
    }
}
fn prop(name: &str, n: i32) -> Term {
    op(Op::Property, text(name), int(n))
}
fn idx(name: &str, n: i32) -> Term {
    op(Op::Index, read(name), int(n))
}
fn add(a: Term, b: Term) -> Term {
    op(Op::Add, a, b)
}
fn sub(a: Term, b: Term) -> Term {
    op(Op::Subtract, a, b)
}
fn mul(a: Term, b: Term) -> Term {
    op(Op::Multiply, a, b)
}
fn half(a: Term) -> Term {
    op(Op::Divide, a, int(2))
}
fn same(e: &Expression, t: &Term) -> Result<()> {
    ensure!(
        ui_expr::normalize(e)?.as_ref() == Some(t),
        "E_IMPORT_HISTORY: altered source expression"
    );
    Ok(())
}
fn empty(e: &Expression) -> Result<()> {
    ensure!(
        ui_expr::normalize(e)?.is_none(),
        "E_IMPORT_HISTORY: expected empty source parameter"
    );
    Ok(())
}
fn set(c: &Command, name: &str, n: i32, value: Term) -> Result<()> {
    let Body::SetProperty {
        target,
        property,
        value: v,
    } = &c.body
    else {
        anyhow::bail!("E_IMPORT_HISTORY: expected property write")
    };
    same(target, &text(name))?;
    same(property, &int(n))?;
    same(v, &value)
}
fn del(c: &Command, name: &str) -> Result<()> {
    let Body::Delete(target) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected object deletion")
    };
    same(target, &text(name))
}
fn condition(c: &Command, t: Term) -> Result<()> {
    let Body::Condition(e) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected branch")
    };
    same(e, &t)
}
fn local(c: &Command, name: &str, ty: u8) -> Result<()> {
    let Body::Variable {
        name: n,
        value_type,
        scope,
        initial,
    } = &c.body
    else {
        anyhow::bail!("E_IMPORT_HISTORY: expected declaration")
    };
    ensure!(
        n == name && *value_type == ty && *scope == 2,
        "E_IMPORT_HISTORY: altered local declaration"
    );
    empty(initial)
}
fn assign(c: &Command, name: &str, value: Term) -> Result<()> {
    let Body::Calc(e) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected calculation")
    };
    ensure!(
        ui_expr::assignment(e)? == (name.into(), value),
        "E_IMPORT_HISTORY: altered assignment"
    );
    Ok(())
}
fn resize(c: &Command, name: &str, count: i32) -> Result<()> {
    let Body::Calc(e) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected array declaration")
    };
    let [call, ret] = e.operations.as_slice() else {
        anyhow::bail!("E_IMPORT_HISTORY: array operation shape")
    };
    ensure!(
        e.functions.len() == 1
            && e.functions.get(&0) == Some(&21)
            && call.0 == 11
            && ui_expr::temporary(&call.1)
            && call.1 != "____arg"
            && matches!(call.2.as_slice(),[Literal::Variable(v),Literal::Int(n)] if v==name&&*n==count)
            && ret.0 == 1
            && ret.1 == "____arg"
            && matches!(ret.2.as_slice(),[Literal::Variable(v)] if v==&call.1),
        "E_IMPORT_HISTORY: unsupported array allocation"
    );
    Ok(())
}
fn slot(c: &Command, name: &str, index: i32) -> Result<Literal> {
    let Body::Calc(e) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected array assignment")
    };
    ensure!(
        e.functions.is_empty(),
        "E_IMPORT_HISTORY: array call side effects"
    );
    let (prefix, tail) = e.operations.split_at(e.operations.len().saturating_sub(2));
    let [at, write] = tail else {
        anyhow::bail!("E_IMPORT_HISTORY: array write shape")
    };
    ensure!(
        at.0 == 10
            && at.1.starts_with("____d_")
            && ui_expr::temporary(&at.1)
            && matches!(at.2.as_slice(),[Literal::Variable(v),Literal::Int(n)] if v==name&&*n==index)
            && write.0 == 1
            && write.1 == at.1,
        "E_IMPORT_HISTORY: altered array destination"
    );
    let value = match (prefix, write.2.as_slice()) {
        ([], [v @ (Literal::Int(_) | Literal::String(_))]) => v.clone(),
        ([value], [Literal::Variable(v)])
            if value.0 == 1 && ui_expr::temporary(&value.1) && value.1 != at.1 && v == &value.1 =>
        {
            let [v @ Literal::String(_)] = value.2.as_slice() else {
                anyhow::bail!("E_IMPORT_HISTORY: array value type")
            };
            v.clone()
        }
        _ => anyhow::bail!("E_IMPORT_HISTORY: unsupported array write"),
    };
    Ok(value)
}
fn history(c: &Command, index: Term) -> Result<()> {
    let Body::HistoryCall { parameters: p } = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected history call")
    };
    ensure!(p.len() == 5, "E_IMPORT_HISTORY: history parameter count");
    for (name, t) in [
        ("target", text(MES)),
        ("index", index),
        ("count", prop(MES, 6)),
        ("cut_break", int(0)),
    ] {
        same(
            p.get(name).context("E_IMPORT_HISTORY: history parameter")?,
            &t,
        )?;
    }
    empty(
        p.get("format_name")
            .context("E_IMPORT_HISTORY: formatter parameter")?,
    )
}

// Every nonempty property must have an explicit meaning or match the stock
// default. Unknown callbacks, alpha, offsets, transforms and source expressions
// fail instead of disappearing from a generated page.
fn object(c: &Command, expected: Vec<(u16, Term)>) -> Result<()> {
    let Body::Object(p) = &c.body else {
        anyhow::bail!("E_IMPORT_HISTORY: expected UI declaration")
    };
    let mut expected: BTreeMap<_, _> = expected.into_iter().collect();
    let defaults: &[(u16, i32)] = match c.kind {
        9 => &[
            (28, 0),
            (30, 0),
            (48, 1),
            (72, 0),
            (81, 1),
            (89, 0),
            (101, 0),
            (112, 0),
            (113, 0),
            (135, 0),
            (148, 0),
        ],
        50 => &[
            (28, 0),
            (48, 1),
            (81, 1),
            (112, 0),
            (113, 0),
            (135, 0),
            (148, 0),
        ],
        8 => &[
            (28, 0),
            (30, 0),
            (48, 1),
            (72, 0),
            (81, 1),
            (135, 0),
            (148, 0),
        ],
        10 => &[
            (21, 16753828),
            (28, 0),
            (48, 1),
            (81, 1),
            (82, 0),
            (83, -1),
            (148, 0),
            (153, 0),
        ],
        52 => &[(28, 0), (48, 1), (81, 0), (89, 0), (148, 1)],
        _ => anyhow::bail!("E_IMPORT_HISTORY: unsupported UI declaration"),
    };
    for &(key, n) in defaults {
        expected.entry(key).or_insert_with(|| int(n));
    }
    for (key, e) in p {
        if let Some(actual) = ui_expr::normalize(e)? {
            ensure!(
                expected.remove(key).as_ref() == Some(&actual),
                "E_IMPORT_HISTORY: unexpected property {key} on {}",
                c.name()
            );
        }
    }
    // Named expected properties are mandatory. Defaults may be omitted.
    ensure!(
        expected
            .keys()
            .all(|key| defaults.iter().any(|(k, _)| k == key)),
        "E_IMPORT_HISTORY: missing required property {:?}",
        expected.keys().collect::<Vec<_>>()
    );
    Ok(())
}
fn image_object(name: &str, index: i32, x: Term, y: Term) -> Vec<(u16, Term)> {
    vec![
        (1, text(name)),
        (2, text(PANEL)),
        (3, idx("s", index)),
        (4, x),
        (5, y),
    ]
}
fn mes_properties(register: bool) -> Vec<(u16, Term)> {
    let mut p = vec![
        (1, text(MES)),
        (6, sub(read(BOX_W), int(16))),
        (7, read(BOX_H)),
        (
            16,
            if register {
                prop("メッセージボックス", 15)
            } else {
                read("StatusFontName")
            },
        ),
        (17, read(SIZE)),
        (19, read(SPACE)),
        (20, read("履歴用フォント色")),
        (22, read("履歴用フォント縁色")),
        (26, op(Op::Property, read("メッセージボックス"), int(25))),
        (142, read("履歴用フォント影色")),
        (143, read("履歴用フォント縁")),
        (144, read("履歴用フォント影")),
    ];
    if register {
        p.retain(|(key, _)| *key != 26);
        p.push((26, prop("メッセージボックス", 25)));
        p.push((148, int(0)));
    } else {
        p.extend([(2, text("履歴用背景")), (4, int(8)), (5, int(0))]);
    }
    p
}
fn check_page(s: &Script) -> Result<[String; 11]> {
    ensure!(
        s.version == 116 && s.commands.len() == 81,
        "E_IMPORT_HISTORY: unsupported history script shape"
    );
    let kinds = [
        18, 15, 15, 15, 8, 15, 14, 0, 14, 14, 14, 14, 14, 14, 14, 14, 14, 2, 14, 14, 14, 14, 14,
        14, 14, 14, 14, 15, 14, 0, 14, 14, 14, 2, 14, 14, 14, 9, 9, 52, 19, 19, 9, 50, 9, 50, 50,
        9, 50, 9, 0, 50, 2, 8, 10, 18, 0, 14, 2, 14, 18, 18, 18, 29, 6, 3, 29, 6, 3, 0, 19, 18, 18,
        6, 3, 0, 18, 29, 1, 18, 29,
    ];
    for (i, (c, kind)) in s.commands.iter().zip(kinds).enumerate() {
        let nested = matches!(i,8..=16|18..=26|30..=32|34..=36|51|53|57|59|70..=72|76..=77|79..=80);
        ensure!(
            c.kind == kind
                && c.indent == u32::from(nested)
                && !c.muted
                && c.not_update == (i < 64 || matches!(i, 65 | 68 | 74)),
            "E_IMPORT_HISTORY: altered command flow at {i}"
        );
    }
    let c = &s.commands;
    set(&c[0], "システムメニュー", 47, int(0))?;
    for (i, name, value) in [
        (
            1,
            "dx",
            half(sub(
                read("@ScrWidth"),
                add(add(add(int(4), read(BOX_W)), int(16)), int(4)),
            )),
        ),
        (
            2,
            "dy",
            half(sub(
                read("@ScrHeight"),
                add(add(int(4), read(BOX_H)), int(4)),
            )),
        ),
    ] {
        let Body::Variable {
            name: n,
            value_type,
            scope,
            initial,
        } = &c[i].body
        else {
            unreachable!()
        };
        ensure!(
            n == name && *value_type == 1 && *scope == 2,
            "E_IMPORT_HISTORY: altered center local"
        );
        same(initial, &value)?;
    }
    local(&c[3], "__y", 1)?;
    object(
        &c[4],
        vec![(1, text(PANEL)), (2, text("メニュー背景")), (9, int(-1))],
    )?;
    local(&c[5], "s", 4)?;
    resize(&c[6], "s", 9)?;
    condition(&c[7], int(0))?;
    local(&c[27], "sc", 4)?;
    resize(&c[28], "sc", 3)?;
    condition(&c[29], int(0))?;
    let mut paths = vec![];
    for i in 0..9 {
        ensure!(
            matches!(slot(&c[8+i],"s",i as i32)?,Literal::String(v) if v.is_empty()),
            "E_IMPORT_HISTORY: altered inactive frame branch"
        );
        let value = slot(&c[18 + i], "s", i as i32)?;
        if i == 4 {
            ensure!(
                matches!(value, Literal::Int(16744576)),
                "E_IMPORT_HISTORY: unknown background color"
            );
        } else {
            let Literal::String(value) = value else {
                anyhow::bail!("E_IMPORT_HISTORY: nonliteral frame source")
            };
            paths.push(value);
        }
    }
    for i in 0..3 {
        ensure!(
            matches!(slot(&c[30+i],"sc",i as i32)?,Literal::String(v) if v.is_empty()),
            "E_IMPORT_HISTORY: altered inactive skin branch"
        );
        let Literal::String(value) = slot(&c[34 + i], "sc", i as i32)? else {
            anyhow::bail!("E_IMPORT_HISTORY: nonliteral skin source")
        };
        paths.push(value);
    }
    for (i, name, array, index) in [(37, "tmp", "s", 0), (38, "tmp2", "sc", 2)] {
        object(
            &c[i],
            vec![(1, text(name)), (2, text("\u{1}")), (3, idx(array, index))],
        )?;
    }
    let maximum = sub(read("@HistoryCount"), read(BOX_H));
    object(
        &c[39],
        vec![
            (1, text(BAR)),
            (2, text(PANEL)),
            (3, idx("sc", 0)),
            (4, add(prop("tmp", 5), read(BOX_W))),
            (5, prop("tmp", 6)),
            (
                34,
                text("ノベルシステム\\シナリオ回想\\初期化.lsc:履歴呼び出し"),
            ),
            (
                114,
                text("ノベルシステム\\シナリオ回想\\初期化.lsc:アイドル時"),
            ),
            (122, idx("sc", 1)),
            (123, idx("sc", 2)),
            (124, int(0)),
            (125, maximum.clone()),
            (126, maximum),
            (127, int(1)),
            (128, add(read(SIZE), read(SPACE))),
            (129, half(read(BOX_H))),
            (
                145,
                text("ノベルシステム\\シナリオ回想\\初期化.lsc:キーダウン"),
            ),
            (158, sub(read(BOX_H), mul(prop("tmp2", 6), int(2)))),
        ],
    )?;
    del(&c[40], "tmp")?;
    del(&c[41], "tmp2")?;
    let left = prop("履歴用枠左上", 5);
    let top = prop("履歴用枠左上", 6);
    let width = add(read(BOX_W), prop(BAR, 5));
    let right = add(add(left.clone(), read(BOX_W)), prop(BAR, 5));
    let bottom = add(top.clone(), read(BOX_H));
    object(&c[42], image_object("履歴用枠左上", 0, int(0), int(0)))?;
    let mut p = image_object("履歴用枠上", 1, left.clone(), int(0));
    p.extend([(6, width.clone()), (7, top.clone())]);
    object(&c[43], p)?;
    object(
        &c[44],
        image_object("履歴用枠右上", 2, right.clone(), int(0)),
    )?;
    let mut p = image_object("履歴用枠左", 3, int(0), top.clone());
    p.extend([(6, left.clone()), (7, read(BOX_H))]);
    object(&c[45], p)?;
    let mut p = image_object("履歴用枠右", 5, right.clone(), top.clone());
    p.extend([(6, prop("履歴用枠右上", 5)), (7, read(BOX_H))]);
    object(&c[46], p)?;
    object(
        &c[47],
        image_object("履歴用枠左下", 6, int(0), bottom.clone()),
    )?;
    let mut p = image_object("履歴用枠下", 7, prop("履歴用枠左下", 5), bottom.clone());
    p.extend([(6, width.clone()), (7, prop("履歴用枠左下", 6))]);
    object(&c[48], p)?;
    object(
        &c[49],
        image_object(
            "履歴用枠右下",
            8,
            add(add(prop("履歴用枠左下", 5), read(BOX_W)), prop(BAR, 5)),
            bottom,
        ),
    )?;
    condition(&c[50], int(0))?;
    let mut p = image_object("履歴用背景", 4, left.clone(), top.clone());
    p.extend([(6, width.clone()), (7, read(BOX_H)), (13, int(-1))]);
    object(&c[51], p)?;
    object(
        &c[53],
        vec![
            (1, text("履歴用背景")),
            (2, text(PANEL)),
            (4, left.clone()),
            (5, top.clone()),
            (6, width.clone()),
            (7, read(BOX_H)),
            (9, idx("s", 4)),
            (13, int(-1)),
        ],
    )?;
    object(&c[54], mes_properties(false))?;
    let full_width = add(
        add(add(left, read(BOX_W)), prop(BAR, 5)),
        prop("履歴用枠右上", 5),
    );
    let full_height = add(add(top, read(BOX_H)), prop("履歴用枠左下", 6));
    set(
        &c[55],
        PANEL,
        3,
        half(sub(read("@ScrWidth"), full_width.clone())),
    )?;
    condition(&c[56], int(1))?;
    assign(&c[57], "__y", int(16))?;
    assign(&c[59], "__y", int(0))?;
    set(
        &c[60],
        PANEL,
        4,
        add(
            read("__y"),
            half(sub(
                sub(read("@ScrHeight"), read("__y")),
                full_height.clone(),
            )),
        ),
    )?;
    set(&c[61], PANEL, 5, full_width)?;
    set(&c[62], PANEL, 6, full_height)?;
    history(&c[63], sub(read("@HistoryCount"), prop(MES, 6)))?;
    for i in [64, 67, 73] {
        let Body::Exit(e) = &c[i].body else {
            unreachable!()
        };
        same(e, &int(1))?;
    }
    for (i, name) in [(65, "履歴呼び出し"), (68, "キーダウン"), (74, "アイドル時")]
    {
        ensure!(
            matches!(&c[i].body,Body::Label(v) if v==name),
            "E_IMPORT_HISTORY: altered callback label"
        );
    }
    history(&c[66], idx("@ParamStr", 0))?;
    condition(&c[69], idx("@KeyClick", 27))?;
    del(&c[70], PANEL)?;
    set(&c[71], "システムメニュー", 47, int(1))?;
    set(&c[72], "タイトルラベル", 50, text("システム画面"))?;
    condition(&c[75], read("@WheelUp"))?;
    condition(&c[78], read("@WheelDown"))?;
    set(
        &c[76],
        BAR,
        125,
        sub(prop(BAR, 125), mul(read(SIZE), int(3))),
    )?;
    set(
        &c[79],
        BAR,
        125,
        add(prop(BAR, 125), mul(read(SIZE), int(3))),
    )?;
    history(&c[77], prop(BAR, 125))?;
    history(&c[80], prop(BAR, 125))?;
    Ok(paths.try_into().unwrap())
}

pub(super) struct HistoryPreview {
    pub menu: ImageMenu,
    assets: BTreeMap<String, RgbaImage>,
    pub evidence: Value,
}
impl HistoryPreview {
    pub fn load(source: &mut Source) -> Result<Self> {
        let (_, s) = source.read(SOURCE)?;
        let paths = check_page(&s)?;
        let (_, globals) = source.read(GLOBALS)?;
        check_globals(&globals)?;
        let (_, message) = source.read("メッセージボックス作成.lsb")?;
        let (size, spacing) = message_style(&message)?;
        let mut rasters = vec![];
        let mut inputs = vec![];
        for path in paths {
            let data = read_binary(&source.path(&path)?)?;
            rasters.push(media::gal(&data)?);
            inputs
                .push(json!({"source":path.replace('\\',"/"),"sha256":nir_content::digest(&data)}));
        }
        let frames: &[RgbaImage] = &rasters[..8];
        // The checked source expressions use actual corner dimensions. Reject
        // asymmetric/inconsistent tiles instead of stretching them to fit.
        let left = frames[0].width();
        let top = frames[0].height();
        let right = frames[2].width();
        let bottom = frames[5].height();
        ensure!(
            frames[2].height() == top
                && frames[5].width() == left
                && frames[7].dimensions() == (right, bottom)
                && frames[1].height() == top
                && frames[3].width() == left
                && frames[4].width() == right
                && frames[6].height() == bottom,
            "E_IMPORT_HISTORY: inconsistent frame geometry"
        );
        let tracks = media::horizontal_states(&rasters[8], 2)?;
        let thumbs = media::horizontal_states(&rasters[9], 3)?;
        let arrows = media::horizontal_states(&rasters[10], 6)?;
        let bar_w = tracks[0].width();
        let arrow_h = arrows[0].height();
        let thumb_h = thumbs[0].height();
        ensure!(
            bar_w == thumbs[0].width() && bar_w == arrows[0].width(),
            "E_IMPORT_HISTORY: inconsistent scrollbar part widths"
        );
        let bw = 1024 - 56;
        let bh = 768 - 46;
        let width = left + bw + bar_w + right;
        let height = top + bh + bottom;
        ensure!(
            width <= 1024 && height <= 752 && bh > 2 * arrow_h + thumb_h,
            "E_IMPORT_HISTORY: history page geometry exceeds stage"
        );
        let x = (1024 - width) as f32 / 2.;
        let y = 16. + (752 - height) as f32 / 2.;
        let mut panel = RgbaImage::new(width, height);
        for py in top..top + bh {
            for px in left..left + bw + bar_w {
                panel.put_pixel(px, py, Rgba([128, 128, 255, 255]));
            }
        }
        let placements = [
            (0, 0, 0, left, top),
            (1, left, 0, bw + bar_w, top),
            (2, left + bw + bar_w, 0, right, top),
            (3, 0, top, left, bh),
            (4, left + bw + bar_w, top, right, bh),
            (5, 0, top + bh, left, bottom),
            (6, left, top + bh, bw + bar_w, bottom),
            (7, left + bw + bar_w, top + bh, right, bottom),
        ];
        for (i, px, py, w, h) in placements {
            let image = if matches!(i, 1 | 3 | 4 | 6) {
                media::tile(&frames[i], w, h)?
            } else {
                frames[i].clone()
            };
            image::imageops::replace(&mut panel, &image, px as i64, py as i64);
        }
        let mut assets = BTreeMap::from([("import.history.frame".into(), panel)]);
        let mut add_states = |name: &str, images: Vec<RgbaImage>| {
            images
                .into_iter()
                .enumerate()
                .map(|(i, image)| {
                    let id = format!("import.history.{name}.{i}");
                    assets.insert(id.clone(), image);
                    id
                })
                .collect::<Vec<_>>()
        };
        let tracks = add_states("track", tracks);
        let thumbs = add_states("thumb", thumbs);
        let arrows = add_states("arrow", arrows);
        let states = |ids: &[String]| json!({"asset":ids[0],"hover_asset":ids[1],"pressed_asset":ids[2],"disabled_asset":ids[0]});
        let menu = serde_json::from_value(
            json!({"background":"import.preview.backdrop","builtin_navigation":false,"buttons":[],"elements":[
                {"id":"source.history.caption","rect":[20,0,980,24],"content":{"type":"text","text":"システム画面　＞　シナリオ回想","size":16,"color":[1,1,1,1]}},
                {"id":"source.history.frame","rect":[x,y,width,height],"content":{"type":"image","asset":"import.history.frame"}},
                {"id":"source.history.records","rect":[x+left as f32+8.,y+top as f32,bw-16,bh],"content":{"type":"history_flow","size":size,"line_height":size+spacing,"gap":0,"wheel_step":size*3,"page_step":bh/2,"max_visible":32,"color":[1,1,1,1]}},
                {"id":"source.history.scroll","rect":[x+(left+bw) as f32,y+top as f32,bar_w,bh],"content":{"type":"history_scrollbar","window":"source.history.records","label":"履歴スクロール","thumb_height":thumb_h,"arrow_height":arrow_h,"line_step":size+spacing,
                    "track":{"asset":tracks[0],"pressed_asset":tracks[1],"disabled_asset":tracks[0]},"thumb":states(&thumbs),"decrease":states(&arrows[..3]),"increase":states(&arrows[3..])}}
            ]}),
        )?;
        let evidence = json!({"format":1,"status":"adapted_incomplete_draft","menu":ID,"sources":[
            {"source":SOURCE,"sha256":s.source_sha256,"version":s.version},
            {"source":GLOBALS,"sha256":globals.source_sha256,"version":globals.version},
            {"source":"メッセージボックス作成.lsb","sha256":message.source_sha256,"version":message.version}],
            "assets":inputs,"state_counts":{"track":2,"thumb":3,"arrows":6},
            "units":"pixels after checked FormatHist registration; NIR recomputes layout using its font plan",
            "frame_rect":[x,y,width,height],"source_track_height":bh-2*arrow_h,"source_total_height":bh,
            "source_font_size":size,"source_line_spacing":spacing,"limitations":LIMITS,
            "documentation":{"repository_commit":"ceeb12911c571fe6da77107bb0eaf7eb29594612",
                "skin_order":"https://github.com/pmrowla/pylivemaker/blob/ceeb12911c571fe6da77107bb0eaf7eb29594612/docs/_static/LiveNovel/option2.html",
                "history_units":"https://github.com/pmrowla/pylivemaker/blob/ceeb12911c571fe6da77107bb0eaf7eb29594612/docs/_static/LiveNovel/calc.html"}});
        Ok(Self {
            menu,
            assets,
            evidence,
        })
    }
    pub fn install(&self, root: &Path) -> Result<()> {
        let path = root.join("assets/catalog.toml");
        let mut catalog: Value = toml::from_str(&fs::read_to_string(&path)?)?;
        let entries = catalog["assets"]
            .as_array_mut()
            .context("E_IMPORT_CATALOG")?;
        let mut total = 0usize;
        for (id, image) in &self.assets {
            let bytes = media::png(image)?;
            total += bytes.len();
            ensure!(
                total <= 16 * 1024 * 1024,
                "E_IMPORT_HISTORY: generated history assets exceed budget"
            );
            let output = format!("imported/{id}.png");
            fs::write(root.join("assets").join(&output), bytes)?;
            entries.push(json!({"id":id,"kind":"image","source":output,"rights":"Imported source game asset; original rights retained.","expected_size":[image.width(),image.height()]}));
        }
        fs::write(path, toml::to_string_pretty(&catalog)?)?;
        super::write_json(&root.join("import-history-preview.json"), &self.evidence)
    }
}

fn check_globals(s: &Script) -> Result<()> {
    ensure!(
        s.version == 116,
        "E_IMPORT_HISTORY: unsupported initialization version"
    );
    let expected = [
        (BOX_W, sub(read("@ScrWidth"), int(56))),
        (BOX_H, sub(read("@ScrHeight"), int(46))),
        (SIZE, prop("メッセージボックス", 16)),
        (SPACE, prop("メッセージボックス", 18)),
        ("履歴用フォント色", int(16777215)),
        ("履歴用フォント影", int(1)),
        ("履歴用フォント影色", int(0)),
        ("履歴用フォント縁", int(0)),
        ("履歴用フォント縁色", int(0)),
    ];
    let starts: Vec<_> = s
        .commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| matches!(&c.body,Body::Variable{name,..} if name==BOX_W).then_some(i))
        .collect();
    ensure!(
        starts.len() == 1,
        "E_IMPORT_HISTORY: ambiguous history initialization"
    );
    let part = s
        .commands
        .get(starts[0]..starts[0] + 23)
        .context("E_IMPORT_HISTORY: incomplete formatter initialization")?;
    for (c, (name, value)) in part.iter().take(9).zip(expected) {
        let Body::Variable {
            name: n,
            value_type,
            scope,
            initial,
        } = &c.body
        else {
            anyhow::bail!("E_IMPORT_HISTORY: expected history setting")
        };
        ensure!(
            c.indent == 0
                && !c.muted
                && c.not_update
                && n == name
                && *value_type == 1
                && *scope == 0,
            "E_IMPORT_HISTORY: history setting flow"
        );
        same(initial, &value)?;
    }
    condition(&part[9], int(0))?;
    ensure!(
        part[9].indent == 0 && !part[9].muted && part[9].not_update,
        "E_IMPORT_HISTORY: conditional formatter variant"
    );
    for c in &part[10..19] {
        ensure!(
            c.indent == 1 && !c.muted && c.not_update && c.kind == 14,
            "E_IMPORT_HISTORY: unexpected inactive override"
        );
    }
    for (c, kind) in part[19..23].iter().zip([10, 58, 19, 14]) {
        ensure!(
            c.kind == kind && c.indent == 0 && !c.muted && c.not_update,
            "E_IMPORT_HISTORY: formatter flow"
        );
    }
    object(&part[19], mes_properties(true))?;
    let Body::HistoryFormat { name, target } = &part[20].body else {
        anyhow::bail!("E_IMPORT_HISTORY: missing formatter registration")
    };
    same(name, &text(MES))?;
    empty(target)?;
    del(&part[21], MES)?;
    assign(&part[22], "@HistoryMaxCount", int(10))?;
    ensure!(
        s.commands
            .iter()
            .filter(|c| matches!(c.body, Body::HistoryFormat { .. }))
            .count()
            == 1,
        "E_IMPORT_HISTORY: ambiguous formatter registration"
    );
    Ok(())
}
fn message_style(s: &Script) -> Result<(i32, i32)> {
    ensure!(
        s.version == 116,
        "E_IMPORT_HISTORY: unsupported message style version"
    );
    let mut styles = vec![];
    for c in &s.commands {
        if c.kind != 10 || c.muted {
            continue;
        }
        let Body::Object(p) = &c.body else {
            continue;
        };
        if p.get(&1).map(ui_expr::normalize).transpose()?.flatten()
            != Some(text("メッセージボックス"))
        {
            continue;
        }
        let number = |key| -> Result<i32> {
            let Some(Term::Int { value }) = ui_expr::normalize(
                p.get(&key)
                    .context("E_IMPORT_HISTORY: missing message style")?,
            )?
            else {
                anyhow::bail!("E_IMPORT_HISTORY: dynamic message style")
            };
            Ok(value)
        };
        styles.push((number(17)?, number(19)?));
    }
    ensure!(
        !styles.is_empty() && styles.iter().all(|v| v == &styles[0]),
        "E_IMPORT_HISTORY: inconsistent message styles"
    );
    let (size, space) = styles[0];
    ensure!(
        (16..=128).contains(&size) && (0..=128).contains(&space),
        "E_IMPORT_HISTORY: unsupported history density"
    );
    Ok((size, space))
}

pub(super) fn verify_selection(s: &Script) -> Result<()> {
    let expected = op(Op::Equal, read("val"), text("シナリオ回想"));
    let candidates: Vec<_> = s
        .commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.body {
            Body::Condition(e)
                if ui_expr::normalize(e).ok().flatten() == Some(expected.clone()) =>
            {
                Some(i)
            }
            _ => None,
        })
        .collect();
    ensure!(
        candidates.len() == 1,
        "E_IMPORT_HISTORY: ambiguous selection callback"
    );
    let index = candidates[0];
    let part = s
        .commands
        .get(index..index + 4)
        .context("E_IMPORT_HISTORY: incomplete selection branch")?;
    for (c, (kind, indent)) in part.iter().zip([(1, 0), (5, 1), (18, 1), (1, 0)]) {
        ensure!(
            c.kind == kind && c.indent == indent && !c.muted && !c.not_update,
            "E_IMPORT_HISTORY: altered selection flow"
        );
    }
    let Body::Call {
        target,
        condition,
        has_params,
        params,
    } = &part[1].body
    else {
        unreachable!()
    };
    ensure!(
        target.page.replace('\\', "/") == SOURCE
            && target.line == 0
            && !*has_params
            && params.is_empty(),
        "E_IMPORT_HISTORY: unknown history entry"
    );
    same(condition, &int(1))?;
    set(
        &part[2],
        "タイトルラベル",
        50,
        text("システム画面　＞　シナリオ回想"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn source_state_strips_and_tiled_alpha_edges_keep_original_pixels() {
        let strip = RgbaImage::from_fn(6, 2, |x, y| Rgba([x as u8, y as u8, 20, 100 + x as u8]));
        let states = media::horizontal_states(&strip, 3).unwrap();
        assert_eq!(states.len(), 3);
        for (i, state) in states.iter().enumerate() {
            assert_eq!(state.dimensions(), (2, 2));
            for y in 0..2 {
                for x in 0..2 {
                    assert_eq!(state.get_pixel(x, y), strip.get_pixel(i as u32 * 2 + x, y));
                }
            }
        }
        assert!(media::horizontal_states(&strip, 4).is_err());
        assert!(media::horizontal_states(&strip, 0).is_err());
        let tiled = media::tile(&states[1], 5, 3).unwrap();
        assert_eq!(tiled.dimensions(), (5, 3));
        for y in 0..3 {
            for x in 0..5 {
                assert_eq!(tiled.get_pixel(x, y), states[1].get_pixel(x % 2, y % 2));
            }
        }
        assert!(media::tile(&states[0], 8193, 1).is_err());
        assert!(media::tile(&states[0], 8192, 8192).is_err());
    }

    #[test]
    #[ignore = "requires NIR_IMPORT_SOURCE"]
    fn source_history_page_contract() {
        let path = std::path::PathBuf::from(
            std::env::var_os("NIR_IMPORT_SOURCE").expect("NIR_IMPORT_SOURCE"),
        );
        let mut source = Source::new(&path).unwrap();
        let preview = HistoryPreview::load(&mut source).unwrap();
        assert_eq!(preview.evidence["frame_rect"], json!([16., 27., 992, 730]));
        assert_eq!(preview.evidence["source_track_height"], 690);
        assert_eq!(preview.assets.len(), 12);
        assert_eq!(
            preview.assets["import.history.thumb.0"].dimensions(),
            (16, 16)
        );
        let (_, selection) = source
            .read("ノベルシステム/システムメニュー/選択時.lsb")
            .unwrap();
        verify_selection(&selection).unwrap();
        let (_, page) = source.read(SOURCE).unwrap();
        for case in 0..8 {
            let mut changed = (*page).clone();
            match case {
                0 => changed.commands[69].muted = true,
                1 => {
                    changed.commands[70].body = Body::Delete(Expression {
                        literal: None,
                        operations: vec![(
                            1,
                            "____arg".into(),
                            vec![Literal::String("another".into())],
                        )],
                        functions: BTreeMap::new(),
                    })
                }
                2 => changed.commands[65].body = Body::Label("unknown".into()),
                3 => {
                    if let Body::Object(p) = &mut changed.commands[39].body {
                        p.get_mut(&158).unwrap().operations[0].2[0] = Literal::Int(9);
                    }
                }
                4 => changed.commands.insert(73, changed.commands[72].clone()),
                5 => changed.commands[39].indent = 1,
                6 => {
                    if let Body::Calc(e) = &mut changed.commands[34].body {
                        e.operations[1].2[1] = Literal::Int(1);
                    }
                }
                _ => {
                    if let Body::HistoryCall { parameters } = &mut changed.commands[63].body {
                        parameters.get_mut("cut_break").unwrap().operations[0].2[0] =
                            Literal::Int(1);
                    }
                }
            }
            assert!(check_page(&changed).is_err(), "mutation {case}");
        }
        let (_, globals) = source.read(GLOBALS).unwrap();
        let mut muted_limit = (*globals).clone();
        let limit = muted_limit.commands.iter_mut().find(|c|
            matches!(&c.body,Body::Calc(e) if ui_expr::assignment(e).ok().is_some_and(|(name,_)|name=="@HistoryMaxCount"))).unwrap();
        limit.muted = true;
        assert!(check_globals(&muted_limit).is_err());
        let mut changed = (*globals).clone();
        if let Body::HistoryFormat { target, .. } = &mut changed.commands[70].body {
            target.operations = vec![(1, "____arg".into(), vec![Literal::Int(0)])];
        }
        source.scripts.insert(GLOBALS.into(), Arc::new(changed));
        assert!(HistoryPreview::load(&mut source).is_err());
    }
}
