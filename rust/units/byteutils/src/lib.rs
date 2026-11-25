//! Byte-order and NTP timestamp helpers from UxPlay's `lib/byteutils.c`.

pub mod port;

pub mod ffi {
    use crate::port;
    use std::ffi::c_int;
    use std::slice;

    /// Returns a C-compatible view of bytes at `offset`.
    ///
    /// # Safety
    /// The caller must provide a non-null pointer to a buffer with at least `offset + len` readable
    /// bytes, and `offset` must be nonnegative.
    unsafe fn read<'a>(buffer: *const u8, offset: c_int, len: usize) -> &'a [u8] {
        // SAFETY: required by this helper's caller contract.
        let start = unsafe { buffer.add(offset as usize) };
        // SAFETY: `start` points into the caller-provided readable buffer for `len` bytes.
        unsafe { slice::from_raw_parts(start, len) }
    }

    /// Returns a C-compatible mutable view of bytes at `offset`.
    ///
    /// # Safety
    /// The caller must provide a non-null pointer to a buffer with at least `offset + len` writable
    /// bytes, and `offset` must be nonnegative.
    unsafe fn write<'a>(buffer: *mut u8, offset: c_int, len: usize) -> &'a mut [u8] {
        // SAFETY: required by this helper's caller contract.
        let start = unsafe { buffer.add(offset as usize) };
        // SAFETY: `start` points into the caller-provided writable buffer for `len` bytes.
        unsafe { slice::from_raw_parts_mut(start, len) }
    }

    // Explicit wrappers keep each unsafe pointer contract visible at the FFI boundary.
    /// `uint16_t byteutils_get_short(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 2` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_short(buffer: *mut u8, offset: c_int) -> u16 {
        // SAFETY: required by the exported function's caller contract.
        u16::from_le_bytes(
            unsafe { read(buffer, offset, 2) }
                .try_into()
                .expect("two bytes"),
        )
    }

    /// `uint16_t byteutils_get_short_be(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 2` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_short_be(buffer: *mut u8, offset: c_int) -> u16 {
        // SAFETY: required by the exported function's caller contract.
        u16::from_be_bytes(
            unsafe { read(buffer, offset, 2) }
                .try_into()
                .expect("two bytes"),
        )
    }

    /// `uint32_t byteutils_get_int(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 4` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_int(buffer: *mut u8, offset: c_int) -> u32 {
        // SAFETY: required by the exported function's caller contract.
        u32::from_le_bytes(
            unsafe { read(buffer, offset, 4) }
                .try_into()
                .expect("four bytes"),
        )
    }

    /// `uint32_t byteutils_get_int_be(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 4` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_int_be(buffer: *mut u8, offset: c_int) -> u32 {
        // SAFETY: required by the exported function's caller contract.
        u32::from_be_bytes(
            unsafe { read(buffer, offset, 4) }
                .try_into()
                .expect("four bytes"),
        )
    }

    /// `uint64_t byteutils_get_long(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 8` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_long(buffer: *mut u8, offset: c_int) -> u64 {
        // SAFETY: required by the exported function's caller contract.
        u64::from_le_bytes(
            unsafe { read(buffer, offset, 8) }
                .try_into()
                .expect("eight bytes"),
        )
    }

    /// `uint64_t byteutils_get_long_be(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 8` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_long_be(buffer: *mut u8, offset: c_int) -> u64 {
        // SAFETY: required by the exported function's caller contract.
        u64::from_be_bytes(
            unsafe { read(buffer, offset, 8) }
                .try_into()
                .expect("eight bytes"),
        )
    }

    /// `float byteutils_get_float(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 4` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_float(buffer: *mut u8, offset: c_int) -> f32 {
        // SAFETY: required by the exported function's caller contract.
        f32::from_bits(u32::from_le_bytes(
            unsafe { read(buffer, offset, 4) }
                .try_into()
                .expect("four bytes"),
        ))
    }

    /// `uint64_t byteutils_get_ntp_timestamp(unsigned char*, int)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 8` readable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_get_ntp_timestamp(buffer: *mut u8, offset: c_int) -> u64 {
        // SAFETY: required by the exported function's caller contract.
        port::get_ntp_timestamp(unsafe { read(buffer, offset, 8) }, 0).expect("eight-byte range")
    }

    /// `void byteutils_put_long_be(unsigned char*, int, uint64_t)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 8` writable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_put_long_be(buffer: *mut u8, offset: c_int, value: u64) {
        // SAFETY: required by the exported function's caller contract.
        unsafe { write(buffer, offset, 8) }.copy_from_slice(&value.to_be_bytes());
    }

    /// `void byteutils_put_ntp_timestamp(unsigned char*, int, uint64_t)`.
    ///
    /// # Safety
    /// `buffer` must point to at least `offset + 8` writable bytes and `offset` must be nonnegative.
    #[cfg(not(feature = "shadow"))]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn byteutils_put_ntp_timestamp(
        buffer: *mut u8,
        offset: c_int,
        ns_since_1970: u64,
    ) {
        let mut encoded = [0_u8; 8];
        port::put_ntp_timestamp(&mut encoded, 0, ns_since_1970).expect("eight-byte range");
        // SAFETY: required by the exported function's caller contract.
        unsafe { write(buffer, offset, 8) }.copy_from_slice(&encoded);
    }
}
