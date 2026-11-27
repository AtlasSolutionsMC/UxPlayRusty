//! Safe byte and NTP helpers with explicit little-/big-endian behavior.

pub const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeError {
    pub offset: usize,
    pub required: usize,
    pub available: usize,
}

fn range(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], RangeError> {
    let end = offset.checked_add(len).ok_or(RangeError {
        offset,
        required: len,
        available: bytes.len().saturating_sub(offset),
    })?;
    bytes.get(offset..end).ok_or(RangeError {
        offset,
        required: len,
        available: bytes.len().saturating_sub(offset),
    })
}

fn range_mut(bytes: &mut [u8], offset: usize, len: usize) -> Result<&mut [u8], RangeError> {
    let available = bytes.len().saturating_sub(offset);
    let end = offset.checked_add(len).ok_or(RangeError {
        offset,
        required: len,
        available,
    })?;
    bytes.get_mut(offset..end).ok_or(RangeError {
        offset,
        required: len,
        available,
    })
}

pub fn get_short(bytes: &[u8], offset: usize) -> Result<u16, RangeError> {
    Ok(u16::from_le_bytes(
        range(bytes, offset, 2)?.try_into().expect("two-byte range"),
    ))
}

pub fn get_int(bytes: &[u8], offset: usize) -> Result<u32, RangeError> {
    Ok(u32::from_le_bytes(
        range(bytes, offset, 4)?
            .try_into()
            .expect("four-byte range"),
    ))
}

pub fn get_long(bytes: &[u8], offset: usize) -> Result<u64, RangeError> {
    Ok(u64::from_le_bytes(
        range(bytes, offset, 8)?
            .try_into()
            .expect("eight-byte range"),
    ))
}

pub fn get_short_be(bytes: &[u8], offset: usize) -> Result<u16, RangeError> {
    Ok(u16::from_be_bytes(
        range(bytes, offset, 2)?.try_into().expect("two-byte range"),
    ))
}

pub fn get_int_be(bytes: &[u8], offset: usize) -> Result<u32, RangeError> {
    Ok(u32::from_be_bytes(
        range(bytes, offset, 4)?
            .try_into()
            .expect("four-byte range"),
    ))
}

pub fn get_long_be(bytes: &[u8], offset: usize) -> Result<u64, RangeError> {
    Ok(u64::from_be_bytes(
        range(bytes, offset, 8)?
            .try_into()
            .expect("eight-byte range"),
    ))
}

pub fn get_float(bytes: &[u8], offset: usize) -> Result<f32, RangeError> {
    Ok(f32::from_bits(get_int(bytes, offset)?))
}

pub fn put_long_be(bytes: &mut [u8], offset: usize, value: u64) -> Result<(), RangeError> {
    range_mut(bytes, offset, 8)?.copy_from_slice(&value.to_be_bytes());
    Ok(())
}

pub fn get_ntp_timestamp(bytes: &[u8], offset: usize) -> Result<u64, RangeError> {
    let ntp = range(bytes, offset, 8)?;
    let seconds = u64::from(u32::from_be_bytes(ntp[..4].try_into().expect("four bytes")))
        .wrapping_sub(NTP_EPOCH_OFFSET);
    let fraction = u64::from(u32::from_be_bytes(ntp[4..].try_into().expect("four bytes")));
    Ok(seconds
        .wrapping_mul(NANOSECONDS_PER_SECOND)
        .wrapping_add((fraction * NANOSECONDS_PER_SECOND) >> 32))
}

pub fn put_ntp_timestamp(
    bytes: &mut [u8],
    offset: usize,
    ns_since_1970: u64,
) -> Result<(), RangeError> {
    let seconds = (ns_since_1970 / NANOSECONDS_PER_SECOND).wrapping_add(NTP_EPOCH_OFFSET) as u32;
    let nanoseconds = ns_since_1970 % NANOSECONDS_PER_SECOND;
    let fraction = ((nanoseconds << 32) / NANOSECONDS_PER_SECOND) as u32;
    let target = range_mut(bytes, offset, 8)?;
    target[..4].copy_from_slice(&seconds.to_be_bytes());
    target[4..].copy_from_slice(&fraction.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NTP_EPOCH_OFFSET, RangeError, get_float, get_int, get_int_be, get_long, get_long_be,
        get_ntp_timestamp, get_short, get_short_be, put_long_be, put_ntp_timestamp,
    };

    #[test]
    fn endian_getters_and_float_keep_the_c_little_endian_host_layout() {
        let bytes: [u8; 16] = [
            0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x90, 0x78, 0x56, 0x00, 0x00,
            0x80, 0x3f,
        ];
        assert_eq!(get_short(&bytes, 0), Ok(0x1234));
        assert_eq!(get_int(&bytes, 0), Ok(0x5678_1234));
        assert_eq!(get_long(&bytes, 0), Ok(0xcdef_1234_5678_1234));
        assert_eq!(get_short_be(&bytes, 0), Ok(0x3412));
        assert_eq!(get_int_be(&bytes, 0), Ok(0x3412_7856));
        assert_eq!(get_long_be(&bytes, 0), Ok(0x3412_7856_3412_efcd));
        assert_eq!(get_float(&bytes, 12), Ok(1.0));
    }

    #[test]
    fn setters_write_exactly_big_endian_bytes_at_the_requested_offset() {
        let mut bytes = [0xaa; 16];
        put_long_be(&mut bytes, 3, 0x0123_4567_89ab_cdef).unwrap();
        assert_eq!(
            bytes,
            [
                0xaa, 0xaa, 0xaa, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xaa, 0xaa, 0xaa,
                0xaa, 0xaa
            ]
        );
    }

    #[test]
    fn ntp_encode_decode_preserves_epoch_fraction_and_wrapping_rules() {
        let mut bytes = [0; 8];
        put_ntp_timestamp(&mut bytes, 0, 1_700_000_000_123_456_789).unwrap();
        assert_eq!(bytes, [0xe8, 0xfe, 0x6f, 0x80, 0x1f, 0x9a, 0xdd, 0x37]);
        assert_eq!(get_ntp_timestamp(&bytes, 0), Ok(1_700_000_000_123_456_788));

        let before_unix_epoch = [0, 0, 0, 0, 0, 0, 0, 0];
        let seconds = 0_u64.wrapping_sub(NTP_EPOCH_OFFSET);
        let expected = seconds.wrapping_mul(1_000_000_000);
        assert_eq!(get_ntp_timestamp(&before_unix_epoch, 0), Ok(expected));
    }

    #[test]
    fn range_errors_cover_short_and_overflowing_offsets() {
        let bytes = [0_u8; 4];
        assert_eq!(
            get_long(&bytes, 0),
            Err(RangeError {
                offset: 0,
                required: 8,
                available: 4,
            })
        );
        assert!(get_int(&bytes, usize::MAX).is_err());
    }

    #[test]
    fn all_exports_match_gcc_reference_vectors() {
        const TIMES_NS: [u64; 5] = [
            0,
            1,
            1_700_000_000_123_456_789,
            u64::MAX,
            2_208_988_800_000_000_000,
        ];
        for (line_number, line) in include_str!("../reference-vectors.tsv").lines().enumerate() {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(
                fields.len(),
                13,
                "reference row {} columns",
                line_number + 1
            );
            let pattern: usize = fields[0].parse().expect("case index");
            let bytes = parse_hex(fields[1]);
            let offset: usize = fields[2].parse().expect("offset");

            assert_eq!(get_short(&bytes, offset).unwrap(), parse_hex_int(fields[3]));
            assert_eq!(get_int(&bytes, offset).unwrap(), parse_hex_int(fields[4]));
            assert_eq!(get_long(&bytes, offset).unwrap(), parse_hex_int(fields[5]));
            assert_eq!(
                get_short_be(&bytes, offset).unwrap(),
                parse_hex_int(fields[6])
            );
            assert_eq!(
                get_int_be(&bytes, offset).unwrap(),
                parse_hex_int(fields[7])
            );
            assert_eq!(
                get_long_be(&bytes, offset).unwrap(),
                parse_hex_int(fields[8])
            );
            assert_eq!(
                get_float(&bytes, offset).unwrap().to_bits(),
                parse_hex_int(fields[9])
            );
            assert_eq!(
                get_ntp_timestamp(&bytes, offset).unwrap(),
                fields[10].parse::<u64>().unwrap()
            );

            let mut long_output = bytes.clone();
            put_long_be(&mut long_output, offset, 0x0123_4567_89ab_cdef).unwrap();
            assert_eq!(long_output, parse_hex(fields[11]));

            let mut ntp_output = bytes;
            put_ntp_timestamp(&mut ntp_output, offset, TIMES_NS[pattern]).unwrap();
            assert_eq!(ntp_output, parse_hex(fields[12]));
        }
    }

    fn parse_hex(hex: &str) -> Vec<u8> {
        let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
        assert!(remainder.is_empty(), "hex fixture length is even");
        pairs
            .iter()
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("hex byte")
            })
            .collect()
    }

    fn parse_hex_int<T>(hex: &str) -> T
    where
        T: TryFrom<u64>,
    {
        T::try_from(u64::from_str_radix(hex, 16).expect("hex integer"))
            .unwrap_or_else(|_| panic!("hex integer does not fit target type: {hex}"))
    }
}
