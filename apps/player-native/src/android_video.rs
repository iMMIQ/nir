//! Pure-Rust native Android video player using Android NDK `libmediandk.so`
//! (`AMediaExtractor` + `AMediaCodec` + `AMediaFormat`, API 21+, zero Java/DEX!).
//!
//! Decodes hardware-accelerated MP4/H.264/HEVC/WebM/VP8/VP9 video frames off the main thread,
//! converts YUV420 (NV12 / NV21 / I420) to RGBA8 premultiplied pixels, decodes the companion
//! audio track into PCM chunks fed to `AudioHost`, and renders through `nir_render::Renderer`
//! without any `SurfaceView` / `NativeActivity` window conflicts.

/// Reserved virtual `AssetId` used to upload decoded video frames into `nir_render::Renderer`.
pub const VIDEO_FRAME_ASSET_ID: nir_ir::AssetId = nir_ir::AssetId(0xFFFF_FF00);

/// Maximum video upload dimension on mobile to keep YUV->RGBA & GPU upload silky-smooth at 60fps.
pub const MAX_VIDEO_DIM: u32 = 1280;

/// Android `MediaCodecInfo.CodecCapabilities` color format constants.
pub const COLOR_FORMAT_YUV420_PLANAR: i32 = 19; // I420
pub const COLOR_FORMAT_YUV420_PACKED_PLANAR: i32 = 20;
pub const COLOR_FORMAT_YUV420_SEMI_PLANAR: i32 = 21; // NV12
pub const COLOR_FORMAT_YUV420_PACKED_SEMI_PLANAR: i32 = 39;
pub const COLOR_FORMAT_TI_YUV420_PACKED_SEMI_PLANAR: i32 = 0x7F00_0100;
pub const COLOR_FORMAT_QCOM_YUV420_SEMI_PLANAR: i32 = 0x7FA3_0C00;

/// Layout metadata for one decoded YUV420 video frame.
#[derive(Debug, Clone, Copy)]
pub struct YuvFrameLayout {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub slice_height: u32,
    pub crop_left: u32,
    pub crop_top: u32,
    pub crop_right: u32,
    pub crop_bottom: u32,
    pub color_format: i32,
}

impl YuvFrameLayout {
    /// Returns visible cropped `(width, height)` clamped to frame bounds.
    pub fn visible_dims(&self) -> (u32, u32) {
        let right = if self.crop_right >= self.crop_left && self.crop_right < self.width {
            self.crop_right + 1
        } else {
            self.width
        };
        let bottom = if self.crop_bottom >= self.crop_top && self.crop_bottom < self.height {
            self.crop_bottom + 1
        } else {
            self.height
        };
        let w = right.saturating_sub(self.crop_left).max(1);
        let h = bottom.saturating_sub(self.crop_top).max(1);
        (w, h)
    }

    pub fn is_planar(&self) -> bool {
        matches!(
            self.color_format,
            COLOR_FORMAT_YUV420_PLANAR | COLOR_FORMAT_YUV420_PACKED_PLANAR
        )
    }
}

/// Converts a decoded YUV420 buffer (NV12 semi-planar or I420 planar) to opaque RGBA8 pixels,
/// automatically applying crop rects and optional 2x downsampling when larger than `max_dim`.
/// Returns `(out_w, out_h, rgba_bytes)`.
pub fn yuv420_to_rgba_premultiplied(
    src: &[u8],
    layout: YuvFrameLayout,
    max_dim: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    let stride = layout.stride.max(layout.width) as usize;
    let slice_h = layout.slice_height.max(layout.height) as usize;
    if stride == 0 || slice_h == 0 {
        return None;
    }
    let y_plane_size = stride.checked_mul(slice_h)?;
    let (vis_w, vis_h) = layout.visible_dims();
    let step = if vis_w.max(vis_h) > max_dim && max_dim > 0 {
        2usize
    } else {
        1usize
    };
    let out_w = (vis_w as usize).div_ceil(step).max(1);
    let out_h = (vis_h as usize).div_ceil(step).max(1);

    let planar = layout.is_planar();
    let uv_stride = if planar { stride / 2 } else { stride };
    let uv_plane_rows = slice_h.div_ceil(2);
    let needed = if planar {
        y_plane_size.checked_add(uv_stride.checked_mul(uv_plane_rows)?.checked_mul(2)?)?
    } else {
        y_plane_size.checked_add(stride.checked_mul(uv_plane_rows)?)?
    };
    if src.len() < needed {
        return None;
    }

    let u_base = y_plane_size;
    let v_base = y_plane_size + uv_stride * uv_plane_rows;
    let mut out = vec![255u8; out_w * out_h * 4];
    let cx = layout.crop_left as usize;
    let cy = layout.crop_top as usize;

    for oy in 0..out_h {
        let sy = (cy + oy * step).min(slice_h - 1);
        let y_row = sy * stride;
        let uv_y = sy >> 1;
        let row_dst = oy * out_w * 4;
        for ox in 0..out_w {
            let sx = (cx + ox * step).min(stride - 1);
            let y_val = src[y_row + sx] as i32;
            let (u_val, v_val) = if planar {
                let uv_x = (sx >> 1).min(uv_stride.saturating_sub(1));
                let idx = uv_y * uv_stride + uv_x;
                (src[u_base + idx] as i32, src[v_base + idx] as i32)
            } else {
                let uv_x = (sx & !1).min(stride.saturating_sub(2));
                let idx = u_base + uv_y * stride + uv_x;
                (src[idx] as i32, src[idx + 1] as i32)
            };

            // Fast integer BT.601 full/limited-safe YUV -> RGB
            let c = (y_val - 16).max(0);
            let d = u_val - 128;
            let e = v_val - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

            let p = row_dst + ox * 4;
            out[p] = r;
            out[p + 1] = g;
            out[p + 2] = b;
            out[p + 3] = 255;
        }
    }

    Some((out_w as u32, out_h as u32, out))
}

/// One decoded RGBA8 video frame ready for GPU upload.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Poll result from `AndroidVideoPlayer::poll_frame`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub enum VideoPoll {
    Idle,
    Playing(Option<VideoFrame>),
    Finished,
}

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use crate::audio::AudioHost;
    use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
    use std::fs::{self, File};
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct AMediaExtractor {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct AMediaCodec {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct AMediaFormat {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Default)]
    struct AMediaCodecBufferInfo {
        offset: i32,
        size: i32,
        presentation_time_us: i64,
        flags: u32,
    }

    const AMEDIA_OK: c_int = 0;
    const AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM: u32 = 4;
    const AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
    const AMEDIACODEC_INFO_TRY_AGAIN_LATER: isize = -1;

    #[link(name = "mediandk")]
    extern "C" {
        fn AMediaExtractor_new() -> *mut AMediaExtractor;
        fn AMediaExtractor_delete(ex: *mut AMediaExtractor) -> c_int;
        fn AMediaExtractor_setDataSourceFd(
            ex: *mut AMediaExtractor,
            fd: c_int,
            offset: i64,
            length: i64,
        ) -> c_int;
        fn AMediaExtractor_getTrackCount(ex: *mut AMediaExtractor) -> usize;
        fn AMediaExtractor_getTrackFormat(
            ex: *mut AMediaExtractor,
            idx: usize,
        ) -> *mut AMediaFormat;
        fn AMediaExtractor_selectTrack(ex: *mut AMediaExtractor, idx: usize) -> c_int;
        fn AMediaExtractor_readSampleData(
            ex: *mut AMediaExtractor,
            buffer: *mut u8,
            capacity: usize,
        ) -> isize;
        fn AMediaExtractor_getSampleFlags(ex: *mut AMediaExtractor) -> u32;
        fn AMediaExtractor_getSampleTime(ex: *mut AMediaExtractor) -> i64;
        fn AMediaExtractor_advance(ex: *mut AMediaExtractor) -> bool;

        fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut AMediaCodec;
        fn AMediaCodec_delete(codec: *mut AMediaCodec) -> c_int;
        fn AMediaCodec_configure(
            codec: *mut AMediaCodec,
            format: *const AMediaFormat,
            surface: *mut c_void,
            crypto: *mut c_void,
            flags: u32,
        ) -> c_int;
        fn AMediaCodec_start(codec: *mut AMediaCodec) -> c_int;
        fn AMediaCodec_stop(codec: *mut AMediaCodec) -> c_int;
        fn AMediaCodec_getInputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            out_size: *mut usize,
        ) -> *mut u8;
        fn AMediaCodec_getOutputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            out_size: *mut usize,
        ) -> *mut u8;
        fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
        fn AMediaCodec_queueInputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            offset: c_long,
            size: usize,
            time: u64,
            flags: u32,
        ) -> c_int;
        fn AMediaCodec_dequeueOutputBuffer(
            codec: *mut AMediaCodec,
            info: *mut AMediaCodecBufferInfo,
            timeout_us: i64,
        ) -> isize;
        fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;
        fn AMediaCodec_releaseOutputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            render: bool,
        ) -> c_int;

        fn AMediaFormat_delete(fmt: *mut AMediaFormat) -> c_int;
        fn AMediaFormat_getInt32(
            fmt: *mut AMediaFormat,
            name: *const c_char,
            out: *mut i32,
        ) -> bool;
        fn AMediaFormat_getString(
            fmt: *mut AMediaFormat,
            name: *const c_char,
            out: *mut *const c_char,
        ) -> bool;
        fn AMediaFormat_setInt32(fmt: *mut AMediaFormat, name: *const c_char, value: i32);
    }

    struct ExtractorGuard(*mut AMediaExtractor);
    impl Drop for ExtractorGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    AMediaExtractor_delete(self.0);
                }
            }
        }
    }

    struct CodecGuard(*mut AMediaCodec, bool);
    impl Drop for CodecGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    if self.1 {
                        AMediaCodec_stop(self.0);
                    }
                    AMediaCodec_delete(self.0);
                }
            }
        }
    }

    struct FormatGuard(*mut AMediaFormat);
    impl Drop for FormatGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    AMediaFormat_delete(self.0);
                }
            }
        }
    }

    unsafe fn format_get_i32(fmt: *mut AMediaFormat, key: &[u8]) -> Option<i32> {
        let mut val = 0i32;
        if AMediaFormat_getInt32(fmt, key.as_ptr().cast::<c_char>(), &mut val) {
            Some(val)
        } else {
            None
        }
    }

    unsafe fn format_get_string(fmt: *mut AMediaFormat, key: &[u8]) -> Option<String> {
        let mut ptr: *const c_char = std::ptr::null();
        if AMediaFormat_getString(fmt, key.as_ptr().cast::<c_char>(), &mut ptr) && !ptr.is_null() {
            CStr::from_ptr(ptr).to_str().ok().map(str::to_owned)
        } else {
            None
        }
    }

    struct ActivePlayback {
        stop_flag: Arc<AtomicBool>,
        paused_flag: Arc<AtomicBool>,
        finished_flag: Arc<AtomicBool>,
        latest_frame: Arc<Mutex<Option<VideoFrame>>>,
        video_thread: Option<JoinHandle<()>>,
        audio_thread: Option<JoinHandle<()>>,
        temp_file: PathBuf,
        skippable: bool,
        last_dims: [u32; 2],
    }

    pub struct AndroidVideoPlayer {
        active: Option<ActivePlayback>,
    }

    impl AndroidVideoPlayer {
        pub fn new() -> Self {
            Self { active: None }
        }

        pub fn is_playing(&self) -> bool {
            self.active.is_some()
        }

        pub fn is_skippable(&self) -> bool {
            self.active.as_ref().is_some_and(|a| a.skippable)
        }

        pub fn last_frame_dims(&self) -> [u32; 2] {
            self.active
                .as_ref()
                .map_or([16, 9], |a| a.last_dims)
        }

        pub fn set_paused(&mut self, paused: bool) {
            if let Some(a) = &self.active {
                a.paused_flag.store(paused, Ordering::Release);
            }
        }

        pub fn play(
            &mut self,
            bytes: &[u8],
            ext: &str,
            skippable: bool,
            temp_dir: &Path,
            audio: &mut AudioHost,
            wake: impl Fn() + Send + 'static,
        ) -> Result<(), String> {
            self.stop(audio);
            if bytes.is_empty() {
                return Err("empty video payload".into());
            }
            let safe_ext = if ext.chars().all(|c| c.is_ascii_alphanumeric()) && !ext.is_empty() {
                ext
            } else {
                "mp4"
            };
            let _ = fs::create_dir_all(temp_dir);
            let temp_file = temp_dir.join(format!("nir_video_tmp.{safe_ext}"));
            fs::write(&temp_file, bytes).map_err(|e| format!("writing video temp file: {e}"))?;

            // Probe tracks synchronously so unsupported videos fail fast and trigger toast fallback!
            let (video_track, video_mime, audio_track_info) = probe_tracks(&temp_file)?;

            let stop_flag = Arc::new(AtomicBool::new(false));
            let paused_flag = Arc::new(AtomicBool::new(false));
            let finished_flag = Arc::new(AtomicBool::new(false));
            let latest_frame = Arc::new(Mutex::new(None));

            let audio_thread = if let Some((a_idx, a_mime, sr, ch)) = audio_track_info {
                let (tx, rx) = mpsc::channel::<Vec<f32>>();
                if audio.start_video_pcm_stream(sr, ch, rx) {
                    let stop = Arc::clone(&stop_flag);
                    let paused = Arc::clone(&paused_flag);
                    let path = temp_file.clone();
                    std::thread::Builder::new()
                        .name("nir-android-vid-aud".into())
                        .spawn(move || {
                            run_audio_decode_loop(&path, a_idx, &a_mime, stop, paused, tx);
                        })
                        .ok()
                } else {
                    None
                }
            } else {
                None
            };

            let v_stop = Arc::clone(&stop_flag);
            let v_paused = Arc::clone(&paused_flag);
            let v_fin = Arc::clone(&finished_flag);
            let v_frame = Arc::clone(&latest_frame);
            let v_path = temp_file.clone();
            let video_thread = std::thread::Builder::new()
                .name("nir-android-video".into())
                .spawn(move || {
                    run_video_decode_loop(
                        &v_path,
                        video_track,
                        &video_mime,
                        v_stop,
                        v_paused,
                        v_frame,
                        wake,
                    );
                    v_fin.store(true, Ordering::Release);
                })
                .map_err(|e| format!("spawning video decode thread: {e}"))?;

            self.active = Some(ActivePlayback {
                stop_flag,
                paused_flag,
                finished_flag,
                latest_frame,
                video_thread: Some(video_thread),
                audio_thread,
                temp_file,
                skippable,
                last_dims: [16, 9],
            });
            Ok(())
        }

        pub fn poll_frame(&mut self) -> VideoPoll {
            let Some(active) = &mut self.active else {
                return VideoPoll::Idle;
            };
            let frame = active
                .latest_frame
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            if let Some(f) = &frame {
                active.last_dims = [f.width.max(1), f.height.max(1)];
            }
            if active.finished_flag.load(Ordering::Acquire) && frame.is_none() {
                self.cleanup_active();
                return VideoPoll::Finished;
            }
            VideoPoll::Playing(frame)
        }

        pub fn stop(&mut self, audio: &mut AudioHost) {
            audio.stop_video_audio();
            self.cleanup_active();
        }

        fn cleanup_active(&mut self) {
            if let Some(mut active) = self.active.take() {
                active.stop_flag.store(true, Ordering::Release);
                if let Some(h) = active.video_thread.take() {
                    let _ = h.join();
                }
                if let Some(h) = active.audio_thread.take() {
                    let _ = h.join();
                }
                let _ = fs::remove_file(&active.temp_file);
            }
        }
    }

    impl Default for AndroidVideoPlayer {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for AndroidVideoPlayer {
        fn drop(&mut self) {
            self.cleanup_active();
        }
    }

    type AudioTrackProbe = (usize, String, u32, u16);

    fn probe_tracks(path: &Path) -> Result<(usize, String, Option<AudioTrackProbe>), String> {
        let file = File::open(path).map_err(|e| format!("opening video file: {e}"))?;
        let file_len = file
            .metadata()
            .map_err(|e| format!("stat video file: {e}"))?
            .len() as i64;
        unsafe {
            let ex = ExtractorGuard(AMediaExtractor_new());
            if ex.0.is_null() {
                return Err("AMediaExtractor_new failed".into());
            }
            if AMediaExtractor_setDataSourceFd(ex.0, file.as_raw_fd(), 0, file_len) != AMEDIA_OK {
                return Err("AMediaExtractor_setDataSourceFd failed".into());
            }
            let count = AMediaExtractor_getTrackCount(ex.0);
            let mut video = None;
            let mut audio = None;
            for i in 0..count {
                let fmt = FormatGuard(AMediaExtractor_getTrackFormat(ex.0, i));
                if fmt.0.is_null() {
                    continue;
                }
                let Some(mime) = format_get_string(fmt.0, b"mime\0") else {
                    continue;
                };
                if video.is_none() && mime.starts_with("video/") {
                    video = Some((i, mime));
                } else if audio.is_none() && mime.starts_with("audio/") {
                    let sr = format_get_i32(fmt.0, b"sample-rate\0")
                        .unwrap_or(48_000)
                        .clamp(8_000, 192_000) as u32;
                    let ch = format_get_i32(fmt.0, b"channel-count\0")
                        .unwrap_or(2)
                        .clamp(1, 8) as u16;
                    audio = Some((i, mime, sr, ch));
                }
            }
            let (v_idx, v_mime) = video.ok_or_else(|| "no video track found".to_owned())?;
            Ok((v_idx, v_mime, audio))
        }
    }

    fn run_video_decode_loop(
        path: &Path,
        track_idx: usize,
        mime: &str,
        stop_flag: Arc<AtomicBool>,
        paused_flag: Arc<AtomicBool>,
        latest_frame: Arc<Mutex<Option<VideoFrame>>>,
        wake: impl Fn(),
    ) {
        let Ok(file) = File::open(path) else { return };
        let Ok(meta) = file.metadata() else { return };
        let Ok(c_mime) = CString::new(mime) else { return };

        unsafe {
            let ex = ExtractorGuard(AMediaExtractor_new());
            if ex.0.is_null()
                || AMediaExtractor_setDataSourceFd(ex.0, file.as_raw_fd(), 0, meta.len() as i64)
                    != AMEDIA_OK
                || AMediaExtractor_selectTrack(ex.0, track_idx) != AMEDIA_OK
            {
                return;
            }
            let fmt = FormatGuard(AMediaExtractor_getTrackFormat(ex.0, track_idx));
            if fmt.0.is_null() {
                return;
            }
            // Request standard semi-planar YUV420 (NV12) if codec allows; we still inspect actual output format.
            AMediaFormat_setInt32(
                fmt.0,
                c"color-format".as_ptr(),
                COLOR_FORMAT_YUV420_SEMI_PLANAR,
            );

            let mut layout = read_yuv_layout(fmt.0);
            let mut codec = CodecGuard(AMediaCodec_createDecoderByType(c_mime.as_ptr()), false);
            if codec.0.is_null() {
                return;
            }
            if AMediaCodec_configure(
                codec.0,
                fmt.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            ) != AMEDIA_OK
            {
                return;
            }
            if AMediaCodec_start(codec.0) != AMEDIA_OK {
                return;
            }
            codec.1 = true;

            let mut input_eos = false;
            let mut output_eos = false;
            let mut start_wall: Option<Instant> = None;
            let mut first_pts_us: Option<i64> = None;
            let mut paused_accum = Duration::ZERO;

            while !stop_flag.load(Ordering::Acquire) && !output_eos {
                if paused_flag.load(Ordering::Acquire) {
                    let p_start = Instant::now();
                    while paused_flag.load(Ordering::Acquire) && !stop_flag.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    paused_accum += p_start.elapsed();
                    continue;
                }

                if !input_eos {
                    let in_idx = AMediaCodec_dequeueInputBuffer(codec.0, 5_000);
                    if in_idx >= 0 {
                        let mut buf_size = 0usize;
                        let buf_ptr =
                            AMediaCodec_getInputBuffer(codec.0, in_idx as usize, &mut buf_size);
                        if !buf_ptr.is_null() && buf_size > 0 {
                            let sample_size =
                                AMediaExtractor_readSampleData(ex.0, buf_ptr, buf_size);
                            if sample_size < 0 {
                                input_eos = true;
                                let _ = AMediaCodec_queueInputBuffer(
                                    codec.0,
                                    in_idx as usize,
                                    0,
                                    0,
                                    0,
                                    AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM,
                                );
                            } else {
                                let pts = AMediaExtractor_getSampleTime(ex.0).max(0) as u64;
                                let flags = AMediaExtractor_getSampleFlags(ex.0);
                                let _ = AMediaCodec_queueInputBuffer(
                                    codec.0,
                                    in_idx as usize,
                                    0,
                                    sample_size as usize,
                                    pts,
                                    flags,
                                );
                                let _ = AMediaExtractor_advance(ex.0);
                            }
                        }
                    }
                }

                let mut info = AMediaCodecBufferInfo::default();
                let out_idx = AMediaCodec_dequeueOutputBuffer(codec.0, &mut info, 10_000);
                if out_idx == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED {
                    let out_fmt = FormatGuard(AMediaCodec_getOutputFormat(codec.0));
                    if !out_fmt.0.is_null() {
                        layout = read_yuv_layout(out_fmt.0);
                    }
                } else if out_idx >= 0 {
                    let mut out_cap = 0usize;
                    let out_ptr =
                        AMediaCodec_getOutputBuffer(codec.0, out_idx as usize, &mut out_cap);
                    let pts_us = info.presentation_time_us.max(0);
                    let t0 = *start_wall.get_or_insert_with(Instant::now);
                    let p0 = *first_pts_us.get_or_insert(pts_us);
                    let target_us = pts_us.saturating_sub(p0) as u64;
                    let elapsed_us = t0
                        .elapsed()
                        .saturating_sub(paused_accum)
                        .as_micros() as u64;

                    // Sleep in small slices so skip/stop responds immediately (<15ms latency).
                    if target_us > elapsed_us {
                        let mut rem_us = target_us - elapsed_us;
                        while rem_us > 2_000 && !stop_flag.load(Ordering::Acquire) {
                            let step = rem_us.min(12_000);
                            std::thread::sleep(Duration::from_micros(step));
                            rem_us = rem_us.saturating_sub(step);
                        }
                    }

                    // Drop very late frames (>80ms behind audio/clock) before YUV->RGBA conversion.
                    let behind_us = elapsed_us.saturating_sub(target_us);
                    if behind_us <= 80_000
                        && !out_ptr.is_null()
                        && info.size > 0
                        && !stop_flag.load(Ordering::Acquire)
                    {
                        let off = info.offset.max(0) as usize;
                        let sz = info.size as usize;
                        if off.saturating_add(sz) <= out_cap {
                            let slice = std::slice::from_raw_parts(out_ptr.add(off), sz);
                            if let Some((w, h, rgba)) =
                                yuv420_to_rgba_premultiplied(slice, layout, MAX_VIDEO_DIM)
                            {
                                if let Ok(mut guard) = latest_frame.lock() {
                                    *guard = Some(VideoFrame {
                                        width: w,
                                        height: h,
                                        rgba,
                                    });
                                }
                                wake();
                            }
                        }
                    }
                    let _ = AMediaCodec_releaseOutputBuffer(codec.0, out_idx as usize, false);
                    if (info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        output_eos = true;
                    }
                } else if out_idx == AMEDIACODEC_INFO_TRY_AGAIN_LATER && input_eos {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            wake();
        }
    }

    fn run_audio_decode_loop(
        path: &Path,
        track_idx: usize,
        mime: &str,
        stop_flag: Arc<AtomicBool>,
        paused_flag: Arc<AtomicBool>,
        tx: mpsc::Sender<Vec<f32>>,
    ) {
        let Ok(file) = File::open(path) else { return };
        let Ok(meta) = file.metadata() else { return };
        let Ok(c_mime) = CString::new(mime) else { return };

        unsafe {
            let ex = ExtractorGuard(AMediaExtractor_new());
            if ex.0.is_null()
                || AMediaExtractor_setDataSourceFd(ex.0, file.as_raw_fd(), 0, meta.len() as i64)
                    != AMEDIA_OK
                || AMediaExtractor_selectTrack(ex.0, track_idx) != AMEDIA_OK
            {
                return;
            }
            let fmt = FormatGuard(AMediaExtractor_getTrackFormat(ex.0, track_idx));
            if fmt.0.is_null() {
                return;
            }
            let mut codec = CodecGuard(AMediaCodec_createDecoderByType(c_mime.as_ptr()), false);
            if codec.0.is_null()
                || AMediaCodec_configure(
                    codec.0,
                    fmt.0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                ) != AMEDIA_OK
                || AMediaCodec_start(codec.0) != AMEDIA_OK
            {
                return;
            }
            codec.1 = true;

            let mut input_eos = false;
            let mut output_eos = false;
            const INV_I16: f32 = 1.0 / 32768.0;

            while !stop_flag.load(Ordering::Acquire) && !output_eos {
                if paused_flag.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                if !input_eos {
                    let in_idx = AMediaCodec_dequeueInputBuffer(codec.0, 5_000);
                    if in_idx >= 0 {
                        let mut buf_size = 0usize;
                        let buf_ptr =
                            AMediaCodec_getInputBuffer(codec.0, in_idx as usize, &mut buf_size);
                        if !buf_ptr.is_null() && buf_size > 0 {
                            let sample_size =
                                AMediaExtractor_readSampleData(ex.0, buf_ptr, buf_size);
                            if sample_size < 0 {
                                input_eos = true;
                                let _ = AMediaCodec_queueInputBuffer(
                                    codec.0,
                                    in_idx as usize,
                                    0,
                                    0,
                                    0,
                                    AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM,
                                );
                            } else {
                                let pts = AMediaExtractor_getSampleTime(ex.0).max(0) as u64;
                                let flags = AMediaExtractor_getSampleFlags(ex.0);
                                let _ = AMediaCodec_queueInputBuffer(
                                    codec.0,
                                    in_idx as usize,
                                    0,
                                    sample_size as usize,
                                    pts,
                                    flags,
                                );
                                let _ = AMediaExtractor_advance(ex.0);
                            }
                        }
                    }
                }

                let mut info = AMediaCodecBufferInfo::default();
                let out_idx = AMediaCodec_dequeueOutputBuffer(codec.0, &mut info, 10_000);
                if out_idx >= 0 {
                    let mut out_cap = 0usize;
                    let out_ptr =
                        AMediaCodec_getOutputBuffer(codec.0, out_idx as usize, &mut out_cap);
                    if !out_ptr.is_null() && info.size > 0 {
                        let off = info.offset.max(0) as usize;
                        let sz = info.size as usize;
                        if off.saturating_add(sz) <= out_cap {
                            let bytes = std::slice::from_raw_parts(out_ptr.add(off), sz);
                            let samples: Vec<f32> = bytes
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|&c| i16::from_le_bytes(c) as f32 * INV_I16)
                                .collect();
                            if !samples.is_empty() && tx.send(samples).is_err() {
                                let _ = AMediaCodec_releaseOutputBuffer(
                                    codec.0,
                                    out_idx as usize,
                                    false,
                                );
                                break;
                            }
                        }
                    }
                    let _ = AMediaCodec_releaseOutputBuffer(codec.0, out_idx as usize, false);
                    if (info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        output_eos = true;
                    }
                }
            }
        }
    }

    unsafe fn read_yuv_layout(fmt: *mut AMediaFormat) -> YuvFrameLayout {
        let width = format_get_i32(fmt, b"width\0").unwrap_or(640).max(1) as u32;
        let height = format_get_i32(fmt, b"height\0").unwrap_or(360).max(1) as u32;
        let stride = format_get_i32(fmt, b"stride\0")
            .unwrap_or(width as i32)
            .max(width as i32) as u32;
        let slice_height = format_get_i32(fmt, b"slice-height\0")
            .unwrap_or(height as i32)
            .max(height as i32) as u32;
        let crop_left = format_get_i32(fmt, b"crop-left\0").unwrap_or(0).max(0) as u32;
        let crop_top = format_get_i32(fmt, b"crop-top\0").unwrap_or(0).max(0) as u32;
        let crop_right = format_get_i32(fmt, b"crop-right\0")
            .unwrap_or((width - 1) as i32)
            .max(0) as u32;
        let crop_bottom = format_get_i32(fmt, b"crop-bottom\0")
            .unwrap_or((height - 1) as i32)
            .max(0) as u32;
        let color_format = format_get_i32(fmt, b"color-format\0")
            .unwrap_or(COLOR_FORMAT_YUV420_SEMI_PLANAR);
        YuvFrameLayout {
            width,
            height,
            stride,
            slice_height,
            crop_left,
            crop_top,
            crop_right,
            crop_bottom,
            color_format,
        }
    }
}

#[cfg(target_os = "android")]
pub use imp::AndroidVideoPlayer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420_nv12_and_i420_convert_cleanly_with_crop_and_downscale() {
        // 4x4 NV12 neutral gray-white (Y=235, U=128, V=128 -> ~255,255,255)
        let mut nv12 = vec![235u8; 16];
        nv12.extend_from_slice(&[128u8; 8]);
        let layout = YuvFrameLayout {
            width: 4,
            height: 4,
            stride: 4,
            slice_height: 4,
            crop_left: 0,
            crop_top: 0,
            crop_right: 3,
            crop_bottom: 3,
            color_format: COLOR_FORMAT_YUV420_SEMI_PLANAR,
        };
        let (w, h, rgba) = yuv420_to_rgba_premultiplied(&nv12, layout, 1280).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(rgba.len(), 64);
        assert!(rgba[0] >= 250 && rgba[1] >= 250 && rgba[2] >= 250 && rgba[3] == 255);

        // Downscale path (max_dim = 2 -> 4x4 becomes 2x2)
        let (dw, dh, drgba) = yuv420_to_rgba_premultiplied(&nv12, layout, 2).unwrap();
        assert_eq!((dw, dh), (2, 2));
        assert_eq!(drgba.len(), 16);

        // I420 planar with crop rect (1..=2, 1..=2 -> 2x2)
        let mut i420 = vec![16u8; 16]; // black Y=16
        i420.extend_from_slice(&[128u8; 8]);
        let i420_layout = YuvFrameLayout {
            crop_left: 1,
            crop_top: 1,
            crop_right: 2,
            crop_bottom: 2,
            color_format: COLOR_FORMAT_YUV420_PLANAR,
            ..layout
        };
        let (cw, ch, crgba) = yuv420_to_rgba_premultiplied(&i420, i420_layout, 1280).unwrap();
        assert_eq!((cw, ch), (2, 2));
        assert_eq!(&crgba[..4], &[0, 0, 0, 255]);
    }
}
