//! Safe Rust APIs ported from `utils.c`.

#![forbid(unsafe_code)]

/// Port of `int parse_int(const char * str)` (`utils.c:351`).
///
/// `str`: the bytes of the C string, without its NUL; `None` for `NULL`.
pub fn parse_int(str: Option<&[u8]>) -> std::ffi::c_int {
    let Some(str) = str else {
        // The C implementation asserts on NULL. Return a stable failure at the Rust boundary.
        return -1;
    };

    parse_int_with_long_max(str, std::ffi::c_long::MAX as u128)
}

fn parse_int_with_long_max(input: &[u8], long_max: u128) -> std::ffi::c_int {
    let mut start = 0;
    while input
        .get(start)
        .is_some_and(|byte| matches!(*byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        start += 1;
    }

    let negative = match input.get(start) {
        Some(b'-') => {
            start += 1;
            true
        }
        Some(b'+') => {
            start += 1;
            false
        }
        _ => false,
    };

    let mut digits = 0;
    let mut value = 0_u128;
    for byte in input.get(start..).unwrap_or_default() {
        if !byte.is_ascii_digit() {
            return -1;
        }
        digits += 1;
        value = value
            .saturating_mul(10)
            .saturating_add(u128::from(byte - b'0'))
            .min(long_max);
    }

    if digits == 0 {
        return -1;
    }
    if negative {
        return if value == 0 { 0 } else { -1 };
    }

    let int_max = std::ffi::c_int::MAX as u128;
    if value > int_max {
        // `strtol` saturates at `LONG_MAX`; on Windows, `long` and `int` are both
        // 32-bit, so overflowing input is observed by this C helper as INT_MAX.
        return if long_max == int_max {
            std::ffi::c_int::MAX
        } else {
            -1
        };
    }

    value as std::ffi::c_int
}

/// Splits a writable buffer at the first full byte-string delimiter.
///
/// The returned range identifies the token. `cursor` starts at `Some(0)` and is
/// updated to the first byte after a delimiter, or to `None` when exhausted.
/// The delimiter's first byte is replaced with NUL, matching `utils_strsep`.
pub fn strsep_in_place(
    buffer: &mut [u8],
    cursor: &mut Option<usize>,
    delimiter: &[u8],
) -> Option<std::ops::Range<usize>> {
    let start = (*cursor)?;
    let remaining = buffer.get(start..)?;
    if let Some(offset) = find_subslice(remaining, delimiter) {
        let end = start + offset;
        let next = end + delimiter.len();
        if let Some(byte) = buffer.get_mut(end) {
            *byte = 0;
        }
        *cursor = Some(next);
        Some(start..end)
    } else {
        *cursor = None;
        Some(start..buffer.len())
    }
}

/// Reads the exact file contents; error values match the C helper's return codes.
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, std::ffi::c_int> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).map_err(|_| -1)?;
    let length = file.seek(SeekFrom::End(0)).map_err(|_| -1)?;
    file.seek(SeekFrom::Start(0)).map_err(|_| -1)?;
    let length = usize::try_from(length).map_err(|_| -2)?;
    if length > std::ffi::c_int::MAX as usize {
        return Err(-2);
    }

    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| -2)?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::OutOfMemory {
            -2
        } else {
            -3
        }
    })?;
    Ok(bytes)
}

/// Writes the RAOP uppercase, separator-free hardware address into a C-style buffer.
pub fn hwaddr_raop_into(output: &mut [u8], address: &[u8]) -> std::ffi::c_int {
    let Some(required) = address.len().checked_mul(2).and_then(|n| n.checked_add(1)) else {
        return -1;
    };
    if output.is_empty() || output.len() < required {
        return -1;
    }

    let mut cursor = 0;
    for byte in address {
        output[cursor] = hex_digit(byte >> 4, true);
        output[cursor + 1] = hex_digit(byte & 0x0f, true);
        cursor += 2;
    }
    output[cursor] = 0;
    std::ffi::c_int::try_from(cursor + 1).unwrap_or(-1)
}

/// Formats a hardware address in the RAOP uppercase form.
pub fn hwaddr_raop(address: &[u8]) -> String {
    let mut output = vec![0; address.len().saturating_mul(2).saturating_add(1)];
    let written = hwaddr_raop_into(&mut output, address);
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&output[..written as usize - 1]).into_owned()
}

/// Writes the AirPlay lowercase, colon-separated hardware address into a C-style buffer.
pub fn hwaddr_airplay_into(output: &mut [u8], address: &[u8]) -> std::ffi::c_int {
    let Some(required) = address.len().checked_mul(3) else {
        return -1;
    };
    if output.is_empty() || output.len() < required {
        return -1;
    }

    let mut cursor = 0;
    for byte in address {
        output[cursor] = hex_digit(byte >> 4, false);
        output[cursor + 1] = hex_digit(byte & 0x0f, false);
        output[cursor + 2] = b':';
        cursor += 3;
    }
    cursor = cursor.saturating_sub(1);
    output[cursor] = 0;
    std::ffi::c_int::try_from(cursor + 1).unwrap_or(-1)
}

/// Formats a hardware address in the AirPlay lowercase form.
pub fn hwaddr_airplay(address: &[u8]) -> String {
    let mut output = vec![0; address.len().saturating_mul(3).saturating_add(1)];
    let written = hwaddr_airplay_into(&mut output, address);
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&output[..written as usize - 1]).into_owned()
}

/// Parses ASCII hexadecimal pairs; odd lengths and invalid digits are rejected.
pub fn parse_hex(input: &[u8]) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }

    let mut output = Vec::with_capacity(input.len() / 2);
    for pair in input.chunks(2) {
        output.push((hex_value(pair[0])? << 4) | hex_value(pair[1])?);
    }
    Some(output)
}

/// Formats bytes as lowercase hexadecimal without a terminator.
pub fn hex_to_string(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().saturating_mul(2));
    for byte in input {
        output.push(hex_digit(byte >> 4, false) as char);
        output.push(hex_digit(byte & 0x0f, false) as char);
    }
    output
}

/// Formats bytes as lowercase hexadecimal, with spaces and a final newline.
pub fn data_to_string(input: &[u8], chars_per_line: usize) -> Option<String> {
    if chars_per_line == 0 {
        return None;
    }
    let line_breaks = input.len().saturating_sub(1) / chars_per_line;
    let capacity = input
        .len()
        .checked_mul(3)?
        .checked_add(line_breaks)?
        .checked_add(1)?;
    let mut output = String::with_capacity(capacity);
    for (index, byte) in input.iter().enumerate() {
        if index != 0 && index % chars_per_line == 0 {
            output.push('\n');
        }
        output.push(hex_digit(byte >> 4, false) as char);
        output.push(hex_digit(byte & 0x0f, false) as char);
        output.push(' ');
    }
    output.push('\n');
    Some(output)
}

/// Copies a C byte string with `strncpy` padding and replaces CR bytes before its NUL with spaces.
pub fn data_to_text(input: &[u8]) -> Vec<u8> {
    let mut output = vec![0; input.len()];
    let copied = input
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(input.len());
    output[..copied].copy_from_slice(&input[..copied]);
    for byte in &mut output[..copied] {
        if *byte == b'\r' {
            *byte = b' ';
        }
    }
    output
}

/// Formats the UxPlay nanosecond timestamp in local time and preserves nine fractional digits.
pub fn ntp_timestamp_to_time(timestamp: u64) -> Option<String> {
    let datetime = local_datetime(timestamp)?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09}",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
        timestamp % 1_000_000_000
    ))
}

/// Formats only the seconds and nine-digit fractional portion of a UxPlay timestamp.
pub fn ntp_timestamp_to_seconds(timestamp: u64) -> Option<String> {
    let datetime = local_datetime(timestamp)?;
    Some(format!(
        "{:02}.{:09}",
        datetime.second(),
        timestamp % 1_000_000_000
    ))
}

/// Returns the original IPv4/IPv6 spelling; invalid lengths and invalid scoped IPv6 return `None`.
pub fn ipaddress_to_string(address: &[u8], zone_id: u32) -> Option<String> {
    match address.len() {
        4 => Some(format!(
            "{}.{}.{}.{}",
            address[0], address[1], address[2], address[3]
        )),
        16 if zone_id != 0 => {
            if address[..8] != [0xfe, 0x80, 0, 0, 0, 0, 0, 0] {
                return None;
            }
            Some(format!(
                "fe80::{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}%{zone_id}",
                address[8],
                address[9],
                address[10],
                address[11],
                address[12],
                address[13],
                address[14],
                address[15]
            ))
        }
        16 => {
            let mut output = String::with_capacity(39);
            for (index, pair) in address.chunks(2).enumerate() {
                if index != 0 {
                    output.push(':');
                }
                output.push(hex_digit(pair[0] >> 4, false) as char);
                output.push(hex_digit(pair[0] & 0x0f, false) as char);
                output.push(hex_digit(pair[1] >> 4, false) as char);
                output.push(hex_digit(pair[1] & 0x0f, false) as char);
            }
            Some(output)
        }
        _ => None,
    }
}

/// Writes an address into a C-style output buffer and returns `snprintf`'s full length.
pub fn ipaddress_to_string_into(
    address: &[u8],
    zone_id: u32,
    output: &mut [u8],
) -> std::ffi::c_int {
    if output.is_empty() {
        return 0;
    }
    let Some(formatted) = ipaddress_to_string(address, zone_id) else {
        output[0] = 0;
        return 0;
    };
    let bytes = formatted.as_bytes();
    let copied = bytes.len().min(output.len() - 1);
    output[..copied].copy_from_slice(&bytes[..copied]);
    output[copied] = 0;
    std::ffi::c_int::try_from(bytes.len()).unwrap_or(std::ffi::c_int::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlistXmlError {
    MissingOpeningLineBreak,
    MissingClosingTag,
}

/// Returns `None` if the XML has no `<data>` tags, otherwise a copy with long data blocks elided.
pub fn strip_data_from_plist_xml(input: &[u8]) -> Result<Option<Vec<u8>>, PlistXmlError> {
    const OPEN: &[u8] = b"<data>";
    const CLOSE: &[u8] = b"</data>";

    let Some(first_open) = find_subslice(input, OPEN) else {
        return Ok(None);
    };
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    let mut search_from = first_open;

    while let Some(relative_open) = find_subslice(&input[search_from..], OPEN) {
        let open = search_from + relative_open;
        let opening_line_end = input[open..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| open + offset + 1)
            .ok_or(PlistXmlError::MissingOpeningLineBreak)?;
        let close = find_subslice(&input[opening_line_end..], CLOSE)
            .map(|offset| opening_line_end + offset)
            .ok_or(PlistXmlError::MissingClosingTag)?;
        let closing_line_start = input[..close]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|offset| offset + 1)
            .ok_or(PlistXmlError::MissingClosingTag)?;
        if closing_line_start < opening_line_end {
            return Err(PlistXmlError::MissingClosingTag);
        }

        output.extend_from_slice(&input[cursor..opening_line_end]);
        let data_lines = input[opening_line_end..closing_line_start]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        if data_lines > 1 {
            output.extend_from_slice(
                format!("        ({data_lines} lines data omitted, 64 chars/line)\n").as_bytes(),
            );
        } else {
            output.extend_from_slice(&input[opening_line_end..closing_line_start]);
        }

        cursor = closing_line_start;
        search_from = close + CLOSE.len();
    }

    output.extend_from_slice(&input[cursor..]);
    Ok(Some(output))
}

/// Formats `strftime("%c GMT")` in the C locale's English representation.
pub fn gmt_time_string_at(datetime: time::OffsetDateTime) -> String {
    let weekday = match datetime.weekday() {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    };
    let month = match datetime.month() {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    };
    format!(
        "{weekday} {month} {:2} {:02}:{:02}:{:02} {:04} GMT",
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
        datetime.year()
    )
}

pub fn gmt_time_string_now() -> String {
    gmt_time_string_at(time::OffsetDateTime::now_utc())
}

fn local_datetime(timestamp: u64) -> Option<time::OffsetDateTime> {
    let seconds = i64::try_from(timestamp / 1_000_000_000).ok()?;
    let utc = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    let offset = time::UtcOffset::local_offset_at(utc).ok()?;
    Some(utc.to_offset(offset))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn hex_digit(nibble: u8, uppercase: bool) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ if uppercase => b'A' + (nibble - 10),
        _ => b'a' + (nibble - 10),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        data_to_string, data_to_text, gmt_time_string_at, hex_to_string, hwaddr_airplay,
        hwaddr_raop, hwaddr_raop_into, ipaddress_to_string, ipaddress_to_string_into,
        ntp_timestamp_to_seconds, parse_hex, parse_int, parse_int_with_long_max, read_file,
        strip_data_from_plist_xml, strsep_in_place,
    };

    #[test]
    fn accepts_c_decimal_forms_and_leading_c_whitespace() {
        for (input, expected) in [
            (b"0".as_slice(), 0),
            (b"-0", 0),
            (b"+0", 0),
            (b"+42", 42),
            (b"010", 10),
            (b" \t\r\n7", 7),
            (b"2147483647", i32::MAX),
        ] {
            assert_eq!(parse_int(Some(input)), expected);
        }
    }

    #[test]
    fn rejects_negative_nonzero_and_incomplete_or_trailing_input() {
        for input in [
            b"".as_slice(),
            b"+",
            b"-",
            b"  ",
            b"-1",
            b"-2147483648",
            b"42 ",
            b"0x2a",
            b"1_000",
            b"9.0",
            "１２".as_bytes(),
        ] {
            assert_eq!(parse_int(Some(input)), -1, "input: {input:?}");
        }
    }

    #[test]
    fn preserves_strtol_overflow_per_c_long_width() {
        let int_max = i32::MAX as u128;
        let input = b"2147483648";

        assert_eq!(parse_int_with_long_max(input, int_max), i32::MAX);
        assert_eq!(parse_int_with_long_max(input, i64::MAX as u128), -1);
        assert_eq!(
            parse_int_with_long_max(
                b"999999999999999999999999999999999999999999999999999999999999",
                int_max,
            ),
            i32::MAX
        );
        assert_eq!(
            parse_int_with_long_max(
                b"999999999999999999999999999999999999999999999999999999999999",
                i64::MAX as u128,
            ),
            -1
        );
    }

    #[test]
    fn null_is_a_stable_failure_at_the_rust_boundary() {
        assert_eq!(parse_int(None), -1);
    }

    #[test]
    fn splits_mutable_buffers_at_full_delimiters() {
        let mut bytes = b"left::right::".to_vec();
        let mut cursor = Some(0);
        assert_eq!(strsep_in_place(&mut bytes, &mut cursor, b"::"), Some(0..4));
        assert_eq!(&bytes[..5], b"left\0");
        assert_eq!(cursor, Some(6));
        assert_eq!(strsep_in_place(&mut bytes, &mut cursor, b"::"), Some(6..11));
        assert_eq!(cursor, Some(13));
        assert_eq!(
            strsep_in_place(&mut bytes, &mut cursor, b"::"),
            Some(13..13)
        );
        assert_eq!(cursor, None);
        assert_eq!(strsep_in_place(&mut bytes, &mut cursor, b"::"), None);
    }

    #[test]
    fn strsep_empty_delimiter_matches_at_the_current_position() {
        let mut bytes = b"abc".to_vec();
        let mut cursor = Some(0);
        assert_eq!(strsep_in_place(&mut bytes, &mut cursor, b""), Some(0..0));
        assert_eq!(bytes[0], 0);
        assert_eq!(cursor, Some(0));
    }

    #[test]
    fn reads_exact_file_bytes() {
        let path = std::env::temp_dir().join(format!("uxplay-utils-{}.bin", std::process::id()));
        let written = std::fs::write(&path, [0_u8, 1, 13, 255]);
        assert!(written.is_ok());
        assert_eq!(
            read_file(&path).ok().as_deref(),
            Some([0_u8, 1, 13, 255].as_slice())
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_hardware_addresses_and_checks_output_capacity() {
        let address = [0, 15, 160, 255];
        assert_eq!(hwaddr_raop(&address), "000FA0FF");
        assert_eq!(hwaddr_airplay(&address), "00:0f:a0:ff");
        let mut short = [0; 8];
        assert_eq!(hwaddr_raop_into(&mut short, &address), -1);
    }

    #[test]
    fn parses_and_formats_hex_bytes() {
        assert_eq!(parse_hex(b"00aFff"), Some(vec![0, 175, 255]));
        assert_eq!(parse_hex(b"0g"), None);
        assert_eq!(parse_hex(b"abc"), None);
        assert_eq!(hex_to_string(&[0, 175, 255]), "00afff");
    }

    #[test]
    fn formats_data_with_c_spacing_and_line_breaks() {
        assert_eq!(
            data_to_string(&[0, 10, 255], 2).as_deref(),
            Some("00 0a \nff \n")
        );
        assert_eq!(data_to_string(&[], 16).as_deref(), Some("\n"));
        assert_eq!(data_to_string(&[1], 0), None);
    }

    #[test]
    fn text_copy_pads_after_nul_and_replaces_carriage_returns() {
        assert_eq!(data_to_text(b"a\rb"), b"a b");
        assert_eq!(data_to_text(b"a\0\rb"), b"a\0\0\0");
    }

    #[test]
    fn formats_ip_addresses_in_the_source_spelling() {
        assert_eq!(
            ipaddress_to_string(&[192, 168, 1, 7], 0).as_deref(),
            Some("192.168.1.7")
        );
        let address = [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(
            ipaddress_to_string(&address, 0).as_deref(),
            Some("0001:0203:0405:0607:0809:0a0b:0c0d:0e0f")
        );
        let link_local = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(
            ipaddress_to_string(&link_local, 3).as_deref(),
            Some("fe80::0001:0203:0405:0607%3")
        );
        assert_eq!(ipaddress_to_string(&address, 3), None);

        let mut truncated = [0; 5];
        assert_eq!(
            ipaddress_to_string_into(&[192, 168, 1, 7], 0, &mut truncated),
            11
        );
        assert_eq!(&truncated, b"192.\0");
    }

    #[test]
    fn ntp_seconds_keep_the_nine_digit_fraction() {
        assert_eq!(
            ntp_timestamp_to_seconds(1_700_000_000_123_456_789).as_deref(),
            Some("20.123456789")
        );
    }

    #[test]
    fn gmt_time_string_matches_the_c_locale_format() {
        assert_eq!(
            gmt_time_string_at(time::OffsetDateTime::UNIX_EPOCH),
            "Thu Jan  1 00:00:00 1970 GMT"
        );
    }

    #[test]
    fn strips_only_multiline_plist_data_payloads() {
        let xml = b"<plist>\n<key>a</key>\n<data>\n  AAAA\n  BBBB\n</data>\n<data>\n  CCCC\n</data>\n</plist>\n";
        let expected = b"<plist>\n<key>a</key>\n<data>\n        (2 lines data omitted, 64 chars/line)\n</data>\n<data>\n  CCCC\n</data>\n</plist>\n";
        assert_eq!(
            strip_data_from_plist_xml(xml).ok().flatten().as_deref(),
            Some(expected.as_slice())
        );
        assert_eq!(strip_data_from_plist_xml(b"<plist/>"), Ok(None));
        assert_eq!(
            strip_data_from_plist_xml(b"<data>missing"),
            Err(super::PlistXmlError::MissingOpeningLineBreak)
        );
    }
}
