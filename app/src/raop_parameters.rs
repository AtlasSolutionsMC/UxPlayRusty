//! RTSP GET_PARAMETER/SET_PARAMETER parsing and callback effects from
//! `lib/raop_handlers.h`.
//!
//! The C handler selects callbacks from connection state. This module accepts
//! those state bits and returns typed effects so the session layer can execute
//! the callback without fabricating server or RTP state here.

const OK: (u16, &[u8]) = (200, b"OK");
const NOT_UNDERSTOOD: (u16, &[u8]) = (451, b"Parameter not understood");
const TEXT_PARAMETERS: &[u8] = b"text/parameters";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeTarget {
    RaopRtp,
    HlsRenderer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMime {
    Jpeg,
    Png,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParameterEffect {
    Volume { target: VolumeTarget, value: f32 },
    Progress { start: u32, current: u32, end: u32 },
    CoverArt { mime: ImageMime, data: Vec<u8> },
    Metadata(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterOutput {
    pub status_code: u16,
    pub status_message: &'static [u8],
    pub content_type: Option<&'static [u8]>,
    pub body: Option<Vec<u8>>,
    pub effect: Option<ParameterEffect>,
}

impl ParameterOutput {
    fn ok() -> Self {
        Self {
            status_code: OK.0,
            status_message: OK.1,
            content_type: None,
            body: None,
            effect: None,
        }
    }

    fn not_understood() -> Self {
        Self {
            status_code: NOT_UNDERSTOOD.0,
            status_message: NOT_UNDERSTOOD.1,
            ..Self::ok()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SetParameterContext {
    /// Whether the connection has an allocated RAOP RTP object.
    pub rtp_present: bool,
    /// Whether that RTP object currently reports that it is running.
    pub rtp_running: bool,
    /// Whether `audio_set_volume` is installed for HLS playback.
    pub hls_volume_callback: bool,
}

/// Port `raop_handler_get_parameter` with the callback's current volume value.
///
/// A missing callback value preserves the source's initialized `volume: 0.0` body.
pub fn get_parameter(
    content_type: Option<&[u8]>,
    body: &[u8],
    client_volume: Option<f32>,
) -> ParameterOutput {
    let Some(content_type) = content_type else {
        return ParameterOutput::not_understood();
    };
    if content_type != TEXT_PARAMETERS || !has_volume_request_line(body) {
        return ParameterOutput::ok();
    }

    let body = client_volume.map_or_else(|| b"volume: 0.0\r\n".to_vec(), format_volume_response);
    ParameterOutput {
        content_type: Some(TEXT_PARAMETERS),
        body: Some(body),
        ..ParameterOutput::ok()
    }
}

/// Port `raop_handler_set_parameter`, preserving callback routing as effects.
pub fn set_parameter(
    content_type: Option<&[u8]>,
    body: &[u8],
    context: SetParameterContext,
) -> ParameterOutput {
    let Some(content_type) = content_type else {
        return ParameterOutput::not_understood();
    };

    let effect = match content_type {
        TEXT_PARAMETERS => parse_text_parameter(body, context),
        b"image/jpeg" if context.rtp_present => Some(ParameterEffect::CoverArt {
            mime: ImageMime::Jpeg,
            data: body.to_vec(),
        }),
        b"image/png" if context.rtp_present => Some(ParameterEffect::CoverArt {
            mime: ImageMime::Png,
            data: body.to_vec(),
        }),
        b"application/x-dmap-tagged" if context.rtp_present => {
            Some(ParameterEffect::Metadata(body.to_vec()))
        }
        _ => None,
    };

    ParameterOutput {
        effect,
        ..ParameterOutput::ok()
    }
}

fn parse_text_parameter(body: &[u8], context: SetParameterContext) -> Option<ParameterEffect> {
    // The C code copies the request body into a zeroed, NUL-terminated buffer,
    // then performs exact prefix tests. These string prefixes are ASCII, so a
    // byte-prefix check has the same result, including when a later NUL exists.
    if body.len() >= 8 && body.starts_with(b"volume: ") {
        let value = parse_c_float_prefix(&body[8..]).unwrap_or(0.0);
        if context.rtp_present && context.rtp_running {
            return Some(ParameterEffect::Volume {
                target: VolumeTarget::RaopRtp,
                value,
            });
        }
        if context.hls_volume_callback {
            return Some(ParameterEffect::Volume {
                target: VolumeTarget::HlsRenderer,
                value,
            });
        }
        return None;
    }

    if context.rtp_present && body.len() >= 10 && body.starts_with(b"progress: ") {
        let (start, current, end) = parse_progress(&body[10..]);
        return Some(ParameterEffect::Progress {
            start,
            current,
            end,
        });
    }
    None
}

fn has_volume_request_line(body: &[u8]) -> bool {
    let mut cursor = 0;
    while cursor < body.len() {
        let remaining = &body[cursor..];
        if remaining.len() >= 8 && remaining.starts_with(b"volume\r\n") {
            return true;
        }

        let Some(carriage_return) = remaining.iter().position(|byte| *byte == b'\r') else {
            return false;
        };
        let next = cursor + carriage_return;
        if body.len() - next < 2 || &body[next..next + 2] != b"\r\n" {
            return false;
        }
        cursor = next + 2;
    }
    false
}

fn format_volume_response(value: f32) -> Vec<u8> {
    let numeric = if value.is_nan() {
        if value.is_sign_negative() {
            "-nan".to_owned()
        } else {
            "nan".to_owned()
        }
    } else if value.is_infinite() {
        if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        }
    } else {
        format!("{value:.6}")
    };
    let formatted = format!("volume: {numeric:>9}\r\n");
    // The original handler writes into `char volume[25]` with snprintf, then
    // sends strlen(volume) bytes. Truncate to 24 visible bytes exactly.
    formatted.as_bytes()[..formatted.len().min(24)].to_vec()
}

fn parse_progress(input: &[u8]) -> (u32, u32, u32) {
    let mut values = [0_u32; 3];
    let mut cursor = 0;
    for (field, slot) in values.iter_mut().enumerate() {
        let Some((value, consumed)) = parse_c_u32_prefix(&input[cursor..]) else {
            break;
        };
        *slot = value;
        cursor += consumed;
        if field < 2 {
            if input.get(cursor) != Some(&b'/') {
                break;
            }
            cursor += 1;
        }
    }
    (values[0], values[1], values[2])
}

fn parse_c_u32_prefix(input: &[u8]) -> Option<(u32, usize)> {
    let mut cursor = 0;
    let field_start = cursor;
    while input.get(cursor).is_some_and(|byte| is_c_space(*byte)) {
        cursor += 1;
    }
    let negative = match input.get(cursor) {
        Some(b'+') => {
            cursor += 1;
            false
        }
        Some(b'-') => {
            cursor += 1;
            true
        }
        _ => false,
    };
    let digits_start = cursor;
    let mut value = 0_u32;
    while let Some(byte @ b'0'..=b'9') = input.get(cursor) {
        value = value.wrapping_mul(10).wrapping_add(u32::from(*byte - b'0'));
        cursor += 1;
    }
    if cursor == digits_start {
        return None;
    }
    if negative {
        value = value.wrapping_neg();
    }
    Some((value, cursor - field_start))
}

/// C `sscanf(..., "%f", ...)` conversion for the numeric prefixes used by
/// SET_PARAMETER. Malformed inputs safely retain the source's initialized 0.
fn parse_c_float_prefix(input: &[u8]) -> Option<f32> {
    let mut start = 0;
    while input.get(start).is_some_and(|byte| is_c_space(*byte)) {
        start += 1;
    }
    let mut cursor = start;
    let sign = match input.get(cursor) {
        Some(b'+') => {
            cursor += 1;
            1.0_f32
        }
        Some(b'-') => {
            cursor += 1;
            -1.0_f32
        }
        _ => 1.0_f32,
    };
    if starts_ascii_case_insensitive(input, cursor, b"infinity")
        || starts_ascii_case_insensitive(input, cursor, b"inf")
    {
        return Some(if sign.is_sign_negative() {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        });
    }
    if starts_ascii_case_insensitive(input, cursor, b"nan") {
        return Some(if sign.is_sign_negative() {
            -f32::NAN
        } else {
            f32::NAN
        });
    }

    if input.get(cursor) == Some(&b'0')
        && input
            .get(cursor + 1)
            .is_some_and(|byte| matches!(byte, b'x' | b'X'))
        && let Some((value, _)) = parse_hex_float(input, cursor + 2)
    {
        return Some(sign * value);
    }

    let number_start = cursor;
    let mut digits = 0;
    while input.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
        digits += 1;
    }
    if input.get(cursor) == Some(&b'.') {
        cursor += 1;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let base_end = cursor;
    if input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        let mut exponent_end = cursor + 1;
        if input
            .get(exponent_end)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            exponent_end += 1;
        }
        let exponent_digits = exponent_end;
        while input.get(exponent_end).is_some_and(u8::is_ascii_digit) {
            exponent_end += 1;
        }
        if exponent_end > exponent_digits {
            cursor = exponent_end;
        } else {
            cursor = base_end;
        }
    }

    let token = std::str::from_utf8(&input[number_start..cursor]).ok()?;
    let signed = if sign.is_sign_negative() {
        format!("-{token}")
    } else {
        token.to_owned()
    };
    signed
        .parse::<f32>()
        .ok()
        .or_else(|| signed.parse::<f64>().ok().map(|value| value as f32))
}

fn parse_hex_float(input: &[u8], mut cursor: usize) -> Option<(f32, usize)> {
    let mut value = 0.0_f64;
    let mut digits = 0;
    while let Some(digit) = input.get(cursor).and_then(hex_digit) {
        value = value * 16.0 + f64::from(digit);
        cursor += 1;
        digits += 1;
    }
    if input.get(cursor) == Some(&b'.') {
        cursor += 1;
        let mut place = 1.0 / 16.0;
        while let Some(digit) = input.get(cursor).and_then(hex_digit) {
            value += f64::from(digit) * place;
            place /= 16.0;
            cursor += 1;
            digits += 1;
        }
    }
    if digits == 0
        || !input
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'p' | b'P'))
    {
        return None;
    }
    cursor += 1;
    let exponent_start = cursor;
    if input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
    {
        cursor += 1;
    }
    let exponent_digits = cursor;
    let mut exponent = 0_i32;
    while let Some(byte @ b'0'..=b'9') = input.get(cursor) {
        exponent = exponent
            .saturating_mul(10)
            .saturating_add(i32::from(*byte - b'0'));
        cursor += 1;
    }
    if cursor == exponent_digits {
        let _ = exponent_start;
        return None;
    }
    if input.get(exponent_start) == Some(&b'-') {
        exponent = -exponent;
    }
    let scaled = value * 2_f64.powi(exponent.clamp(-2048, 2048));
    Some((scaled as f32, cursor))
}

fn hex_digit(byte: &u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(*byte - b'0'),
        b'a'..=b'f' => Some(*byte - b'a' + 10),
        b'A'..=b'F' => Some(*byte - b'A' + 10),
        _ => None,
    }
}

fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn starts_ascii_case_insensitive(input: &[u8], start: usize, needle: &[u8]) -> bool {
    input
        .get(start..start.saturating_add(needle.len()))
        .is_some_and(|value| value.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_ORACLE: &str = include_str!("../testdata/raop_parameters_oracle.expected.txt");

    fn oracle_records(kind: &str) -> Vec<Vec<String>> {
        C_ORACLE
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                line.split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|fields| fields.first().is_some_and(|field| field == kind))
            .collect()
    }

    #[test]
    fn missing_content_type_is_451_for_both_handlers() {
        for output in [
            get_parameter(None, b"volume\r\n", Some(0.5)),
            set_parameter(None, b"volume: 0.5", SetParameterContext::default()),
        ] {
            assert_eq!(output.status_code, 451);
            assert_eq!(output.status_message, b"Parameter not understood");
            assert_eq!(output.content_type, None);
            assert_eq!(output.body, None);
            assert_eq!(output.effect, None);
        }
    }

    #[test]
    fn get_parameter_requires_exact_type_and_volume_crlf_line() {
        let expected = get_parameter(Some(TEXT_PARAMETERS), b"other\r\nvolume\r\n", Some(0.5));
        assert_eq!(expected.content_type, Some(TEXT_PARAMETERS));
        assert_eq!(
            expected.body.as_deref(),
            Some(&b"volume:  0.500000\r\n"[..])
        );

        for (content_type, body) in [
            (b"Text/parameters".as_slice(), b"volume\r\n".as_slice()),
            (TEXT_PARAMETERS, b"volume"),
            (TEXT_PARAMETERS, b"volume\n"),
            (TEXT_PARAMETERS, b"xvolume\r\n"),
            (b"application/octet-stream", b"volume\r\n"),
        ] {
            let output = get_parameter(Some(content_type), body, Some(0.5));
            assert_eq!(output.status_code, 200);
            assert_eq!(output.content_type, None);
            assert_eq!(output.body, None);
        }
    }

    #[test]
    fn get_parameter_uses_default_volume_without_callback() {
        assert_eq!(
            get_parameter(Some(TEXT_PARAMETERS), b"volume\r\n", None)
                .body
                .as_deref(),
            Some(&b"volume: 0.0\r\n"[..])
        );
    }

    #[test]
    fn volume_response_format_and_fixed_c_buffer_match_oracle() {
        let cases = [
            ("half", 0.5_f32),
            ("negative", -1.25_f32),
            ("negative-zero", -0.0_f32),
            ("large", 12345.75_f32),
            ("maximum", f32::MAX),
        ];
        let rows = oracle_records("F");
        for (name, value) in cases {
            let row = rows.iter().find(|fields| fields[1] == name).unwrap();
            let encoded = row[2].as_bytes();
            let (pairs, remainder) = encoded.as_chunks::<2>();
            assert!(remainder.is_empty());
            let expected = pairs
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(format_volume_response(value), expected, "{name}");
        }
    }

    #[test]
    fn numeric_and_progress_parsing_match_the_c_oracle() {
        let inputs = [
            ("decimal", b"volume: 0.5".as_slice()),
            ("trailing", b"volume: -1.25junk"),
            ("space", b"volume: \t+0.25 tail"),
            ("invalid", b"volume: nope"),
            ("hex", b"volume: 0x1.8p+1tail"),
            ("overflow", b"volume: 1e9999"),
            ("underflow", b"volume: 1e-9999"),
        ];
        let volume_rows = oracle_records("V");
        for (name, input) in inputs {
            let row = volume_rows.iter().find(|fields| fields[1] == name).unwrap();
            let effect = set_parameter(
                Some(TEXT_PARAMETERS),
                input,
                SetParameterContext {
                    rtp_present: true,
                    rtp_running: true,
                    hls_volume_callback: true,
                },
            )
            .effect
            .unwrap();
            let ParameterEffect::Volume { value, .. } = effect else {
                panic!("volume input produces a volume effect")
            };
            assert_eq!(
                value.to_bits(),
                u32::from_str_radix(&row[2], 16).unwrap(),
                "{name}"
            );
        }

        let progress_inputs = [
            ("full", b"progress: 1/2/3".as_slice()),
            ("signed", b"progress: -1/+2/3"),
            ("partial", b"progress: 9/x/3"),
            ("invalid", b"progress: x/y/z"),
            ("spaces", b"progress:  7/ 8/9"),
        ];
        let progress_rows = oracle_records("P");
        for (name, body) in progress_inputs {
            let row = progress_rows
                .iter()
                .find(|fields| fields[1] == name)
                .unwrap();
            let effect = set_parameter(
                Some(TEXT_PARAMETERS),
                body,
                SetParameterContext {
                    rtp_present: true,
                    ..SetParameterContext::default()
                },
            )
            .effect
            .unwrap();
            let ParameterEffect::Progress {
                start,
                current,
                end,
            } = effect
            else {
                panic!("progress input produces a progress effect")
            };
            assert_eq!(
                [start, current, end],
                [
                    row[2].parse::<u32>().unwrap(),
                    row[3].parse::<u32>().unwrap(),
                    row[4].parse::<u32>().unwrap()
                ],
                "{name}"
            );
        }
    }

    #[test]
    fn set_parameter_routes_volume_progress_art_and_metadata_to_source_callbacks() {
        let active = SetParameterContext {
            rtp_present: true,
            rtp_running: true,
            hls_volume_callback: true,
        };
        assert_eq!(
            set_parameter(Some(TEXT_PARAMETERS), b"volume: .75", active).effect,
            Some(ParameterEffect::Volume {
                target: VolumeTarget::RaopRtp,
                value: 0.75,
            })
        );
        assert_eq!(
            set_parameter(
                Some(TEXT_PARAMETERS),
                b"volume: 0.25",
                SetParameterContext {
                    rtp_present: true,
                    rtp_running: false,
                    hls_volume_callback: true,
                }
            )
            .effect,
            Some(ParameterEffect::Volume {
                target: VolumeTarget::HlsRenderer,
                value: 0.25,
            })
        );
        assert_eq!(
            set_parameter(Some(b"image/jpeg"), &[0, 1, 2], active).effect,
            Some(ParameterEffect::CoverArt {
                mime: ImageMime::Jpeg,
                data: vec![0, 1, 2],
            })
        );
        assert_eq!(
            set_parameter(Some(b"image/png"), &[3, 4], active).effect,
            Some(ParameterEffect::CoverArt {
                mime: ImageMime::Png,
                data: vec![3, 4],
            })
        );
        assert_eq!(
            set_parameter(Some(b"application/x-dmap-tagged"), &[5, 6], active).effect,
            Some(ParameterEffect::Metadata(vec![5, 6]))
        );
    }

    #[test]
    fn set_parameter_preserves_rtp_and_callback_presence_rules() {
        let no_state = SetParameterContext::default();
        assert_eq!(
            set_parameter(Some(TEXT_PARAMETERS), b"volume: 1.0", no_state).effect,
            None
        );
        assert_eq!(
            set_parameter(Some(TEXT_PARAMETERS), b"progress: 1/2/3", no_state).effect,
            None
        );
        assert_eq!(
            set_parameter(Some(b"image/jpeg"), b"image", no_state).effect,
            None
        );
        assert_eq!(
            set_parameter(Some(b"application/x-dmap-tagged"), b"meta", no_state).effect,
            None
        );
        assert_eq!(
            set_parameter(
                Some(b"Text/parameters"),
                b"volume: 1",
                SetParameterContext {
                    hls_volume_callback: true,
                    ..no_state
                }
            )
            .effect,
            None
        );
    }
}
