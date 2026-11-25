//! AirPlay and RAOP mDNS TXT record bytes from `dnssd_mdnsd.c`.
//!
//! This builds service metadata only; publishing and mDNS packet handling live
//! in a separate discovery layer.

const MAX_TXT_ENTRY_BYTES: usize = u8::MAX as usize;
const MAX_TXT_RECORD_BYTES: usize = 512;
const MAX_HWADDR_LEN: usize = 6;

const AIRPLAY_PI: &str = "2e388006-13ba-4041-9a67-25dd4a43d536";

#[derive(Clone, Copy, Debug)]
pub struct TxtRecordInputs<'a> {
    pub model: &'a str,
    pub version: &'a str,
    pub public_key: &'a str,
    pub hardware_address: &'a [u8],
    pub features1: u32,
    pub features2: u32,
    /// 0: no PIN; 1: legacy PIN; 2 or 3: other PIN/password modes.
    pub pin_mode: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxtRecordError {
    EntryLengthOverflow { entry_len: usize },
    RecordCapacityExceeded { record_len: usize },
    HardwareAddressUnsupportedLength { address_len: usize },
}

/// Build the source-ordered `_raop._tcp` TXT bytes.
pub fn build_raop_txt(input: TxtRecordInputs<'_>) -> Result<Vec<u8>, TxtRecordError> {
    let feature_text = format!("0x{:X},0x{:X}", input.features1, input.features2);
    let (pin_required, status_flags) = match input.pin_mode {
        1 => ("true", "0x8c"),
        2 | 3 => ("true", "0x84"),
        _ => ("false", "0x4"),
    };

    let entries = [
        ("ch", "2"),
        ("cn", "0,1,2,3"),
        ("da", "true"),
        ("et", "0,3,5"),
        ("vv", "2"),
        ("ft", feature_text.as_str()),
        ("am", input.model),
        ("md", "0,1,2"),
        ("rhd", "5.6.0.0"),
        ("pw", pin_required),
        ("sr", "44100"),
        ("ss", "16"),
        ("sv", "false"),
        ("tp", "UDP"),
        ("txtvers", "1"),
        ("sf", status_flags),
        ("vs", input.version),
        ("vn", "65537"),
        ("pk", input.public_key),
    ];
    encode_entries(entries)
}

/// Build the source-ordered `_airplay._tcp` TXT bytes.
pub fn build_airplay_txt(input: TxtRecordInputs<'_>) -> Result<Vec<u8>, TxtRecordError> {
    if input.hardware_address.len() > MAX_HWADDR_LEN {
        return Err(TxtRecordError::HardwareAddressUnsupportedLength {
            address_len: input.hardware_address.len(),
        });
    }
    let feature_text = format!("0x{:X},0x{:X}", input.features1, input.features2);
    let device_id = format_airplay_device_id(input.hardware_address);
    let pin_required = if input.pin_mode == 0 { "false" } else { "true" };

    let entries = [
        ("deviceid", device_id.as_str()),
        ("features", feature_text.as_str()),
        ("pw", pin_required),
        ("flags", "0x4"),
        ("model", input.model),
        ("pk", input.public_key),
        ("pi", AIRPLAY_PI),
        ("srcvers", input.version),
        ("vv", "2"),
    ];
    encode_entries(entries)
}

fn format_airplay_device_id(address: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(address.len().saturating_mul(3).saturating_sub(1));
    for (index, byte) in address.iter().copied().enumerate() {
        if index > 0 {
            output.push(':');
        }
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn encode_entries<const N: usize>(entries: [(&str, &str); N]) -> Result<Vec<u8>, TxtRecordError> {
    let mut record = Vec::new();
    for (key, value) in entries {
        let entry_len = key.len().saturating_add(1).saturating_add(value.len());
        if entry_len > MAX_TXT_ENTRY_BYTES {
            return Err(TxtRecordError::EntryLengthOverflow { entry_len });
        }
        let record_len = record
            .len()
            .checked_add(entry_len)
            .and_then(|length| length.checked_add(1))
            .ok_or(TxtRecordError::RecordCapacityExceeded {
                record_len: usize::MAX,
            })?;
        if record_len > MAX_TXT_RECORD_BYTES {
            return Err(TxtRecordError::RecordCapacityExceeded { record_len });
        }
        record.push(entry_len as u8);
        record.extend_from_slice(key.as_bytes());
        record.push(b'=');
        record.extend_from_slice(value.as_bytes());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_INPUT: TxtRecordInputs<'static> = TxtRecordInputs {
        model: "AppleTV3,2",
        version: "220.68",
        public_key: "0123456789abcdef",
        hardware_address: &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
        features1: 0x5a7f_fee6,
        features2: 0,
        pin_mode: 0,
    };

    fn decode_entries(record: &[u8]) -> Vec<&[u8]> {
        let mut entries = Vec::new();
        let mut cursor = 0;
        while cursor < record.len() {
            let length = usize::from(record[cursor]);
            cursor += 1;
            let end = cursor + length;
            entries.push(&record[cursor..end]);
            cursor = end;
        }
        entries
    }

    #[test]
    fn raop_default_txt_matches_source_values_and_order() {
        let record = build_raop_txt(DEFAULT_INPUT).unwrap();
        assert_eq!(
            decode_entries(&record),
            [
                &b"ch=2"[..],
                b"cn=0,1,2,3",
                b"da=true",
                b"et=0,3,5",
                b"vv=2",
                b"ft=0x5A7FFEE6,0x0",
                b"am=AppleTV3,2",
                b"md=0,1,2",
                b"rhd=5.6.0.0",
                b"pw=false",
                b"sr=44100",
                b"ss=16",
                b"sv=false",
                b"tp=UDP",
                b"txtvers=1",
                b"sf=0x4",
                b"vs=220.68",
                b"vn=65537",
                b"pk=0123456789abcdef",
            ]
        );
    }

    #[test]
    fn raop_pin_modes_keep_source_pw_and_status_flag_mapping() {
        let cases = [
            (0, b"pw=false".as_slice(), b"sf=0x4".as_slice()),
            (1, b"pw=true", b"sf=0x8c"),
            (2, b"pw=true", b"sf=0x84"),
            (3, b"pw=true", b"sf=0x84"),
            (4, b"pw=false", b"sf=0x4"),
        ];
        for (pin_mode, expected_pw, expected_sf) in cases {
            let input = TxtRecordInputs {
                pin_mode,
                ..DEFAULT_INPUT
            };
            let record = build_raop_txt(input).unwrap();
            let entries = decode_entries(&record);
            assert_eq!(entries[9], expected_pw);
            assert_eq!(entries[15], expected_sf);
        }
    }

    #[test]
    fn airplay_default_txt_matches_source_values_order_and_lowercase_mac() {
        let record = build_airplay_txt(DEFAULT_INPUT).unwrap();
        assert_eq!(
            decode_entries(&record),
            [
                &b"deviceid=00:11:22:aa:bb:ff"[..],
                b"features=0x5A7FFEE6,0x0",
                b"pw=false",
                b"flags=0x4",
                b"model=AppleTV3,2",
                b"pk=0123456789abcdef",
                b"pi=2e388006-13ba-4041-9a67-25dd4a43d536",
                b"srcvers=220.68",
                b"vv=2",
            ]
        );
    }

    #[test]
    fn airplay_uses_nonzero_pin_mode_as_true_and_uppercase_features() {
        let input = TxtRecordInputs {
            pin_mode: 3,
            features1: 0x12ab,
            features2: 0xabc,
            ..DEFAULT_INPUT
        };
        let record = build_airplay_txt(input).unwrap();
        let entries = decode_entries(&record);
        assert_eq!(entries[1], b"features=0x12AB,0xABC");
        assert_eq!(entries[2], b"pw=true");
    }

    #[test]
    fn encoder_rejects_source_entry_and_record_overflows() {
        let too_long_value = "x".repeat(255);
        assert_eq!(
            encode_entries([("k", too_long_value.as_str())]),
            Err(TxtRecordError::EntryLengthOverflow { entry_len: 257 })
        );
        let long_key = "k".repeat(255);
        assert_eq!(
            encode_entries([(long_key.as_str(), "v")]),
            Err(TxtRecordError::EntryLengthOverflow { entry_len: 257 })
        );
        let max_entry_value = "x".repeat(253);
        assert_eq!(
            encode_entries([
                ("a", max_entry_value.as_str()),
                ("b", max_entry_value.as_str()),
                ("c", max_entry_value.as_str()),
            ]),
            Err(TxtRecordError::RecordCapacityExceeded { record_len: 768 })
        );
    }

    #[test]
    fn airplay_rejects_addresses_that_overflow_the_source_buffer() {
        let input = TxtRecordInputs {
            hardware_address: &[0; 7],
            ..DEFAULT_INPUT
        };
        assert_eq!(
            build_airplay_txt(input),
            Err(TxtRecordError::HardwareAddressUnsupportedLength { address_len: 7 })
        );
    }
}
