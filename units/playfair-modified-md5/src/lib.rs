//! C ABI for `modified_md5.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

#![expect(
    non_snake_case,
    reason = "Preserve the source C ABI symbol and parameter names."
)]

pub mod ffi;
pub mod port;

/// `uint32_t F(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:14`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn F(B: u32, C: u32, D: u32) -> u32 {
    port::F(B, C, D)
}

/// `F` as `rust_anything_F`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_F(B: u32, C: u32, D: u32) -> u32 {
    port::F(B, C, D)
}

/// `uint32_t G(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:19`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn G(B: u32, C: u32, D: u32) -> u32 {
    port::G(B, C, D)
}

/// `G` as `rust_anything_G`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_G(B: u32, C: u32, D: u32) -> u32 {
    port::G(B, C, D)
}

/// `uint32_t H(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:24`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn H(B: u32, C: u32, D: u32) -> u32 {
    port::H(B, C, D)
}

/// `H` as `rust_anything_H`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_H(B: u32, C: u32, D: u32) -> u32 {
    port::H(B, C, D)
}

/// `uint32_t I(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:29`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn I(B: u32, C: u32, D: u32) -> u32 {
    port::I(B, C, D)
}

/// `I` as `rust_anything_I`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_I(B: u32, C: u32, D: u32) -> u32 {
    port::I(B, C, D)
}

/// `uint32_t rol(uint32_t input, int count)` (`modified_md5.c:35`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn rol(input: u32, count: std::ffi::c_int) -> u32 {
    port::rol(input, count)
}

/// `rol` as `rust_anything_rol`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_rol(input: u32, count: std::ffi::c_int) -> u32 {
    port::rol(input, count)
}
