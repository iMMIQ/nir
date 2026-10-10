//! LiveCinema112 image tracks, sampled at the original integer-millisecond
//! clock. The commercial player is inspected statically, never executed.
//! TLiveCinema 135644 uses Round(ms / double(1000/60)); image clips 147ac4
//! hide outside [start,end), inherit predecessor endpoints, and retain the
//! movie's last frame. Property 138 stops waiting at frame_count-1 (135480).
use super::*;
use crate::import::cinema::{Cinema, Clip, ImageClip};
use nir_format::{Micros, SpriteKeyframe, SpriteTimeline, SpriteTimelineTrack, SpriteTransform};

// Exact double divisor used by the x87 player. Multiplying ms by 60 first
// changes the 25ms tie, so use rational arithmetic for the rounded quotient.
fn source_frame(ms: u64) -> u32 {
    let numerator = u128::from(ms) * 281_474_976_710_656;
    let denominator = 4_691_249_611_844_267u128;
    let q = numerator / denominator;
    let r = numerator % denominator;
    (q + u128::from(r * 2 > denominator || (r * 2 == denominator && q % 2 == 1))) as u32
}
fn frame_time(frame: u32) -> u64 {
    if frame == 0 {
        return 0;
    }
    let mut lo = 0;
    let mut hi = u64::from(frame) * 17 + 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if source_frame(mid) < frame {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo * 1000
}
fn real(a: f64, b: f64, curve: u8, t: u32, d: u32) -> f64 {
    if t == 0 {
        return a;
    }
    if t == d - 1 {
        return b;
    }
    match curve {
        0 => a + (b - a) * f64::from(t) / f64::from(d),
        1 => {
            b - (b - a) * (f64::from(d - t - 1) * std::f64::consts::FRAC_PI_2 / f64::from(d)).sin()
        }
        _ => a + (b - a) * (f64::from(t) * std::f64::consts::FRAC_PI_2 / f64::from(d)).sin(),
    }
}
fn integer(a: i32, b: i32, curve: u8, t: u32, d: u32) -> i32 {
    if t == 0 {
        return a;
    }
    if t == d - 1 {
        return b;
    }
    let delta = i64::from(b) - i64::from(a);
    match curve {
        0 => (i64::from(a) + delta * i64::from(t) / i64::from(d - 1)) as i32,
        1 => {
            b - ((delta as f64)
                * (f64::from(d - t - 1) * std::f64::consts::FRAC_PI_2 / f64::from(d)).sin())
            .round_ties_even() as i32
        }
        _ => {
            a + ((delta as f64) * (f64::from(t) * std::f64::consts::FRAC_PI_2 / f64::from(d)).sin())
                .round_ties_even() as i32
        }
    }
}
fn opacity(v: i32) -> Result<i32> {
    ensure!(
        (0..=255).contains(&v),
        "E_IMPORT_CINEMA_ALPHA: blend variant"
    );
    Ok(if v == 0 { 255 } else { v })
}
fn pose(
    c: &ImageClip,
    predecessor: Option<&ImageClip>,
    successor: bool,
    size: [u32; 2],
    t: u32,
) -> Result<SpriteKeyframe> {
    let d = c.duration + u32::from(successor);
    let start_position = predecessor.map_or(c.position_start, |p| p.position_end);
    let start_x = predecessor.map_or(c.scale_x[0], |p| p.scale_x[1]);
    let start_y = predecessor.map_or(c.scale_y[0], |p| p.scale_y[1]);
    let start_angle = predecessor.map_or(c.angle[0], |p| p.angle[1]);
    let start_opacity = predecessor.map_or(c.opacity[0], |p| p.opacity[1]);
    let x = integer(
        start_position[0],
        c.position_end[0],
        c.position_curve[0],
        t,
        d,
    );
    let y = integer(
        start_position[1],
        c.position_end[1],
        c.position_curve[1],
        t,
        d,
    );
    let sx = real(start_x, c.scale_x[1], c.scale_curve[0], t, d);
    let sy = real(start_y, c.scale_y[1], c.scale_curve[1], t, d);
    let angle = real(start_angle, c.angle[1], c.angle_curve, t, d).to_radians();
    let (sin, cos) = angle.sin_cos();
    let basis_x = [cos * sx, sin * sx];
    let basis_y = [-sin * sy, cos * sy];
    let origin = [
        -f64::from(c.pivot[0]) * basis_x[0] - f64::from(c.pivot[1]) * basis_y[0],
        -f64::from(c.pivot[0]) * basis_x[1] - f64::from(c.pivot[1]) * basis_y[1],
    ];
    let mut frame = SpriteKeyframe {
        at_us: Micros(0),
        rect: [x as f32, y as f32, size[0] as f32, size[1] as f32],
        opacity: integer(opacity(start_opacity)?, opacity(c.opacity[1])?, 0, t, d) as f32 / 255.,
        color: [1.; 4],
        transform: Some(SpriteTransform {
            origin: origin.map(|v| v as f32),
            basis_x: basis_x.map(|v| v as f32),
            basis_y: basis_y.map(|v| v as f32),
        }),
    };
    if origin == [0., 0.] && basis_x == [1., 0.] && basis_y == [0., 1.] {
        frame.transform = None;
    }
    ensure!(
        frame.valid(),
        "E_IMPORT_CINEMA_POSE: transform exceeds runtime bounds"
    );
    Ok(frame)
}

struct Movie {
    nodes: Vec<Node>,
    timeline: SpriteTimeline,
    size: [u32; 2],
    background: [f32; 4],
    clip: Option<[f32; 4]>,
}
#[cfg(test)]
fn lower(film: &Cinema, root: &str, id: &str, media: &[(String, [u32; 2])]) -> Result<Movie> {
    lower_certified(film, root, id, media, false)
}

fn lower_certified(
    film: &Cinema,
    root: &str,
    id: &str,
    media: &[(String, [u32; 2])],
    inert_extras: bool,
) -> Result<Movie> {
    ensure!(
        !film.anchor_enabled && film.anchor == [0, 0] && film.parameter == 0,
        "E_IMPORT_CINEMA_HEADER: anchor or loop policy"
    );
    ensure!(
        film.clips.len() <= 256 && film.clips.len() == media.len(),
        "E_IMPORT_CINEMA_LIMIT: image tracks"
    );
    let clips: Vec<&ImageClip> = film
        .clips
        .iter()
        .map(|c| match c {
            Clip::Image(c) => Ok(c),
            _ => bail!("E_IMPORT_CINEMA_CLIP: non-image playback needs adaptation"),
        })
        .collect::<Result<_>>()?;
    let count = clips
        .iter()
        .map(|c| c.start + c.duration)
        .max()
        .context("E_IMPORT_CINEMA: empty film")?;
    ensure!(count >= 2, "E_IMPORT_CINEMA_CLOCK: single frame film");
    // Cinema's property138 completes on its last frame (135480). Motion's
    // independent clock holds that frame until the next index reaches count
    // and sets its completion flag (170dcf..170e01).
    let duration_us = Micros(frame_time(if film.version == 111 {
        count
    } else {
        count - 1
    }));
    let mut nodes = vec![];
    let mut tracks = vec![];
    let mut budget = 0;
    for (index, c) in clips.iter().enumerate() {
        ensure!(
            c.period <= 1
                && c.flags == [false; 3]
                && c.clip_start == [0; 4]
                && c.clip_end == [0; 4]
                && (c.extra == [0; 4] || inert_extras),
            "E_IMPORT_CINEMA_IMAGE: crop, flips or afterimages need adaptation"
        );
        ensure!(
            c.position_start
                .iter()
                .chain(c.position_end.iter())
                .chain(c.pivot.iter())
                .all(|v| v.abs_diff(0) <= 32768),
            "E_IMPORT_CINEMA_POSITION: source geometry bounds"
        );
        let predecessor = c.previous.map(|i| clips[i]);
        let successor = clips.iter().any(|next| next.previous == Some(index));
        let (asset, size) = &media[index];
        let node_id = format!(
            "lm_movie_{}_{}",
            &nir_content::digest(id.as_bytes())[..20],
            index
        );
        let mut hidden = pose(c, predecessor, successor, *size, 0)?;
        hidden.opacity = 0.;
        let mut frames = vec![hidden];
        for frame in c.start..(c.start + c.duration).min(count) {
            let mut value = pose(c, predecessor, successor, *size, frame - c.start)?;
            value.at_us = Micros(frame_time(frame));
            if value.at_us.0 == 0 {
                frames[0] = value;
            } else {
                let mut prior = frames.last().unwrap().clone();
                prior.at_us = value.at_us;
                if prior != value {
                    frames.push(value);
                }
            }
        }
        if c.start + c.duration < count {
            let mut value = frames.last().unwrap().clone();
            value.at_us = Micros(frame_time(c.start + c.duration));
            value.opacity = 0.;
            frames.push(value);
        }
        budget += frames.len();
        ensure!(budget <= 65_536, "E_IMPORT_CINEMA_LIMIT: keyframes");
        let mut node: Node = serde_json::from_value(
            json!({"id":node_id,"parent":root,"asset":asset,"x":0,"y":0,"width":size[0],"height":size[1],"order":c.parameter}),
        )?;
        frames[0].apply(&mut node);
        nodes.push(node);
        tracks.push(SpriteTimelineTrack {
            node: node_id,
            frames,
        });
    }
    let timeline = SpriteTimeline {
        id: id.into(),
        duration_us,
        tracks,
    };
    ensure!(
        timeline.valid(),
        "E_IMPORT_CINEMA_TIMELINE: invalid resource"
    );
    let background = if film.background < 0 {
        [0.; 4]
    } else {
        let c = film.background as u32;
        [
            (c & 255) as f32 / 255.,
            ((c >> 8) & 255) as f32 / 255.,
            ((c >> 16) & 255) as f32 / 255.,
            1.,
        ]
    };
    Ok(Movie {
        nodes,
        timeline,
        size: film.size,
        background,
        clip: (film.background >= 0).then_some([0., 0., film.size[0] as f32, film.size[1] as f32]),
    })
}

/// These original players calculate the two extra interpolants into stack
/// slots 0x40/0x44, but TCustomImageFrameList's final draw call passes neither.
/// The final four scalar pushes read original slots 0x2c/0x30/0x38/0x3c after
/// accounting for each push. No extra-slot address escapes to the renderer.
/// Frame functions: 147ac4, 148630 and 14f80c respectively. This certificate
/// deliberately reads PE bytes only; it never executes the source player.
fn inert_extra_certificate(source: &Source) -> Result<bool> {
    let mut count = 0;
    for entry in fs::read_dir(&source.root)? {
        let entry = entry?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("exe"))
        {
            continue;
        }
        count += 1;
        ensure!(
            count <= 64,
            "E_IMPORT_CINEMA: player certificate candidate limit"
        );
        if !matches!(entry.metadata()?.len(), 1_987_584 | 1_991_168 | 2_037_248) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|x| x.to_str())
            .context("E_IMPORT_CINEMA: player path encoding")?;
        let bytes = read_binary(&source.path(name)?)?;
        if matches!(
            nir_content::digest(&bytes).as_str(),
            "01b5cf28d44968785aeca1ba15ae9abe6fdb3fdc47d0016dbe607e00385de590"
                | "7f67f9ed32279200204e1449e8db7d506c82dd1fd6e130ea64421e419fb5728e"
                | "85ff0838ae589dbae057a1382d19fe484f05e9baba57f0c54281d41996dcaf76"
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn attach(adapter: &mut Adapter, root: &str, path: &str) -> Result<()> {
    // Replacing a cinema removes its complete old subtree and task binding.
    let old: Vec<_> = adapter
        .nodes
        .values()
        .filter(|n| n.parent.as_deref() == Some(root))
        .map(|n| n.id.clone())
        .collect();
    for id in old {
        adapter.nodes.remove(&id);
    }
    adapter.movie_tasks.remove(root);
    if path.starts_with('$') {
        return Ok(());
    }
    let resolved = adapter.source.path(path)?;
    let cinema = resolved
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("lcm") || s.eq_ignore_ascii_case("lmt"));
    if !cinema {
        return Ok(());
    }
    let film = crate::import::cinema::parse(&read_binary(&resolved)?)?;
    let mut media = vec![];
    for clip in &film.clips {
        let Clip::Image(c) = clip else {
            bail!("E_IMPORT_CINEMA_CLIP: non-image playback needs adaptation");
        };
        ensure!(
            !c.media.contains('?'),
            "E_IMPORT_CINEMA_MEDIA: resource mask or trimming policy"
        );
        let name = resolved.parent().unwrap().join(&c.media);
        let name = name
            .strip_prefix(&adapter.source.root)?
            .to_str()
            .context("E_IMPORT_CINEMA_MEDIA: encoding")?;
        let (asset, size) = adapter.image(name)?;
        // GAL export also validates the complete pixels. Check frame count
        // now so preflight cannot certify a multi-frame resource as static.
        ensure!(
            media::gal_file_static_size(&adapter.source.path(name)?)? == (size[0], size[1]),
            "E_IMPORT_CINEMA_MEDIA: inconsistent static directory"
        );
        media.push((asset, size));
    }
    let name = resolved
        .strip_prefix(&adapter.source.root)?
        .to_str()
        .context("E_IMPORT_CINEMA_MEDIA")?;
    let id = format!(
        "lm.timeline.{}",
        nir_content::digest(
            format!(
                "{root}:{name}:{}:{}",
                adapter.location.source, adapter.location.index
            )
            .as_bytes()
        )
    );
    let has_extras = film
        .clips
        .iter()
        .any(|clip| matches!(clip, Clip::Image(c) if c.extra != [0; 4]));
    let inert_extras = has_extras && inert_extra_certificate(&adapter.source)?;
    let movie = lower_certified(&film, root, &id, &media, inert_extras)
        .with_context(|| format!("cinema {name}"))?;
    let node = adapter
        .nodes
        .get_mut(root)
        .context("E_IMPORT_CINEMA: missing root")?;
    node.asset = None;
    node.color = movie.background;
    node.clip = movie.clip;
    node.width = movie.size[0] as f32;
    node.height = movie.size[1] as f32;
    node.timeline_binding = Some(id.clone());
    for node in movie.nodes {
        adapter.nodes.insert(node.id.clone(), node);
    }
    let task = format!("lm_movie_{}", nir_content::digest(root.as_bytes()));
    adapter.pending_movies.push(json!({"id":task,"scope":"session","effect":{"type":"sprite_timeline","timeline":id,"root":root,"duration_us":movie.timeline.duration_us.0.to_string()}}));
    adapter.movie_tasks.insert(root.into(), task);
    adapter.sprite_timelines.insert(id, movie.timeline);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "private image-movie inventory supplied through environment"]
    fn external_movie_preflight() {
        let rows: Vec<Value> = serde_json::from_slice(
            &fs::read(std::env::var("NIR_CINEMA_INVENTORY").unwrap()).unwrap(),
        )
        .unwrap();
        let mut report = vec![];
        for row in rows {
            let path = Path::new(row["path"].as_str().unwrap());
            let root = path
                .ancestors()
                .find(|p| p.join("live.lpb").is_file())
                .unwrap();
            let source = Source::new(root).unwrap();
            let mut adapter = Adapter::new(source);
            adapter.nodes.insert(
                "movie".into(),
                serde_json::from_value(json!({"id":"movie","x":0,"y":0,"width":1,"height":1}))
                    .unwrap(),
            );
            let name = path.strip_prefix(root).unwrap().to_str().unwrap();
            let result = match attach(&mut adapter, "movie", name) {
                Ok(()) => {
                    let resource = adapter.sprite_timelines.values().next().unwrap();
                    json!({"status":"passed","tracks":resource.tracks.len(),"keyframes":resource.tracks.iter().map(|t|t.frames.len()).sum::<usize>(),"durationUs":resource.duration_us.0,"assets":adapter.assets.len()})
                }
                Err(error) => json!({"status":"unsupported","error":format!("{error:#}")}),
            };
            report.push(json!({"source":row,"result":result}));
        }
        fs::write(
            std::env::var("NIR_CINEMA_PROBE").unwrap(),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    fn image(start: u32, duration: u32, previous: Option<usize>) -> ImageClip {
        ImageClip {
            previous,
            start,
            duration,
            order: 0,
            period: 1,
            parameter: 9,
            flags: [false; 3],
            pivot: [0; 2],
            position_start: [0; 2],
            position_end: [100, 0],
            position_curve: [0; 2],
            angle: [0.; 2],
            angle_curve: 0,
            scale_x: [1.; 2],
            scale_y: [1.; 2],
            scale_curve: [0; 2],
            opacity: [0; 2],
            clip_start: [0; 4],
            clip_end: [0; 4],
            clip_curve: [0; 2],
            extra: [0; 4],
            media: "neutral.gal".into(),
        }
    }
    #[test]
    fn motion_holds_last_image_until_its_independent_clock_completes() {
        let film = |version| Cinema {
            version,
            size: [320, 240],
            background: -1,
            anchor_enabled: false,
            anchor: [0, 0],
            parameter: 0,
            clips: vec![Clip::Image(image(0, 3, None))],
        };
        let media = [("picture".into(), [32, 24])];
        let cinema = lower(&film(112), "root", "cinema", &media).unwrap();
        let motion = lower(&film(111), "root", "motion", &media).unwrap();
        assert_eq!(cinema.timeline.duration_us.0, frame_time(2));
        assert_eq!(motion.timeline.duration_us.0, frame_time(3));
        let last = motion.timeline.tracks[0].frames.last().unwrap();
        assert_eq!(last.at_us.0, frame_time(2));
        assert_eq!(last.rect[0], 100.);
    }
    fn film(clips: Vec<Clip>) -> Cinema {
        Cinema {
            version: 112,
            size: [320, 240],
            background: 0,
            anchor_enabled: false,
            anchor: [0; 2],
            parameter: 0,
            clips,
        }
    }
    #[test]
    fn certified_unused_extras_preserve_every_frame_and_uncertified_players_fail() {
        let media = vec![("original".into(), [32, 24])];
        let mut source = film(vec![Clip::Image(image(0, 130, None))]);
        let expected = lower(&source, "actor", "neutral", &media).unwrap();
        let Clip::Image(c) = &mut source.clips[0] else {
            unreachable!()
        };
        c.extra = [1000, 0, 500, 500];
        assert!(lower(&source, "actor", "neutral", &media).is_err());
        let actual = lower_certified(&source, "actor", "neutral", &media, true).unwrap();
        assert_eq!(
            serde_json::to_value(&actual.timeline).unwrap(),
            serde_json::to_value(&expected.timeline).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&actual.nodes).unwrap(),
            serde_json::to_value(&expected.nodes).unwrap()
        );
        let Clip::Image(c) = &mut source.clips[0] else {
            unreachable!()
        };
        c.clip_end[0] = 10;
        assert!(lower_certified(&source, "actor", "neutral", &media, true).is_err());
    }

    #[test]
    fn clip_hides_at_end_inherits_endpoint_and_movie_retains_last_frame() {
        let first = image(0, 4, None);
        let mut second = image(4, 3, Some(0));
        second.position_start = [999, 999];
        second.position_end = [200, 0];
        let mut source = film(vec![Clip::Image(first), Clip::Image(second)]);
        let media = vec![("first".into(), [32, 24]), ("second".into(), [32, 24])];
        let movie = lower(&source, "actor", "neutral", &media).unwrap();
        assert_eq!(movie.background, [0., 0., 0., 1.]);
        assert_eq!(movie.clip, Some([0., 0., 320., 240.]));
        assert_eq!(movie.timeline.duration_us, Micros(frame_time(6)));
        let mut nodes = movie.nodes.clone();
        movie.timeline.apply(&mut nodes, frame_time(3));
        assert_eq!((nodes[0].x, nodes[0].opacity), (75., 1.));
        assert_eq!(nodes[1].opacity, 0.);
        movie.timeline.apply(&mut nodes, frame_time(4));
        assert_eq!(nodes[0].opacity, 0.);
        assert_eq!((nodes[1].x, nodes[1].opacity), (100., 1.));
        movie.timeline.apply(&mut nodes, frame_time(7));
        assert_eq!(nodes[1].x, 200.);
        assert_eq!(nodes[1].order, 9);
        source.background = -1;
        let transparent = lower(&source, "actor", "neutral", &media).unwrap();
        assert_eq!(transparent.background, [0.; 4]);
        assert!(transparent.clip.is_none());
        let Clip::Image(c) = &mut source.clips[0] else {
            unreachable!()
        };
        c.extra = [0, 0, 500, 500];
        assert!(lower(&source, "actor", "neutral", &media).is_err());
    }
    #[test]
    fn pivot_and_anisotropic_rotation_use_original_texture_without_raster_frames() {
        let mut c = image(0, 4, None);
        c.pivot = [8, 6];
        c.position_start = [50, 100];
        c.position_end = c.position_start;
        c.angle = [90.; 2];
        c.scale_x = [2.; 2];
        c.scale_y = [0.5; 2];
        c.opacity = [100, 200];
        let movie = lower(
            &film(vec![Clip::Image(c)]),
            "actor",
            "rotate",
            &[("original".into(), [32, 24])],
        )
        .unwrap();
        let node = &movie.nodes[0];
        assert_eq!(node.asset.as_deref(), Some("original"));
        assert_eq!([node.width, node.height], [32., 24.]);
        let transform = node.sprite_transform.unwrap();
        assert!((transform.origin[0] - 3.).abs() < 1e-5);
        assert!((transform.origin[1] + 16.).abs() < 1e-5);
        assert!((transform.basis_x[1] - 2.).abs() < 1e-5);
        assert!((transform.basis_y[0] + 0.5).abs() < 1e-5);
        let last = movie.timeline.tracks[0].frames.last().unwrap();
        assert_eq!(last.opacity, 200. / 255.);
    }
    #[test]
    fn movie_starts_in_scene_commit_cue_before_wipe_wait_and_waitplay_uses_movie_task() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.pending_movies.push(json!({"id":"movie","scope":"session","effect":{"type":"sprite_timeline","timeline":"neutral","root":"actor","duration_us":"90000"}}));
        a.movie_tasks.insert("actor".into(), "movie".into());
        let mut blocks = vec![];
        a.scene(&mut blocks, 20000);
        let cue = &a.cues[blocks[0]["terminator"]["cue"].as_str().unwrap()];
        assert_eq!(cue["effects"][0]["effect"]["type"], "stage_present");
        assert_eq!(cue["effects"][1]["effect"]["type"], "sprite_timeline");
        assert_eq!(blocks[1]["terminator"]["conditions"][0]["task"], "stage");
        assert!(a.pending_movies.is_empty());
        a.event(
            &["WAITPLAY".into(), "actor".into(), "NORMAL".into()],
            &mut blocks,
        )
        .unwrap();
        assert_eq!(blocks[2]["terminator"]["conditions"][0]["task"], "movie");
        a.event(
            &["WAITPLAY".into(), "actor".into(), "CLICK".into()],
            &mut blocks,
        )
        .unwrap();
        assert_eq!(blocks.last().unwrap()["terminator"]["on_advance"], "NEXT");
    }
    #[test]
    fn clock_keeps_divisor_rounding_and_wait_finishes_on_last_frame() {
        assert_eq!(source_frame(25), 1);
        assert_eq!(
            [frame_time(1), frame_time(2), frame_time(3)],
            [9000, 26000, 42000]
        );
        assert_eq!(frame_time(299), 4_976_000);
        for frame in 1..10_000 {
            let t = frame_time(frame) / 1000;
            assert_eq!(source_frame(t), frame);
            assert_eq!(source_frame(t - 1), frame - 1);
        }
    }
    #[test]
    fn integer_and_real_source_curves_have_distinct_denominators() {
        assert_eq!(integer(0, 100, 0, 1, 5), 25);
        assert_eq!(real(0., 100., 0, 1, 5), 20.);
        assert_eq!(integer(100, 0, 0, 1, 5), 75);
        assert_eq!(integer(0, 100, 0, 4, 5), 100);
        assert_eq!(integer(0, 100, 0, 4, 6), 80);
        assert_eq!(real(0., 100., 0, 4, 6), 100. * 4. / 6.);
    }
}
