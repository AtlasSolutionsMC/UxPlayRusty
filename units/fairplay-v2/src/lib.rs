//! Rust implementation of `lib/fairplay_v2/fairplay_v2.c`.
//!
//! The protocol implementation is safe Rust in [`port`]. The exported C ABI
//! preserves the five handles and fixed-size calls declared by
//! `fairplay_v2.h`.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod port;

/// Opaque C-compatible FairPlay state handle.
#[repr(C)]
pub struct fairplay_v2_t {
    inner: port::FairPlayV2,
}

mod ffi {
    use super::{fairplay_v2_t, port};
    use std::{ptr, slice};

    /// Creates a context with fresh cryptographic receiver SAP bytes.
    ///
    /// # Safety
    /// The returned pointer must be destroyed exactly once with
    /// [`fairplay_v2_destroy`].
    // SAFETY: this export returns a boxed opaque handle whose matching destroy function owns it.
    #[unsafe(no_mangle)]
    pub extern "C" fn fairplay_v2_init() -> *mut fairplay_v2_t {
        // SAFETY: calloc allocates enough correctly aligned storage for one handle or returns null.
        let allocation = unsafe {
            libc::calloc(1, core::mem::size_of::<fairplay_v2_t>()).cast::<fairplay_v2_t>()
        };
        if allocation.is_null() {
            return ptr::null_mut();
        }
        match port::FairPlayV2::new() {
            Ok(inner) => {
                // SAFETY: allocation points to one writable handle-sized region returned by calloc.
                unsafe { ptr::write(allocation, fairplay_v2_t { inner }) };
                allocation
            }
            Err(_) => {
                // SAFETY: allocation came from calloc above and no Rust value has been written into it.
                unsafe { libc::free(allocation.cast()) };
                ptr::null_mut()
            }
        }
    }

    /// Destroys a context, accepting null like the original C function.
    ///
    /// # Safety
    /// `fairplay` must be null or a live pointer returned by
    /// [`fairplay_v2_init`], not previously destroyed.
    // SAFETY: the function body validates null and reconstructs only the caller-owned init handle.
    #[unsafe(no_mangle)]
    // SAFETY: callers must pass null or the live context returned from fairplay_v2_init.
    pub unsafe extern "C" fn fairplay_v2_destroy(fairplay: *mut fairplay_v2_t) {
        if !fairplay.is_null() {
            // SAFETY: the C header requires a live init-produced pointer destroyed exactly once.
            unsafe { ptr::drop_in_place(fairplay) };
            // SAFETY: init allocated this handle with calloc and it has now been dropped.
            unsafe { libc::free(fairplay.cast()) };
        }
    }

    /// Processes one fixed-size setup request.
    ///
    /// # Safety
    /// Non-null pointers must reference a live context and valid buffers of at
    /// least 16 and 142 bytes respectively. The output must not alias the
    /// context or request.
    // SAFETY: the exported wrapper follows fairplay_v2.h fixed lengths and documents pointer validity.
    #[unsafe(no_mangle)]
    // SAFETY: callers must pass a valid handle and the documented fixed-size input/output buffers.
    pub unsafe extern "C" fn fairplay_v2_setup(
        fairplay: *mut fairplay_v2_t,
        request: *const u8,
        response: *mut u8,
    ) -> i32 {
        // Reset the state before validating the other pointers, matching C.
        // SAFETY: a non-null context pointer must point to a live handle per the ABI contract.
        let Some(fairplay) = (unsafe { fairplay.as_mut() }) else {
            return port::INVALID;
        };
        fairplay.inner.reset_setup_state();
        if request.is_null() || response.is_null() {
            return port::INVALID;
        }
        // SAFETY: fixed sizes are part of the C ABI contract documented above.
        let request = unsafe { slice::from_raw_parts(request, 16) };
        // SAFETY: fixed sizes are part of the C ABI contract documented above.
        let response = unsafe { slice::from_raw_parts_mut(response, 142) };
        let request: &[u8; 16] = request.try_into().expect("fixed-length slice");
        let response: &mut [u8; 142] = response.try_into().expect("fixed-length slice");
        fairplay.inner.setup(request, response)
    }

    /// Processes one fixed-size handshake request.
    ///
    /// # Safety
    /// Non-null pointers must reference a live context and valid buffers of at
    /// least 164 and 32 bytes respectively. The output must not alias the
    /// context or request.
    // SAFETY: the exported wrapper follows fairplay_v2.h fixed lengths and documents pointer validity.
    #[unsafe(no_mangle)]
    // SAFETY: callers must pass a valid handle and the documented fixed-size input/output buffers.
    pub unsafe extern "C" fn fairplay_v2_handshake(
        fairplay: *mut fairplay_v2_t,
        request: *const u8,
        response: *mut u8,
    ) -> i32 {
        // Reset the state before validating the other pointers, matching C.
        // SAFETY: a non-null context pointer must point to a live handle per the ABI contract.
        let Some(fairplay) = (unsafe { fairplay.as_mut() }) else {
            return port::INVALID;
        };
        fairplay.inner.reset_handshake_state();
        if request.is_null() || response.is_null() {
            return port::INVALID;
        }
        // SAFETY: fixed sizes are part of the C ABI contract documented above.
        let request = unsafe { slice::from_raw_parts(request, 164) };
        // SAFETY: fixed sizes are part of the C ABI contract documented above.
        let response = unsafe { slice::from_raw_parts_mut(response, 32) };
        let request: &[u8; 164] = request.try_into().expect("fixed-length slice");
        let response: &mut [u8; 32] = response.try_into().expect("fixed-length slice");
        fairplay.inner.handshake(request, response)
    }

    /// Authenticates and decrypts one fixed-size media-key record.
    ///
    /// # Safety
    /// `raw_key` must be null or reference 16 writable bytes. If non-null,
    /// `fairplay` and `encrypted_key` must reference a live context and 72
    /// readable bytes respectively. Buffers must not overlap.
    // SAFETY: the exported wrapper follows fairplay_v2.h fixed lengths and documents pointer validity.
    #[unsafe(no_mangle)]
    // SAFETY: callers must pass the documented fixed-size output and, when non-null, valid input buffers.
    pub unsafe extern "C" fn fairplay_v2_decrypt_key(
        fairplay: *mut fairplay_v2_t,
        encrypted_key: *const u8,
        raw_key: *mut u8,
    ) -> i32 {
        if raw_key.is_null() {
            return port::INVALID;
        }
        // SAFETY: raw_key's documented output span is exactly 16 bytes.
        let raw_key = unsafe { slice::from_raw_parts_mut(raw_key, 16) };
        let raw_key: &mut [u8; 16] = raw_key.try_into().expect("fixed-length slice");
        raw_key.fill(0);

        // SAFETY: a non-null context pointer must point to a live handle per the ABI contract.
        let Some(fairplay) = (unsafe { fairplay.as_mut() }) else {
            return port::INVALID;
        };
        if encrypted_key.is_null() {
            return port::INVALID;
        }
        // SAFETY: fixed sizes are part of the C ABI contract documented above.
        let encrypted_key = unsafe { slice::from_raw_parts(encrypted_key, 72) };
        let encrypted_key: &[u8; 72] = encrypted_key.try_into().expect("fixed-length slice");
        fairplay.inner.decrypt_key(encrypted_key, raw_key)
    }
}

#[cfg(test)]
mod tests;
