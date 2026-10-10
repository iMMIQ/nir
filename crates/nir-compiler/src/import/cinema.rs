//! Bounded structural reader for LiveCinema 112 image, box, and sound clips. This reader is
//! deliberately separate from GAL decoding: an LCM is a timeline with external
//! media, not a raster, and parsing it does not imply playback support.
//!
//! The byte layout and predecessor clock come from the original engine's
//! TFilmManager/TCustomFrameList/TCustomImageFrameList stream readers. The
//! default TLiveCinema frame rate is 60; the file itself has no frame-rate field.
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct Cinema {
    pub version: u32,
    pub size: [u32; 2],
    pub background: i32,
    pub anchor_enabled: bool,
    pub anchor: [i32; 2],
    pub parameter: i32,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub(super) enum Clip {
    Image(ImageClip),
    Other(OtherClip),
}
impl Clip {
    fn kind(&self) -> u8 {
        match self {
            Self::Image(_) => 0,
            Self::Other(c) => c.kind,
        }
    }
    fn start(&self) -> u32 {
        match self {
            Self::Image(c) => c.start,
            Self::Other(c) => c.start,
        }
    }
    fn duration(&self) -> u32 {
        match self {
            Self::Image(c) => c.duration,
            Self::Other(c) => c.duration,
        }
    }
}
#[derive(Debug, Serialize)]
pub(super) struct OtherClip {
    pub kind: u8,
    pub previous: Option<usize>,
    pub start: u32,
    pub duration: u32,
    pub order: i32,
    pub period: u32,
    pub parameter: i32,
    pub flags: [bool; 3],
    pub fields: OtherFields,
}
/// The physical field layout is established by the stock stream readers.
/// Until their playback semantics are mapped, retain every value verbatim;
/// names describe stream groups rather than guessing a color or gain policy.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OtherFields {
    Sound {
        media: String,
        values: [i32; 4],
        curves: [u8; 2],
    },
    Box {
        points: [[i32; 2]; 2],
        point_curves: [u8; 2],
        values: [i32; 12],
        reals: [f64; 4],
        real_curves: [u8; 2],
        extra: [i32; 4],
    },
}

#[derive(Debug, Serialize)]
pub(super) struct ImageClip {
    pub previous: Option<usize>,
    pub start: u32,
    pub duration: u32,
    pub order: i32,
    pub period: u32,
    // Retain unclassified source fields; do not silently interpret or discard
    // them when adding a runtime adapter.
    pub parameter: i32,
    pub flags: [bool; 3],
    pub pivot: [i32; 2],
    pub position_start: [i32; 2],
    pub position_end: [i32; 2],
    pub position_curve: [u8; 2],
    pub angle: [f64; 2],
    pub angle_curve: u8,
    pub scale_x: [f64; 2],
    pub scale_y: [f64; 2],
    pub scale_curve: [u8; 2],
    pub opacity: [i32; 2],
    pub clip_start: [i32; 4],
    pub clip_end: [i32; 4],
    pub clip_curve: [u8; 2],
    pub extra: [i32; 4],
    pub media: String,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).context("E_IMPORT_CINEMA: offset")?;
        let out = self
            .bytes
            .get(self.at..end)
            .context("E_IMPORT_CINEMA: truncated timeline")?;
        self.at = end;
        Ok(out)
    }
    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn positive(&mut self, maximum: u32) -> Result<u32> {
        let value = self.int()?;
        ensure!(
            value > 0 && value as u32 <= maximum,
            "E_IMPORT_CINEMA: positive field exceeds bounds"
        );
        Ok(value as u32)
    }
    fn flag(&mut self) -> Result<bool> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => bail!("E_IMPORT_CINEMA: invalid flag"),
        }
    }
    fn curve(&mut self) -> Result<u8> {
        let value = self.take(1)?[0];
        ensure!(value <= 2, "E_IMPORT_CINEMA: unknown interpolation");
        Ok(value)
    }
    fn real(&mut self) -> Result<f64> {
        let value = f64::from_le_bytes(self.take(8)?.try_into()?);
        ensure!(
            value.is_finite() && value.abs() <= 65_536.,
            "E_IMPORT_CINEMA: invalid transform"
        );
        Ok(value)
    }
    fn point(&mut self) -> Result<[i32; 2]> {
        Ok([self.int()?, self.int()?])
    }
    fn media(&mut self) -> Result<String> {
        let length = self.positive(4096)? as usize;
        let (value, errors) =
            encoding_rs::SHIFT_JIS.decode_without_bom_handling(self.take(length)?);
        ensure!(!errors, "E_IMPORT_CINEMA: invalid media encoding");
        let value = value.replace('\\', "/");
        ensure!(
            !value.contains(['\0', ':']) && !value.starts_with('/'),
            "E_IMPORT_CINEMA: media path must be relative"
        );
        // Parent segments occur in legitimate LCMs. The caller must resolve
        // these against the cinema directory and enforce the game-root bound.
        Ok(value)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Cinema> {
    ensure!(
        bytes.len() <= 1024 * 1024,
        "E_IMPORT_CINEMA: timeline limit"
    );
    let mut r = Reader { bytes, at: 0 };
    // TFilmManager's shared loader (14cf5c) distinguishes these prefixes,
    // then passes the version to the same frame-list readers.
    let version = match r.take(13)? {
        b"LiveCinema112" => 112,
        b"LiveMotion111" => 111,
        _ => bail!("E_IMPORT_CINEMA_VERSION: expected LiveCinema112 or LiveMotion111"),
    };
    let size = [r.positive(8192)?, r.positive(8192)?];
    let background = r.int()?;
    ensure!(
        (-1..=0xffffff).contains(&background),
        "E_IMPORT_CINEMA: background color"
    );
    let anchor_enabled = r.flag()?;
    let anchor = r.point()?;
    let parameter = r.int()?;
    let count = r.positive(4096)? as usize;
    let mut clips: Vec<Clip> = Vec::with_capacity(count);
    for index in 0..count {
        let kind = r.take(1)?[0];
        ensure!(
            matches!(kind, 0 | 2 | 4),
            "E_IMPORT_CINEMA_CLIP: unsupported clip type {kind} at index {index}"
        );
        let previous = r.int()?;
        ensure!(
            previous == -1 || (previous >= 0 && (previous as usize) < index),
            "E_IMPORT_CINEMA_REFERENCE: forward or cyclic predecessor"
        );
        let previous = (previous >= 0).then_some(previous as usize);
        let duration = r.positive(216_000)?;
        let order = r.int()?;
        let authored_start = r.int()?;
        let period = r.int()?;
        ensure!(
            (0..=216_000).contains(&period),
            "E_IMPORT_CINEMA: period exceeds bounds"
        );
        let period = period as u32;
        let parameter = r.int()?;
        let flags = [r.flag()?, r.flag()?, r.flag()?];
        let start = if let Some(previous) = previous {
            clips[previous]
                .start()
                .checked_add(clips[previous].duration())
                .context("E_IMPORT_CINEMA: clock overflow")?
        } else {
            ensure!(authored_start >= 0, "E_IMPORT_CINEMA: negative start");
            authored_start as u32
        };
        ensure!(
            start.checked_add(duration).is_some_and(|n| n <= 216_000),
            "E_IMPORT_CINEMA: clock limit"
        );
        ensure!(
            previous.is_none_or(|p| clips[p].kind() == kind),
            "E_IMPORT_CINEMA_REFERENCE: predecessor clip type differs"
        );
        if kind != 0 {
            let fields = if kind == 4 {
                OtherFields::Sound {
                    media: r.media()?,
                    values: [r.int()?, r.int()?, r.int()?, r.int()?],
                    curves: [r.curve()?, r.curve()?],
                }
            } else {
                let points = [r.point()?, r.point()?];
                let point_curves = [r.curve()?, r.curve()?];
                let mut values = [0; 12];
                for value in &mut values {
                    *value = r.int()?;
                }
                OtherFields::Box {
                    points,
                    point_curves,
                    values,
                    reals: [r.real()?, r.real()?, r.real()?, r.real()?],
                    real_curves: [r.curve()?, r.curve()?],
                    extra: [r.int()?, r.int()?, r.int()?, r.int()?],
                }
            };
            clips.push(Clip::Other(OtherClip {
                kind,
                previous,
                start,
                duration,
                order,
                period,
                parameter,
                flags,
                fields,
            }));
            continue;
        }
        let pivot = r.point()?;
        let position_start = r.point()?;
        let position_end = r.point()?;
        let position_curve = [r.curve()?, r.curve()?];
        let angle = [r.real()?, r.real()?];
        let scale_x = [r.real()?, r.real()?];
        let scale_x_curve = r.curve()?;
        // Stream order is start opacity, end opacity. Start is inherited from
        // the predecessor's end during playback, as are several other fields.
        let opacity = [r.int()?, r.int()?];
        let angle_curve = r.curve()?;
        let p0 = r.point()?;
        let s0 = r.point()?;
        let p1 = r.point()?;
        let s1 = r.point()?;
        let clip_start = [p0[0], p0[1], s0[0], s0[1]];
        let clip_end = [p1[0], p1[1], s1[0], s1[1]];
        let clip_curve = [r.curve()?, r.curve()?];
        let scale_y = [r.real()?, r.real()?];
        let scale_curve = [scale_x_curve, r.curve()?];
        let extra = [r.int()?, r.int()?, r.int()?, r.int()?];
        let media = r.media()?;
        clips.push(Clip::Image(ImageClip {
            previous,
            start,
            duration,
            order,
            period,
            parameter,
            flags,
            pivot,
            position_start,
            position_end,
            position_curve,
            angle,
            angle_curve,
            scale_x,
            scale_y,
            scale_curve,
            opacity,
            clip_start,
            clip_end,
            clip_curve,
            extra,
            media,
        }));
    }
    ensure!(r.at == bytes.len(), "E_IMPORT_CINEMA: trailing data");
    Ok(Cinema {
        version,
        size,
        background,
        anchor_enabled,
        anchor,
        parameter,
        clips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn integer(out: &mut Vec<u8>, value: i32) {
        out.extend(value.to_le_bytes());
    }
    fn fixture() -> Vec<u8> {
        let mut out = b"LiveCinema112".to_vec();
        for value in [320, 240, -1] {
            integer(&mut out, value);
        }
        out.push(0);
        for value in [0, 0, 0, 2] {
            integer(&mut out, value);
        }
        for (previous, duration, authored_start) in [(-1, 60, 30), (0, 20, 777)] {
            out.push(0);
            for value in [previous, duration, 2, authored_start, 1, 0] {
                integer(&mut out, value);
            }
            out.extend([0; 3]);
            for value in [10, 12, 50, 70, 80, 90] {
                integer(&mut out, value);
            }
            out.extend([1, 2]);
            for value in [0f64, 45., 1., 2.] {
                out.extend(value.to_le_bytes());
            }
            out.push(0);
            integer(&mut out, 100);
            integer(&mut out, 200);
            out.push(2);
            for value in [1, 2, 3, 4, 5, 6, 7, 8] {
                integer(&mut out, value);
            }
            out.extend([1, 2]);
            for value in [1f64, 0.5] {
                out.extend(value.to_le_bytes());
            }
            out.push(0);
            for value in [11, 12, 13, 14] {
                integer(&mut out, value);
            }
            integer(&mut out, 9);
            out.extend(b"image.gal");
        }
        out
    }
    #[test]
    fn motion111_shares_image_layout_and_keeps_version_gates_and_bounds() {
        let mut motion = fixture();
        motion[..13].copy_from_slice(b"LiveMotion111");
        let film = parse(&motion).unwrap();
        assert_eq!(film.version, 111);
        assert_eq!(film.clips[1].start(), 90);
        let Clip::Image(image) = &film.clips[1] else {
            panic!("image")
        };
        assert_eq!(image.media, "image.gal");
        assert_eq!(image.extra, [11, 12, 13, 14]);
        for length in 0..motion.len() {
            assert!(parse(&motion[..length]).is_err());
        }
        for header in [b"LiveMotion110", b"LiveMotion112", b"LiveCinema111"] {
            motion[..13].copy_from_slice(header);
            assert!(parse(&motion).is_err());
        }
    }
    #[test]
    fn image_stream_keeps_transforms_and_uses_predecessor_end() {
        let film = parse(&fixture()).unwrap();
        assert_eq!(film.size, [320, 240]);
        assert_eq!(film.clips[0].start(), 30);
        assert_eq!(film.clips[1].start(), 90);
        let Clip::Image(c) = &film.clips[1] else {
            panic!("image fixture");
        };
        assert_eq!(c.pivot, [10, 12]);
        assert_eq!(c.angle, [0., 45.]);
        assert_eq!(c.scale_x, [1., 2.]);
        assert_eq!(c.scale_y, [1., 0.5]);
        assert_eq!(c.opacity, [100, 200]);
        assert_eq!(c.clip_start, [1, 2, 3, 4]);
        assert_eq!(c.clip_end, [5, 6, 7, 8]);
        assert_eq!(c.extra, [11, 12, 13, 14]);
        assert_eq!(c.media, "image.gal");
    }
    fn mixed_fixture() -> Vec<u8> {
        let mut out = fixture();
        out[38..42].copy_from_slice(&5i32.to_le_bytes());
        for previous in [-1, 2] {
            out.push(4);
            for v in [previous, 12, 3, 15, 0, 0] {
                integer(&mut out, v);
            }
            out.extend([0; 3]);
            integer(&mut out, 9);
            out.extend(b"music.ogg");
            for v in [1000, 2000, 0, 0] {
                integer(&mut out, v);
            }
            out.extend([1, 2]);
        }
        out.push(2);
        for v in [-1, 30, 4, 0, 1, 0] {
            integer(&mut out, v);
        }
        out.extend([0; 3]);
        for v in [10, 20, 30, 40] {
            integer(&mut out, v);
        }
        out.extend([0, 1]);
        for v in 0..12 {
            integer(&mut out, v);
        }
        for v in [1f64, 0.5, 2., 3.] {
            out.extend(v.to_le_bytes());
        }
        out.extend([1, 2]);
        for v in [20, 30, 40, 50] {
            integer(&mut out, v);
        }
        out
    }
    #[test]
    fn mixed_stream_keeps_sound_and_box_fields_and_global_predecessor_clock() {
        let bytes = mixed_fixture();
        let film = parse(&bytes).unwrap();
        assert_eq!(film.clips.len(), 5);
        assert_eq!(film.clips[2].start(), 15);
        assert_eq!(film.clips[3].start(), 27);
        let Clip::Other(sound) = &film.clips[3] else {
            panic!("sound fixture");
        };
        assert!(
            matches!(&sound.fields, OtherFields::Sound { media, values, curves }
            if media == "music.ogg" && *values == [1000, 2000, 0, 0] && *curves == [1, 2])
        );
        let serialized = serde_json::to_value(&film).unwrap();
        assert_eq!(serialized["clips"][3]["record"], "other");
        assert_eq!(serialized["clips"][3]["kind"], 4);
        let Clip::Other(rect) = &film.clips[4] else {
            panic!("box fixture");
        };
        assert!(
            matches!(&rect.fields, OtherFields::Box { points, reals, extra, .. }
            if *points == [[10, 20], [30, 40]] && *reals == [1., 0.5, 2., 3.] && *extra == [20, 30, 40, 50])
        );
        for length in 0..bytes.len() {
            assert!(parse(&bytes[..length]).is_err());
        }
        let second_sound = fixture().len() + 1 + 24 + 3 + 4 + 9 + 16 + 2;
        let mut bad = bytes;
        bad[second_sound + 1..second_sound + 5].copy_from_slice(&0i32.to_le_bytes());
        assert!(parse(&bad).is_err(), "cross-type predecessor");
    }
    #[test]
    #[ignore = "private source inventory supplied through environment"]
    fn external_cinema_inventory() {
        let inventory = std::env::var("NIR_CINEMA_INVENTORY").unwrap();
        let output = std::env::var("NIR_CINEMA_PROBE").unwrap();
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(inventory).unwrap()).unwrap();
        let mut records = Vec::new();
        for row in rows {
            let path = row["path"].as_str().unwrap();
            let bytes = std::fs::read(path).unwrap();
            let result = match parse(&bytes) {
                Ok(film) => serde_json::json!({"film": film}),
                Err(error) => serde_json::json!({"error": format!("{error:#}")}),
            };
            records.push(serde_json::json!({"source": row, "result": result}));
        }
        std::fs::write(output, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    }
    #[test]
    fn hostile_timeline_rejects_truncation_cycles_flags_types_and_nonfinite() {
        let base = fixture();
        for length in 0..base.len() {
            assert!(parse(&base[..length]).is_err());
        }
        for (offset, replacement) in [
            (43, 0i32.to_le_bytes().to_vec()),
            (42, vec![7]),
            (67, vec![2]),
            (96, f64::NAN.to_le_bytes().to_vec()),
            (38, 4097i32.to_le_bytes().to_vec()),
        ] {
            let mut bad = base.clone();
            bad[offset..offset + replacement.len()].copy_from_slice(&replacement);
            assert!(parse(&bad).is_err(), "offset {offset}");
        }
        let mut trailing = base;
        trailing.push(0);
        assert!(parse(&trailing).is_err());
    }
}
