//! C ABI for `renderers/mux_renderer.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;

/// `void mux_renderer_choose_audio_codec(unsigned char audio_ct)` (`renderers/mux_renderer.c:201`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn mux_renderer_choose_audio_codec(audio_ct: std::ffi::c_uchar) {
    port::mux_renderer_choose_audio_codec(audio_ct)
}

/// `mux_renderer_choose_audio_codec` as `rust_anything_mux_renderer_choose_audio_codec`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_mux_renderer_choose_audio_codec(audio_ct: std::ffi::c_uchar) {
    port::mux_renderer_choose_audio_codec(audio_ct)
}

/// `void mux_renderer_choose_video_codec(bool is_h265)` (`renderers/mux_renderer.c:216`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn mux_renderer_choose_video_codec(is_h265: bool) {
    port::mux_renderer_choose_video_codec(is_h265)
}

/// `mux_renderer_choose_video_codec` as `rust_anything_mux_renderer_choose_video_codec`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_mux_renderer_choose_video_codec(is_h265: bool) {
    port::mux_renderer_choose_video_codec(is_h265)
}

/// `void mux_renderer_stop(void)` (`renderers/mux_renderer.c:298`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn mux_renderer_stop() {
    port::mux_renderer_stop()
}

/// `mux_renderer_stop` as `rust_anything_mux_renderer_stop`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_mux_renderer_stop() {
    port::mux_renderer_stop()
}

/// `void mux_renderer_destroy(void)` (`renderers/mux_renderer.c:337`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn mux_renderer_destroy() {
    port::mux_renderer_destroy()
}

/// `mux_renderer_destroy` as `rust_anything_mux_renderer_destroy`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_mux_renderer_destroy() {
    port::mux_renderer_destroy()
}
