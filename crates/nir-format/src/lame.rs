//! LAME gapless metadata (`Info`/`Xing` tag) parsing.
//!
//! MP3 encoders prepend roughly one frame of decoder delay and pad the last
//! frame so players that ignore gapless metadata play extra silence at both
//! ends. LAME records the exact `enc_delay` and `enc_padding` sample counts in
//! a tag inside the first MP3 frame. Tag-aware decoders — the browser
//! `decodeAudioData` path and symphonia (the native loader's backend) — trim
//! both themselves and return exactly the authored sample count, while a
//! tag-unaware decode yields whole frames. The native loader parses the tag to
//! tell those decode shapes apart and trim only the unaware one back into
//! alignment. The compiler writes the same tag for every packaged MP3
//! (`media.mp3.v1`). Field layout follows LAME's own reader and `PutLameVBR`
//! writer (libmp3lame/VbrTag.c).

/// Encoder delay and padding in samples (per channel) from a LAME tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gapless {
    pub delay: u32,
    pub padding: u32,
    /// Sample rate of the tagged frame, so callers can detect files whose
    /// encoder silently resampled away from the authored rate.
    pub rate: u32,
}

/// The bound LAME's own tag reader applies before trusting the values.
const MAX_GAPLESS: u32 = 3000;

/// Bitrates in kbps for MPEG-1 Layer III frames, indexed by header value.
const BITRATES_V1_L3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
/// Bitrates in kbps for MPEG-2/2.5 Layer III frames, indexed by header value.
const BITRATES_V2_L3: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];
const RATES_V1: [u32; 4] = [44100, 48000, 32000, 0];
const RATES_V2: [u32; 4] = [22050, 24000, 16000, 0];
const RATES_V2_5: [u32; 4] = [11025, 12000, 8000, 0];

fn be32(b: &[u8]) -> u32 {
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | b[3] as u32
}
fn syncsafe32(b: &[u8]) -> u32 {
    ((b[0] as u32 & 0x7f) << 21)
        | ((b[1] as u32 & 0x7f) << 14)
        | ((b[2] as u32 & 0x7f) << 7)
        | (b[3] as u32 & 0x7f)
}

/// Skips an `ID3v2` header when present and returns the offset of MP3 frames.
fn skip_id3v2(b: &[u8]) -> Option<usize> {
    if b.len() < 10 || &b[0..3] != b"ID3" {
        return Some(0);
    }
    let flags = b[5];
    if flags & 0x40 != 0 || flags & 0x80 != 0 {
        // Extended header or unsynchronization make offset math unreliable;
        // callers then treat the file as non-gapless.
        return None;
    }
    let size = syncsafe32(&b[6..10]) as usize;
    b.len().checked_sub(10 + size).map(|_| 10 + size)
}

/// One parsed MPEG audio frame header. Only Layer III is accepted.
struct FrameHeader {
    mpeg1: bool,
    stereo: bool,
    rate: u32,
}
fn frame_header(b: &[u8]) -> Option<FrameHeader> {
    if b.len() < 4 || b[0] != 0xff || b[1] & 0xe0 != 0xe0 {
        return None;
    }
    let version_bits = (b[1] >> 3) & 0x3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
    let layer_bits = (b[1] >> 1) & 0x3; // 1 = Layer III
    if layer_bits != 1 || version_bits == 1 {
        return None;
    }
    let mpeg1 = version_bits == 3;
    let bitrate = ((b[2] >> 4) & 0xf) as usize;
    let rate_index = ((b[2] >> 2) & 0x3) as usize;
    let padding = (b[2] >> 1) & 0x1;
    let channel_bits = (b[3] >> 6) & 0x3;
    if bitrate == 0 || bitrate == 15 || rate_index == 3 {
        return None; // free/bad bitrate, reserved rate
    }
    let rate = match version_bits {
        3 => RATES_V1[rate_index],
        2 => RATES_V2[rate_index],
        _ => RATES_V2_5[rate_index],
    };
    if rate == 0 {
        return None;
    }
    let kbps = if mpeg1 {
        BITRATES_V1_L3[bitrate]
    } else {
        BITRATES_V2_L3[bitrate]
    };
    if kbps == 0 {
        return None;
    }
    let unit = if mpeg1 { 144 } else { 72 };
    let length = unit * kbps * 1000 / rate + padding as u32;
    if length < 8 {
        return None;
    }
    Some(FrameHeader {
        mpeg1,
        stereo: channel_bits != 3,
        rate,
    })
}

/// Parses the `Info`/`Xing` tag with its LAME extension for delay/padding.
///
/// Returns `None` for any structural surprise; callers must fall back to
/// untrimmed decoding, never guess.
pub fn parse(b: &[u8]) -> Option<Gapless> {
    let frames = &b[skip_id3v2(b)?..];
    let header = frame_header(frames)?;
    // The tag follows the 4-byte header and the side information block, whose
    // size depends on MPEG version and channel mode.
    let side_info = if header.mpeg1 {
        if header.stereo {
            32
        } else {
            17
        }
    } else if header.stereo {
        17
    } else {
        9
    };
    let tag = frames.get(4 + side_info..)?;
    // `get` hands back an empty slice for input truncated at the side-info
    // boundary; the magic and flags words need 8 real bytes.
    if tag.len() < 8 {
        return None;
    }
    let magic = &tag[0..4];
    if magic != b"Xing" && magic != b"Info" {
        return None;
    }
    let flags = be32(&tag[4..8]);
    let mut offset = 8;
    if flags & 1 != 0 {
        offset += 4; // frame count
    }
    if flags & 2 != 0 {
        offset += 4; // stream bytes
    }
    if flags & 4 != 0 {
        offset += 100; // seek table
    }
    if flags & 8 != 0 {
        offset += 4; // quality
    }
    // LAME extension, laid out by PutLameVBR: when the Xing header reserved a
    // quality field (flags bit 3) it doubles as the extension's first 4 bytes,
    // so the version string always starts exactly here. Then 9 bytes starting
    // with "LAME", revision/method(1), lowpass(1), peak(4), replay gain pair(4),
    // flags(1), abr(1), and the 3-byte delay/padding pair.
    let ext = tag.get(offset..)?;
    if ext.len() < 24 || &ext[0..4] != b"LAME" {
        return None;
    }
    let delay = ((ext[21] as u32) << 4) | (ext[22] as u32 >> 4);
    let padding = ((ext[22] as u32 & 0x0f) << 8) | ext[23] as u32;
    if delay > MAX_GAPLESS || padding > MAX_GAPLESS {
        return None;
    }
    Some(Gapless {
        delay,
        padding,
        rate: header.rate,
    })
}

/// Total decoded sample count (per channel) a gapless-unaware decoder yields
/// for an MP3 whose LAME tag reports `source` authored samples: the authored
/// audio plus encoder delay and padding, rounded up to whole frames.
pub fn untrimmed_samples(gapless: &Gapless, source: u64, samples_per_frame: u32) -> u64 {
    let total = gapless.delay as u64 + source + gapless.padding as u64;
    total.div_ceil(samples_per_frame as u64) * samples_per_frame as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a first frame carrying a minimal LAME tag with the given values.
    fn tagged_file(delay: u32, padding: u32, id3: bool) -> Vec<u8> {
        // 44.1kHz stereo MPEG1 Layer III 128kbps frame: length 417, side info 32.
        let mut file = Vec::new();
        if id3 {
            file.extend_from_slice(b"ID3\x04\x00\x00\x00\x00\x00\x0a");
            file.extend([0u8; 10]);
        }
        file.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        file.extend([0u8; 32]);
        let mut tag: Vec<u8> = Vec::new();
        tag.extend_from_slice(b"Info");
        tag.extend_from_slice(&1u32.to_be_bytes()); // frame-count field present
        tag.extend_from_slice(&10u32.to_be_bytes());
        // The LAME extension starts right after the optional header fields:
        // version string, revision/method, lowpass, peak, replay gains, flags,
        // abr, then delay/padding at extension offsets 21..24.
        tag.extend_from_slice(b"LAME3.100");
        tag.push(0x00); // revision + vbr method
        tag.push(0x00); // lowpass
        tag.extend([0u8; 4]); // peak
        tag.extend([0u8; 4]); // replay gain pair
        tag.push(0x00); // flags
        tag.push(0xa0); // abr
        tag.push((delay >> 4) as u8);
        tag.push(((delay << 4) as u8) | (padding >> 8) as u8);
        tag.push(padding as u8);
        tag.extend([0u8; 4]); // misc + unused + preset prefix
        file.extend_from_slice(&tag);
        file
    }

    #[test]
    fn parses_delay_and_padding_from_hand_built_tag() {
        let file = tagged_file(1105, 536, false);
        let gapless = parse(&file).expect("tag parses");
        assert_eq!(gapless.delay, 1105);
        assert_eq!(gapless.padding, 536);
        assert_eq!(gapless.rate, 44100);
        assert_eq!(untrimmed_samples(&gapless, 44100, 1152), 40 * 1152);
    }

    #[test]
    fn skips_id3v2_prefix() {
        let file = tagged_file(576, 12, true);
        let gapless = parse(&file).expect("tag parses behind id3");
        assert_eq!(gapless.delay, 576);
        assert_eq!(gapless.padding, 12);
    }

    #[test]
    fn rejects_implausible_and_absent_tags() {
        assert!(parse(&[]).is_none());
        assert!(parse(b"RIFF____WAVE").is_none());
        assert!(parse(&tagged_file(0x0fff, 0x0fff, false)).is_none());
        // Version string not from LAME: not a gapless source.
        let mut file = tagged_file(576, 12, false);
        let at = file.windows(4).position(|w| w == b"LAME").unwrap();
        file[at..at + 4].copy_from_slice(b"Xing");
        assert!(parse(&file).is_none());
    }

    #[test]
    fn rejects_files_truncated_inside_the_first_frame() {
        // Header + side info only, then every shorter prefix: the slice from
        // the side-info boundary is empty or too short for magic + flags.
        let file = tagged_file(576, 12, false);
        let boundary = 4 + 32;
        for end in 0..=(boundary + 8).min(file.len()) {
            assert!(
                parse(&file[..end]).is_none(),
                "prefix of {end} bytes parsed"
            );
        }
    }
}
