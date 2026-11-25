//! Spec-compliant RFC 7845 Ogg Opus encoder and zero-copy header prober
//! backed by official Xiph `libopus 1.5` + `libopusenc` (100% safe Rust API).

use anyhow::{bail, Result};
use libopusenc::{
    OpusEncApplication, OpusEncBandwidth, OpusEncBitrate, OpusEncChannelMapping, OpusEncComments,
    OpusEncSampleRate, OpusEncSignalBias, OpusEncoder,
};

const OPUS_RATE: u32 = 48_000;
const OGG_SERIAL: u32 = 0x4E49_5232; // "NIR2"

/// High-quality Catmull-Rom cubic Hermite resampler to Opus native 48 kHz.
fn resample_pcm16_to_48k(samples: &[i16], src_rate: u32, channels: u16) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    if src_rate == OPUS_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let src_frames = samples.len() / ch;
    if src_frames < 2 {
        return samples.to_vec();
    }
    let dst_frames = ((src_frames as u128 * OPUS_RATE as u128) / src_rate.max(1) as u128) as usize;
    let mut out = Vec::with_capacity(dst_frames.saturating_mul(ch));
    let ratio = src_rate as f64 / OPUS_RATE as f64;

    let sample_at = |frame: isize, c: usize| -> f32 {
        let clamped = frame.clamp(0, (src_frames - 1) as isize) as usize;
        samples[clamped * ch + c] as f32
    };

    for i in 0..dst_frames {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as isize;
        let t = (pos - idx as f64) as f32;
        let t2 = t * t;
        let t3 = t2 * t;
        for c in 0..ch {
            let y0 = sample_at(idx - 1, c);
            let y1 = sample_at(idx, c);
            let y2 = sample_at(idx + 1, c);
            let y3 = sample_at(idx + 2, c);
            let a0 = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let a1 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let a2 = -0.5 * y0 + 0.5 * y2;
            let a3 = y1;
            let v = ((a0 * t3 + a1 * t2 + a2 * t + a3).round()).clamp(-32768.0, 32767.0) as i16;
            out.push(v);
        }
    }
    out
}

/// Encodes interleaved 16-bit PCM (1 or 2 channels, any standard rate) into an RFC 7845 Ogg Opus stream.
/// Uses `mono_kbps` (default 40) for mono speech and `stereo_kbps` (default 96) for stereo music/SFX.
pub fn encode_ogg_opus(
    samples: &[i16],
    src_rate: u32,
    channels: u16,
    mono_kbps: u16,
    stereo_kbps: u16,
) -> Result<Vec<u8>> {
    encode_ogg_opus_with_bias(
        samples,
        src_rate,
        channels,
        mono_kbps,
        stereo_kbps,
        channels >= 2,
    )
}

/// Encodes interleaved 16-bit PCM into an RFC 7845 Ogg Opus stream with explicit speech vs music
/// psychoacoustic signal bias and maximum libopus complexity (`10`) for perceptual transparency.
pub fn encode_ogg_opus_with_bias(
    samples: &[i16],
    src_rate: u32,
    channels: u16,
    mono_kbps: u16,
    stereo_kbps: u16,
    prefer_music: bool,
) -> Result<Vec<u8>> {
    if !(1..=2).contains(&channels) {
        bail!("Opus encoder supports 1 or 2 channels, got {channels}");
    }
    if samples.is_empty() {
        bail!("cannot encode empty PCM buffer to Opus");
    }

    let (pcm_owned, rate_enum) = match src_rate {
        8_000 => (None, OpusEncSampleRate::Hz8000),
        12_000 => (None, OpusEncSampleRate::Hz12000),
        16_000 => (None, OpusEncSampleRate::Hz16000),
        24_000 => (None, OpusEncSampleRate::Hz24000),
        48_000 => (None, OpusEncSampleRate::Hz48000),
        _ => (
            Some(resample_pcm16_to_48k(samples, src_rate, channels)),
            OpusEncSampleRate::Hz48000,
        ),
    };
    let pcm: &[i16] = pcm_owned.as_deref().unwrap_or(samples);

    let target_bps: u32 = if channels == 1 {
        (mono_kbps.clamp(16, 128) as u32) * 1000
    } else {
        (stereo_kbps.clamp(32, 256) as u32) * 1000
    };

    let mut comments =
        OpusEncComments::create().map_err(|e| anyhow::anyhow!("libopusenc comments: {e}"))?;
    let mut enc = OpusEncoder::create_pull(
        &mut comments,
        rate_enum,
        channels as u8,
        OpusEncChannelMapping::MonoStereo,
    )
    .map_err(|e| anyhow::anyhow!("libopusenc init: {e}"))?;

    enc.set_serial_no(OGG_SERIAL)
        .and_then(|e| e.set_comment_padding(0))
        .and_then(|e| e.set_vbr(true))
        .and_then(|e| e.set_complexity(10))
        .and_then(|e| e.set_application(OpusEncApplication::Audio))
        .and_then(|e| {
            e.set_signal_bias(if prefer_music {
                OpusEncSignalBias::MDCT
            } else {
                OpusEncSignalBias::LpcHybrid
            })
        })
        .and_then(|e| e.set_bandwidth(OpusEncBandwidth::Fullband20kHz))
        .and_then(|e| e.set_bitrate(OpusEncBitrate::Explicit(target_bps)))
        .map_err(|e| anyhow::anyhow!("libopusenc ctl: {e}"))?;

    let mut out = Vec::with_capacity(pcm.len() / 8 + 512);
    let chunk_samples = 48_000 * channels as usize;
    for chunk in pcm.chunks(chunk_samples) {
        enc.write(chunk, channels as usize)
            .map_err(|e| anyhow::anyhow!("libopusenc write: {e}"))?;
        while let Some(page) = enc.get_page(false) {
            out.extend_from_slice(page);
        }
    }
    enc.drain()
        .map_err(|e| anyhow::anyhow!("libopusenc drain: {e}"))?;
    while let Some(page) = enc.get_page(true) {
        out.extend_from_slice(page);
    }

    if out.is_empty() {
        bail!("libopusenc produced empty output");
    }
    Ok(out)
}

/// Zero-copy metadata probe for Ogg Opus (`OpusHead` + last page granule position).
/// Returns `(48000, channels, duration_us)`.
pub fn probe_ogg_opus_meta(bytes: &[u8]) -> Option<(u32, u16, u64)> {
    let idx = bytes.windows(8).position(|w| w == b"OpusHead")?;
    if idx + 19 > bytes.len() {
        return None;
    }
    let channels = bytes[idx + 9] as u16;
    let pre_skip = u16::from_le_bytes([bytes[idx + 10], bytes[idx + 11]]) as u64;

    let mut last_granule = 0u64;
    let search_start = bytes.len().saturating_sub(65_536);
    if let Some(rel) = bytes[search_start..].windows(4).rposition(|w| w == b"OggS") {
        let off = search_start + rel;
        if off + 14 <= bytes.len() {
            let g = u64::from_le_bytes(bytes[off + 6..off + 14].try_into().ok()?);
            if g != u64::MAX {
                last_granule = g;
            }
        }
    }
    let effective_samples = last_granule.saturating_sub(pre_skip);
    let duration_us = effective_samples.saturating_mul(1_000_000) / OPUS_RATE as u64;
    Some((OPUS_RATE, channels.max(1), duration_us))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogg::reading::PacketReader;
    use std::io::Cursor;

    #[test]
    fn ogg_opus_encodes_valid_rfc7845_stream_and_probes_duration() {
        let sr = 44_100u32;
        let len = (sr / 4) as usize;
        let pcm: Vec<i16> = (0..len)
            .map(|i| {
                let t = i as f32 / sr as f32;
                ((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 16000.0) as i16
            })
            .collect();

        let ogg_opus = encode_ogg_opus(&pcm, sr, 1, 48, 96).unwrap();
        assert!(ogg_opus.len() < 3_000, "opus len={}", ogg_opus.len());

        let (rate, ch, dur_us) = probe_ogg_opus_meta(&ogg_opus).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(ch, 1);
        assert!((230_000..=270_000).contains(&dur_us), "dur_us={dur_us}");

        let mut reader = PacketReader::new(Cursor::new(&ogg_opus));
        let head = reader.read_packet().unwrap().unwrap();
        assert!(head.data.starts_with(b"OpusHead"));
        let tags = reader.read_packet().unwrap().unwrap();
        assert!(tags.data.starts_with(b"OpusTags"));
        let mut audio_pkts = 0usize;
        while let Some(_pkt) = reader.read_packet().unwrap() {
            audio_pkts += 1;
        }
        assert!(audio_pkts >= 10, "audio_pkts={audio_pkts}");
    }
}
