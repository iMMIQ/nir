//! Full lowering of all LiveNovel `TpWord` embedded narrative events into NIR v2 bytecode.
//!
//! Handles all 24 events observed in non-RPG LiveNovel games:
//! - Stage & CG: `CREATECG`, `CHANGECG`, `DELETECG` (static, solid `$RRGGBB`, `.lcm`/`.lmt` timelines, and `?var` dynamic paths)
//! - Audio: `PLAYSND`, `STOPSND`, `REPLAYSND`, `CHGVOL`, `PAUSEMEDIA`, `RESUMEMEDIA`
//! - Dialogue / Flow: `MESON`, `MESOFF`, `MESEND`, `PAUSE`, `WAIT`, `WAITPLAY`, `MENUENABLED`, `MENUDISABLED`
//! - Animation & Effects: `MOTION`, `QUAKE`, `QUAKESTOP`
//! - Decorations & Bitmap Text: `FACE`, `NAMELABEL`, `CGCHARNEW`, `CGCHARCHG`

use nir_ir::{
    AssetId, AudioBus, ChannelId, Easing, LayerId, Op, ShakeMode, ShakeSpec, StrId, TextAlign,
    TimelineId, Transform2D, Transition, TweenProp, TweenTarget, VarId, WipeDir,
};

/// Bridge supplied by the LiveNovel compiler so `events.rs` stays focused purely on event semantics.
pub trait EventBridge {
    fn stage_size(&self) -> [u32; 2];
    fn intern_str(&mut self, s: &str) -> StrId;
    fn resolve_var(&mut self, name: &str) -> VarId;
    fn resolve_layer(&mut self, name: &str) -> LayerId;
    fn resolve_channel(&mut self, name: &str) -> (ChannelId, AudioBus);
    /// Resolves a static image path (relative to root or `グラフィック/`) to an `AssetId`.
    fn register_image(&mut self, raw_path: &str) -> Option<AssetId>;
    /// Resolves a static audio path (relative to root or `サウンド/`) to an `AssetId`.
    fn register_audio(&mut self, raw_path: &str) -> Option<AssetId>;
    /// Resolves a static `.lcm`/`.lmt` timeline path to a `TimelineId`.
    fn register_timeline(&mut self, raw_path: &str) -> Option<TimelineId>;
    /// Returns the CG gallery unlock flag name if `asset` is listed in `CGモード.TXT`.
    fn cg_unlock_flag(&self, asset: AssetId) -> Option<StrId>;
    /// Notes that the game uses a dynamic `?var` image path via `var_name`.
    fn mark_dynamic_image_var(&mut self, var_name: &str);
    /// Notes that the game uses a dynamic `?var` audio path via `var_name`.
    fn mark_dynamic_audio_var(&mut self, var_name: &str);
    /// Records an exact conversion item.
    fn record_exact(&mut self);
    /// Records an adapted conversion item.
    fn record_adapted(&mut self, detail: String);
}

/// Decodes LiveMaker's `#ddd` CP932 decimal byte escape notation (e.g. `#131#112#131#088` -> `"パス"`).
pub fn decode_lm_escaped(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#'
            && i + 3 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let d = (bytes[i + 1] - b'0') as u16 * 100
                + (bytes[i + 2] - b'0') as u16 * 10
                + (bytes[i + 3] - b'0') as u16;
            if d <= 255 {
                out.push(d as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    if let Ok(s) = std::str::from_utf8(&out) {
        s.to_owned()
    } else {
        let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(&out);
        cow.into_owned()
    }
}

/// Parses a LiveNovel `TpWord` event string vector into `(event_name, args)`,
/// stripping optional leading `\x01` and trailing empty CRLF entry.
pub fn normalize_event_parts(raw_parts: &[String]) -> Option<(String, &[String])> {
    let head = raw_parts.first()?.trim_start_matches('\u{1}').trim();
    if head.is_empty() {
        return None;
    }
    let mut end = raw_parts.len();
    while end > 1 && raw_parts[end - 1].is_empty() {
        end -= 1;
    }
    Some((head.to_ascii_uppercase(), &raw_parts[1..end]))
}

/// Lowers one normalized LiveNovel event into `ops`.
/// Returns `true` if the event was an inline dialogue-box pause (`PAUSE`) rather than an op.
pub fn lower_event(
    name: &str,
    args: &[String],
    bridge: &mut dyn EventBridge,
    ops: &mut Vec<Op>,
) -> bool {
    let arg = |i: usize| -> &str { args.get(i).map(String::as_str).unwrap_or("") };

    match name {
        "PAUSE" => {
            bridge.record_exact();
            return true;
        }
        "CREATECG" => {
            lower_create_cg(args, bridge, ops);
        }
        "CHANGECG" => {
            lower_change_cg(args, bridge, ops);
        }
        "DELETECG" => {
            let target = arg(0);
            let wipe = arg(1).parse::<i32>().unwrap_or(0);
            let dur_ms = arg(2).parse::<u64>().unwrap_or(0).min(30_000);
            if !target.is_empty() {
                let lid = bridge.resolve_layer(target);
                ops.push(Op::LayerRemove { layer: lid });
                if dur_ms > 0 {
                    ops.push(Op::StageCommit {
                        transition: lm_wipe_to_transition(wipe),
                        duration_us: dur_ms * 1000,
                        wait: true,
                    });
                }
            }
            bridge.record_exact();
        }
        "PLAYSND" => {
            let src = arg(0);
            let ch_name = arg(1);
            let mode = arg(2);
            let vol = arg(3).parse::<f32>().unwrap_or(1000.0).clamp(0.0, 2000.0) / 1000.0;
            let fade_ms = arg(4).parse::<u64>().unwrap_or(0).min(30_000);
            let (channel, bus) = bridge.resolve_channel(ch_name);
            let looped = mode.eq_ignore_ascii_case("REPEAT");
            if let Some(var_raw) = src.strip_prefix('?') {
                let var_name = decode_lm_escaped(var_raw);
                bridge.mark_dynamic_audio_var(&var_name);
                let vid = bridge.resolve_var(&var_name);
                ops.push(Op::LoadVar(vid));
                ops.push(Op::ResolveAsset);
                ops.push(Op::AudioPlayDyn {
                    channel,
                    bus,
                    gain: vol,
                    looped,
                    fade_in_us: fade_ms * 1000,
                });
                bridge.record_exact();
            } else if let Some(asset) = bridge.register_audio(src) {
                ops.push(Op::AudioPlay {
                    channel,
                    bus,
                    asset,
                    gain: vol,
                    looped,
                    fade_in_us: fade_ms * 1000,
                    loop_region: None,
                });
                bridge.record_exact();
            } else if !src.is_empty() {
                bridge.record_adapted(format!("PLAYSND audio `{src}` not found in archive"));
            }
        }
        "REPLAYSND" => {
            // REPLAYSND: [0:channel, 1:src, 2:vol]
            let ch_name = arg(0);
            let src = arg(1);
            let vol = arg(2).parse::<f32>().unwrap_or(1000.0).clamp(0.0, 2000.0) / 1000.0;
            let (channel, bus) = bridge.resolve_channel(ch_name);
            if let Some(var_raw) = src.strip_prefix('?') {
                let var_name = decode_lm_escaped(var_raw);
                bridge.mark_dynamic_audio_var(&var_name);
                let vid = bridge.resolve_var(&var_name);
                ops.push(Op::LoadVar(vid));
                ops.push(Op::ResolveAsset);
                ops.push(Op::AudioPlayDyn {
                    channel,
                    bus,
                    gain: vol,
                    looped: false,
                    fade_in_us: 0,
                });
                bridge.record_exact();
            } else if let Some(asset) = bridge.register_audio(src) {
                ops.push(Op::AudioPlay {
                    channel,
                    bus,
                    asset,
                    gain: vol,
                    looped: false,
                    fade_in_us: 0,
                    loop_region: None,
                });
                bridge.record_exact();
            } else if !src.is_empty() {
                bridge.record_adapted(format!("REPLAYSND audio `{src}` not found in archive"));
            }
        }
        "STOPSND" => {
            let ch_name = arg(0);
            let fade_ms = arg(1).parse::<u64>().unwrap_or(0).min(30_000);
            let (channel, _) = bridge.resolve_channel(ch_name);
            ops.push(Op::AudioStop {
                channel,
                fade_out_us: fade_ms * 1000,
            });
            bridge.record_exact();
        }
        "CHGVOL" => {
            let ch_name = arg(0);
            let vol = arg(1).parse::<f32>().unwrap_or(1000.0).clamp(0.0, 2000.0) / 1000.0;
            let dur_ms = arg(2).parse::<u64>().unwrap_or(0).min(30_000);
            let wait = arg(3).eq_ignore_ascii_case("WAIT");
            let (channel, _) = bridge.resolve_channel(ch_name);
            ops.push(Op::AudioGain {
                channel,
                gain: vol,
                duration_us: dur_ms * 1000,
                wait,
            });
            bridge.record_exact();
        }
        "PAUSEMEDIA" | "RESUMEMEDIA" => {
            let paused = name == "PAUSEMEDIA";
            let (_, bus) = bridge.resolve_channel(arg(0));
            ops.push(Op::AudioPause { bus, paused });
            bridge.record_exact();
        }
        "MESON" => {
            let ms = arg(0).parse::<i64>().unwrap_or(0);
            let dur_us = if ms <= 0 { 0 } else { (ms as u64).min(10_000) * 1000 };
            ops.push(Op::TextWindow {
                visible: true,
                transition: if dur_us > 0 {
                    Transition::Dissolve
                } else {
                    Transition::Cut
                },
                duration_us: dur_us,
            });
            bridge.record_exact();
        }
        "MESOFF" => {
            let ms = arg(0).parse::<i64>().unwrap_or(0);
            let dur_us = if ms <= 0 { 0 } else { (ms as u64).min(10_000) * 1000 };
            ops.push(Op::TextWindow {
                visible: false,
                transition: if dur_us > 0 {
                    Transition::Dissolve
                } else {
                    Transition::Cut
                },
                duration_us: dur_us,
            });
            bridge.record_exact();
        }
        "MESEND" => {
            ops.push(Op::SetDialoguePortrait(None));
            ops.push(Op::TextWindow {
                visible: false,
                transition: Transition::Cut,
                duration_us: 0,
            });
            bridge.record_exact();
        }
        "WAIT" => {
            let ms = arg(0).parse::<u64>().unwrap_or(0).min(60_000);
            let skippable = !arg(2).eq_ignore_ascii_case("NOSKIP");
            if ms > 0 {
                ops.push(Op::Wait {
                    duration_us: ms * 1000,
                    skippable,
                });
            }
            bridge.record_exact();
        }
        "WAITPLAY" => {
            let target = arg(0);
            let click_skippable = arg(1).eq_ignore_ascii_case("CLICK");
            if !target.is_empty() {
                let lid = bridge.resolve_layer(target);
                ops.push(Op::WaitPlay {
                    layer: lid,
                    click_skippable,
                });
            }
            bridge.record_exact();
        }
        "MOTION" => {
            // [0:motion_name, 1:target, 2:prop(3=X,4=Y,11=Opacity), 3:to, 4:dur_ms, 5:curve, 6:PASS/WAIT]
            let target = arg(1);
            let prop_id = arg(2);
            let raw_to = arg(3).parse::<f32>().unwrap_or(0.0);
            let dur_ms = arg(4).parse::<u64>().unwrap_or(0).min(60_000);
            let easing = match arg(5).to_ascii_uppercase().as_str() {
                "INC" => Easing::EaseIn,
                "DEC" => Easing::EaseOut,
                _ => Easing::Linear,
            };
            let wait = arg(6).eq_ignore_ascii_case("WAIT");
            let prop_and_val = match prop_id {
                "3" => Some((TweenProp::X, raw_to)),
                "4" => Some((TweenProp::Y, raw_to)),
                "11" => Some((TweenProp::Opacity, (raw_to / 255.0).clamp(0.0, 1.0))),
                _ => None,
            };
            if let Some((prop, to)) = prop_and_val {
                let lid = bridge.resolve_layer(target);
                ops.push(Op::Tween {
                    target: TweenTarget::Layer(lid),
                    prop,
                    to,
                    duration_us: dur_ms * 1000,
                    easing,
                    wait,
                });
                bridge.record_exact();
            } else {
                bridge.record_adapted(format!("MOTION unsupported property `{prop_id}`"));
            }
        }
        "QUAKE" => {
            // [0:target_list, 1:mode(QUAKE/WAVE/BOUND), 2:random(0/1), 3:amp_x, 4:amp_y, 5:dur_ms, 6:period_ms]
            let targets_raw = arg(0);
            let mode = match arg(1).to_ascii_uppercase().as_str() {
                "WAVE" => ShakeMode::Wave,
                "BOUND" => ShakeMode::Bound,
                _ => ShakeMode::Quake,
            };
            let amp_x = arg(3).parse::<f32>().unwrap_or(8.0);
            let amp_y = arg(4).parse::<f32>().unwrap_or(8.0);
            let dur_ms = arg(5).parse::<u64>().unwrap_or(400).min(30_000);
            let period_ms = arg(6).parse::<f32>().unwrap_or(80.0).max(16.0);
            let freq_hz = (1000.0 / period_ms).clamp(1.0, 60.0);
            let spec = ShakeSpec {
                amplitude: [amp_x, amp_y],
                freq_hz,
                duration_us: dur_ms * 1000,
                decay: false,
                mode,
            };
            let targets: Vec<&str> = targets_raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            for (idx, t) in targets.iter().enumerate() {
                let target = if *t == "メッセージボックス" || *t == "メッセージボックス土台" {
                    TweenTarget::DialogueBox
                } else {
                    TweenTarget::Layer(bridge.resolve_layer(t))
                };
                let is_last = idx + 1 == targets.len();
                ops.push(Op::Shake {
                    target,
                    spec,
                    wait: is_last && dur_ms > 0,
                });
            }
            bridge.record_exact();
        }
        "QUAKESTOP" => {
            ops.push(Op::ShakeStop {
                target: TweenTarget::DialogueBox,
            });
            bridge.record_exact();
        }
        "FACE" => {
            let src = arg(0).trim();
            if src.is_empty() {
                ops.push(Op::SetDialoguePortrait(None));
                bridge.record_exact();
            } else if let Some(var_raw) = src.strip_prefix('?') {
                let var_name = decode_lm_escaped(var_raw);
                bridge.mark_dynamic_image_var(&var_name);
                let vid = bridge.resolve_var(&var_name);
                ops.push(Op::LoadVar(vid));
                ops.push(Op::ResolveAsset);
                ops.push(Op::SetDialoguePortraitDyn);
                bridge.record_exact();
            } else if let Some(asset) = bridge.register_image(src) {
                ops.push(Op::SetDialoguePortrait(Some(asset)));
                bridge.record_exact();
            } else {
                bridge.record_adapted(format!("FACE portrait `{src}` not found"));
            }
        }
        "CGCHARNEW" => {
            // [0:name, 1:parent, 2:x, 3:y, 4:z, 5:halign, 6:valign, 7:text_or_var, 8:is_var, ...]
            let layer_name = arg(0);
            let parent_raw = arg(1);
            let x = arg(2).parse::<f32>().unwrap_or(0.0);
            let y = arg(3).parse::<f32>().unwrap_or(0.0);
            let z = arg(4).parse::<i32>().unwrap_or(100);
            let align = match arg(5).to_ascii_uppercase().as_str() {
                "C" | "1" => TextAlign::Center,
                "R" | "2" => TextAlign::Right,
                _ => TextAlign::Left,
            };
            let raw_text = arg(7);
            let is_var = arg(8) == "1";
            let layer = bridge.resolve_layer(layer_name);
            let parent = if parent_raw.is_empty() || parent_raw == "#1" {
                None
            } else {
                Some(bridge.resolve_layer(parent_raw))
            };
            let tf = Transform2D {
                x,
                y,
                z_order: z,
                ..Transform2D::default()
            };
            if is_var {
                let var_name = decode_lm_escaped(raw_text);
                let vid = bridge.resolve_var(&var_name);
                ops.push(Op::LoadVar(vid));
                ops.push(Op::LayerSetText {
                    layer,
                    parent,
                    transform: Some(Box::new(tf)),
                    text: None,
                    font_size: 28.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align,
                });
            } else {
                let decoded = decode_lm_escaped(raw_text);
                let sid = bridge.intern_str(&decoded);
                ops.push(Op::LayerSetText {
                    layer,
                    parent,
                    transform: Some(Box::new(tf)),
                    text: Some(sid),
                    font_size: 28.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align,
                });
            }
            bridge.record_exact();
        }
        "CGCHARCHG" => {
            // [0:name, 1:text_or_var, 2:is_var]
            let layer_name = arg(0);
            let raw_text = arg(1);
            let is_var = arg(2) == "1";
            let layer = bridge.resolve_layer(layer_name);
            if is_var {
                let var_name = decode_lm_escaped(raw_text);
                let vid = bridge.resolve_var(&var_name);
                ops.push(Op::LoadVar(vid));
                ops.push(Op::LayerSetText {
                    layer,
                    parent: None,
                    transform: None,
                    text: None,
                    font_size: 28.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Left,
                });
            } else {
                let decoded = decode_lm_escaped(raw_text);
                let sid = bridge.intern_str(&decoded);
                ops.push(Op::LayerSetText {
                    layer,
                    parent: None,
                    transform: None,
                    text: Some(sid),
                    font_size: 28.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Left,
                });
            }
            bridge.record_exact();
        }
        "MENUENABLED" | "MENUDISABLED" | "NAMELABEL" => {
            // NIR runtime manages right-click menu cleanly; NAMELABEL handled by speaker extraction.
            bridge.record_adapted(format!("event `{name}` managed by NIR runtime HUD"));
        }
        other => {
            bridge.record_adapted(format!("unhandled TpWord event `{other}` omitted"));
        }
    }
    false
}

fn lower_create_cg(args: &[String], bridge: &mut dyn EventBridge, ops: &mut Vec<Op>) {
    let arg = |i: usize| -> &str { args.get(i).map(String::as_str).unwrap_or("") };
    let layer_name = arg(0);
    let src = arg(1);
    let mode = arg(2);
    let parent_raw = arg(3);
    let x_raw = arg(4);
    let y_raw = arg(5);
    let z = arg(6).parse::<i32>().unwrap_or(0);
    let wipe = arg(7).parse::<i32>().unwrap_or(0);
    let dur_ms = arg(8).parse::<u64>().unwrap_or(0).min(30_000);

    let layer = bridge.resolve_layer(layer_name);
    let parent = if parent_raw.is_empty() || parent_raw == "#1" {
        None
    } else {
        Some(bridge.resolve_layer(parent_raw))
    };
    let stage = bridge.stage_size();
    let (x, ax) = parse_axis(x_raw, stage[0] as f32, true);
    let (y, ay) = parse_axis(y_raw, stage[1] as f32, false);

    let mut tf = Transform2D {
        x,
        y,
        anchor: [ax, ay],
        z_order: z,
        ..Transform2D::default()
    };

    if let Some(var_raw) = src.strip_prefix('?') {
        let var_name = decode_lm_escaped(var_raw);
        bridge.mark_dynamic_image_var(&var_name);
        let vid = bridge.resolve_var(&var_name);
        ops.push(Op::LoadVar(vid));
        ops.push(Op::ResolveAsset);
        ops.push(Op::LayerSetDyn {
            layer,
            parent,
            transform: Box::new(tf),
        });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
        return;
    }

    if let Some(rgba) = parse_lm_solid_color(src) {
        tf.color = rgba;
        tf.width = stage[0] as f32;
        tf.height = stage[1] as f32;
        tf.anchor = [0.0, 0.0];
        tf.x = 0.0;
        tf.y = 0.0;
        ops.push(Op::LayerSet {
            layer,
            parent,
            asset: None,
            transform: Box::new(tf),
        });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
        return;
    }

    let low = src.to_ascii_lowercase();
    if low.ends_with(".lcm") || low.ends_with(".lmt") {
        if let Some(tl_id) = bridge.register_timeline(src) {
            // Host layer stores base position/z-order for timeline clips
            tf.opacity = 1.0;
            tf.color = [1.0, 1.0, 1.0, 0.0]; // invisible container quad
            tf.width = 1.0;
            tf.height = 1.0;
            ops.push(Op::LayerSet {
                layer,
                parent,
                asset: None,
                transform: Box::new(tf),
            });
            ops.push(Op::TimelinePlay {
                timeline: tl_id,
                layer,
                scrap_on_end: mode.eq_ignore_ascii_case("SCRAP"),
                wait: mode.eq_ignore_ascii_case("WAIT"),
                click_skippable: false,
            });
            bridge.record_exact();
        } else {
            bridge.record_adapted(format!("CREATECG timeline `{src}` not found/unsupported"));
        }
        return;
    }

    if let Some(asset) = bridge.register_image(src) {
        if let Some(flag) = bridge.cg_unlock_flag(asset) {
            ops.push(Op::ProfileSetFlag(flag));
        }
        ops.push(Op::LayerSet {
            layer,
            parent,
            asset: Some(asset),
            transform: Box::new(tf),
        });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
    } else if !src.is_empty() {
        bridge.record_adapted(format!("CREATECG image `{src}` not found"));
    }
}

fn lower_change_cg(args: &[String], bridge: &mut dyn EventBridge, ops: &mut Vec<Op>) {
    let arg = |i: usize| -> &str { args.get(i).map(String::as_str).unwrap_or("") };
    let layer_name = arg(0);
    let src = arg(1);
    let mode = arg(2);
    let wipe = arg(3).parse::<i32>().unwrap_or(0);
    let dur_ms = arg(4).parse::<u64>().unwrap_or(0).min(30_000);
    let layer = bridge.resolve_layer(layer_name);

    if let Some(var_raw) = src.strip_prefix('?') {
        let var_name = decode_lm_escaped(var_raw);
        bridge.mark_dynamic_image_var(&var_name);
        let vid = bridge.resolve_var(&var_name);
        ops.push(Op::LoadVar(vid));
        ops.push(Op::ResolveAsset);
        ops.push(Op::LayerAssetDyn { layer });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
        return;
    }

    if let Some(rgba) = parse_lm_solid_color(src) {
        let stage = bridge.stage_size();
        ops.push(Op::LayerSet {
            layer,
            parent: None,
            asset: None,
            transform: Box::new(Transform2D {
                width: stage[0] as f32,
                height: stage[1] as f32,
                color: rgba,
                ..Transform2D::default()
            }),
        });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
        return;
    }

    let low = src.to_ascii_lowercase();
    if low.ends_with(".lcm") || low.ends_with(".lmt") {
        if let Some(tl_id) = bridge.register_timeline(src) {
            ops.push(Op::TimelinePlay {
                timeline: tl_id,
                layer,
                scrap_on_end: mode.eq_ignore_ascii_case("SCRAP"),
                wait: mode.eq_ignore_ascii_case("WAIT"),
                click_skippable: false,
            });
            bridge.record_exact();
        } else {
            bridge.record_adapted(format!("CHANGECG timeline `{src}` not found/unsupported"));
        }
        return;
    }

    if let Some(asset) = bridge.register_image(src) {
        if let Some(flag) = bridge.cg_unlock_flag(asset) {
            ops.push(Op::ProfileSetFlag(flag));
        }
        ops.push(Op::LayerAsset {
            layer,
            asset: Some(asset),
        });
        if dur_ms > 0 {
            ops.push(Op::StageCommit {
                transition: lm_wipe_to_transition(wipe),
                duration_us: dur_ms * 1000,
                wait: true,
            });
        }
        bridge.record_exact();
    } else if !src.is_empty() {
        bridge.record_adapted(format!("CHANGECG image `{src}` not found"));
    }
}

fn parse_axis(token: &str, extent: f32, horizontal: bool) -> (f32, f32) {
    match token.trim().to_ascii_uppercase().as_str() {
        "L" if horizontal => (0.0, 0.0),
        "C" => (extent * 0.5, 0.5),
        "R" if horizontal => (extent, 1.0),
        "T" if !horizontal => (0.0, 0.0),
        "B" if !horizontal => (extent, 1.0),
        other => (other.parse::<f32>().unwrap_or(0.0), 0.0),
    }
}

fn parse_lm_solid_color(s: &str) -> Option<[f32; 4]> {
    let hex = s.trim().strip_prefix('$').or_else(|| s.trim().strip_prefix('#'))?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
    Some([r, g, b, 1.0])
}

fn lm_wipe_to_transition(wipe: i32) -> Transition {
    match wipe {
        0 => Transition::Dissolve, // LiveNovel wipe=0 with dur>0 is standard crossfade dissolve
        1 => Transition::Dissolve,
        2 => Transition::Wipe {
            dir: WipeDir::LeftToRight,
            softness: 0.15,
        },
        3 => Transition::Wipe {
            dir: WipeDir::RightToLeft,
            softness: 0.15,
        },
        4 => Transition::Wipe {
            dir: WipeDir::TopToBottom,
            softness: 0.15,
        },
        5 => Transition::Wipe {
            dir: WipeDir::BottomToTop,
            softness: 0.15,
        },
        _ => Transition::Dissolve,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_lm_cp932_decimal_escapes() {
        // #131#112#131#088 = 0x83 0x70 0x83 0x58 = "パス" in CP932
        assert_eq!(decode_lm_escaped("#131#112#131#088"), "パス");
        assert_eq!(decode_lm_escaped("plain_var_1"), "plain_var_1");
    }
}
