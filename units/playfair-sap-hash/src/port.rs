//! The defined-domain Rust port of the two scalar helpers in `sap_hash.c`.
//!
//! `rol8` is only defined by the original C expression for counts in 0..=8.
//! `rol8x` has the same count domain and intentionally returns the 32-bit
//! expression, which can contain more than eight bits.

#![forbid(unsafe_code)]

/// Port of `unsigned char rol8(unsigned char input, int count)` (`sap_hash.c:9`).
pub fn rol8(input: std::ffi::c_uchar, count: std::ffi::c_int) -> std::ffi::c_uchar {
    input.rotate_left(count.rem_euclid(8) as u32)
}

/// Port of `uint32_t rol8x(unsigned char input, int count)` (`sap_hash.c:14`).
pub fn rol8x(input: std::ffi::c_uchar, count: std::ffi::c_int) -> u32 {
    if !(0..=8).contains(&count) {
        // Outside this range the source expression has an invalid shift count.
        return 0;
    }

    let count = count as u32;
    (u32::from(input) << count) | (u32::from(input) >> (8 - count))
}

/// Safe fixed-size implementation of `sap_hash` (`sap_hash.c:20`).
///
/// The C implementation assumes a little-endian host and reads exactly 64 input bytes, then writes
/// exactly 16 output bytes. This API makes those buffer sizes explicit and preserves its
/// 32-bit-index wraparound and byte order.
pub fn sap_hash(block_in: &[u8; 64]) -> [u8; 16] {
    const BUFFER0_INIT: [u8; 20] = [
        0x96, 0x5f, 0xc6, 0x53, 0xf8, 0x46, 0xcc, 0x18, 0xdf, 0xbe, 0xb2, 0xf8, 0x38, 0xd7, 0xec,
        0x22, 0x03, 0xd1, 0x20, 0x8f,
    ];
    const BUFFER2_INIT: [u8; 35] = [
        0x43, 0x54, 0x62, 0x7a, 0x18, 0xc3, 0xd6, 0xb3, 0x9a, 0x56, 0xf6, 0x1c, 0x14, 0x3f, 0x0c,
        0x1d, 0x3b, 0x36, 0x83, 0xb1, 0x39, 0x51, 0x4a, 0xaa, 0x09, 0x3e, 0xfe, 0x44, 0xaf, 0xde,
        0xc3, 0x20, 0x9d, 0x42, 0x3a,
    ];
    const BUFFER4_INIT: [u8; 21] = [
        0xed, 0x25, 0xd1, 0xbb, 0xbc, 0x27, 0x9f, 0x02, 0xa2, 0xa9, 0x11, 0x00, 0x0c, 0xb3, 0x52,
        0xc0, 0xbd, 0xe3, 0x1b, 0x49, 0xc7,
    ];
    const OUTPUT_INDICES: [usize; 11] = [18, 22, 23, 0, 5, 19, 32, 31, 10, 21, 30];

    let mut buffer0 = BUFFER0_INIT;
    let mut buffer1 = [0_u8; 210];
    let mut buffer2 = BUFFER2_INIT;
    let mut buffer3 = [0_u8; 132];
    let buffer4 = BUFFER4_INIT;

    // The C implementation reads 16 little-endian words, then emits each word's bytes most
    // significant first into the 210-byte working buffer.
    for (index, byte) in buffer1.iter_mut().enumerate() {
        let word_offset = (index % 64) & !3;
        let word = u32::from_le_bytes([
            block_in[word_offset],
            block_in[word_offset + 1],
            block_in[word_offset + 2],
            block_in[word_offset + 3],
        ]);
        let shift = (3 - (index % 4)) * 8;
        *byte = (word >> shift) as u8;
    }

    for index in 0..840_usize {
        let x = buffer1[index.wrapping_sub(155) as u32 as usize % 210];
        let y = buffer1[index.wrapping_sub(57) as u32 as usize % 210];
        let z = buffer1[index.wrapping_sub(13) as u32 as usize % 210];
        let w = buffer1[index % 210];
        buffer1[index % 210] = rol8(y, 5)
            .wrapping_add(rol8(z, 3) ^ w)
            .wrapping_sub(rol8(x, 7));
    }

    hand_garble_rs::port::garble(
        &mut buffer0,
        &mut buffer1,
        &mut buffer2,
        &mut buffer3,
        &buffer4,
    );

    let mut key_out = [0xe1_u8; 16];
    for (index, source_index) in OUTPUT_INDICES.into_iter().enumerate() {
        if index == 3 {
            key_out[index] = 0x3d;
        } else {
            key_out[index] = key_out[index].wrapping_add(buffer3[source_index * 4]);
        }
    }
    for (index, byte) in buffer0.into_iter().enumerate() {
        key_out[index % 16] ^= byte;
    }
    for (index, byte) in buffer2.into_iter().enumerate() {
        key_out[index % 16] ^= byte;
    }
    for (index, byte) in buffer1.into_iter().enumerate() {
        key_out[index % 16] ^= byte;
    }

    for _ in 0..16 {
        for index in 0..16_usize {
            let x = key_out[index.wrapping_sub(7) as u32 as usize % 16];
            let y = key_out[index];
            let z = key_out[index.wrapping_sub(37) as u32 as usize % 16];
            let w = key_out[index.wrapping_sub(177) as u32 as usize % 16];
            key_out[index] = rol8(x, 1) ^ y ^ rol8(z, 6) ^ rol8(w, 5);
        }
    }
    key_out
}

#[cfg(test)]
mod tests {
    use super::{rol8, rol8x, sap_hash};

    #[test]
    fn rol8_wraps_within_eight_bits() {
        assert_eq!(rol8(0x01, 0), 0x01);
        assert_eq!(rol8(0x01, 7), 0x80);
        assert_eq!(rol8(0x80, 1), 0x01);
        assert_eq!(rol8(0xff, 8), 0xff);
    }

    #[test]
    fn rol8x_preserves_the_source_32_bit_expression() {
        assert_eq!(rol8x(0x01, 0), 0x01);
        assert_eq!(rol8x(0x01, 8), 0x101);
        assert_eq!(rol8x(0xff, 1), 0x1ff);
        assert_eq!(rol8x(0xff, 8), 0xffff);
    }

    #[test]
    fn rol8x_handles_counts_outside_the_defined_c_domain_without_panicking() {
        assert_eq!(rol8x(0xff, -1), 0);
        assert_eq!(rol8x(0xff, 9), 0);
    }

    #[test]
    fn sap_hash_matches_c_reference_vectors() {
        for (line_number, line) in include_str!("../sap-hash-cases.tsv").lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (input, expected) = line.split_once('\t').expect("input/output TSV row");
            let input: Vec<u8> = input
                .split_ascii_whitespace()
                .map(|byte| u8::from_str_radix(byte, 16).expect("hex input byte"))
                .collect();
            let block: [u8; 64] = input
                .try_into()
                .unwrap_or_else(|_| panic!("line {} must contain 64 input bytes", line_number + 1));
            let expected: Vec<u8> = expected
                .split_ascii_whitespace()
                .map(|byte| u8::from_str_radix(byte, 16).expect("hex output byte"))
                .collect();
            let expected: [u8; 16] = expected.try_into().unwrap_or_else(|_| {
                panic!("line {} must contain 16 output bytes", line_number + 1)
            });
            assert_eq!(
                sap_hash(&block),
                expected,
                "reference vector line {}",
                line_number + 1
            );
        }
    }
}
