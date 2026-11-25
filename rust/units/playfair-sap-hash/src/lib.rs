//! C ABI for `sap_hash.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

pub mod port;

/// Replaces C `sap_hash` with its fixed 64-byte input / 16-byte output contract.
///
/// # Safety
/// Both pointers must be valid for their full buffer lengths and must not overlap: `block_in` must
/// point to 64 readable bytes and `key_out` to 16 writable bytes.
#[cfg(not(feature = "shadow"))]
// SAFETY: the wrapper preserves the original C symbol and documents its exact pointer contract.
#[unsafe(no_mangle)]
// SAFETY: callers must satisfy the documented 64-byte input and 16-byte output contract.
pub unsafe extern "C" fn sap_hash(block_in: *mut u8, key_out: *mut u8) {
    // SAFETY: upheld by the caller contract documented above.
    let input = unsafe { std::slice::from_raw_parts(block_in.cast_const(), 64) };
    let block: &[u8; 64] = input.try_into().expect("fixed-width input slice");
    let output = port::sap_hash(block);
    // SAFETY: upheld by the caller contract documented above.
    unsafe { std::slice::from_raw_parts_mut(key_out, 16) }.copy_from_slice(&output);
}

/// Shadow-compatible symbol for `sap_hash`, avoiding collision with the C original.
///
/// # Safety
/// Both pointers must be valid for their full buffer lengths and must not overlap: `block_in` must
/// point to 64 readable bytes and `key_out` to 16 writable bytes.
// SAFETY: this shadow alias has the same documented pointer contract as the replacement export.
#[unsafe(no_mangle)]
// SAFETY: callers must satisfy the documented 64-byte input and 16-byte output contract.
pub unsafe extern "C" fn rust_anything_sap_hash(block_in: *mut u8, key_out: *mut u8) {
    // SAFETY: the exported function has the same contract documented above.
    unsafe { sap_hash(block_in, key_out) };
}

/// `unsigned char rol8(unsigned char input, int count)` (`sap_hash.c:9`).
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

/// `uint32_t rol8x(unsigned char input, int count)` (`sap_hash.c:14`).
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
