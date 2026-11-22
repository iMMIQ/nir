//! Lean Modern Web App WASM host for NIR v2 (`player-web`).
//!
//! Single WASM instance (zero `asset-worker.js` duplication), WebGPU-first with
//! automatic WebGL2 fallback, native `createImageBitmap` zero-copy GPU texture
//! upload, exact dirty-gated rendering, and live hot-reload support.
#![forbid(unsafe_code)]

use nir_ir::{AssetId, AssetKind, AudioBus, AudioLoopRegion, MediaCodec, Preferences};
use nir_runtime::HostCommand;
use serde::Serialize;

/// Compact JSON-serializable command bridge sent from `WebEngine` to `host/app.js`.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum JsHostCommand {
    DecodeImage {
        id: u32,
        mime: &'static str,
    },
    PlayAudio {
        channel: u32,
        bus: &'static str,
        asset: u32,
        mime: &'static str,
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
    SaveProfile {
        profile_json: String,
    },
    SavePreferences {
        prefs: Preferences,
    },
}

pub fn codec_mime(codec: MediaCodec) -> &'static str {
    match codec {
        MediaCodec::Webp => "image/webp",
        MediaCodec::Png => "image/png",
        MediaCodec::Jpeg => "image/jpeg",
        MediaCodec::Ktx2 => "image/ktx2",
        MediaCodec::Mp3 => "audio/mpeg",
        MediaCodec::Ogg => "audio/ogg",
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
        HostCommand::LoadAsset { id, .. } => {
            let meta = program.asset(id)?;
            if meta.kind == AssetKind::Image {
                Some(JsHostCommand::DecodeImage {
                    id: id.0,
                    mime: codec_mime(meta.codec),
                })
            } else {
                None
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
            let mime = program
                .asset(asset)
                .map(|m| codec_mime(m.codec))
                .unwrap_or("audio/mpeg");
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
                mime,
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
            let mime = program
                .asset(asset)
                .map(|m| codec_mime(m.codec))
                .unwrap_or("video/mp4");
            Some(JsHostCommand::PlayVideo {
                asset: asset.0,
                mime,
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
        HostCommand::SaveProfile(profile) => Some(JsHostCommand::SaveProfile {
            profile_json: serde_json::to_string(&profile).unwrap_or_default(),
        }),
        HostCommand::SavePreferences(prefs) => Some(JsHostCommand::SavePreferences { prefs }),
        HostCommand::Quit => None,
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_host {
    use super::*;
    use js_sys::Uint8Array;
    use nir_ir::{decode_nirpak_header, nirpak_asset_slice, ChannelId, UiAction};
    use nir_render::{wgpu, Renderer, RendererBackend};
    use nir_runtime::Runtime;
    use nir_ui::SlotSummary;
    use nir_vm::{ProfileData, SaveSnapshot};
    use std::sync::Arc;
    use wasm_bindgen::prelude::*;
    use web_sys::{HtmlCanvasElement, ImageBitmap};

    #[wasm_bindgen]
    pub struct WebEngine {
        pak_bytes: Vec<u8>,
        media_base: usize,
        runtime: Runtime,
        renderer: Renderer,
        logical_width: f32,
        logical_height: f32,
        dpr: f32,
    }

    #[wasm_bindgen]
    impl WebEngine {
        #[wasm_bindgen]
        pub async fn create(
            canvas: HtmlCanvasElement,
            nirpak_bytes: Uint8Array,
            prefs_json: &str,
            profile_json: &str,
            slots_json: &str,
            dpr: f32,
        ) -> Result<WebEngine, JsValue> {
            let pak_bytes = nirpak_bytes.to_vec();
            let (program, media_base) = decode_nirpak_header(&pak_bytes)
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

            let mut engine = WebEngine {
                pak_bytes,
                media_base,
                runtime,
                renderer,
                logical_width: (width as f32 / dpr).max(1.0),
                logical_height: (height as f32 / dpr).max(1.0),
                dpr,
            };
            engine.upload_all_fonts();
            Ok(engine)
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
        pub fn backend_info(&self) -> String {
            self.renderer.adapter_info().to_owned()
        }

        #[wasm_bindgen]
        pub fn asset_bytes(&self, asset_id: u32) -> Option<Uint8Array> {
            let meta = self.runtime.program().asset(AssetId(asset_id))?;
            let slice = nirpak_asset_slice(&self.pak_bytes, self.media_base, meta).ok()?;
            Some(Uint8Array::from(slice))
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
        pub fn pointer_move(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.set_hover(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn pointer_click(&mut self, logical_x: f32, logical_y: f32) {
            self.runtime.pointer_click(logical_x, logical_y);
        }

        #[wasm_bindgen]
        pub fn cancel_or_menu(&mut self) {
            self.runtime.cancel_or_menu();
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
        pub fn hot_reload(&mut self, new_nirpak_bytes: Uint8Array) -> Result<(), JsValue> {
            let bytes = new_nirpak_bytes.to_vec();
            let (program, media_base) = decode_nirpak_header(&bytes)
                .map_err(|e| JsValue::from_str(&format!("hot reload parse failed: {e}")))?;
            self.pak_bytes = bytes;
            self.media_base = media_base;
            self.runtime.apply_hot_reload(Arc::new(program));
            self.upload_all_fonts();
            Ok(())
        }

        #[wasm_bindgen]
        pub fn tick_and_render(&mut self, delta_ms: f64, unix_secs: f64) -> String {
            let delta_us = (delta_ms.clamp(0.0, 100.0) * 1000.0) as u64;
            self.runtime.set_unix_time(unix_secs.max(0.0) as u64);
            self.runtime.tick(delta_us);

            let cmds = self.runtime.take_commands();
            let mut pinned = None;
            let mut js_cmds = Vec::new();
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
                                if let Ok(slice) =
                                    nirpak_asset_slice(&self.pak_bytes, self.media_base, meta)
                                {
                                    let _ = self.renderer.load_font(id, slice.to_vec());
                                }
                                self.runtime.mark_asset_loaded(id);
                                continue;
                            }
                            AssetKind::Audio | AssetKind::Video | AssetKind::Raw => {
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
                self.renderer.evict_lru(&pin);
            }

            if let Some(packet) = self
                .runtime
                .prepare_frame(self.logical_width, self.logical_height)
            {
                let _ = self.renderer.render(packet, self.dpr);
            }
            serde_json::to_string(&js_cmds).unwrap_or_else(|_| "[]".into())
        }

        fn upload_all_fonts(&mut self) {
            let fonts: Vec<AssetId> = self
                .runtime
                .program()
                .assets
                .iter()
                .filter(|a| a.kind == AssetKind::Font)
                .map(|a| a.id)
                .collect();
            for id in fonts {
                if let Some(meta) = self.runtime.program().asset(id) {
                    if let Ok(slice) = nirpak_asset_slice(&self.pak_bytes, self.media_base, meta) {
                        if self.renderer.load_font(id, slice.to_vec()).is_ok() {
                            self.runtime.mark_asset_loaded(id);
                        }
                    }
                }
            }
        }
    }

    async fn create_web_renderer(
        canvas: HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<Renderer, String> {
        for (backends, kind) in [
            (wgpu::Backends::BROWSER_WEBGPU, RendererBackend::WebGpu),
            (wgpu::Backends::GL, RendererBackend::WebGl2),
        ] {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                ..Default::default()
            });
            let target = wgpu::SurfaceTarget::Canvas(canvas.clone());
            if let Ok(surface) = instance.create_surface(target) {
                if let Ok(r) = Renderer::new(instance, surface, width, height, kind).await {
                    return Ok(r);
                }
            }
        }
        Err("Failed to initialize WebGPU or WebGL2 renderer".into())
    }
}
