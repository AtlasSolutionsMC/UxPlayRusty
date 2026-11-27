//! Reverse HTTP / `EVENT` frame-boundary logic from `lib/http_request.c`.

const HEADER_END: &[u8] = b"\r\n\r\n";
const CONTENT_LENGTH: &[u8] = b"Content-Length:";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventFrame {
    pub leading_junk_len: usize,
    pub header_len: usize,
    pub body_len: usize,
    pub consumed_len: usize,
}

/// Find one complete message in a reverse-HTTP/event buffer.
///
/// The source skips leading control bytes and ASCII whitespace, requires CRLF header
/// termination, finds the first case-insensitive `Content-Length:` substring using
/// its original bounded scan, and waits for the declared body bytes.
pub fn parse_event_frame(data: &[u8]) -> Option<EventFrame> {
    let leading_junk_len = data
        .iter()
        .position(|byte| !(*byte <= 0x20 || *byte == 0x7f))?;
    let candidate = &data[leading_junk_len..];
    let c_string_len = candidate
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(candidate.len());
    let c_string = &candidate[..c_string_len];
    let header_end = c_string
        .windows(HEADER_END.len())
        .position(|window| window == HEADER_END)?;
    let header_len = header_end + HEADER_END.len();

    let body_len = content_length(c_string, header_len).unwrap_or(0);
    let consumed_len = header_len
        .checked_add(body_len)?
        .checked_add(leading_junk_len)?;
    if candidate.len() < header_len.checked_add(body_len)? {
        return None;
    }

    Some(EventFrame {
        leading_junk_len,
        header_len,
        body_len,
        consumed_len,
    })
}

fn content_length(c_string: &[u8], header_len: usize) -> Option<usize> {
    // C loops while `i < header_len - 15`, so the final possible 15-byte window is
    // excluded. `strtoul` then reads to the C-string terminator, which can include
    // bytes after the header delimiter when the same read also contains the body.
    let header = c_string.get(..header_len)?;
    let scan_len = header.len().checked_sub(CONTENT_LENGTH.len())?;
    for offset in 0..scan_len {
        if header[offset..offset + CONTENT_LENGTH.len()].eq_ignore_ascii_case(CONTENT_LENGTH) {
            let parsed = parse_c_strtoul(&c_string[offset + CONTENT_LENGTH.len()..]);
            return Some(parsed);
        }
    }
    None
}

fn parse_c_strtoul(input: &[u8]) -> usize {
    let mut cursor = 0;
    while input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        cursor += 1;
    }
    let negative = match input.get(cursor) {
        Some(b'-') => {
            cursor += 1;
            true
        }
        Some(b'+') => {
            cursor += 1;
            false
        }
        _ => false,
    };
    let max_value = if cfg!(windows) {
        u32::MAX as usize
    } else {
        usize::MAX
    };
    let mut value = 0usize;
    let mut has_digit = false;
    let mut overflow = false;
    while let Some(byte @ b'0'..=b'9') = input.get(cursor).copied() {
        has_digit = true;
        let digit = usize::from(byte - b'0');
        if let Some(next) = value
            .checked_mul(10)
            .and_then(|number| number.checked_add(digit))
            .filter(|next| *next <= max_value)
        {
            value = next;
        } else {
            overflow = true;
        }
        cursor += 1;
    }
    if !has_digit {
        0
    } else if overflow {
        usize::MAX
    } else if negative {
        if cfg!(windows) {
            (value as u32).wrapping_neg() as usize
        } else {
            value.wrapping_neg()
        }
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_event_without_content_length() {
        assert_eq!(
            parse_event_frame(b"EVENT /ctrl HTTP/1.1\r\n\r\n"),
            Some(EventFrame {
                leading_junk_len: 0,
                header_len: 24,
                body_len: 0,
                consumed_len: 24,
            })
        );
    }

    #[test]
    fn waits_for_full_content_length_body_and_leaves_next_message() {
        let header = b"EVENT /ctrl HTTP/1.1\r\nContent-Length: 4\r\n\r\n";
        let mut message = header.to_vec();
        message.extend_from_slice(b"bodyEVENT /next HTTP/1.1\r\n\r\n");
        assert_eq!(
            parse_event_frame(&message),
            Some(EventFrame {
                leading_junk_len: 0,
                header_len: header.len(),
                body_len: 4,
                consumed_len: header.len() + 4,
            })
        );
        assert!(parse_event_frame(header).is_none());
    }

    #[test]
    fn skips_leading_control_and_whitespace_bytes() {
        assert_eq!(
            parse_event_frame(b"\0\x7f \r\nEVENT /ctrl HTTP/1.1\r\n\r\n"),
            Some(EventFrame {
                leading_junk_len: 5,
                header_len: 24,
                body_len: 0,
                consumed_len: 29,
            })
        );
    }

    #[test]
    fn content_length_name_is_ascii_case_insensitive_and_value_uses_strtoul_prefix() {
        let message = b"EVENT /ctrl HTTP/1.1\r\ncOnTeNt-LeNgTh:\t+3tail\r\n\r\nabc";
        let frame = parse_event_frame(message).unwrap();
        assert_eq!(frame.body_len, 3);
        assert_eq!(frame.consumed_len, message.len());
    }

    #[test]
    fn malformed_length_with_no_digits_is_zero_like_strtoul() {
        let message = b"EVENT /ctrl HTTP/1.1\r\nContent-Length: nope\r\n\r\n";
        assert_eq!(parse_event_frame(message).unwrap().body_len, 0);
    }

    #[test]
    fn content_length_search_matches_any_substring_in_the_header() {
        let message = b"EVENT /ctrl HTTP/1.1\r\nX-Content-Length: 2\r\n\r\nxy";
        assert_eq!(parse_event_frame(message).unwrap().body_len, 2);
    }

    #[test]
    fn empty_content_length_reads_coalesced_body_bytes_like_c_strtoul() {
        let header = b"EVENT /ctrl HTTP/1.1\r\nContent-Length:\r\n\r\n";
        let mut coalesced = header.to_vec();
        coalesced.extend_from_slice(b"2x");
        assert_eq!(
            parse_event_frame(&coalesced),
            Some(EventFrame {
                leading_junk_len: 0,
                header_len: header.len(),
                body_len: 2,
                consumed_len: header.len() + 2,
            })
        );

        // With a read boundary after the header, C's data buffer is NUL terminated
        // there and `strtoul` sees no body digits before completing the frame.
        assert_eq!(
            parse_event_frame(header),
            Some(EventFrame {
                leading_junk_len: 0,
                header_len: header.len(),
                body_len: 0,
                consumed_len: header.len(),
            })
        );
        assert_eq!(parse_event_frame(b"2x"), None);
    }
}
