//! C ABI for `hand_garble.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;

/// `uint32_t weird_ror8(uint8_t input, int count)` (`hand_garble.c:9`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn weird_ror8(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_ror8(input, count)
}

/// `weird_ror8` as `rust_anything_weird_ror8`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_weird_ror8(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_ror8(input, count)
}

/// `uint32_t weird_rol8(uint8_t input, int count)` (`hand_garble.c:17`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn weird_rol8(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_rol8(input, count)
}

/// `weird_rol8` as `rust_anything_weird_rol8`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_weird_rol8(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_rol8(input, count)
}

/// `uint32_t weird_rol32(uint8_t input, int count)` (`hand_garble.c:24`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn weird_rol32(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_rol32(input, count)
}

/// `weird_rol32` as `rust_anything_weird_rol32`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_weird_rol32(input: u8, count: std::ffi::c_int) -> u32 {
    port::weird_rol32(input, count)
}
