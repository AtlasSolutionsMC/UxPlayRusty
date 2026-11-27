//! Narrow C ABI wrappers for the two fixed-size pointer exports in `modified_md5.c`.
//!
//! All algorithm work lives in safe slice-based helpers in `port.rs`. These wrappers copy the
//! caller's fixed-size buffers into owned Rust arrays before doing any mutation, so input/output
//! aliasing does not create overlapping Rust references.

use crate::port;
use std::ptr;

// SAFETY: callers must uphold the validity, alignment, writability and non-overlap contract stated
// by the public C ABI wrappers below.
unsafe fn swap_impl(a: *mut u32, b: *mut u32) {
    if a.is_null() || b.is_null() || a == b {
        return;
    }

    // SAFETY: the caller must provide two valid, aligned u32 locations; equal pointers were
    // handled above, and distinct aligned u32 locations do not overlap.
    let mut words = [unsafe { a.read() }, unsafe { b.read() }];
    let _ = port::swap_pair(&mut words);

    // SAFETY: the same caller preconditions keep both destinations writable and non-overlapping.
    unsafe {
        a.write(words[0]);
        b.write(words[1]);
    }
}

/// C ABI for `void swap(uint32_t *a, uint32_t *b)`.
///
/// # Safety
/// Each non-null pointer must refer to a readable and writable aligned `u32` for the duration of
/// the call. The function accepts equal pointers as a no-op; distinct pointers must not overlap.
#[cfg(not(feature = "shadow"))]
// SAFETY: this symbol replaces the source C function; its pointer contract is documented above.
#[unsafe(no_mangle)]
// SAFETY: callers must uphold this wrapper's documented pointer preconditions.
pub unsafe extern "C" fn swap(a: *mut u32, b: *mut u32) {
    // SAFETY: forwarded caller preconditions are exactly those documented for this wrapper.
    unsafe { swap_impl(a, b) };
}

/// Shadow-build alias for `swap`.
///
/// # Safety
/// Each non-null pointer must refer to a readable and writable aligned `u32` for the duration of
/// the call. The function accepts equal pointers as a no-op; distinct pointers must not overlap.
// SAFETY: the `rust_anything_` name is unique alongside the original C symbol.
#[unsafe(no_mangle)]
// SAFETY: callers must uphold this wrapper's documented pointer preconditions.
pub unsafe extern "C" fn rust_anything_swap(a: *mut u32, b: *mut u32) {
    // SAFETY: forwarded caller preconditions are exactly those documented for this wrapper.
    unsafe { swap_impl(a, b) };
}

// SAFETY: callers must uphold the fixed readable/writable buffer sizes stated by public wrappers.
unsafe fn modified_md5_impl(block: *const u8, key: *const u8, output: *mut u8) {
    if block.is_null() || key.is_null() || output.is_null() {
        return;
    }

    let mut block_copy = [0_u8; 64];
    let mut key_copy = [0_u8; 16];

    // SAFETY: the caller provides 64 readable bytes; `ptr::copy` permits source aliasing with
    // either other input or output buffers, and the local destination cannot alias the caller.
    unsafe { ptr::copy(block, block_copy.as_mut_ptr(), block_copy.len()) };
    // SAFETY: the caller provides 16 readable bytes; destination is a separate local array.
    unsafe { ptr::copy(key, key_copy.as_mut_ptr(), key_copy.len()) };

    let Ok(result) = port::modified_md5(&block_copy, &key_copy) else {
        return;
    };

    // SAFETY: the caller provides 16 writable bytes; `ptr::copy` permits overlap with either
    // source buffer, both of which have already been copied into local arrays.
    unsafe { ptr::copy(result.as_ptr(), output, result.len()) };
}

/// C ABI for `void modified_md5(unsigned char *block, unsigned char *key, unsigned char *out)`.
///
/// # Safety
/// `block` must point to 64 readable bytes, `key` to 16 readable bytes, and `output` to 16 writable
/// bytes for the duration of the call. The buffers may alias because inputs are copied before the
/// output is written. Null pointers are treated as a no-op; the C source does not define them.
#[cfg(not(feature = "shadow"))]
// SAFETY: this symbol replaces the source C function; fixed buffer sizes are documented above.
#[unsafe(no_mangle)]
// SAFETY: callers must uphold this wrapper's documented pointer preconditions.
pub unsafe extern "C" fn modified_md5(block: *const u8, key: *const u8, output: *mut u8) {
    // SAFETY: forwarded caller preconditions are exactly those documented for this wrapper.
    unsafe { modified_md5_impl(block, key, output) };
}

/// Shadow-build alias for `modified_md5`.
///
/// # Safety
/// `block` must point to 64 readable bytes, `key` to 16 readable bytes, and `output` to 16 writable
/// bytes for the duration of the call. The buffers may alias because inputs are copied before the
/// output is written. Null pointers are treated as a no-op; the C source does not define them.
// SAFETY: the `rust_anything_` name is unique alongside the original C symbol.
#[unsafe(no_mangle)]
// SAFETY: callers must uphold this wrapper's documented pointer preconditions.
pub unsafe extern "C" fn rust_anything_modified_md5(
    block: *const u8,
    key: *const u8,
    output: *mut u8,
) {
    // SAFETY: forwarded caller preconditions are exactly those documented for this wrapper.
    unsafe { modified_md5_impl(block, key, output) };
}

#[cfg(test)]
mod tests {
    use super::{modified_md5, swap};
    use crate::port;

    #[test]
    fn swap_ffi_wrapper_swaps_two_words_and_handles_aliasing() {
        let mut first = 0x0123_4567;
        let mut second = 0x89ab_cdef;
        // SAFETY: both pointers refer to live, aligned, non-overlapping u32 locals.
        unsafe { swap(&mut first, &mut second) };
        assert_eq!([first, second], [0x89ab_cdef, 0x0123_4567]);

        // SAFETY: the equal pointer refers to one live, aligned, writable u32; the wrapper returns
        // before constructing any references or writing through it.
        unsafe { swap(&mut first, &mut first) };
        assert_eq!(first, 0x89ab_cdef);
    }

    #[test]
    fn modified_md5_ffi_wrapper_matches_the_safe_slice_helper() {
        let block: [u8; 64] = std::array::from_fn(|i| (i * 37 + 11) as u8);
        let key = [0x10; 16];
        let expected = port::modified_md5(&block, &key).unwrap_or([0; 16]);
        let mut output = [0_u8; 16];
        // SAFETY: all three pointers refer to live arrays of the exact source-defined lengths.
        unsafe { modified_md5(block.as_ptr(), key.as_ptr(), output.as_mut_ptr()) };
        assert_eq!(output, expected);
    }
}
