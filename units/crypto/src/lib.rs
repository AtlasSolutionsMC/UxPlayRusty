//! Rust implementation of the UxPlay `crypto.h` C facade.
//!
//! Safe operations live in `port`; `ffi` is the deliberately narrow C ABI
//! boundary. Pointer arguments to exported C functions must meet the same
//! validity and buffer-size requirements as the original C API.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod port;

#[repr(C)]
pub struct aes_ctx_s {
    inner: port::AesContext,
}
#[repr(C)]
pub struct x25519_key_s {
    inner: port::X25519Key,
}
#[repr(C)]
pub struct ed25519_key_s {
    inner: port::Ed25519Key,
}
#[repr(C)]
pub struct sha_ctx_s {
    inner: port::DigestContext,
}
#[repr(C)]
pub struct md5_ctx_s {
    inner: port::DigestContext,
}

#[allow(non_snake_case)]
/// C ABI adapter for the opaque handles declared by `include/crypto.h`.
///
/// # Safety
/// Every exported function that accepts pointers requires valid readable or writable
/// ranges of the documented lengths. Context pointers must originate from the matching
/// initializer and be destroyed exactly once. Input and output buffers may overlap for
/// AES/GCM operations because those inputs are copied before output is written.
mod ffi {
    use super::*;
    use std::{ffi::CStr, os::raw::c_char, path::Path, ptr, slice};

    // SAFETY: these OpenSSL functions consume only the error code and the caller-owned
    // 256-byte buffer. The symbols come from the same libcrypto linked by `openssl`.
    unsafe extern "C" {
        fn ERR_get_error() -> libc::c_ulong;
        fn ERR_error_string(error: libc::c_ulong, buffer: *mut c_char) -> *mut c_char;
    }

    fn source_handle_error(location: &str, stack: Option<openssl::error::ErrorStack>) -> ! {
        if let Some(stack) = stack {
            // Recreate the OpenSSL thread-local queue so ERR_get_error consumes the same
            // oldest error that the C `handle_error` helper would consume.
            stack.put();
        }
        // SAFETY: ERR_get_error has no pointer arguments and is linked from libcrypto.
        let code = unsafe { ERR_get_error() };
        let mut error_text = [0 as c_char; 256];
        // SAFETY: the buffer has the documented 256-byte capacity and is NUL-terminated by OpenSSL.
        let text = unsafe { ERR_error_string(code, error_text.as_mut_ptr()) };
        let error = if text.is_null() {
            String::new()
        } else {
            // SAFETY: ERR_error_string returns the supplied NUL-terminated buffer.
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        };
        eprintln!("Crypto error at {location}: {error}");
        std::process::exit(1);
    }

    fn source_result<T>(result: port::Result<T>, location: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(port::CryptoError::OpenSsl(stack)) => source_handle_error(location, Some(stack)),
            Err(_) => source_handle_error(location, None),
        }
    }

    // SAFETY: each call is valid only when its caller supplies the non-null pointer and
    // byte range required by crypto.h. Zero-length ranges may use null pointers.
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
        if len == 0 {
            return Some(&[]);
        }
        if ptr.is_null() {
            None
        } else {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            Some(unsafe { slice::from_raw_parts(ptr, len) })
        }
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn bytes_mut<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [u8]> {
        if len == 0 {
            return Some(&mut []);
        }
        if ptr.is_null() {
            None
        } else {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            Some(unsafe { slice::from_raw_parts_mut(ptr, len) })
        }
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn owned_bytes(ptr: *const u8, len: usize) -> Option<Vec<u8>> {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        unsafe { bytes(ptr, len) }.map(<[u8]>::to_vec)
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_init(key: *const u8, iv: *const u8) -> *mut aes_ctx_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(key), Some(iv)) = (unsafe { bytes(key, 16) }, unsafe { bytes(iv, 16) }) else {
            return ptr::null_mut();
        };
        let Some(inner) = source_result(
            port::AesContext::new(port::AesMode::Ctr, port::Direction::Encrypt, key, iv),
            "aes_init",
        ) else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(aes_ctx_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_reset(ctx: *mut aes_ctx_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        if let Some(ctx) = unsafe { ctx.as_mut() } {
            let _ = source_result(ctx.inner.reset(), "aes_reset");
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_encrypt(
        ctx: *mut aes_ctx_s,
        input: *const u8,
        output: *mut u8,
        len: i32,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(input) = unsafe { len_bytes(input, len) }.map(<[u8]>::to_vec) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(output) = (unsafe { out_bytes(output, len) }) else {
            return;
        };
        if let Some(result) = source_result(ctx.inner.ctr_encrypt(&input), "aes_encrypt")
            && result.len() <= output.len()
        {
            output[..result.len()].copy_from_slice(&result);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_decrypt(
        ctx: *mut aes_ctx_s,
        input: *const u8,
        output: *mut u8,
        len: i32,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(input) = unsafe { len_bytes(input, len) }.map(<[u8]>::to_vec) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(output) = (unsafe { out_bytes(output, len) }) else {
            return;
        };
        if let Some(result) = source_result(ctx.inner.ctr_decrypt(&input), "aes_encrypt")
            && result.len() <= output.len()
        {
            output[..result.len()].copy_from_slice(&result);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_start_fresh_block(ctx: *mut aes_ctx_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        if let Some(ctx) = unsafe { ctx.as_mut() } {
            let _ = source_result(ctx.inner.ctr_start_fresh_block(), "aes_encrypt");
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_ctr_destroy(ctx: *mut aes_ctx_s) {
        if !ctx.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(ctx) });
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_cbc_init(
        key: *const u8,
        iv: *const u8,
        direction: i32,
    ) -> *mut aes_ctx_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(key), Some(iv)) = (unsafe { bytes(key, 16) }, unsafe { bytes(iv, 16) }) else {
            return ptr::null_mut();
        };
        let direction = match direction {
            0 => port::Direction::Decrypt,
            1 => port::Direction::Encrypt,
            _ => return ptr::null_mut(),
        };
        let Some(inner) = source_result(
            port::AesContext::new(port::AesMode::Cbc, direction, key, iv),
            "aes_init",
        ) else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(aes_ctx_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_cbc_reset(ctx: *mut aes_ctx_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        if let Some(ctx) = unsafe { ctx.as_mut() } {
            let _ = source_result(ctx.inner.reset(), "aes_reset");
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_cbc_encrypt(
        ctx: *mut aes_ctx_s,
        input: *const u8,
        output: *mut u8,
        len: i32,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(input) = unsafe { len_bytes(input, len) }.map(<[u8]>::to_vec) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(output) = (unsafe { out_bytes(output, len) }) else {
            return;
        };
        if let Some(result) = source_result(
            ctx.inner.cbc_crypt(&input, port::Direction::Encrypt),
            "aes_encrypt",
        ) && result.len() <= output.len()
        {
            output[..result.len()].copy_from_slice(&result);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_cbc_decrypt(
        ctx: *mut aes_ctx_s,
        input: *const u8,
        output: *mut u8,
        len: i32,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(input) = unsafe { len_bytes(input, len) }.map(<[u8]>::to_vec) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(output) = (unsafe { out_bytes(output, len) }) else {
            return;
        };
        if let Some(result) = source_result(
            ctx.inner.cbc_crypt(&input, port::Direction::Decrypt),
            "aes_decrypt",
        ) && result.len() <= output.len()
        {
            output[..result.len()].copy_from_slice(&result);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn aes_cbc_destroy(ctx: *mut aes_ctx_s) {
        if !ctx.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(ctx) });
        }
    }

    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn len_bytes<'a>(ptr: *const u8, len: i32) -> Option<&'a [u8]> {
        if len < 0 {
            None
        } else {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(ptr, len as usize) }
        }
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn out_bytes<'a>(ptr: *mut u8, len: i32) -> Option<&'a mut [u8]> {
        if len < 0 {
            None
        } else {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes_mut(ptr, len as usize) }
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    pub extern "C" fn x25519_key_generate() -> *mut x25519_key_s {
        let Some(inner) = source_result(port::X25519Key::generate(), "x25519_key_generate") else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(x25519_key_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn x25519_key_from_raw(data: *const u8) -> *mut x25519_key_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(raw) = (unsafe { bytes(data, 32) }) else {
            return ptr::null_mut();
        };
        let Some(inner) =
            source_result(port::X25519Key::from_raw_public(raw), "x25519_key_from_raw")
        else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(x25519_key_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn x25519_key_get_raw(data: *mut u8, key: *const x25519_key_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(data), Some(key)) = (unsafe { bytes_mut(data, 32) }, unsafe { key.as_ref() })
        else {
            return;
        };
        if let Some(raw) = source_result(key.inner.raw_public(), "x25519_key_get_raw") {
            data.copy_from_slice(&raw);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn x25519_key_destroy(key: *mut x25519_key_s) {
        if !key.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(key) });
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn x25519_derive_secret(
        secret: *mut u8,
        ours: *const x25519_key_s,
        theirs: *const x25519_key_s,
    ) {
        let (Some(secret), Some(ours), Some(theirs)) = (
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes_mut(secret, 32) },
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { ours.as_ref() },
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { theirs.as_ref() },
        ) else {
            return;
        };
        if let Some(shared) =
            source_result(ours.inner.derive(&theirs.inner), "x25519_derive_secret")
        {
            secret.copy_from_slice(&shared);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn get_random_bytes(buf: *mut u8, num: i32) -> i32 {
        if num < 0 {
            return 0;
        }
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(buf) = (unsafe { bytes_mut(buf, num as usize) }) else {
            return 0;
        };
        match openssl::rand::rand_bytes(buf) {
            Ok(()) => 1,
            Err(_) => 0,
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn pk_to_base64(
        pk: *const u8,
        pk_len: i32,
        output: *mut c_char,
        len: i32,
    ) {
        if pk_len < 0 || len < 0 {
            return;
        }
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(pk), Some(output)) = (unsafe { owned_bytes(pk, pk_len as usize) }, unsafe {
            bytes_mut(output.cast(), len as usize)
        }) else {
            return;
        };
        let encoded = port::pk_to_base64(&pk);
        if output.len() > encoded.len() {
            output.fill(0);
            output[..encoded.len()].copy_from_slice(encoded.as_bytes());
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn gcm_encrypt(
        plaintext: *const u8,
        plaintext_len: i32,
        ciphertext: *mut u8,
        key: *mut u8,
        iv: *mut u8,
        tag: *mut u8,
    ) -> i32 {
        let (Some(plaintext), Some(key), Some(iv)) = (
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { len_bytes(plaintext, plaintext_len) }.map(<[u8]>::to_vec),
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(key.cast_const(), 16) }.map(<[u8]>::to_vec),
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(iv.cast_const(), 16) }.map(<[u8]>::to_vec),
        ) else {
            return -1;
        };
        let (Some(ciphertext), Some(tag)) =
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            (unsafe { out_bytes(ciphertext, plaintext_len) }, unsafe {
                bytes_mut(tag, 16)
            })
        else {
            return -1;
        };
        let Some((result, auth_tag)) =
            source_result(port::gcm_encrypt(&plaintext, &key, &iv), "gcm_encrypt")
        else {
            return -1;
        };
        if result.len() <= ciphertext.len() {
            ciphertext[..result.len()].copy_from_slice(&result);
            tag.copy_from_slice(&auth_tag);
            result.len() as i32
        } else {
            -1
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn gcm_decrypt(
        ciphertext: *mut u8,
        ciphertext_len: i32,
        plaintext: *mut u8,
        key: *mut u8,
        iv: *mut u8,
        tag: *mut u8,
    ) -> i32 {
        let (Some(ciphertext), Some(key), Some(iv), Some(tag)) = (
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { len_bytes(ciphertext.cast_const(), ciphertext_len) }.map(<[u8]>::to_vec),
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(key.cast_const(), 16) }.map(<[u8]>::to_vec),
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(iv.cast_const(), 16) }.map(<[u8]>::to_vec),
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(tag.cast_const(), 16) }.map(<[u8]>::to_vec),
        ) else {
            return -1;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(plaintext) = (unsafe { out_bytes(plaintext, ciphertext_len) }) else {
            return -1;
        };
        let Some(result) = source_result(
            port::gcm_decrypt(&ciphertext, &key, &iv, &tag),
            "gcm_decrypt",
        ) else {
            return -1;
        };
        if result.plaintext.len() <= plaintext.len() {
            plaintext[..result.plaintext.len()].copy_from_slice(&result.plaintext);
        }
        if result.authenticated {
            result.plaintext.len() as i32
        } else {
            -1
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_key_generate(
        device_id: *const c_char,
        keyfile: *const c_char,
        result: *mut i32,
    ) -> *mut ed25519_key_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(device_id) = unsafe { cstr_bytes(device_id) }.map(<[u8]>::to_vec) else {
            return ptr::null_mut();
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(keyfile) = unsafe { cstr_bytes(keyfile) }.map(<[u8]>::to_vec) else {
            return ptr::null_mut();
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(result) = (unsafe { result.as_mut() }) else {
            return ptr::null_mut();
        };
        *result = 0;
        let path = if keyfile.is_empty() {
            None
        } else {
            Some(Path::new(std::str::from_utf8(&keyfile).unwrap_or_default()))
        };
        let Some((inner, wrote)) = source_result(
            port::Ed25519Key::load_or_generate(&device_id, path),
            "ed25519_key_generate",
        ) else {
            return ptr::null_mut();
        };
        *result = i32::from(wrote);
        Box::into_raw(Box::new(ed25519_key_s { inner }))
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn cstr_bytes<'a>(s: *const c_char) -> Option<&'a [u8]> {
        if s.is_null() {
            None
        } else {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            Some(unsafe { CStr::from_ptr(s) }.to_bytes())
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_key_from_raw(data: *const u8) -> *mut ed25519_key_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(raw) = (unsafe { bytes(data, 32) }) else {
            return ptr::null_mut();
        };
        let Some(inner) = source_result(
            port::Ed25519Key::from_raw_public(raw),
            "ed25519_key_from_raw",
        ) else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(ed25519_key_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_key_get_raw(data: *mut u8, key: *const ed25519_key_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(data), Some(key)) = (unsafe { bytes_mut(data, 32) }, unsafe { key.as_ref() })
        else {
            return;
        };
        if let Some(raw) = source_result(key.inner.raw_public(), "ed25519_key_get_raw") {
            data.copy_from_slice(&raw);
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_key_copy(key: *const ed25519_key_s) -> *mut ed25519_key_s {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(key) = (unsafe { key.as_ref() }) else {
            return ptr::null_mut();
        };
        let Some(inner) = source_result(key.inner.copy(), "ed25519_key_copy") else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(ed25519_key_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_key_destroy(key: *mut ed25519_key_s) {
        if !key.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(key) });
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_sign(
        signature: *mut u8,
        signature_len: usize,
        data: *const u8,
        data_len: usize,
        key: *const ed25519_key_s,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(data), Some(key)) = (unsafe { owned_bytes(data, data_len) }, unsafe {
            key.as_ref()
        }) else {
            return;
        };
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(signature) = (unsafe { bytes_mut(signature, signature_len) }) else {
            return;
        };
        let _ = source_result(key.inner.sign_into(&data, signature), "ed25519_sign");
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn ed25519_verify(
        signature: *const u8,
        signature_len: usize,
        data: *const u8,
        data_len: usize,
        key: *const ed25519_key_s,
    ) -> i32 {
        let (Some(signature), Some(data), Some(key)) = (
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(signature, signature_len) },
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { bytes(data, data_len) },
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            unsafe { key.as_ref() },
        ) else {
            return 0;
        };
        match source_result(key.inner.verify(data, signature), "ed25519_verify") {
            Some(true) => 1,
            Some(false) | None => 0,
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    pub extern "C" fn sha_init() -> *mut sha_ctx_s {
        let Some(inner) = source_result(port::sha512(), "sha_init") else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(sha_ctx_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn sha_update(ctx: *mut sha_ctx_s, input: *const u8, len: i32) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(ctx), Some(input)) = (unsafe { ctx.as_mut() }, unsafe { len_bytes(input, len) })
        else {
            return;
        };
        let _ = source_result(ctx.inner.update(input), "sha_update");
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn sha_final(ctx: *mut sha_ctx_s, output: *mut u8, len: *mut u32) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        unsafe { digest_final(ctx, output, len, "sha_final") };
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn digest_final(ctx: *mut sha_ctx_s, output: *mut u8, len: *mut u32, location: &str) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        if let Some(result) = source_result(ctx.inner.finish(), location) {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            if let Some(output) = unsafe { bytes_mut(output, result.len()) } {
                output.copy_from_slice(&result);
            }
            if !len.is_null() {
                // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
                unsafe { *len = result.len() as u32 };
            }
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn sha_reset(ctx: *mut sha_ctx_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        if let Some(ctx) = unsafe { ctx.as_mut() } {
            let _ = source_result(ctx.inner.reset(), "sha_reset");
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn sha_destroy(ctx: *mut sha_ctx_s) {
        if !ctx.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(ctx) });
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    pub extern "C" fn md5_init() -> *mut md5_ctx_s {
        let Some(inner) = source_result(port::md5(), "md5_init") else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(md5_ctx_s { inner }))
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn md5_update(ctx: *mut md5_ctx_s, input: *const u8, len: i32) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let (Some(ctx), Some(input)) = (unsafe { ctx.as_mut() }, unsafe { len_bytes(input, len) })
        else {
            return;
        };
        let _ = source_result(ctx.inner.update(input), "md5_update");
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn md5_final(ctx: *mut md5_ctx_s, output: *mut u8, len: *mut u32) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        unsafe { digest_final_md5(ctx, output, len, "md5_final") };
    }
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    unsafe fn digest_final_md5(
        ctx: *mut md5_ctx_s,
        output: *mut u8,
        len: *mut u32,
        location: &str,
    ) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return;
        };
        if let Some(result) = source_result(ctx.inner.finish(), location) {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            if let Some(output) = unsafe { bytes_mut(output, result.len()) } {
                output.copy_from_slice(&result);
            }
            if !len.is_null() {
                // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
                unsafe { *len = result.len() as u32 };
            }
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn md5_reset(ctx: *mut md5_ctx_s) {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        if let Some(ctx) = unsafe { ctx.as_mut() } {
            let _ = source_result(ctx.inner.reset(), "md5_reset");
        }
    }
    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn md5_destroy(ctx: *mut md5_ctx_s) {
        if !ctx.is_null() {
            // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
            drop(unsafe { Box::from_raw(ctx) });
        }
    }

    // SAFETY: crypto.h defines this export boundary and its pointer contract.
    #[unsafe(no_mangle)]
    // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
    pub unsafe extern "C" fn get_md5(input: *mut c_char) -> *mut c_char {
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let Some(input) = (unsafe { cstr_bytes(input) }) else {
            return ptr::null_mut();
        };
        let Some(mut digest) = source_result(port::md5(), "md5_init") else {
            return ptr::null_mut();
        };
        let _ = source_result(digest.update(input), "md5_update");
        let Some(raw) = source_result(digest.finish(), "md5_final") else {
            return ptr::null_mut();
        };
        let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        // SAFETY: memory is allocated with the platform C allocator because callers free it
        // with free(), matching the original C API. The returned allocation has a NUL byte.
        let size = hex.len() + 1;
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        let output = unsafe { libc::malloc(size) }.cast::<u8>();
        if output.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: crypto.h defines the valid pointer, buffer-length, and ownership contract for this C call.
        unsafe {
            ptr::copy_nonoverlapping(hex.as_ptr(), output, hex.len());
            *output.add(hex.len()) = 0;
        }
        output.cast()
    }
}

#[cfg(test)]
mod tests;
