//! Stateless AirPlay HTTP control effects from `lib/http_handlers.h`.
//!
//! The C callbacks mutate renderer/session state. This module only recognizes
//! the matching POST routes and returns the value the callback would receive;
//! `raop.c` route selection and renderer callbacks remain separate.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AirPlayControl {
    Scrub { position: f32 },
    Rate { rate: f32 },
    Stop,
}

/// Convert one request into a stateless renderer effect when it matches one of
/// `raop.c`'s POST `/scrub?`, `/rate?`, or `/stop` routes.
pub fn parse_control(method: &[u8], url: &[u8]) -> Option<AirPlayControl> {
    parse_control_with_numeric_status(method, url).map(|(control, _)| control)
}

/// Parse a control and report whether its numeric query value had a valid
/// C-style prefix. The C handlers log the `got ...` message only in that case.
pub fn parse_control_with_numeric_status(
    method: &[u8],
    url: &[u8],
) -> Option<(AirPlayControl, bool)> {
    if method != b"POST" {
        return None;
    }

    if url.starts_with(b"/scrub?") {
        let parsed_position = query_value(url).and_then(c_strtod_prefix);
        let position = parsed_position.unwrap_or(0.0) as f32;
        return Some((
            AirPlayControl::Scrub { position },
            parsed_position.is_some(),
        ));
    }
    if url.starts_with(b"/rate?") {
        let parsed_rate = query_value(url).and_then(c_strtof_prefix);
        let rate = parsed_rate.unwrap_or(0.0);
        return Some((AirPlayControl::Rate { rate }, parsed_rate.is_some()));
    }
    if url == b"/stop" {
        return Some((AirPlayControl::Stop, false));
    }
    None
}

/// Match `strstr(url, "?")`, then the first `strstr(data, "=")` in the
/// handler. A missing `=` makes the C code perform pointer arithmetic on a
/// null pointer (undefined behavior); the safe port treats that malformed URL
/// as having no numeric value, which the handler's initialized value maps to
/// `0.0`.
fn query_value(url: &[u8]) -> Option<&[u8]> {
    let question = url.iter().position(|byte| *byte == b'?')?;
    let query = &url[question + 1..];
    let equals = query.iter().position(|byte| *byte == b'=')?;
    Some(&query[equals + 1..])
}

fn c_strtod_prefix(input: &[u8]) -> Option<f64> {
    match scan_number(input)? {
        Number::Decimal(token) => std::str::from_utf8(token).ok()?.parse().ok(),
        Number::Hex {
            digits,
            fraction_nibbles,
            exponent,
            negative,
        } => {
            let binary_exponent =
                exponent.saturating_sub((fraction_nibbles as i64).saturating_mul(4));
            let bits = round_binary(&digits_to_bits(&digits), binary_exponent, FloatFormat::F64);
            let sign = if negative { 1_u64 << 63 } else { 0 };
            Some(f64::from_bits(bits | sign))
        }
        Number::Infinity { negative } => Some(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }),
        Number::Nan { negative } => Some(if negative { -f64::NAN } else { f64::NAN }),
    }
}

fn c_strtof_prefix(input: &[u8]) -> Option<f32> {
    match scan_number(input)? {
        Number::Decimal(token) => {
            let token = std::str::from_utf8(token).ok()?;
            match token.parse::<f32>() {
                Ok(value) => Some(value),
                // Rust can reject finite decimal overflow on some toolchains;
                // `strtof` returns the signed infinity for that input.
                Err(_) => token.parse::<f64>().ok().map(|value| value as f32),
            }
        }
        Number::Hex {
            digits,
            fraction_nibbles,
            exponent,
            negative,
        } => {
            let binary_exponent =
                exponent.saturating_sub((fraction_nibbles as i64).saturating_mul(4));
            let bits =
                round_binary(&digits_to_bits(&digits), binary_exponent, FloatFormat::F32) as u32;
            let sign = if negative { 1_u32 << 31 } else { 0 };
            Some(f32::from_bits(bits | sign))
        }
        Number::Infinity { negative } => Some(if negative {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        }),
        Number::Nan { negative } => Some(if negative { -f32::NAN } else { f32::NAN }),
    }
}

enum Number<'a> {
    Decimal(&'a [u8]),
    Hex {
        digits: Vec<u8>,
        fraction_nibbles: usize,
        exponent: i64,
        negative: bool,
    },
    Infinity {
        negative: bool,
    },
    Nan {
        negative: bool,
    },
}

#[derive(Clone, Copy)]
enum FloatFormat {
    F32,
    F64,
}

impl FloatFormat {
    fn precision(self) -> usize {
        match self {
            Self::F32 => 24,
            Self::F64 => 53,
        }
    }

    fn fraction_bits(self) -> usize {
        self.precision() - 1
    }

    fn min_normal_exponent(self) -> i64 {
        match self {
            Self::F32 => -126,
            Self::F64 => -1022,
        }
    }

    fn max_normal_exponent(self) -> i64 {
        match self {
            Self::F32 => 127,
            Self::F64 => 1023,
        }
    }

    fn exponent_bias(self) -> u64 {
        match self {
            Self::F32 => 127,
            Self::F64 => 1023,
        }
    }

    fn infinity_bits(self) -> u64 {
        let exponent_bits = match self {
            Self::F32 => 8,
            Self::F64 => 11,
        };
        ((1_u64 << exponent_bits) - 1) << self.fraction_bits()
    }
}

/// Scan the longest initial C floating-point subject sequence. The default C
/// locale's ASCII whitespace and the decimal/hexadecimal forms accepted by
/// `strtod`/`strtof` are handled; bytes after the numeric prefix are ignored.
fn scan_number(input: &[u8]) -> Option<Number<'_>> {
    let mut start = 0;
    while input.get(start).is_some_and(|byte| is_c_space(*byte)) {
        start += 1;
    }
    let mut cursor = start;
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

    if starts_ascii_case_insensitive(input, cursor, b"infinity")
        || starts_ascii_case_insensitive(input, cursor, b"inf")
    {
        return Some(Number::Infinity { negative });
    }
    if starts_ascii_case_insensitive(input, cursor, b"nan") {
        return Some(Number::Nan { negative });
    }

    if input.get(cursor) == Some(&b'0')
        && input
            .get(cursor + 1)
            .is_some_and(|byte| matches!(byte, b'x' | b'X'))
        && let Some(hex) = scan_hex(input, cursor + 2, negative)
    {
        return Some(hex);
    }

    scan_decimal(input, start, cursor)
}

fn scan_hex(input: &[u8], mut cursor: usize, negative: bool) -> Option<Number<'_>> {
    let mut digits = Vec::new();
    let mut fraction_nibbles = 0;
    while let Some(value) = input.get(cursor).and_then(hex_digit) {
        digits.push(value);
        cursor += 1;
    }
    if input.get(cursor) == Some(&b'.') {
        cursor += 1;
        while let Some(value) = input.get(cursor).and_then(hex_digit) {
            digits.push(value);
            fraction_nibbles += 1;
            cursor += 1;
        }
    }
    if digits.is_empty() {
        return None;
    }

    let mut exponent = 0_i64;
    if input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'p' | b'P'))
    {
        let marker = cursor;
        cursor += 1;
        let exponent_negative = match input.get(cursor) {
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
        let exp_start = cursor;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            exponent = exponent
                .saturating_mul(10)
                .saturating_add(i64::from(input[cursor] - b'0'));
            cursor += 1;
        }
        if cursor == exp_start {
            cursor = marker;
        } else if exponent_negative {
            exponent = -exponent;
        }
    }
    let _ = cursor; // the caller only needs the parsed value, not the consumed length.
    Some(Number::Hex {
        digits,
        fraction_nibbles,
        exponent,
        negative,
    })
}

fn scan_decimal(input: &[u8], start: usize, mut cursor: usize) -> Option<Number<'_>> {
    let mut digits_before_dot = 0;
    while input.get(cursor).is_some_and(u8::is_ascii_digit) {
        digits_before_dot += 1;
        cursor += 1;
    }

    let mut digits_after_dot = 0;
    if input.get(cursor) == Some(&b'.') {
        cursor += 1;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            digits_after_dot += 1;
            cursor += 1;
        }
    }
    if digits_before_dot == 0 && digits_after_dot == 0 {
        return None;
    }

    if input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        let marker = cursor;
        cursor += 1;
        if input
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            cursor += 1;
        }
        let exponent_start = cursor;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == exponent_start {
            cursor = marker;
        }
    }
    Some(Number::Decimal(&input[start..cursor]))
}

fn digits_to_bits(digits: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(digits.len().saturating_mul(4));
    for digit in digits {
        bits.push((digit >> 3) & 1);
        bits.push((digit >> 2) & 1);
        bits.push((digit >> 1) & 1);
        bits.push(digit & 1);
    }
    let first_nonzero = bits.iter().position(|bit| *bit != 0).unwrap_or(bits.len());
    bits.drain(..first_nonzero);
    bits
}

/// Round an unsigned binary significand times 2^`binary_exponent` into an
/// IEEE-754 format, using round-to-nearest, ties-to-even as the C conversion
/// does in the default floating-point environment.
fn round_binary(bits: &[u8], binary_exponent: i64, format: FloatFormat) -> u64 {
    if bits.is_empty() {
        return 0;
    }
    let precision = format.precision();
    let fraction_bits = format.fraction_bits();
    let highest_exponent = binary_exponent.saturating_add(bits.len() as i64 - 1);
    if highest_exponent > format.max_normal_exponent() {
        return format.infinity_bits();
    }

    if highest_exponent >= format.min_normal_exponent() {
        let keep = bits.len().min(precision);
        let mut significand = bits[..keep]
            .iter()
            .fold(0_u64, |value, bit| (value << 1) | u64::from(*bit));
        significand <<= precision - keep;
        if round_up(bits, keep, significand) {
            significand += 1;
        }
        let mut exponent = highest_exponent;
        if significand == (1_u64 << precision) {
            significand >>= 1;
            exponent += 1;
        }
        if exponent > format.max_normal_exponent() {
            return format.infinity_bits();
        }
        let exponent_field = (exponent + format.exponent_bias() as i64) as u64;
        let fraction_mask = (1_u64 << fraction_bits) - 1;
        return (exponent_field << fraction_bits) | (significand & fraction_mask);
    }

    let unit_exponent = format.min_normal_exponent() - fraction_bits as i64;
    let top_subnormal_bit = highest_exponent.saturating_sub(unit_exponent);
    if top_subnormal_bit < -1 {
        return 0;
    }
    if top_subnormal_bit == -1 {
        // Exactly half a least-subnormal rounds to even zero; any lower set
        // bit makes the value strictly greater than that midpoint.
        return u64::from(bits.iter().skip(1).any(|bit| *bit != 0));
    }

    let keep = (top_subnormal_bit as usize + 1).min(bits.len());
    let mut significand = bits[..keep]
        .iter()
        .fold(0_u64, |value, bit| (value << 1) | u64::from(*bit));
    significand <<= (top_subnormal_bit as usize + 1).saturating_sub(keep);
    if round_up(bits, keep, significand) {
        significand += 1;
    }
    significand
}

fn round_up(bits: &[u8], keep: usize, retained: u64) -> bool {
    let Some(guard) = bits.get(keep) else {
        return false;
    };
    *guard != 0 && (bits.iter().skip(keep + 1).any(|bit| *bit != 0) || retained & 1 != 0)
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

    #[test]
    fn dispatches_only_the_source_post_control_routes() {
        assert_eq!(
            parse_control(b"POST", b"/scrub?position=0.75"),
            Some(AirPlayControl::Scrub { position: 0.75 })
        );
        assert_eq!(
            parse_control(b"POST", b"/rate?value=-1"),
            Some(AirPlayControl::Rate { rate: -1.0 })
        );
        assert_eq!(parse_control(b"POST", b"/stop"), Some(AirPlayControl::Stop));
        assert_eq!(parse_control(b"GET", b"/stop"), None);
        assert_eq!(parse_control(b"POST", b"/scrub"), None);
        assert_eq!(parse_control(b"POST", b"/scrubbing?x=1"), None);
        assert_eq!(parse_control(b"POST", b"/stop?"), None);
    }

    #[test]
    fn numeric_status_distinguishes_zero_from_an_invalid_prefix() {
        assert_eq!(
            parse_control_with_numeric_status(b"POST", b"/scrub?position=0"),
            Some((AirPlayControl::Scrub { position: 0.0 }, true))
        );
        assert_eq!(
            parse_control_with_numeric_status(b"POST", b"/scrub?position=invalid"),
            Some((AirPlayControl::Scrub { position: 0.0 }, false))
        );
        assert_eq!(
            parse_control_with_numeric_status(b"POST", b"/rate?value=-1.25tail"),
            Some((AirPlayControl::Rate { rate: -1.25 }, true))
        );
    }

    #[test]
    fn query_uses_first_equals_after_the_first_question_mark() {
        assert_eq!(
            parse_control(b"POST", b"/scrub?x=0.5&other=1"),
            Some(AirPlayControl::Scrub { position: 0.5 })
        );
        assert_eq!(
            parse_control(b"POST", b"/rate?x?y=-2.25tail"),
            Some(AirPlayControl::Rate { rate: -2.25 })
        );
    }

    #[test]
    fn decimal_prefix_parsing_matches_c_strtod_then_float_cast() {
        let cases = [
            (b"12.5tail".as_slice(), 12.5_f32),
            (b"\t -1.25junk", -1.25_f32),
            (b"1e+tail", 1.0_f32),
            (b".75/", 0.75_f32),
            (b"abc", 0.0_f32),
            (b"1e9999", f32::INFINITY),
            (b"1e-9999", 0.0_f32),
        ];
        for (input, expected) in cases {
            let actual = c_strtod_prefix(input).unwrap_or(0.0) as f32;
            assert_eq!(actual.to_bits(), expected.to_bits(), "{input:?}");
        }
    }

    #[test]
    fn numeric_results_match_the_recorded_c_oracle_fixtures() {
        const INPUTS: [(&str, &[u8]); 11] = [
            ("decimal-prefix", b"12.5tail"),
            ("space-sign", b"\t -1.25junk"),
            ("invalid-exponent", b"1e+tail"),
            ("leading-dot", b".75/"),
            ("no-number", b"abc"),
            ("overflow", b"1e9999"),
            ("underflow", b"1e-9999"),
            ("hex-prefix", b"0x1.8p+1tail"),
            ("hex-halfway", b"0x1.000001p0"),
            ("negative-infinity", b"-INFmore"),
            ("nan-payload", b"NaN(payload)"),
        ];
        let expected =
            include_str!("../../../tools/record-shims/airplay_control_oracle.expected.txt");

        for line in expected
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            let mut fields = line.split_ascii_whitespace();
            let label = fields.next().expect("oracle label");
            let scrub_bits = u32::from_str_radix(fields.next().expect("scrub bits"), 16).unwrap();
            let rate_bits = u32::from_str_radix(fields.next().expect("rate bits"), 16).unwrap();
            let (name, input) = INPUTS.iter().find(|(name, _)| *name == label).unwrap();
            let scrub = c_strtod_prefix(input).unwrap_or(0.0) as f32;
            let rate = c_strtof_prefix(input).unwrap_or(0.0);
            if name == &"nan-payload" {
                assert!(scrub.is_nan());
                assert!(rate.is_nan());
            } else {
                assert_eq!(scrub.to_bits(), scrub_bits, "scrub: {label}");
                assert_eq!(rate.to_bits(), rate_bits, "rate: {label}");
            }
        }
    }

    #[test]
    fn direct_strtof_prefix_parsing_and_special_values() {
        assert_eq!(c_strtof_prefix(b"3.25rest").unwrap(), 3.25);
        assert_eq!(c_strtof_prefix(b"-INFmore").unwrap(), f32::NEG_INFINITY);
        assert!(c_strtof_prefix(b"NaN(payload)").unwrap().is_nan());
        assert_eq!(c_strtof_prefix(b"not-a-number"), None);
    }

    #[test]
    fn hexadecimal_float_prefixes_round_to_the_requested_width() {
        assert_eq!(
            c_strtod_prefix(b"0x1.8p+1tail").unwrap().to_bits(),
            3.0_f64.to_bits()
        );
        assert_eq!(
            c_strtof_prefix(b"0x1.000001p0").unwrap().to_bits(),
            1.0_f32.to_bits()
        );
        assert_eq!(c_strtof_prefix(b"-0x0p999").unwrap().to_bits(), 1_u32 << 31);
    }

    #[test]
    fn hexadecimal_subnormals_and_ties_round_to_even() {
        assert_eq!(c_strtod_prefix(b"0x1p-1074").unwrap().to_bits(), 1);
        assert_eq!(c_strtod_prefix(b"0x1p-1075").unwrap().to_bits(), 0);
        assert_eq!(c_strtod_prefix(b"0x1.8p-1075").unwrap().to_bits(), 1);
        assert_eq!(c_strtof_prefix(b"0x1p-149").unwrap().to_bits(), 1);
        assert_eq!(c_strtof_prefix(b"0x1p-150").unwrap().to_bits(), 0);
        assert_eq!(c_strtof_prefix(b"0x1.8p-150").unwrap().to_bits(), 1);
    }

    #[test]
    fn extreme_hexadecimal_exponents_saturate_without_panicking() {
        assert_eq!(c_strtof_prefix(b"0x1p-999999999999999999999999"), Some(0.0));
        assert_eq!(
            c_strtof_prefix(b"0x1p999999999999999999999999"),
            Some(f32::INFINITY)
        );
    }

    #[test]
    fn malformed_missing_equals_uses_the_safe_zero_default() {
        // C's `strstr(data, "=") + 1` is undefined if '=' is absent.
        assert_eq!(
            parse_control(b"POST", b"/scrub?position"),
            Some(AirPlayControl::Scrub { position: 0.0 })
        );
    }
}
