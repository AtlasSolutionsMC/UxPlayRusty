//! Safe Rust ports of the five scalar helpers in `modified_md5.c`.
//!
//! The pointer-based `swap` and `modified_md5` exports are intentionally outside this unit's
//! bridge surface; they need a separate buffer/ownership design.

#![forbid(unsafe_code)]

/// Port of `uint32_t F(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:14`).
#[expect(
    non_snake_case,
    reason = "Preserve the source C function and parameter names."
)]
pub fn F(B: u32, C: u32, D: u32) -> u32 {
    (B & C) | (!B & D)
}

/// Port of `uint32_t G(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:19`).
#[expect(
    non_snake_case,
    reason = "Preserve the source C function and parameter names."
)]
pub fn G(B: u32, C: u32, D: u32) -> u32 {
    (B & D) | (C & !D)
}

/// Port of `uint32_t H(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:24`).
#[expect(
    non_snake_case,
    reason = "Preserve the source C function and parameter names."
)]
pub fn H(B: u32, C: u32, D: u32) -> u32 {
    B ^ C ^ D
}

/// Port of `uint32_t I(uint32_t B, uint32_t C, uint32_t D)` (`modified_md5.c:29`).
#[expect(
    non_snake_case,
    reason = "Preserve the source C function and parameter names."
)]
pub fn I(B: u32, C: u32, D: u32) -> u32 {
    C ^ (B | !D)
}

/// Port of `uint32_t rol(uint32_t input, int count)` (`modified_md5.c:35`).
pub fn rol(input: u32, count: std::ffi::c_int) -> u32 {
    input.rotate_left(count.rem_euclid(u32::BITS as i32) as u32)
}

/// Describes an input buffer with the wrong fixed length for `modified_md5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferLengthError {
    Block(usize),
    Key(usize),
}

const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

// Printed by the source C expression in `fixtures/generate_reference.c`.
const ROUND_CONSTANTS: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// Swaps the two words held in a two-element slice, matching `swap(uint32_t*, uint32_t*)`.
pub fn swap_pair(words: &mut [u32]) -> bool {
    if words.len() != 2 {
        return false;
    }
    words.swap(0, 1);
    true
}

fn nibble(value: u32, shift: u32) -> usize {
    ((value >> shift) & 0x0f) as usize
}

/// Safe slice-based port of the pointer-based `modified_md5` export.
///
/// The original reads one 64-byte block and four native-endian `uint32_t` key words, then writes
/// four native-endian output words. The byte input to each round is assembled big-endian, exactly
/// as in the source. The returned error applies only to this Rust helper; the C ABI wrapper uses
/// the source's fixed buffer lengths.
pub fn modified_md5(block: &[u8], key: &[u8]) -> Result<[u8; 16], BufferLengthError> {
    let block: &[u8; 64] = block
        .try_into()
        .map_err(|_| BufferLengthError::Block(block.len()))?;
    let key: &[u8; 16] = key
        .try_into()
        .map_err(|_| BufferLengthError::Key(key.len()))?;

    let mut block_words = [0_u32; 16];
    let (block_chunks, _) = block.as_chunks::<4>();
    for (word, bytes) in block_words.iter_mut().zip(block_chunks) {
        *word = u32::from_ne_bytes(*bytes);
    }

    let key_words: [u32; 4] = std::array::from_fn(|i| {
        let start = i * 4;
        u32::from_ne_bytes([key[start], key[start + 1], key[start + 2], key[start + 3]])
    });

    let (mut a, mut b, mut c, mut d) = (key_words[0], key_words[1], key_words[2], key_words[3]);

    for i in 0..64 {
        let word_index = match i {
            0..=15 => i,
            16..=31 => (5 * i + 1) % 16,
            32..=47 => (3 * i + 5) % 16,
            _ => (7 * i) % 16,
        };
        let input = u32::from_be_bytes(block_words[word_index].to_ne_bytes());
        let function = match i {
            0..=15 => F(b, c, d),
            16..=31 => G(b, c, d),
            32..=47 => H(b, c, d),
            _ => I(b, c, d),
        };
        let next_b = a
            .wrapping_add(input)
            .wrapping_add(ROUND_CONSTANTS[i])
            .wrapping_add(function)
            .rotate_left(SHIFTS[i])
            .wrapping_add(b);
        (a, b, c, d) = (d, next_b, b, c);

        if i == 31 {
            block_words.swap(nibble(a, 0), nibble(b, 0));
            block_words.swap(nibble(c, 0), nibble(d, 0));
            block_words.swap(nibble(a, 4), nibble(b, 4));
            block_words.swap(nibble(a, 8), nibble(b, 8));
            block_words.swap(nibble(a, 12), nibble(b, 12));
        }
    }

    let output_words = [
        key_words[0].wrapping_add(a),
        key_words[1].wrapping_add(b),
        key_words[2].wrapping_add(c),
        key_words[3].wrapping_add(d),
    ];
    let mut output = [0_u8; 16];
    let (output_chunks, _) = output.as_chunks_mut::<4>();
    for (chunk, word) in output_chunks.iter_mut().zip(output_words) {
        *chunk = word.to_ne_bytes();
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{BufferLengthError, F, G, H, I, ROUND_CONSTANTS, modified_md5, rol, swap_pair};

    fn parse_hex(value: &str) -> Option<Vec<u8>> {
        let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
        if !remainder.is_empty() {
            return None;
        }
        pairs
            .iter()
            .map(|pair| {
                let pair = std::str::from_utf8(pair).ok()?;
                u8::from_str_radix(pair, 16).ok()
            })
            .collect()
    }

    #[test]
    fn boolean_helpers_match_their_full_truth_tables() {
        // Bit n is the output for input index n, where index = B*4 + C*2 + D.
        type BooleanFn = fn(u32, u32, u32) -> u32;
        let truth_tables: [(BooleanFn, u32); 4] = [(F, 0xCA), (G, 0xE4), (H, 0x96), (I, 0x39)];
        for (function, truth_table) in truth_tables {
            for index in 0..8 {
                let [b, c, d] = [4, 2, 1].map(|bit| if index & bit == 0 { 0 } else { u32::MAX });
                let expected = if truth_table & (1 << index) == 0 {
                    0
                } else {
                    u32::MAX
                };
                assert_eq!(function(b, c, d), expected, "input index {index}");
            }
        }
    }

    #[test]
    fn rol_matches_the_defined_md5_rotation_counts() {
        assert_eq!(rol(0x1234_5678, 8), 0x3456_7812);
        assert_eq!(rol(0x1234_5678, 16), 0x5678_1234);
        assert_eq!(rol(0x1234_5678, 31), 0x091a_2b3c);
        assert_eq!(rol(0x0000_0001, 31), 0x8000_0000);
        assert_eq!(rol(u32::MAX, 15), u32::MAX);
    }

    #[test]
    fn swap_pair_swaps_exactly_two_words() {
        let mut words = [0x0123_4567, 0x89ab_cdef];
        assert!(swap_pair(&mut words));
        assert_eq!(words, [0x89ab_cdef, 0x0123_4567]);

        assert!(!swap_pair(&mut words[..1]));
        assert_eq!(words, [0x89ab_cdef, 0x0123_4567]);
    }

    #[test]
    fn c_generated_round_constants_match_the_rust_table() {
        let line = include_str!("../fixtures/reference-vectors.txt")
            .lines()
            .next()
            .unwrap_or_default();
        let mut fields = line.split_whitespace();
        assert_eq!(fields.next(), Some("round-constants"));
        let actual: Vec<u32> = fields
            .filter_map(|value| u32::from_str_radix(value, 16).ok())
            .collect();
        assert_eq!(actual, ROUND_CONSTANTS);
    }

    #[test]
    fn modified_md5_matches_c_generated_reference_vectors() {
        for line in include_str!("../fixtures/reference-vectors.txt")
            .lines()
            .skip(1)
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 4, "malformed vector: {line}");
            let block = parse_hex(fields[1]).unwrap_or_default();
            let key = parse_hex(fields[2]).unwrap_or_default();
            let expected = parse_hex(fields[3]).unwrap_or_default();
            match modified_md5(&block, &key) {
                Ok(actual) => assert_eq!(actual.as_slice(), expected, "vector {}", fields[0]),
                Err(error) => panic!("invalid static vector {}: {error:?}", fields[0]),
            }
        }
    }

    #[test]
    fn modified_md5_reports_invalid_slice_lengths() {
        assert_eq!(
            modified_md5(&[0; 63], &[0; 16]),
            Err(BufferLengthError::Block(63))
        );
        assert_eq!(
            modified_md5(&[0; 64], &[0; 15]),
            Err(BufferLengthError::Key(15))
        );
    }
}
