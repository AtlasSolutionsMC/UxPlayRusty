//! C ABI for `renderers/audio_renderer.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;

/// `bool gstreamer_init()` (`renderers/audio_renderer.c:124`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn gstreamer_init() -> bool {
    port::gstreamer_init()
}

/// `gstreamer_init` as `rust_anything_gstreamer_init`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_gstreamer_init() -> bool {
    port::gstreamer_init()
}

/// `void audio_renderer_stop()` (`renderers/audio_renderer.c:245`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn audio_renderer_stop() {
    port::audio_renderer_stop()
}

/// `audio_renderer_stop` as `rust_anything_audio_renderer_stop`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_audio_renderer_stop() {
    port::audio_renderer_stop()
}

/// `void audio_renderer_set_volume(double volume)` (`renderers/audio_renderer.c:378`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn audio_renderer_set_volume(volume: f64) {
    port::audio_renderer_set_volume(volume)
}

/// `audio_renderer_set_volume` as `rust_anything_audio_renderer_set_volume`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_audio_renderer_set_volume(volume: f64) {
    port::audio_renderer_set_volume(volume)
}

/// `void audio_renderer_flush()` (`renderers/audio_renderer.c:392`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn audio_renderer_flush() {
    port::audio_renderer_flush()
}

/// `audio_renderer_flush` as `rust_anything_audio_renderer_flush`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_audio_renderer_flush() {
    port::audio_renderer_flush()
}

/// `void audio_renderer_destroy()` (`renderers/audio_renderer.c:395`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn audio_renderer_destroy() {
    port::audio_renderer_destroy()
}

/// `audio_renderer_destroy` as `rust_anything_audio_renderer_destroy`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_audio_renderer_destroy() {
    port::audio_renderer_destroy()
}
