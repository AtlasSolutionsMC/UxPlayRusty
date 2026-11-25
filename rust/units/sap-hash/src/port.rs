//! The Rust port of `lib/playfair/sap_hash.c`: safe Rust; `lib.rs` exports each function under its C name.
//!
//! Replace each `unimplemented!` with the port of the C function. C has no error channel:
//! return what the C code returns, and never panic (a panic aborts at the C boundary).

#![forbid(unsafe_code)]

/// Port of `unsigned char rol8(unsigned char input, int count)` (`lib/playfair/sap_hash.c:9`).
pub fn rol8(input: std::ffi::c_uchar, count: std::ffi::c_int) -> std::ffi::c_uchar {
    if !(0..=8).contains(&count) {
        return 0;
    }
    let value = u32::from(input);
    let count = count as u32;
    (((value << count) & 0xff) | (value >> (8 - count))) as std::ffi::c_uchar
}

/// Port of `uint32_t rol8x(unsigned char input, int count)` (`lib/playfair/sap_hash.c:14`).
pub fn rol8x(input: std::ffi::c_uchar, count: std::ffi::c_int) -> u32 {
    if !(0..=8).contains(&count) {
        return 0;
    }
    let value = u32::from(input);
    let count = count as u32;
    (value << count) | (value >> (8 - count))
}

#[cfg(test)]
mod tests {
    use super::{rol8, rol8x};

    #[test]
    fn rol8_matches_the_source_expression_on_its_defined_domain() {
        assert_eq!(rol8(0x01, 0), 0x01);
        assert_eq!(rol8(0x80, 1), 0x01);
        assert_eq!(rol8(0x01, 7), 0x80);
        assert_eq!(rol8(0xff, 8), 0xff);
        assert_eq!(rol8(0xff, -1), 0);
        assert_eq!(rol8(0xff, 9), 0);
    }

    #[test]
    fn rol8x_preserves_the_unmasked_source_expression() {
        assert_eq!(rol8x(0x01, 0), 0x01);
        assert_eq!(rol8x(0xff, 1), 0x1ff);
        assert_eq!(rol8x(0x01, 8), 0x101);
        assert_eq!(rol8x(0xff, 8), 0xffff);
        assert_eq!(rol8x(0xff, -1), 0);
        assert_eq!(rol8x(0xff, 9), 0);
    }
}
