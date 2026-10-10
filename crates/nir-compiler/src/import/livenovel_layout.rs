//! Source-side static geometry. Symbolic reads are confined to known stage
//! dimensions and the current message-window dimensions, never game state.
use super::super::ui_expr::{normalize, Op, Term};
use super::*;

fn coordinate(term: &Term, stage: [u32; 2], size: [u32; 2]) -> Result<f32> {
    let value = match term {
        Term::Int { value } => *value as f32,
        Term::Read { name } if name == "@ScrWidth" => stage[0] as f32,
        Term::Read { name } if name == "@ScrHeight" => stage[1] as f32,
        Term::Apply {
            op: Op::Property,
            args,
        } => {
            ensure!(
                matches!(args.first(), Some(Term::String { value }) if value == "メッセージボックス土台"),
                "E_IMPORT_TEXT_LAYOUT: unknown geometry object"
            );
            match args.get(1) {
                Some(Term::Int { value: 5 }) => size[0] as f32,
                Some(Term::Int { value: 6 }) => size[1] as f32,
                _ => bail!("E_IMPORT_TEXT_LAYOUT: unknown geometry property"),
            }
        }
        Term::Apply { op, args } if args.len() == 2 => {
            let a = coordinate(&args[0], stage, size)?;
            let b = coordinate(&args[1], stage, size)?;
            match op {
                Op::Add => a + b,
                Op::Subtract => a - b,
                Op::Multiply => a * b,
                Op::Divide if b != 0. => a / b,
                _ => bail!("E_IMPORT_TEXT_LAYOUT: unsupported geometry expression"),
            }
        }
        _ => bail!("E_IMPORT_TEXT_LAYOUT: dynamic geometry"),
    };
    ensure!(
        value.is_finite() && value.abs() <= 8192.,
        "E_IMPORT_TEXT_LAYOUT: geometry exceeds bounds"
    );
    Ok(value)
}

pub(super) fn theme(adapter: &mut Adapter, script: &Script) -> Result<Value> {
    theme_named(adapter, script, "(標準)")
}

fn branch_name(expression: &Expression) -> Result<Option<String>> {
    Ok(match normalize(expression)? {
        Some(Term::Apply {
            op: Op::Equal,
            args,
        }) if args.len() == 2
            && args[0]
                == (Term::Apply {
                    op: Op::Index,
                    args: vec![
                        Term::Read {
                            name: "@ParamStr".into(),
                        },
                        Term::Int { value: 0 },
                    ],
                }) =>
        {
            match &args[1] {
                Term::String { value } => Some(value.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

fn theme_named(adapter: &mut Adapter, script: &Script, name: &str) -> Result<Value> {
    let mut selected = false;
    let mut index = None;
    for (i, command) in script.commands.iter().enumerate().filter(|(_, c)| !c.muted) {
        if command.indent == 0 {
            selected = match &command.body {
                Body::Condition(e) => branch_name(e)?.as_deref() == Some(name),
                _ => false,
            };
        }
        if selected
            && command.indent == 1
            && matches!(command.kind, 8 | 9)
            && matches!(&command.body, Body::Object(fields) if fields.get(&1).and_then(|e| e.literal.as_ref()).is_some_and(|v| matches!(v, Literal::String(value) if value == "メッセージボックス土台")))
        {
            index = Some(i);
            break;
        }
    }
    let index = index.context("E_IMPORT_TEXT_LAYOUT: missing named window")?;
    let window = &script.commands[index];
    let branch = script.commands[..index]
        .iter()
        .rev()
        .find(|c| !c.muted && c.indent == 0)
        .context("E_IMPORT_TEXT_LAYOUT: missing standard branch")?;
    let Body::Condition(e) = &branch.body else {
        bail!("E_IMPORT_TEXT_LAYOUT: missing standard condition");
    };
    ensure!(
        normalize(e)?
            == Some(Term::Apply {
                op: Op::Equal,
                args: vec![
                    Term::Apply {
                        op: Op::Index,
                        args: vec![
                            Term::Read {
                                name: "@ParamStr".into()
                            },
                            Term::Int { value: 0 }
                        ]
                    },
                    Term::String { value: name.into() },
                ]
            }),
        "E_IMPORT_TEXT_LAYOUT: unsupported standard branch"
    );
    ensure!(window.indent == 1, "E_IMPORT_TEXT_LAYOUT: window scope");
    let Body::Object(fields) = &window.body else {
        unreachable!()
    };
    let field = |key| {
        fields
            .get(&key)
            .context("E_IMPORT_TEXT_LAYOUT: missing window property")
    };
    let opacity = literal_int(field(12)?)?;
    ensure!(
        (0..=255).contains(&opacity),
        "E_IMPORT_TEXT_LAYOUT: opacity"
    );
    let (background, size, opacity) = if window.kind == 9 {
        let (id, size) = adapter.image(literal_string(field(3)?)?)?;
        adapter.textbox = id.clone();
        (Some(id), size, opacity as f32 / 255.)
    } else {
        ensure!(
            coordinate(
                &normalize(field(9)?)?.context("E_IMPORT_TEXT_LAYOUT: box color")?,
                adapter.stage,
                [1, 1]
            )? == -1.,
            "E_IMPORT_TEXT_LAYOUT: nontransparent box needs color adaptation"
        );
        let w = literal_int(field(6)?)?;
        let h = literal_int(field(7)?)?;
        ensure!(
            w > 0 && h > 0 && w <= 8192 && h <= 8192,
            "E_IMPORT_TEXT_LAYOUT: box dimensions"
        );
        (None, [w as u32, h as u32], 0.)
    };
    let evaluate = |e: &Expression| -> Result<f32> {
        coordinate(
            &normalize(e)?.context("E_IMPORT_TEXT_LAYOUT: empty geometry")?,
            adapter.stage,
            size,
        )
    };
    let x = evaluate(field(4)?)?;
    let y = evaluate(field(5)?)?;
    let child = script
        .commands
        .get(index + 1)
        .context("E_IMPORT_TEXT_LAYOUT: missing text object")?;
    let Body::Object(text) = &child.body else {
        bail!("E_IMPORT_TEXT_LAYOUT: missing text object")
    };
    let field = |key| {
        text.get(&key)
            .context("E_IMPORT_TEXT_LAYOUT: missing text property")
    };
    ensure!(
        child.kind == 10
            && child.indent == 1
            && literal_string(field(2)?)? == "メッセージボックス土台",
        "E_IMPORT_TEXT_LAYOUT: text parent"
    );
    let tx = evaluate(field(4)?)?;
    let ty = evaluate(field(5)?)?;
    let tw = evaluate(field(6)?)?;
    let th = evaluate(field(7)?)?;
    ensure!(
        tx >= 0.
            && ty >= 0.
            && tw > 0.
            && th > 0.
            && tx + tw <= size[0] as f32
            && ty + th <= size[1] as f32,
        "E_IMPORT_TEXT_LAYOUT: text exceeds window"
    );
    let font_size = literal_int(field(17)?)?;
    let leading = literal_int(field(19)?)?;
    ensure!(
        (18..=64).contains(&font_size) && (0..=font_size).contains(&leading),
        "E_IMPORT_TEXT_LAYOUT: font metrics"
    );
    let color = literal_int(field(20)?)? as u32;
    adapter.text_color = [
        (color & 255) as f32 / 255.,
        ((color >> 8) & 255) as f32 / 255.,
        ((color >> 16) & 255) as f32 / 255.,
        1.,
    ];
    let mut theme = json!({"height":size[1],"padding":0,"font_size":font_size,
        "line_height":(font_size+leading) as f32/font_size as f32,"opacity":opacity,
        "rect":[x,y,size[0],size[1]],"text_rect":[x+tx,y+ty,tw,th]});
    if let Some(background) = background {
        theme["background"] = json!(background);
    }
    Ok(theme)
}

/// Runtime switches only use full certified factory bodies. Geometry remains
/// static; source parent/visibility are retained by the player's dialogue root.
pub(super) fn style(adapter: &mut Adapter, script: &Script, name: &str) -> Result<String> {
    ensure!(
        script.version == 117
            && matches!(
                script.source_sha256.as_str(),
                "b8237985e258a7459e81e8a8bb613ea2ec513c62364144f7f477b6fc9b446271"
                    | "70c556ae8adb29599853d08e51ae5ca487938f8ec5367ea90a5d533c5119c5cf"
                    | "6536f7b498a34c6e414673f3015f674898840e39135bd1222eb7a407d10d0688"
                    | "557ca18abf361d700e6362f28bced02cc8b407cb3064350584194495252e5622"
            ),
        "E_IMPORT_TEXT_LAYOUT: uncertified window factory"
    );
    ensure!(
        name != "(編集用)",
        "E_IMPORT_TEXT_LAYOUT: editor window needs box-color adaptation"
    );
    let id = format!("lm.window.{}", &nir_content::digest(name.as_bytes())[..24]);
    if !adapter.dialogue_styles.contains_key(&id) {
        let original_textbox = adapter.textbox.clone();
        let original_color = adapter.text_color;
        let dialogue = theme_named(adapter, script, name)?;
        let text = adapter.text_color;
        adapter.textbox = original_textbox;
        adapter.text_color = original_color;
        adapter
            .dialogue_styles
            .insert(id.clone(), json!({"dialogue":dialogue,"text":text}));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_geometry_keeps_half_pixels_and_rejects_unknown_reads() {
        let term = Term::Apply {
            op: Op::Divide,
            args: vec![
                Term::Apply {
                    op: Op::Subtract,
                    args: vec![
                        Term::Read {
                            name: "@ScrWidth".into(),
                        },
                        Term::Apply {
                            op: Op::Property,
                            args: vec![
                                Term::String {
                                    value: "メッセージボックス土台".into(),
                                },
                                Term::Int { value: 5 },
                            ],
                        },
                    ],
                },
                Term::Int { value: 2 },
            ],
        };
        assert_eq!(coordinate(&term, [1280, 720], [961, 188]).unwrap(), 159.5);
        assert!(coordinate(
            &Term::Read {
                name: "game_variable".into()
            },
            [1280, 720],
            [961, 188]
        )
        .is_err());
        assert!(coordinate(
            &Term::Apply {
                op: Op::Divide,
                args: vec![Term::Int { value: 1 }, Term::Int { value: 0 }]
            },
            [1280, 720],
            [961, 188]
        )
        .is_err());
    }
}
