//! C ABI for `renderers/video_renderer.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;
mod x11_fix;

use std::ffi::{c_char, c_int, c_void};
use std::sync::Arc;

use glib::prelude::*;
use glib::translate::from_glib_none;
use gstreamer_video::prelude::VideoOverlayExtManual;

/// Install the platform window handle on an element that advertises
/// `GstVideoOverlay`. This is the only native-window cast in the crate.
pub(crate) fn set_video_overlay_window_handle(element: &gstreamer::Element, handle: usize) {
    if !element
        .type_()
        .is_a(gstreamer_video::VideoOverlay::static_type())
    {
        return;
    }

    // SAFETY: the GType check above proves this GstElement instance implements
    // GstVideoOverlay, whose interface wrapper shares the same GObject pointer.
    let overlay: gstreamer_video::VideoOverlay = unsafe {
        from_glib_none(
            element
                .as_ptr()
                .cast::<gstreamer_video::ffi::GstVideoOverlay>(),
        )
    };
    // SAFETY: the handle is supplied by the embedding window system and this
    // call runs synchronously in GStreamer's prepare-window-handle callback.
    unsafe { overlay.set_window_handle(handle) };
}

mod __rust_anything_c_abi {
    use std::ffi::{CStr, c_char};

    /// A `const char *` argument as bytes; `None` for `NULL`.
    pub(super) fn c_str<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
        if ptr.is_null() {
            return None;
        }

        // SAFETY: the C API takes NUL-terminated strings that stay valid for the call;
        // NULL is handled above.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes())
    }
}

/// `void video_renderer_pause()` (`renderers/video_renderer.c:502`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_pause() {
    port::video_renderer_pause()
}

/// `video_renderer_pause` as `rust_anything_video_renderer_pause`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_pause() {
    port::video_renderer_pause()
}

/// `void video_renderer_resume()` (`renderers/video_renderer.c:510`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_resume() {
    port::video_renderer_resume()
}

/// `video_renderer_resume` as `rust_anything_video_renderer_resume`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_resume() {
    port::video_renderer_resume()
}

/// `void video_renderer_start()` (`renderers/video_renderer.c:525`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_start() {
    port::video_renderer_start()
}

/// `video_renderer_start` as `rust_anything_video_renderer_start`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_start() {
    port::video_renderer_start()
}

/// `bool waiting_for_x11_window()` (`renderers/video_renderer.c:549`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn waiting_for_x11_window() -> bool {
    port::waiting_for_x11_window()
}

/// `waiting_for_x11_window` as `rust_anything_waiting_for_x11_window`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_waiting_for_x11_window() -> bool {
    port::waiting_for_x11_window()
}

/// `int video_renderer_cycle()` (`renderers/video_renderer.c:572`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_cycle() -> std::ffi::c_int {
    port::video_renderer_cycle()
}

/// `video_renderer_cycle` as `rust_anything_video_renderer_cycle`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_cycle() -> std::ffi::c_int {
    port::video_renderer_cycle()
}

/// `void video_renderer_flush()` (`renderers/video_renderer.c:681`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_flush() {
    port::video_renderer_flush()
}

/// `video_renderer_flush` as `rust_anything_video_renderer_flush`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_flush() {
    port::video_renderer_flush()
}

/// `void video_renderer_hls_ready()` (`renderers/video_renderer.c:684`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_hls_ready() {
    port::video_renderer_hls_ready()
}

/// `video_renderer_hls_ready` as `rust_anything_video_renderer_hls_ready`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_hls_ready() {
    port::video_renderer_hls_ready()
}

/// `void video_renderer_stop()` (`renderers/video_renderer.c:697`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_stop() {
    port::video_renderer_stop()
}

/// `video_renderer_stop` as `rust_anything_video_renderer_stop`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_stop() {
    port::video_renderer_stop()
}

/// `void video_renderer_set_device_model(const char *model, const char *name)` (`renderers/video_renderer.c:708`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_set_device_model(model: *const c_char, name: *const c_char) {
    port::video_renderer_set_device_model(
        crate::__rust_anything_c_abi::c_str(model),
        crate::__rust_anything_c_abi::c_str(name),
    )
}

/// `video_renderer_set_device_model` as `rust_anything_video_renderer_set_device_model`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_set_device_model(
    model: *const c_char,
    name: *const c_char,
) {
    port::video_renderer_set_device_model(
        crate::__rust_anything_c_abi::c_str(model),
        crate::__rust_anything_c_abi::c_str(name),
    )
}

/// `void video_renderer_set_track_metadata(const char *title, const char *artist, const char *album)` (`renderers/video_renderer.c:714`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_set_track_metadata(
    title: *const c_char,
    artist: *const c_char,
    album: *const c_char,
) {
    port::video_renderer_set_track_metadata(
        crate::__rust_anything_c_abi::c_str(title),
        crate::__rust_anything_c_abi::c_str(artist),
        crate::__rust_anything_c_abi::c_str(album),
    )
}

/// `video_renderer_set_track_metadata` as `rust_anything_video_renderer_set_track_metadata`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_set_track_metadata(
    title: *const c_char,
    artist: *const c_char,
    album: *const c_char,
) {
    port::video_renderer_set_track_metadata(
        crate::__rust_anything_c_abi::c_str(title),
        crate::__rust_anything_c_abi::c_str(artist),
        crate::__rust_anything_c_abi::c_str(album),
    )
}

/// `void video_renderer_destroy()` (`renderers/video_renderer.c:784`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_destroy() {
    port::video_renderer_destroy()
}

/// `video_renderer_destroy` as `rust_anything_video_renderer_destroy`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_destroy() {
    port::video_renderer_destroy()
}

/// `void video_renderer_set_start(float position)` (`renderers/video_renderer.c:1181`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_set_start(position: f32) {
    port::video_renderer_set_start(position)
}

/// `video_renderer_set_start` as `rust_anything_video_renderer_set_start`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_set_start(position: f32) {
    port::video_renderer_set_start(position)
}

/// `void video_renderer_seek(float position)` (`renderers/video_renderer.c:1187`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_seek(position: f32) {
    port::video_renderer_seek(position)
}

/// `video_renderer_seek` as `rust_anything_video_renderer_seek`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_seek(position: f32) {
    port::video_renderer_seek(position)
}

/// `bool video_renderer_eos_watch()` (`renderers/video_renderer.c:1213`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_eos_watch() -> bool {
    port::video_renderer_eos_watch()
}

/// `video_renderer_eos_watch` as `rust_anything_video_renderer_eos_watch`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_eos_watch() -> bool {
    port::video_renderer_eos_watch()
}

/// `void video_renderer_hls_set_volume(double volume)` (`renderers/video_renderer.c:1221`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_hls_set_volume(volume: f64) {
    port::video_renderer_hls_set_volume(volume)
}

/// `video_renderer_hls_set_volume` as `rust_anything_video_renderer_hls_set_volume`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_video_renderer_hls_set_volume(volume: f64) {
    port::video_renderer_hls_set_volume(volume)
}

/// `bool gstreamer_decoder_check(const char *decoder)` (`renderers/video_renderer.c:1233`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn gstreamer_decoder_check(decoder: *const c_char) -> bool {
    port::gstreamer_decoder_check(crate::__rust_anything_c_abi::c_str(decoder))
}

/// `gstreamer_decoder_check` as `rust_anything_gstreamer_decoder_check`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_gstreamer_decoder_check(decoder: *const c_char) -> bool {
    port::gstreamer_decoder_check(crate::__rust_anything_c_abi::c_str(decoder))
}

/// C layout of `video_geometry_t` from `video_renderer.h`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VideoGeometry {
    pub width_source: c_int,
    pub height_source: c_int,
    pub x_offset: c_int,
    pub y_offset: c_int,
    pub width: c_int,
    pub height: c_int,
}

fn owned_c_string(ptr: *const c_char, fallback: &str) -> String {
    crate::__rust_anything_c_abi::c_str(ptr)
        .map(String::from_utf8_lossy)
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

/// Source API initializer, retained for callers still using `video_renderer.h`.
///
/// # Safety
/// Every non-null pointer argument must be readable for the duration of the call;
/// `videoflip` must point to two enum values.
#[cfg(not(feature = "shadow"))]
// SAFETY: this export preserves the original C ABI; pointer preconditions are documented below.
#[unsafe(no_mangle)]
// SAFETY: callers of the exported C function must uphold its documented pointer contract.
pub unsafe extern "C" fn video_renderer_init(
    _logger: *mut c_void,
    server_name: *const c_char,
    videoflip: *const c_int,
    parser: *const c_char,
    rtp_pipeline: *const c_char,
    decoder: *const c_char,
    converter: *const c_char,
    videosink: *const c_char,
    videosink_options: *const c_char,
    initial_fullscreen: bool,
    video_sync: bool,
    h265_support: bool,
    coverart_support: bool,
    playbin_version: u32,
    uri: *const c_char,
) {
    let flip_values = if videoflip.is_null() {
        [0, 0]
    } else {
        // SAFETY: the C API declares `videoflip` as a two-element enum array.
        unsafe { [*videoflip, *videoflip.add(1)] }
    };
    let config = port::VideoRendererConfig {
        server_name: owned_c_string(server_name, "UxPlay"),
        flip: flip_values.map(port::VideoFlip::from_source_value),
        parser: owned_c_string(parser, "h264parse"),
        rtp_pipeline: owned_c_string(rtp_pipeline, ""),
        decoder: owned_c_string(decoder, "avdec_h264"),
        converter: owned_c_string(converter, "videoconvert"),
        video_sink: owned_c_string(videosink, "autovideosink"),
        video_sink_options: owned_c_string(videosink_options, ""),
        initial_fullscreen,
        video_sync,
        h265_support,
        coverart_support,
        playbin_version,
        uri: crate::__rust_anything_c_abi::c_str(uri)
            .map(String::from_utf8_lossy)
            .map(std::borrow::Cow::into_owned),
    };
    let _ = port::video_renderer_init(config, Arc::new(|_, _| {}));
}

/// Push an encoded video packet through the active appsrc.
///
/// # Safety
/// `data_len` and `ntp_time` must point to readable values. `data` must point to
/// at least `*data_len` readable bytes when the length is nonzero.
#[cfg(not(feature = "shadow"))]
// SAFETY: this export preserves the original C ABI; pointer preconditions are documented below.
#[unsafe(no_mangle)]
// SAFETY: callers of the exported C function must uphold its documented pointer contract.
pub unsafe extern "C" fn video_renderer_render_buffer(
    data: *const u8,
    data_len: *const c_int,
    _nal_count: *const c_int,
    ntp_time: *const u64,
) -> u64 {
    if data_len.is_null() || ntp_time.is_null() {
        return 0;
    }
    // SAFETY: the source API provides a length and timestamp pointer for the call.
    let len = unsafe { *data_len };
    // SAFETY: the source API provides a readable NTP timestamp pointer for the call.
    let timestamp = unsafe { *ntp_time };
    if len < 0 || (len > 0 && data.is_null()) {
        return 0;
    }
    // SAFETY: `data` points to at least `len` bytes for the duration of this C call.
    let bytes = if len == 0 {
        &[]
    } else {
        // SAFETY: `data` points to at least `len` bytes for the duration of this C call.
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    port::video_renderer_render_buffer(bytes, timestamp)
}

/// Display a JPEG cover-art packet when the JPEG renderer is active.
///
/// # Safety
/// `data_len` must point to a readable integer and `data` must point to at least
/// that many readable bytes when the length is positive.
#[cfg(not(feature = "shadow"))]
// SAFETY: this export preserves the original C ABI; pointer preconditions are documented below.
#[unsafe(no_mangle)]
// SAFETY: callers of the exported C function must uphold its documented pointer contract.
pub unsafe extern "C" fn video_renderer_display_jpeg(data: *const c_void, data_len: *const c_int) {
    if data_len.is_null() {
        return;
    }
    // SAFETY: the source API provides a pointer to its packet length.
    let len = unsafe { *data_len };
    if len <= 0 || data.is_null() {
        return;
    }
    // SAFETY: the source API guarantees the packet buffer contains `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) };
    port::video_renderer_display_jpeg(bytes);
}

/// Choose an H.264, H.265, or JPEG pipeline.
#[cfg(not(feature = "shadow"))]
// SAFETY: the signature and layout match `video_renderer.h`.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_choose_codec_and_size(
    video_is_jpeg: bool,
    video_is_h265: bool,
    _video_geometry: *const VideoGeometry,
) -> c_int {
    port::video_renderer_choose_codec_and_size(video_is_jpeg, video_is_h265)
}

/// Query HLS playback and buffering information.
///
/// # Safety
/// All seven output pointers must be valid and writable.
#[cfg(not(feature = "shadow"))]
// SAFETY: this export preserves the original C ABI; pointer preconditions are documented below.
#[unsafe(no_mangle)]
// SAFETY: callers of the exported C function must uphold its documented pointer contract.
pub unsafe extern "C" fn video_get_playback_info(
    duration: *mut f64,
    position: *mut f64,
    seek_start: *mut f64,
    seek_duration: *mut f64,
    rate: *mut f32,
    buffer_empty: *mut bool,
    buffer_full: *mut bool,
) -> bool {
    if [
        duration.cast::<c_void>(),
        position.cast(),
        seek_start.cast(),
        seek_duration.cast(),
        rate.cast(),
        buffer_empty.cast(),
        buffer_full.cast(),
    ]
    .iter()
    .any(|value| value.is_null())
    {
        return false;
    }
    let info = port::video_get_playback_info();
    let has_active_renderer = port::video_renderer_has_active_renderer();
    // SAFETY: the original C signature requires valid output pointers for all seven values.
    unsafe {
        *duration = info.duration;
        *position = info.position;
        *seek_start = info.seek_start;
        *seek_duration = info.seek_duration;
        *rate = info.rate;
        if has_active_renderer {
            *buffer_empty = info.buffer_empty;
            *buffer_full = info.buffer_full;
        }
    }
    true
}

/// Attach the renderer's bus callback to the caller's GLib loop.
///
/// # Safety
/// `loop_` must point to a live `GMainLoop` that remains valid until its watches
/// are removed or the renderer is destroyed.
#[cfg(not(feature = "shadow"))]
// SAFETY: this export preserves the original C ABI; pointer preconditions are documented below.
#[unsafe(no_mangle)]
// SAFETY: callers of the exported C function must uphold its documented pointer contract.
pub unsafe extern "C" fn video_renderer_listen(loop_: *mut c_void, id: c_int) -> u32 {
    if loop_.is_null() || id < 0 {
        return 0;
    }
    // SAFETY: the C API passes a live `GMainLoop*` and keeps it alive while the watch runs.
    let main_loop: glib::MainLoop = unsafe { from_glib_none(loop_.cast::<glib::ffi::GMainLoop>()) };
    port::video_renderer_listen(&main_loop, id as usize).unwrap_or(0)
}

/// Supply a native handle for GStreamer sinks implementing `GstVideoOverlay`.
#[cfg(not(feature = "shadow"))]
// SAFETY: the signature matches the `uintptr_t` argument in `video_renderer.h`.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_set_window_handle(handle: usize) {
    port::video_renderer_set_window_handle(handle)
}

/// Whether the active renderer is paused; declared in the source header.
#[cfg(not(feature = "shadow"))]
// SAFETY: the signature matches the C declaration in `video_renderer.h`.
#[unsafe(no_mangle)]
pub extern "C" fn video_renderer_is_paused() -> bool {
    port::video_renderer_is_paused()
}
