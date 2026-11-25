//! C ABI for `lib/playfair/sap_hash.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;

/// `unsigned char rol8(unsigned char input, int count)` (`lib/playfair/sap_hash.c:9`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn rol8(input: std::ffi::c_uchar, count: std::ffi::c_int) -> std::ffi::c_uchar {
    port::rol8(input, count)
}

/// `rol8` as `rust_anything_rol8`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_rol8(
    input: std::ffi::c_uchar,
    count: std::ffi::c_int,
) -> std::ffi::c_uchar {
    port::rol8(input, count)
}

/// `uint32_t rol8x(unsigned char input, int count)` (`lib/playfair/sap_hash.c:14`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn rol8x(input: std::ffi::c_uchar, count: std::ffi::c_int) -> u32 {
    port::rol8x(input, count)
}

/// `rol8x` as `rust_anything_rol8x`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_rol8x(input: std::ffi::c_uchar, count: std::ffi::c_int) -> u32 {
    port::rol8x(input, count)
}
