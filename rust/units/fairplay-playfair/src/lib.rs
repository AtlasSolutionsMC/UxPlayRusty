//! Rust port of `fairplay_playfair.c` and its legacy `playfair_decrypt` path.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod port;

#[repr(C)]
pub struct fairplay_t {
    inner: port::FairPlay,
}

mod ffi {
    use super::{fairplay_t, port};
    use std::{
        alloc::{self, Layout},
        ffi::c_void,
        ptr, slice,
    };

    /// Creates the FairPlay adapter state. The logger pointer is retained as an opaque handle.
    ///
    /// # Safety
    /// A returned pointer must be destroyed exactly once with `fairplay_destroy`.
    // SAFETY: this ABI export returns a Rust-allocated opaque handle for the matching destroy export.
    #[unsafe(no_mangle)]
    pub extern "C" fn fairplay_init(logger: *mut c_void) -> *mut fairplay_t {
        // SAFETY: alloc returns null or a correctly aligned region for this nonzero layout.
        let allocation = unsafe { alloc::alloc(Layout::new::<fairplay_t>()).cast::<fairplay_t>() };
        if allocation.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: allocation is writable for one fairplay_t and initialization has no fallible work.
        unsafe {
            ptr::write(
                allocation,
                fairplay_t {
                    inner: port::FairPlay::new(logger),
                },
            )
        };
        allocation
    }

    /// Resets and destroys a context. Null is accepted.
    ///
    /// # Safety
    /// `fp` must be null or a live pointer returned by `fairplay_init`, destroyed no more than once.
    // SAFETY: this ABI export accepts only a null or init-owned pointer as documented.
    #[unsafe(no_mangle)]
    // SAFETY: caller must provide a null or live fairplay_init handle.
    pub unsafe extern "C" fn fairplay_destroy(fp: *mut fairplay_t) {
        if !fp.is_null() {
            // SAFETY: the caller contract guarantees a live init-produced handle.
            unsafe { ptr::drop_in_place(fp) };
            // SAFETY: fairplay_init allocated this exact layout with alloc::alloc.
            unsafe { alloc::dealloc(fp.cast(), Layout::new::<fairplay_t>()) };
        }
    }

    /// Applies one fixed-size setup request.
    ///
    /// # Safety
    /// `fp`, `req`, and `res` must reference a live context and readable/writable buffers of 16/142 bytes.
    // SAFETY: the export matches fairplay.h and documents each pointer range.
    #[unsafe(no_mangle)]
    // SAFETY: caller must supply a live context and fixed-size input/output buffers.
    pub unsafe extern "C" fn fairplay_setup(
        fp: *mut fairplay_t,
        req: *const u8,
        res: *mut u8,
    ) -> i32 {
        // SAFETY: the documented contract requires a live context pointer.
        let Some(fp) = (unsafe { fp.as_mut() }) else {
            return port::INVALID;
        };
        if req.is_null() || res.is_null() {
            fp.inner.reset();
            return port::INVALID;
        }
        // SAFETY: the documented fixed-size buffers are exactly 16 and 142 bytes.
        let req = unsafe { slice::from_raw_parts(req, 16) };
        // SAFETY: the documented fixed-size buffers are exactly 16 and 142 bytes.
        let res = unsafe { slice::from_raw_parts_mut(res, 142) };
        let Ok(request) = <&[u8; 16]>::try_from(req) else {
            return port::INVALID;
        };
        let Ok(response) = <&mut [u8; 142]>::try_from(res) else {
            return port::INVALID;
        };
        fp.inner.setup(request, response)
    }

    /// Processes one fixed-size handshake request.
    ///
    /// # Safety
    /// `fp`, `req`, and `res` must reference a live context and readable/writable buffers of 164/32 bytes.
    // SAFETY: the export matches fairplay.h and documents each pointer range.
    #[unsafe(no_mangle)]
    // SAFETY: caller must supply a live context and fixed-size input/output buffers.
    pub unsafe extern "C" fn fairplay_handshake(
        fp: *mut fairplay_t,
        req: *const u8,
        res: *mut u8,
    ) -> i32 {
        // SAFETY: the documented contract requires a live context pointer.
        let Some(fp) = (unsafe { fp.as_mut() }) else {
            return port::INVALID;
        };
        if req.is_null() || res.is_null() {
            fp.inner.reset();
            return port::INVALID;
        }
        // SAFETY: the documented fixed-size buffers are exactly 164 and 32 bytes.
        let req = unsafe { slice::from_raw_parts(req, 164) };
        // SAFETY: the documented fixed-size buffers are exactly 164 and 32 bytes.
        let res = unsafe { slice::from_raw_parts_mut(res, 32) };
        let Ok(request) = <&[u8; 164]>::try_from(req) else {
            return port::INVALID;
        };
        let Ok(response) = <&mut [u8; 32]>::try_from(res) else {
            return port::INVALID;
        };
        fp.inner.handshake(request, response)
    }

    /// Decrypts a 72-byte legacy or selector-2 key record into 16 bytes.
    ///
    /// # Safety
    /// `fp`, `input`, and `output` must reference a live context and readable/writable buffers of 72/16 bytes.
    // SAFETY: the export matches fairplay.h and documents each pointer range.
    #[unsafe(no_mangle)]
    // SAFETY: caller must supply a live context and fixed-size input/output buffers.
    pub unsafe extern "C" fn fairplay_decrypt(
        fp: *mut fairplay_t,
        input: *const u8,
        output: *mut u8,
    ) -> i32 {
        if output.is_null() {
            return port::INVALID;
        }
        // SAFETY: the documented fixed-size output buffer is 16 bytes.
        let output = unsafe { slice::from_raw_parts_mut(output, 16) };
        output.fill(0);
        // SAFETY: the documented contract requires a live context pointer.
        let Some(fp) = (unsafe { fp.as_ref() }) else {
            return port::INVALID;
        };
        if input.is_null() {
            return port::INVALID;
        }
        // SAFETY: the documented fixed-size input buffer is 72 bytes.
        let input = unsafe { slice::from_raw_parts(input, 72) };
        let Ok(input) = <&[u8; 72]>::try_from(input) else {
            return port::INVALID;
        };
        let Ok(output) = <&mut [u8; 16]>::try_from(output) else {
            return port::INVALID;
        };
        fp.inner.decrypt(input, output)
    }
}

#[cfg(test)]
mod tests;
