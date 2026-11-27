//! C ABI for `utils.c`: drop-in replacements with the original names and signatures, generated
//! by rust-anything from `rust-anything.json`. All of the crate's `unsafe` code lives in this file.

mod ffi;
pub mod port;

use std::ffi::c_char;

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

/// `int parse_int(const char * str)` (`utils.c:351`).
#[cfg(not(feature = "shadow"))]
// SAFETY: exported under the C name it replaces: link it instead of the original object.
#[unsafe(no_mangle)]
pub extern "C" fn parse_int(str: *const c_char) -> std::ffi::c_int {
    port::parse_int(crate::__rust_anything_c_abi::c_str(str))
}

/// `parse_int` as `rust_anything_parse_int`, for shadow builds beside the original.
// SAFETY: a name no C code defines.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_parse_int(str: *const c_char) -> std::ffi::c_int {
    port::parse_int(crate::__rust_anything_c_abi::c_str(str))
}
