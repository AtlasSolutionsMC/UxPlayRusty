//! Standalone Rust port of the enabled Apple-variant server side of `lib/srp.c`.
//!
//! Protocol math is in [`port`]. The exported C functions form the only unsafe
//! boundary and preserve the source API's ownership model for returned buffers.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod port;

#[repr(C)]
pub struct SRPVerifier {
    inner: port::Verifier,
}

mod ffi {
    #![allow(non_snake_case)]

    use super::{SRPVerifier, port};
    use std::{
        ffi::CStr,
        os::raw::{c_char, c_int},
        ptr, slice,
    };

    // SAFETY: callers must provide readable ranges for nonzero lengths, valid
    // NUL-terminated strings, and writable output pointers as required by srp.h.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    unsafe fn input<'a>(ptr: *const u8, len: c_int) -> Option<&'a [u8]> {
        if len < 0 {
            return None;
        }
        if len == 0 {
            return Some(&[]);
        }
        if ptr.is_null() {
            return None;
        }
        // SAFETY: the C API contract requires `ptr` to reference `len` readable bytes.
        Some(unsafe { slice::from_raw_parts(ptr, len as usize) })
    }

    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    unsafe fn c_string<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: the API declares this pointer as a NUL-terminated C string.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes())
    }

    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    unsafe fn group_strings<'a>(
        n_hex: *const c_char,
        g_hex: *const c_char,
    ) -> Option<(Option<&'a str>, Option<&'a str>)> {
        let n = if n_hex.is_null() {
            None
        } else {
            // SAFETY: group strings are documented NUL-terminated C strings.
            std::str::from_utf8(unsafe { CStr::from_ptr(n_hex) }.to_bytes()).ok()
        };
        let g = if g_hex.is_null() {
            None
        } else {
            // SAFETY: group strings are documented NUL-terminated C strings.
            std::str::from_utf8(unsafe { CStr::from_ptr(g_hex) }.to_bytes()).ok()
        };
        if (!n_hex.is_null() && n.is_none()) || (!g_hex.is_null() && g.is_none()) {
            return None;
        }
        Some((n, g))
    }

    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    unsafe fn write_bytes(out: *mut *const u8, len_out: *mut c_int, value: &[u8]) -> bool {
        if out.is_null() || len_out.is_null() || value.len() > c_int::MAX as usize {
            return false;
        }
        // Keep ownership compatible with the original C API: callers free
        // registration/ephemeral outputs with the process C allocator.
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        let allocation = unsafe { libc::malloc(value.len().max(1)) } as *mut u8;
        if allocation.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *out = ptr::null();
                *len_out = 0;
            }
            return false;
        }
        // SAFETY: `allocation` has at least one byte and the source length fits.
        unsafe {
            ptr::copy_nonoverlapping(value.as_ptr(), allocation, value.len());
            *out = allocation;
            *len_out = value.len() as c_int;
        }
        true
    }

    fn algorithm(raw: c_int) -> Option<port::HashAlgorithm> {
        port::HashAlgorithm::try_from(raw).ok()
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// C boundary. Non-null `random_data` must address `data_length` bytes.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_random_seed(random_data: *const u8, data_length: c_int) {
        if data_length < 0 || (data_length > 0 && random_data.is_null()) {
            return;
        }
        if data_length == 0 {
            port::seed_random(&[]);
            return;
        }
        // SAFETY: the caller must provide the input range documented by srp.h.
        if let Some(bytes) = unsafe { input(random_data, data_length) } {
            port::seed_random(bytes);
        }
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// C boundary. Output pointers must be writable; inputs must satisfy srp.h.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_create_salted_verification_key(
        alg: c_int,
        ng_type: c_int,
        username: *const c_char,
        password: *const u8,
        len_password: c_int,
        bytes_s: *mut *const u8,
        len_s: *mut c_int,
        bytes_v: *mut *const u8,
        len_v: *mut c_int,
        n_hex: *const c_char,
        g_hex: *const c_char,
    ) {
        if !bytes_s.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *bytes_s = ptr::null();
            }
        }
        if !len_s.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *len_s = 0;
            }
        }
        if !bytes_v.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *bytes_v = ptr::null();
            }
        }
        if !len_v.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *len_v = 0;
            }
        }
        let (Some(alg), Some(username), Some(password), Some((n_hex, g_hex))) = (
            algorithm(alg),
            // SAFETY: these pointer/length pairs follow srp.h's C contract.
            unsafe { c_string(username) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(password, len_password) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { group_strings(n_hex, g_hex) },
        ) else {
            return;
        };
        if bytes_s.is_null() || len_s.is_null() || bytes_v.is_null() || len_v.is_null() {
            return;
        }
        let Ok((salt, verifier)) =
            port::create_salted_verification_key(alg, ng_type, username, password, n_hex, g_hex)
        else {
            return;
        };
        // SAFETY: output pointers were validated and the C API transfers ownership.
        if !unsafe { write_bytes(bytes_s, len_s, &salt) } {
            return;
        }
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        if !unsafe { write_bytes(bytes_v, len_v, &verifier) } {
            // SAFETY: the first output was allocated by libc::malloc above.
            unsafe {
                libc::free(*bytes_s as *mut libc::c_void);
            }
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *bytes_s = ptr::null();
                *len_s = 0;
            }
        }
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// C boundary. Output pointers must be writable; byte ranges follow srp.h.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_create_server_ephemeral_key(
        alg: c_int,
        ng_type: c_int,
        bytes_v: *const u8,
        len_v: c_int,
        bytes_b: *const u8,
        len_b: c_int,
        bytes_B: *mut *const u8,
        len_B: *mut c_int,
        n_hex: *const c_char,
        g_hex: *const c_char,
        rfc5054_compat: c_int,
    ) {
        if !bytes_B.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *bytes_B = ptr::null();
            }
        }
        if !len_B.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *len_B = 0;
            }
        }
        if bytes_B.is_null() || len_B.is_null() {
            return;
        }
        let (Some(alg), Some(v), Some(b), Some((n_hex, g_hex))) = (
            algorithm(alg),
            // SAFETY: these pointer/length pairs follow srp.h's C contract.
            unsafe { input(bytes_v, len_v) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(bytes_b, len_b) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { group_strings(n_hex, g_hex) },
        ) else {
            return;
        };
        let Ok(server_public) = port::create_server_ephemeral_key(
            alg,
            ng_type,
            v,
            b,
            n_hex,
            g_hex,
            rfc5054_compat != 0,
        ) else {
            return;
        };
        // SAFETY: validated writable output pointers; allocation follows C ownership.
        unsafe {
            write_bytes(bytes_B, len_B, &server_public);
        }
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// C boundary. Pointer/length pairs and returned verifier lifetime follow srp.h.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_new(
        alg: c_int,
        ng_type: c_int,
        username: *const c_char,
        bytes_s: *const u8,
        len_s: c_int,
        bytes_v: *const u8,
        len_v: c_int,
        bytes_A: *const u8,
        len_A: c_int,
        bytes_b: *const u8,
        len_b: c_int,
        bytes_B: *mut *const u8,
        len_B: *mut c_int,
        n_hex: *const c_char,
        g_hex: *const c_char,
        rfc5054_compat: c_int,
    ) -> *mut SRPVerifier {
        if !bytes_B.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *bytes_B = ptr::null();
            }
        }
        if !len_B.is_null() {
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe {
                *len_B = 0;
            }
        }
        if bytes_B.is_null() || len_B.is_null() {
            return ptr::null_mut();
        }
        let (
            Some(alg),
            Some(username),
            Some(salt),
            Some(verifier),
            Some(client_public),
            Some(server_secret),
            Some((n_hex, g_hex)),
        ) = (
            algorithm(alg),
            // SAFETY: these pointers are required by srp.h to be valid.
            unsafe { c_string(username) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(bytes_s, len_s) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(bytes_v, len_v) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(bytes_A, len_A) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { input(bytes_b, len_b) },
            // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
            unsafe { group_strings(n_hex, g_hex) },
        )
        else {
            return ptr::null_mut();
        };
        let Ok(Some(inner)) = port::Verifier::new(
            alg,
            ng_type,
            username,
            salt,
            verifier,
            client_public,
            server_secret,
            n_hex,
            g_hex,
            rfc5054_compat != 0,
        ) else {
            return ptr::null_mut();
        };
        let result = Box::new(SRPVerifier { inner });
        // SAFETY: `result` owns the returned B bytes for the verifier lifetime.
        unsafe {
            *len_B = result.inner.server_public().len() as c_int;
            *bytes_B = result.inner.server_public().as_ptr();
        }
        Box::into_raw(result)
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be null or a live handle returned by `srp_verifier_new`, deleted once.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_delete(ver: *mut SRPVerifier) {
        if !ver.is_null() {
            // SAFETY: contract requires a unique live Box pointer from srp_verifier_new.
            drop(unsafe { Box::from_raw(ver) });
        }
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be null or a live verifier handle.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_is_authenticated(ver: *const SRPVerifier) -> c_int {
        // SAFETY: the caller promises a live verifier pointer when non-null.
        unsafe { ver.as_ref() }.map_or(0, |value| i32::from(value.inner.is_authenticated()))
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be null or a live verifier handle.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_get_username(ver: *const SRPVerifier) -> *const c_char {
        // SAFETY: the caller promises a live verifier pointer when non-null.
        unsafe { ver.as_ref() }.map_or(ptr::null(), |value| value.inner.username_ptr())
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be a live verifier; `key_length`, when non-null, must be writable.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_get_session_key(
        ver: *const SRPVerifier,
        key_length: *mut c_int,
    ) -> *const u8 {
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        let Some(ver) = (unsafe { ver.as_ref() }) else {
            return ptr::null();
        };
        if !key_length.is_null() {
            // SAFETY: caller promises writable key_length storage.
            unsafe {
                *key_length = ver.inner.session_key().len() as c_int;
            }
        }
        ver.inner.session_key().as_ptr()
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be null or a live verifier handle.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_get_session_key_length(ver: *const SRPVerifier) -> c_int {
        // SAFETY: the caller promises a live verifier pointer when non-null.
        unsafe { ver.as_ref() }.map_or(0, |value| value.inner.session_key().len() as c_int)
    }

    // SAFETY: The C export contract and pointer requirements are documented on the following function.
    #[unsafe(no_mangle)]
    /// # Safety
    /// `ver` must be live; `user_M` must address exactly the hash length bytes and
    /// `bytes_HAMK` must be writable. The returned pointer remains valid until delete.
    // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
    pub unsafe extern "C" fn srp_verifier_verify_session(
        ver: *mut SRPVerifier,
        user_M: *const u8,
        bytes_HAMK: *mut *const u8,
    ) {
        if bytes_HAMK.is_null() {
            return;
        }
        // SAFETY: caller promises writable output storage.
        unsafe {
            *bytes_HAMK = ptr::null();
        }
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        let Some(ver) = (unsafe { ver.as_mut() }) else {
            return;
        };
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        let Some(proof) = (unsafe { input(user_M, ver.inner.proof_len() as c_int) }) else {
            return;
        };
        if ver.inner.verify_session(proof) {
            // SAFETY: pointer refers to stable verifier-owned H_AMK storage.
            unsafe {
                *bytes_HAMK = ver.inner.hamk().as_ptr();
            }
        }
    }
}
