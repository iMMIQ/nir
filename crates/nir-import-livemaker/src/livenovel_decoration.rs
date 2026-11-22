//! Literal, fixed-coordinate portrait/name events from certified stock helpers.
//! Auto reflow and animated portraits remain errors until their clocks/layout
//! can be represented, rather than flattening them into stage decorations.
use super::*;

fn certify(adapter: &mut Adapter) -> Result<[f32; 2]> {
    if let Some(point) = adapter.decoration_point {
        return Ok(point);
    }
    let (_, events) = adapter
        .source
        .read("ノベルシステム/メッセージボックス/イベント.lsb")?;
    let (_, factory) = adapter.source.read("メッセージボックス作成.lsb")?;
    ensure!(
        events.version == 117
            && factory.version == 117
            && matches!(
                (
                    events.source_sha256.as_str(),
                    factory.source_sha256.as_str()
                ),
                (
                    "3e7df74bdfcbbca60e46863ab59f1b66e2f9c9ad3f99403cb3ab450ed39734fe",
                    "6536f7b498a34c6e414673f3015f674898840e39135bd1222eb7a407d10d0688"
                ) | (
                    "522cf5722a0a2ce17561d0988e02e00663bae0dadcf5b3673f402b5fea90152e",
                    "557ca18abf361d700e6362f28bced02cc8b407cb3064350584194495252e5622"
                )
            ),
        "E_IMPORT_DECORATION: unverified event/window helpers"
    );
    // These players dispatch PR_ONNOTIFY through TStringList.Text
    // in the two certified player variants. The CR/LF scanner tests NUL
    // after consuming a separator,
    // so a final CRLF contributes no additional parameter. Read PE bytes
    // only; an unknown player must not inherit this parser certificate.
    let mut candidates = 0;
    let mut certified = false;
    for entry in fs::read_dir(&adapter.source.root)? {
        let entry = entry?;
        if !entry
            .path()
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("exe"))
        {
            continue;
        }
        candidates += 1;
        ensure!(
            candidates <= 64,
            "E_IMPORT_DECORATION: player candidate limit"
        );
        if !matches!(entry.metadata()?.len(), 1_991_168 | 2_037_248) {
            continue;
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .context("E_IMPORT_DECORATION: player path encoding")?;
        let bytes = read_binary(&adapter.source.path(name)?)?;
        certified |= matches!(
            nir_content::digest(&bytes).as_str(),
            "7f67f9ed32279200204e1449e8db7d506c82dd1fd6e130ea64421e419fb5728e"
                | "85ff0838ae589dbae057a1382d19fe484f05e9baba57f0c54281d41996dcaf76"
        );
    }
    ensure!(
        certified,
        "E_IMPORT_DECORATION: unverified native event parser"
    );
    // All admitted runtime styles (standard, その２, 色替え) initialize
    // the same coordinate mode. Editor/automatic layout is not admitted.
    let mut point = None;
    for (xline, yline) in [(24, 25), (77, 78), (105, 106)] {
        let coordinate = |line, name| -> Result<f32> {
            let command = factory
                .commands
                .iter()
                .find(|c| c.line == line && !c.muted)
                .context("E_IMPORT_DECORATION: missing fixed-coordinate initializer")?;
            let Body::Calc(expression) = &command.body else {
                bail!("E_IMPORT_DECORATION: expected coordinate assignment");
            };
            let [(1, target, values)] = expression.operations.as_slice() else {
                bail!("E_IMPORT_DECORATION: dynamic coordinate initializer");
            };
            let [Literal::Int(value)] = values.as_slice() else {
                bail!("E_IMPORT_DECORATION: nonliteral coordinate initializer");
            };
            ensure!(
                target == name && value.abs_diff(0) <= 8192,
                "E_IMPORT_DECORATION: invalid coordinate initializer"
            );
            Ok(*value as f32)
        };
        let current = [
            coordinate(xline, "__テキスト顔座標X")?,
            coordinate(yline, "__テキスト顔座標Y")?,
        ];
        ensure!(
            point.is_none_or(|point| point == current),
            "E_IMPORT_DECORATION: style-dependent portrait placement"
        );
        point = Some(current);
    }
    adapter.decoration_point = point;
    Ok(point.unwrap())
}

fn native_arguments(args: &[String]) -> &[String] {
    // Drop exactly the empty field introduced by split("\r\n") at EOF.
    // Two separators still carry one genuinely empty parameter.
    if args.last().is_some_and(String::is_empty) {
        &args[..args.len() - 1]
    } else {
        args
    }
}

fn source_path(name: &str, value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && !value.starts_with('?'),
        "E_IMPORT_DECORATION: literal nonempty image required"
    );
    let mut value = value.replace('\\', "/");
    if Path::new(&value).extension().is_none() {
        value.push_str(".gal");
    }
    ensure!(
        Path::new(&value)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gal")),
        "E_IMPORT_DECORATION: animated or unknown portrait format"
    );
    let folder = if name == "FACE" {
        "グラフィック/顔/"
    } else {
        "グラフィック/名前/"
    };
    if !value.starts_with(folder) {
        value.insert_str(0, folder);
    }
    Ok(value)
}

pub(super) fn event(
    adapter: &mut Adapter,
    name: &str,
    args: &[String],
    blocks: &mut Vec<Value>,
) -> Result<()> {
    let point = certify(adapter)?;
    let args = native_arguments(args);
    ensure!(args.len() <= 1, "E_IMPORT_DECORATION: event argument count");
    let portrait = name == "FACE";
    let image = if let Some(value) = args.first() {
        let (asset, size) = adapter.image(&source_path(name, value)?)?;
        let placement = if portrait {
            // Source Y is the image's vertical centre, including custom mode.
            json!({"type":"absolute","point":[point[0],point[1]-size[1] as f32/2.]})
        } else {
            // Capture the current styled text origin at this event. A later
            // window switch does not move the existing root-level name image.
            json!({"type":"text_origin","offset":[0.,-(size[1] as f32)-10.]})
        };
        json!({"asset":asset,"size":size,"placement":placement})
    } else {
        Value::Null
    };
    adapter.effect(blocks, if portrait { "lm.portrait" } else { "lm.name" }, "session",
        json!({"type":"dialogue_decoration","slot":if portrait {"portrait"} else {"name"},"image":image}), false);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_separator_is_not_a_parameter_but_internal_empty_is() {
        let parse = |value: &str| {
            value
                .split("\r\n")
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        for value in ["FACE", "FACE\r\n"] {
            assert!(native_arguments(&parse(value)).is_empty());
        }
        for value in ["FACE\r\nactor", "FACE\r\nactor\r\n"] {
            assert_eq!(native_arguments(&parse(value)), ["actor"]);
        }
        let args = parse("FACE\r\n\r\n");
        assert_eq!(native_arguments(&args), [""]);
        assert!(source_path("FACE", &native_arguments(&args)[0]).is_err());
        assert_eq!(
            native_arguments(&parse("FACE\r\n\r\nactor\r\n")),
            ["", "actor"]
        );
    }
    #[test]
    fn literal_paths_preserve_extension_and_reject_dynamic_or_animated_content() {
        assert_eq!(
            source_path("FACE", "actor").unwrap(),
            "グラフィック/顔/actor.gal"
        );
        assert_eq!(
            source_path("NAMELABEL", "グラフィック\\名前\\actor.GAL").unwrap(),
            "グラフィック/名前/actor.GAL"
        );
        for value in ["", "?expression", "actor.lmt", "actor.lcm", "actor.png"] {
            assert!(source_path("FACE", value).is_err());
        }
    }
}
