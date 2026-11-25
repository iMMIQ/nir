//! Lean Modern Web App WASM host for NIR v2 (`player-web`).
//!
//! - Zero whole-`.nirpak` copy into WASM linear memory: WASM only holds the compact `NirProgram` index!
//! - JS host serves asset byte ranges via HTTP Range streaming OR zero-copy `Uint8Array.subarray()`
//! - WebGPU-first with automatic WebGL2 fallback + `createImageBitmap` zero-copy GPU upload
//! - True idle-pausing `requestAnimationFrame` (`needs_frame()`) + `urgent` priority hints + audio lookahead
#![forbid(unsafe_code)]

use nir_ir::{AssetId, AssetKind, AudioBus, AudioLoopRegion, MediaCodec, Preferences};
use nir_runtime::HostCommand;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct JsDeltaDesc {
    pub base_id: u32,
    pub base_mime: &'static str,
    pub base_offset: u64,
    pub base_size: u64,
    pub x: u32,
    pub y: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tile_mask: Vec<u8>,
}

/// Compact JSON-serializable command bridge sent from `WebEngine` to `host/app.js`.
/// Includes exact `(offset, size)` inside the `.nirpak` media section so JS can slice or Range-fetch directly!
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum JsHostCommand {
    DecodeImage {
        id: u32,
        mime: &'static str,
        offset: u64,
        size: u64,
        urgent: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        delta: Option<JsDeltaDesc>,
    },
    LoadFont {
        id: u32,
        offset: u64,
        size: u64,
    },
    PreloadAudio {
        asset: u32,
        mime: &'static str,
        offset: u64,
        size: u64,
    },
    PlayAudio {
        channel: u32,
        bus: &'static str,
        asset: u32,
        mime: &'static str,
        offset: u64,
        size: u64,
        gain: f32,
        looped: bool,
        fade_in_ms: f64,
        loop_start_s: Option<f64>,
        loop_end_s: Option<f64>,
    },
    StopAudio {
        channel: u32,
        fade_out_ms: f64,
    },
    SetAudioGain {
        channel: u32,
        gain: f32,
    },
    PauseAudioBus {
        bus: &'static str,
        paused: bool,
    },
    PlayVideo {
        asset: u32,
        mime: &'static str,
        offset: u64,
        size: u64,
        skippable: bool,
    },
    StopVideo,
    SaveSlot {
        slot: u32,
        snapshot_json: String,
    },
    RequestLoadSlot {
        slot: u32,
    },
    DeleteSlot {
        slot: u32,
    },
    SaveAuto {
        snapshot_json: String,
    },
    RequestLoadAuto,
    SaveQuick {
        snapshot_json: String,
    },
    RequestLoadQuick,
    SaveProfile {
        profile_json: String,
    },
    SavePreferences {
        prefs: Preferences,
    },
    ReturnToShelf,
}

pub fn codec_mime(codec: MediaCodec) -> &'static str {
    match codec {
        MediaCodec::Webp => "image/webp",
        MediaCodec::Png => "image/png",
        MediaCodec::Jpeg => "image/jpeg",
        MediaCodec::Ktx2 => "image/ktx2",
        MediaCodec::Mp3 => "audio/mpeg",
        MediaCodec::Ogg => "audio/ogg",
        MediaCodec::Opus => "audio/ogg; codecs=opus",
        MediaCodec::Wav => "audio/wav",
        MediaCodec::Flac => "audio/flac",
        MediaCodec::Webm => "video/webm",
        MediaCodec::Mp4 => "video/mp4",
        MediaCodec::Ttf => "font/ttf",
        MediaCodec::Otf => "font/otf",
        MediaCodec::Raw => "application/octet-stream",
    }
}

pub fn bus_name(bus: AudioBus) -> &'static str {
    match bus {
        AudioBus::Bgm => "bgm",
        AudioBus::Voice => "voice",
        AudioBus::Sfx => "sfx",
    }
}

#[allow(dead_code)]
fn convert_host_command(
    cmd: HostCommand,
    program: &nir_ir::NirProgram,
    pinned_out: &mut Option<std::collections::BTreeSet<AssetId>>,
) -> Option<JsHostCommand> {
    match cmd {
        HostCommand::LoadAsset { id, urgent } => {
            let meta = program.asset(id)?;
            match meta.kind {
                AssetKind::Image => {
                    let delta = meta.delta.as_ref().and_then(|d| {
                        let base_meta = program.asset(d.base)?;
                        Some(JsDeltaDesc {
                            base_id: d.base.0,
                            base_mime: codec_mime(base_meta.codec),
                            base_offset: base_meta.offset,
                            base_size: base_meta.size,
                            x: d.x,
                            y: d.y,
                            tile_mask: d.tile_mask.clone(),
                        })
                    });
                    Some(JsHostCommand::DecodeImage {
                        id: id.0,
                        mime: codec_mime(meta.codec),
                        offset: meta.offset,
                        size: meta.size,
                        urgent,
                        delta,
                    })
                }
                AssetKind::Font => Some(JsHostCommand::LoadFont {
                    id: id.0,
                    offset: meta.offset,
                    size: meta.size,
                }),
                AssetKind::Audio => Some(JsHostCommand::PreloadAudio {
                    asset: id.0,
                    mime: codec_mime(meta.codec),
                    offset: meta.offset,
                    size: meta.size,
                }),
                AssetKind::Video | AssetKind::Raw => None,
            }
        }
        HostCommand::HintPinnedAssets(pinned) => {
            *pinned_out = Some(pinned);
            None
        }
        HostCommand::PlayAudio {
            channel,
            bus,
            asset,
            gain,
            looped,
            fade_in_us,
            loop_region,
            ..
        } => {
            let meta = program.asset(asset)?;
            let (loop_start_s, loop_end_s) = match loop_region {
                Some(AudioLoopRegion { start_us, end_us }) => {
                    (Some(start_us as f64 / 1e6), Some(end_us as f64 / 1e6))
                }
                None => (None, None),
            };
            Some(JsHostCommand::PlayAudio {
                channel: channel.0,
                bus: bus_name(bus),
                asset: asset.0,
                mime: codec_mime(meta.codec),
                offset: meta.offset,
                size: meta.size,
                gain,
                looped,
                fade_in_ms: fade_in_us as f64 / 1000.0,
                loop_start_s,
                loop_end_s,
            })
        }
        HostCommand::StopAudio {
            channel,
            fade_out_us,
        } => Some(JsHostCommand::StopAudio {
            channel: channel.0,
            fade_out_ms: fade_out_us as f64 / 1000.0,
        }),
        HostCommand::SetAudioGain { channel, gain } => Some(JsHostCommand::SetAudioGain {
            channel: channel.0,
            gain,
        }),
        HostCommand::PauseAudioBus { bus, paused } => Some(JsHostCommand::PauseAudioBus {
            bus: bus_name(bus),
            paused,
        }),
        HostCommand::PlayVideo { asset, skippable } => {
            let meta = program.asset(asset)?;
            Some(JsHostCommand::PlayVideo {
                asset: asset.0,
                mime: codec_mime(meta.codec),
                offset: meta.offset,
                size: meta.size,
                skippable,
            })
        }
        HostCommand::StopVideo => Some(JsHostCommand::StopVideo),
        HostCommand::SaveSlot { slot, snapshot } => Some(JsHostCommand::SaveSlot {
            slot,
            snapshot_json: serde_json::to_string(&snapshot).unwrap_or_default(),
        }),
        HostCommand::RequestLoadSlot(slot) => Some(JsHostCommand::RequestLoadSlot { slot }),
        HostCommand::DeleteSlot(slot) => Some(JsHostCommand::DeleteSlot { slot }),
        HostCommand::SaveAuto { snapshot } => Some(JsHostCommand::SaveAuto {
            snapshot_json: serde_json::to_string(&snapshot).unwrap_or_default(),
        }),
        HostCommand::RequestLoadAuto => Some(JsHostCommand::RequestLoadAuto),
        HostCommand::SaveQuick { snapshot } => Some(JsHostCommand::SaveQuick {
            snapshot_json: serde_json::to_string(&snapshot).unwrap_or_default(),
        }),
        HostCommand::RequestLoadQuick => Some(JsHostCommand::RequestLoadQuick),
        HostCommand::SaveProfile(profile) => Some(JsHostCommand::SaveProfile {
            profile_json: serde_json::to_string(&profile).unwrap_or_default(),
        }),
        HostCommand::SavePreferences(prefs) => Some(JsHostCommand::SavePreferences { prefs }),
        HostCommand::ReturnToShelf => Some(JsHostCommand::ReturnToShelf),
        HostCommand::Quit => None,
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_host {
    use super::*;
    use js_sys::Uint8Array;
    use nir_ir::{decode_nirpak_header_only, ChannelId, UiAction};
    use nir_render::{wgpu, Renderer, RendererBackend};
    use nir_runtime::Runtime;
    use nir_ui::{InputMode, SafeInsets, SlotSummary};
    use nir_vm::{ProfileData, SaveSnapshot};
    use std::sync::Arc;
    use wasm_bindgen::prelude::*;
    use web_sys::{HtmlCanvasElement, ImageBitmap};

    #[derive(Serialize)]
    struct FontAssetDesc {
        id: u32,
        offset: u64,
        size: u64,
    }

    #[wasm_bindgen]
    pub struct WebEngine {
        media_base: usize,
        runtime: Runtime,
        renderer: Renderer,
        logical_width: f32,
        logical_height: f32,
        dpr: f32,
    }

    #[wasm_bindgen]
    impl WebEngine {
        /// Initializes the engine from ONLY the `.nirpak` header+program prefix (typically ~50 KiB),
        /// never copying the multi-hundred-MB media payload into WASM linear memory!
        #[wasm_bindgen]
        pub async fn create(
            canvas: HtmlCanvasElement,
            nirpak_header_bytes: Uint8Array,
            prefs_json: &str,
            profile_json: &str,
            slots_json: &str,
            dpr: f32,
        ) -> Result<WebEngine, JsValue> {
            let header_bytes = nirpak_header_bytes.to_vec();
            let (program, media_base, _media_len) = decode_nirpak_header_only(&header_bytes)
                .map_err(|e| JsValue::from_str(&format!("invalid game.nirpak: {e}")))?;
            let program = Arc::new(program);

            let prefs: Preferences = serde_json::from_str(prefs_json).unwrap_or_default();
            let profile: ProfileData = serde_json::from_str(profile_json).unwrap_or_default();
            let slots: Vec<SlotSummary> = serde_json::from_str(slots_json).unwrap_or_default();

            let width = canvas.width().max(1);
            let height = canvas.height().max(1);
            let dpr = dpr.clamp(0.5, 4.0);

            // Try WebGPU first, then gracefully fall back to WebGL2!
            let renderer = create_web_renderer(canvas, width, height)
                .await
                .map_err(|e| JsValue::from_str(&e))?;

            let runtime = Runtime::new(program, prefs, profile, slots)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;

            Ok(WebEngine {
                media_base,
                runtime,
                renderer,
                logical_width: (width as f32 / dpr).max(1.0),
                logical_height: (height as f32 / dpr).max(1.0),
                dpr,
            })
        }

        #[wasm_bindgen]
        pub fn title(&self) -> String {
            self.runtime.program().meta.title.clone()
        }

        #[wasm_bindgen]
        pub fn game_id(&self) -> String {
            self.runtime.program().meta.game_id.clone()
        }

        #[wasm_bindgen]
        pub fn media_base(&self) -> f64 {
            self.media_base as f64
        }

        #[wasm_bindgen]
        pub fn backend_info(&self) -> String {
            self.renderer.adapter_info().to_owned()
        }

        /// Returns whether the engine needs another `requestAnimationFrame` tick (false = idle sleep!).
        #[wasm_bindgen]
        pub fn needs_frame(&self) -> bool {
            self.runtime.dirty || self.runtime.needs_clock()
        }

        /// Lists all bundled font asset byte ranges so JS can stream/slice and load them once.
        #[wasm_bindgen]
        pub fn font_assets_json(&self) -> String {
            let list: Vec<FontAssetDesc> = self
                .runtime
                .program()
                .assets
                .iter()
                .filter(|a| a.kind == AssetKind::Font)
                .map(|a| FontAssetDesc {
                    id: a.id.0,
                    offset: a.offset,
                    size: a.size,
                })
                .collect();
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
        }

        #[wasm_bindgen]
        pub fn load_font(&mut self, asset_id: u32, bytes: Uint8Array) -> Result<(), JsValue> {
            let id = AssetId(asset_id);
            if !self.renderer.has_font(id) {
                self.renderer
                    .load_font(id, bytes.to_vec())
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        /// Loads a shared fallback font (e.g. Web Pan-CJK fallback `.otf`/`.ttf`) used whenever
        /// a `.nirpak` ships no bundled font or lacks CJK/UI/symbol glyphs.
        #[wasm_bindgen]
        pub fn load_fallback_font(&mut self, bytes: Uint8Array) -> Result<(), JsValue> {
            self.renderer
                .load_fallback_font(bytes.to_vec())
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            self.runtime.mark_fonts_changed();
            Ok(())
        }

        #[wasm_bindgen]
        pub fn upload_image_bitmap(
            &mut self,
            asset_id: u32,
            bitmap: &ImageBitmap,
        ) -> Result<(), JsValue> {
            let id = AssetId(asset_id);
            self.renderer
                .upload_image_bitmap(id, bitmap)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let evicted = self.renderer.take_evicted_assets();
            if !evicted.is_empty() {
                self.runtime.mark_assets_evicted(&evicted);
            }
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        #[wasm_bindgen]
        pub fn upload_ktx2_bytes(
            &mut self,
            asset_id: u32,
            bytes: Uint8Array,
        ) -> Result<(), JsValue> {
            let id = AssetId(asset_id);
            let raw = bytes.to_vec();
            self.renderer
                .upload_ktx2_bytes(id, &raw)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let evicted = self.renderer.take_evicted_assets();
            if !evicted.is_empty() {
                self.runtime.mark_assets_evicted(&evicted);
            }
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        #[wasm_bindgen]
        pub fn mark_asset_failed(&mut self, asset_id: u32, err: &str) {
            self.runtime.mark_asset_failed(AssetId(asset_id), err);
        }

        #[wasm_bindgen]
        pub fn resize(&mut self, phys_width: u32, phys_height: u32, dpr: f32) {
            let w = phys_width.max(1);
            let h = phys_height.max(1);
            self.dpr = dpr.clamp(0.5, 4.0);
            self.logical_width = (w as f32 / self.dpr).max(1.0);
            self.logical_height = (h as f32 / self.dpr).max(1.0);
            self.renderer.resize(w, h);
            self.runtime.dirty = true;
        }

        #[wasm_bindgen]
        pub fn set_safe_insets(&mut self, top: f32, right: f32, bottom: f32, left: f32) {
            self.runtime.set_safe_insets(SafeInsets {
                top,
                right,
                bottom,
                left,
            });
        }

        #[wasm_bindgen]
        pub fn set_input_mode(&mut self, is_touch: bool) {
            self.runtime.set_input_mode(if is_touch {
                InputMode::Touch
            } else {
                InputMode::Pointer
            });
        }

        #[wasm_bindgen]
        pub fn set_texture_cache_limit_mb(&mut self, mb: u32) {
            self.renderer
                .set_texture_cache_limit((mb.max(24) as u64) * 1024 * 1024);
            let evicted = self.renderer.take_evicted_assets();
            if !evicted.is_empty() {
                self.runtime.mark_assets_evicted(&evicted);
            }
        }

        #[wasm_bindgen]
        pub fn clear_gpu_cache(&mut self) {
            let removed = self.renderer.clear_textures();
            if !removed.is_empty() {
                self.runtime.mark_assets_evicted(&removed);
            }
        }

        #[wasm_bindgen]
        pub fn set_host_paused(&mut self, paused: bool) -> String {
            self.runtime.set_host_paused(paused);
            self.drain_js_commands()
        }

        #[wasm_bindgen]
        pub fn wants_wake_lock(&self) -> bool {
            self.runtime.is_auto_or_skip_active()
        }

        #[wasm_bindgen]
        pub fn has_missing_glyphs(&self) -> bool {
            self.renderer.has_missing_glyphs()
        }

        #[wasm_bindgen]
        pub fn is_device_lost(&self) -> bool {
            self.renderer.is_lost()
        }

        #[wasm_bindgen]
        pub async fn recreate_renderer(
            &mut self,
            canvas: HtmlCanvasElement,
            phys_width: u32,
            phys_height: u32,
            dpr: f32,
        ) -> Result<(), JsValue> {
            let w = phys_width.max(1);
            let h = phys_height.max(1);
            self.dpr = dpr.clamp(0.5, 4.0);
            self.logical_width = (w as f32 / self.dpr).max(1.0);
            self.logical_height = (h as f32 / self.dpr).max(1.0);
            self.renderer = create_web_renderer(canvas, w, h)
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            self.runtime.invalidate_all_textures();
            Ok(())
        }

        #[wasm_bindgen]
        pub fn show_toast(&mut self, message: &str, duration_ms: u32) {
            self.runtime.show_toast(message, duration_ms as u64);
        }

        #[wasm_bindgen]
        pub fn pointer_move(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.set_hover(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn pointer_down(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.pointer_down(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn pointer_up(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.pointer_up(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn pointer_cancel(&mut self) {
            self.runtime.pointer_cancel();
        }

        #[wasm_bindgen]
        pub fn pointer_click(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.pointer_click(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn touch_start(&mut self, id: f64, logical_x: f32, logical_y: f32) {
            self.runtime
                .touch_start(id.max(0.0) as u64, logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn touch_move(&mut self, id: f64, logical_x: f32, logical_y: f32) {
            self.runtime
                .touch_move(id.max(0.0) as u64, logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn touch_end(&mut self, id: f64, logical_x: f32, logical_y: f32) {
            self.runtime
                .touch_end(id.max(0.0) as u64, logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn touch_cancel(&mut self, id: f64) {
            self.runtime.touch_cancel(id.max(0.0) as u64);
        }

        #[wasm_bindgen]
        pub fn cancel_or_menu(&mut self) {
            self.runtime.cancel_or_menu();
        }

        #[wasm_bindgen]
        pub fn scroll_wheel(&mut self, delta_y: f32) {
            self.runtime.scroll_wheel(delta_y);
        }

        #[wasm_bindgen]
        pub fn set_host_has_shelf(&mut self, has_shelf: bool) {
            self.runtime.set_host_has_shelf(has_shelf);
        }

        #[wasm_bindgen]
        pub fn set_tz_offset_mins(&mut self, offset_mins: i32) {
            self.runtime.set_tz_offset_mins(offset_mins);
        }

        #[wasm_bindgen]
        pub fn set_autosave_summary_json(&mut self, json: &str) {
            let s: Option<SlotSummary> = serde_json::from_str(json).ok();
            self.runtime.set_autosave_summary(s);
        }

        #[wasm_bindgen]
        pub fn set_quicksave_summary_json(&mut self, json: &str) {
            let s: Option<SlotSummary> = serde_json::from_str(json).ok();
            self.runtime.set_quicksave_summary(s);
        }

        #[wasm_bindgen]
        pub fn upload_slot_thumb_bitmap(
            &mut self,
            slot: u32,
            bitmap: &ImageBitmap,
        ) -> Result<(), JsValue> {
            let id = AssetId(nir_ui::SAVE_THUMB_ASSET_BASE + slot);
            self.renderer
                .upload_image_bitmap(id, bitmap)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            self.runtime.mark_slot_has_thumb(slot, true);
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        #[wasm_bindgen]
        pub fn upload_autosave_thumb_bitmap(&mut self, bitmap: &ImageBitmap) -> Result<(), JsValue> {
            let id = nir_ui::AUTOSAVE_THUMB_ASSET_ID;
            self.renderer
                .upload_image_bitmap(id, bitmap)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            self.runtime.mark_autosave_has_thumb(true);
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        #[wasm_bindgen]
        pub fn upload_quicksave_thumb_bitmap(&mut self, bitmap: &ImageBitmap) -> Result<(), JsValue> {
            let id = nir_ui::QUICKSAVE_THUMB_ASSET_ID;
            self.renderer
                .upload_image_bitmap(id, bitmap)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            self.runtime.mark_quicksave_has_thumb(true);
            self.runtime.mark_asset_loaded(id);
            Ok(())
        }

        #[wasm_bindgen]
        pub fn try_resume_latest(&mut self) -> bool {
            self.runtime.try_resume_latest()
        }

        #[wasm_bindgen]
        pub fn emit_autosave(&mut self) -> String {
            self.runtime.emit_autosave();
            self.drain_js_commands()
        }

        #[wasm_bindgen]
        pub fn load_translation_patch(
            &mut self,
            locale: &str,
            patch_json: &str,
        ) -> Result<u32, JsValue> {
            self.runtime
                .load_translation_patch(locale, patch_json)
                .map(|n| n as u32)
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen]
        pub fn dispatch_action_json(&mut self, action_json: &str) {
            if let Ok(action) = serde_json::from_str::<UiAction>(action_json) {
                self.runtime.dispatch(action);
            }
        }

        #[wasm_bindgen]
        pub fn notify_audio_ended(&mut self, channel: u32) {
            self.runtime.notify_audio_ended(ChannelId(channel));
        }

        #[wasm_bindgen]
        pub fn notify_video_finished(&mut self) {
            self.runtime.notify_video_finished();
        }

        #[wasm_bindgen]
        pub fn apply_loaded_snapshot_json(&mut self, snapshot_json: &str) {
            if let Ok(snap) = serde_json::from_str::<SaveSnapshot>(snapshot_json) {
                self.runtime.apply_loaded_snapshot(snap);
            }
        }

        #[wasm_bindgen]
        pub fn hot_reload(&mut self, new_header_bytes: Uint8Array) -> Result<(), JsValue> {
            let bytes = new_header_bytes.to_vec();
            let (program, media_base, _media_len) = decode_nirpak_header_only(&bytes)
                .map_err(|e| JsValue::from_str(&format!("hot reload parse failed: {e}")))?;
            self.media_base = media_base;
            self.runtime.apply_hot_reload(Arc::new(program));
            Ok(())
        }

        /// Flushes any debounced persistent profile updates (call on `visibilitychange` / `pagehide`).
        #[wasm_bindgen]
        pub fn flush_pending_saves(&mut self) -> String {
            self.runtime.flush_pending_saves();
            self.drain_js_commands()
        }

        #[wasm_bindgen]
        pub fn tick_and_render(&mut self, delta_ms: f64, unix_secs: f64) -> String {
            let delta_us = (delta_ms.clamp(0.0, 100.0) * 1000.0) as u64;
            self.runtime.set_unix_time(unix_secs.max(0.0) as u64);
            self.runtime.tick(delta_us);

            let cmds_json = self.drain_js_commands();

            if let Some(packet) = self
                .runtime
                .prepare_frame(self.logical_width, self.logical_height)
            {
                let _ = self.renderer.render(packet, self.dpr);
            }
            cmds_json
        }

        fn drain_js_commands(&mut self) -> String {
            let cmds = self.runtime.take_commands();
            if cmds.is_empty() {
                return String::new();
            }
            let mut pinned = None;
            let mut js_cmds = Vec::with_capacity(cmds.len());
            for cmd in cmds {
                if let HostCommand::LoadAsset { id, .. } = cmd {
                    if let Some(meta) = self.runtime.program().asset(id) {
                        match meta.kind {
                            AssetKind::Image => {
                                if self.renderer.has_texture(id) {
                                    self.runtime.mark_asset_loaded(id);
                                    continue;
                                }
                            }
                            AssetKind::Font => {
                                if self.renderer.has_font(id) {
                                    self.runtime.mark_asset_loaded(id);
                                    continue;
                                }
                            }
                            AssetKind::Audio => {
                                // Emit PreloadAudio to JS, mark loaded so Runtime doesn't block.
                                self.runtime.mark_asset_loaded(id);
                            }
                            AssetKind::Video | AssetKind::Raw => {
                                self.runtime.mark_asset_loaded(id);
                                continue;
                            }
                        }
                    }
                }
                if let Some(jc) = convert_host_command(cmd, self.runtime.program(), &mut pinned) {
                    js_cmds.push(jc);
                }
            }
            if let Some(pin) = pinned {
                let evicted = self.renderer.evict_lru(&pin);
                if !evicted.is_empty() {
                    self.runtime.mark_assets_evicted(&evicted);
                }
            }
            if js_cmds.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&js_cmds).unwrap_or_default()
            }
        }
    }

    async fn create_web_renderer(
        canvas: HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<Renderer, String> {
        // Probe WebGPU adapter BEFORE creating a canvas surface! Calling `canvas.getContext("webgpu")`
        // permanently locks the HTMLCanvasElement to WebGPU and breaks the WebGL2 fallback if no
        // WebGPU adapter is available (e.g. insecure HTTP origins, older GPUs, or software GL).
        let gpu_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let has_webgpu_adapter = gpu_instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .is_ok();
        if has_webgpu_adapter {
            if let Ok(surface) =
                gpu_instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            {
                if let Ok(r) = Renderer::new(
                    gpu_instance,
                    surface,
                    width,
                    height,
                    RendererBackend::WebGpu,
                )
                .await
                {
                    return Ok(r);
                }
            }
        }

        let gl_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        if let Ok(surface) = gl_instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)) {
            if let Ok(r) =
                Renderer::new(gl_instance, surface, width, height, RendererBackend::WebGl2).await
            {
                return Ok(r);
            }
        }
        Err("Failed to initialize WebGPU or WebGL2 renderer".into())
    }
}
