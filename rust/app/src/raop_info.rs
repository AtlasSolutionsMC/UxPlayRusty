//! Binary-plist response data for the RTSP `GET /info` handler.
//!
//! This ports the response selection and fields from
//! `lib/raop_handlers.h::raop_handler_info`. DNS-SD and renderer-owned values
//! are inputs in `InfoContext`; the handler does not invent them. Response
//! serialization calls the same native libplist API as the C source because a
//! pure-Rust writer emits different binary-plist layouts. The checked C
//! fixtures pin exact bytes for libplist 2.7.0.

#[cfg(test)]
use plist::Dictionary;
use plist::Value;
use std::ffi::{CString, c_char, c_int, c_void};
use std::io::Cursor;
use std::ptr::{self, NonNull};
use std::slice;

pub const BINARY_PLIST_CONTENT_TYPE: &[u8] = b"application/x-apple-binary-plist";

#[derive(Clone, Copy, Debug)]
pub struct InfoContext<'a> {
    pub raw_hardware_address: &'a [u8],
    pub airplay_txt: &'a [u8],
    pub raop_txt: &'a [u8],
    pub public_key_hex: &'a [u8],
    pub airplay_features: u64,
    pub name: &'a str,
    pub initial_volume: f64,
    pub width: u16,
    pub height: u16,
    pub refresh_rate: u8,
    pub max_fps: u8,
    pub overscanned: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct InfoRequest<'a> {
    pub url: &'a [u8],
    pub content_type: Option<&'a [u8]>,
    pub cseq: Option<&'a [u8]>,
    pub body: Option<&'a [u8]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InfoError {
    message: String,
}

impl std::fmt::Display for InfoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InfoError {}

/// Build the binary plist body returned by the C handler.
///
/// Invalid binary-plist request bodies simply select no TXT record, matching
/// the source's null-result path. Odd or malformed public-key hex safely yields
/// an empty data field; the normal source key is a fixed even-length hex string.
pub fn response_body(
    request: InfoRequest<'_>,
    context: InfoContext<'_>,
) -> Result<Vec<u8>, InfoError> {
    let url = c_string_prefix(request.url);
    let content_type = request.content_type.map(c_string_prefix);
    let has_content_type = request.content_type.is_some();
    let has_cseq = request.cseq.is_some();

    let mut add_airplay_txt = false;
    let mut add_raop_txt = false;
    if content_type.is_some_and(|value| contains(value, BINARY_PLIST_CONTENT_TYPE))
        && let Some(qualifier) = request.body.and_then(binary_plist_qualifier)
    {
        match qualifier.as_str() {
            "txtAirPlay" => add_airplay_txt = true,
            "txtRAOP" => add_raop_txt = true,
            _ => {}
        }
    }

    // The unsequenced BLE path overwrites the qualifier flags with URL matches.
    if !has_cseq {
        add_airplay_txt = contains(url, b"txtAirPlay");
        add_raop_txt = contains(url, b"txtRAOP");
    }

    let mut response = PlistNode::dictionary()?;
    if add_airplay_txt {
        response.set("txtAirPlay", PlistNode::data(context.airplay_txt)?)?;
    }
    if add_raop_txt {
        response.set("txtRAOP", PlistNode::data(context.raop_txt)?)?;
    }

    // Any Content-Type header takes this early exit after optional TXT records.
    if has_content_type {
        return serialize(response);
    }

    let hardware_address = airplay_hardware_address(context.raw_hardware_address);
    response.set("deviceID", PlistNode::string(&hardware_address)?)?;
    response.set("macAddress", PlistNode::string(&hardware_address)?)?;
    response.set(
        "pk",
        PlistNode::data(&decode_hex(context.public_key_hex).unwrap_or_default())?,
    )?;
    response.set("features", PlistNode::uint(context.airplay_features)?)?;
    response.set("name", PlistNode::string(context.name)?)?;
    response.set(
        "pi",
        PlistNode::string("2e388006-13ba-4041-9a67-25dd4a43d536")?,
    )?;
    response.set("vv", PlistNode::uint(2)?)?;
    response.set("statusFlags", PlistNode::uint(68)?)?;
    response.set("keepAliveLowPower", PlistNode::boolean(true)?)?;
    response.set("sourceVersion", PlistNode::string("220.68")?)?;
    response.set("keepAliveSendStatsAsBody", PlistNode::boolean(true)?)?;
    response.set("model", PlistNode::string("AppleTV3,2")?)?;

    // The no-CSeq BLE response contains the common identity/TXT fields only.
    if !has_cseq {
        return serialize(response);
    }

    response.set("initialVolume", PlistNode::real(context.initial_volume)?)?;
    response.set("audioLatencies", audio_latencies()?)?;
    response.set("audioFormats", audio_formats()?)?;
    response.set("displays", displays(context)?)?;

    serialize(response)
}

/// Parse the first string in the request's `qualifier` array, if present.
fn binary_plist_qualifier(body: &[u8]) -> Option<String> {
    if !body.starts_with(b"bplist00") {
        return None;
    }
    let value = Value::from_reader(Cursor::new(body)).ok()?;
    let qualifier = value
        .as_dictionary()?
        .get("qualifier")?
        .as_array()?
        .first()?;
    Some(qualifier.as_string()?.to_owned())
}

fn serialize(root: PlistNode) -> Result<Vec<u8>, InfoError> {
    let mut output = ptr::null_mut();
    let mut length = 0_u32;
    // SAFETY: `root` remains alive for the call, and both output pointers are valid locals.
    let status = unsafe { plist_to_bin(root.as_ptr(), &mut output, &mut length) };
    if status != 0 {
        if !output.is_null() {
            // SAFETY: a non-null output pointer was allocated by plist_to_bin.
            unsafe { plist_mem_free(output.cast()) };
        }
        return Err(InfoError {
            message: format!("libplist failed to serialize AirPlay info plist (status {status})"),
        });
    }
    let output = NonNull::new(output).ok_or_else(|| InfoError {
        message: format!("libplist failed to serialize AirPlay info plist (status {status})"),
    })?;
    // SAFETY: libplist returns `length` readable bytes and requires plist_mem_free for this buffer.
    let bytes =
        unsafe { slice::from_raw_parts(output.as_ptr().cast::<u8>(), length as usize) }.to_vec();
    // SAFETY: `output` is the allocation returned by plist_to_bin.
    unsafe { plist_mem_free(output.as_ptr().cast()) };
    Ok(bytes)
}

fn audio_latencies() -> Result<PlistNode, InfoError> {
    let mut array = PlistNode::array()?;
    let mut first = PlistNode::dictionary()?;
    first.set("type", PlistNode::uint(100)?)?;
    first.set("inputLatencyMicros", PlistNode::uint(0)?)?;
    first.set("audioType", PlistNode::string("default")?)?;
    first.set("outputLatencyMicros", PlistNode::boolean(false)?)?;
    array.append(first)?;

    let mut second = PlistNode::dictionary()?;
    second.set("type", PlistNode::uint(101)?)?;
    second.set("audioType", PlistNode::string("default")?)?;
    second.set("inputLatencyMicros", PlistNode::uint(0)?)?;
    second.set("outputLatencyMicros", PlistNode::boolean(false)?)?;
    array.append(second)?;
    Ok(array)
}

fn audio_formats() -> Result<PlistNode, InfoError> {
    let mut array = PlistNode::array()?;
    for kind in [100, 101] {
        let mut format = PlistNode::dictionary()?;
        format.set("audioOutputFormats", PlistNode::uint(0x03ff_fffc)?)?;
        format.set("type", PlistNode::uint(kind)?)?;
        format.set("audioInputFormats", PlistNode::uint(0x03ff_fffc)?)?;
        array.append(format)?;
    }
    Ok(array)
}

fn displays(context: InfoContext<'_>) -> Result<PlistNode, InfoError> {
    let mut array = PlistNode::array()?;
    let mut display = PlistNode::dictionary()?;
    display.set(
        "uuid",
        PlistNode::string("e0ff8a27-6738-3d56-8a16-cc53aacee925")?,
    )?;
    display.set("widthPhysical", PlistNode::uint(0)?)?;
    display.set("heightPhysical", PlistNode::uint(0)?)?;
    display.set("width", PlistNode::uint(u64::from(context.width))?)?;
    display.set("height", PlistNode::uint(u64::from(context.height))?)?;
    display.set("widthPixels", PlistNode::uint(u64::from(context.width))?)?;
    display.set("heightPixels", PlistNode::uint(u64::from(context.height))?)?;
    display.set("rotation", PlistNode::boolean(false)?)?;
    display.set(
        "refreshRate",
        PlistNode::real(1.0 / f64::from(context.refresh_rate))?,
    )?;
    display.set("maxFPS", PlistNode::uint(u64::from(context.max_fps))?)?;
    display.set("overscanned", PlistNode::boolean(context.overscanned != 0)?)?;
    display.set("features", PlistNode::uint(14)?)?;
    array.append(display)?;
    Ok(array)
}

struct PlistNode(NonNull<c_void>);

impl PlistNode {
    fn from_raw(raw: *mut c_void) -> Result<Self, InfoError> {
        NonNull::new(raw).map(Self).ok_or_else(|| InfoError {
            message: "libplist could not allocate an AirPlay info plist node".to_owned(),
        })
    }

    fn dictionary() -> Result<Self, InfoError> {
        // SAFETY: plist_new_dict has no preconditions and returns an owned node or null.
        Self::from_raw(unsafe { plist_new_dict() })
    }

    fn array() -> Result<Self, InfoError> {
        // SAFETY: plist_new_array has no preconditions and returns an owned node or null.
        Self::from_raw(unsafe { plist_new_array() })
    }

    fn string(value: &str) -> Result<Self, InfoError> {
        let value =
            CString::new(value.split('\0').next().unwrap_or_default()).map_err(|_| InfoError {
                message: "AirPlay info string contains an interior NUL".to_owned(),
            })?;
        // SAFETY: `value` is a valid NUL-terminated UTF-8 string for this call.
        Self::from_raw(unsafe { plist_new_string(value.as_ptr()) })
    }

    fn data(value: &[u8]) -> Result<Self, InfoError> {
        // SAFETY: `value` is readable for its length; libplist copies the supplied bytes.
        Self::from_raw(unsafe { plist_new_data(value.as_ptr().cast(), value.len() as u64) })
    }

    fn uint(value: u64) -> Result<Self, InfoError> {
        // SAFETY: plist_new_uint accepts every u64 value and returns an owned node or null.
        Self::from_raw(unsafe { plist_new_uint(value) })
    }

    fn boolean(value: bool) -> Result<Self, InfoError> {
        // SAFETY: plist_new_bool accepts a byte and returns an owned node or null.
        Self::from_raw(unsafe { plist_new_bool(u8::from(value)) })
    }

    fn real(value: f64) -> Result<Self, InfoError> {
        // SAFETY: plist_new_real accepts every f64 bit pattern and returns an owned node or null.
        Self::from_raw(unsafe { plist_new_real(value) })
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    fn into_raw(self) -> *mut c_void {
        let raw = self.as_ptr();
        std::mem::forget(self);
        raw
    }

    fn set(&mut self, key: &str, value: Self) -> Result<(), InfoError> {
        let key = CString::new(key).map_err(|_| InfoError {
            message: "AirPlay info plist key contains an interior NUL".to_owned(),
        })?;
        // SAFETY: self is a dictionary, key is NUL-terminated, and the function takes ownership
        // of the valid child node returned by value.into_raw().
        unsafe { plist_dict_set_item(self.as_ptr(), key.as_ptr(), value.into_raw()) };
        Ok(())
    }

    fn append(&mut self, value: Self) -> Result<(), InfoError> {
        // SAFETY: self is an array and the function takes ownership of the valid child node.
        unsafe { plist_array_append_item(self.as_ptr(), value.into_raw()) };
        Ok(())
    }
}

impl Drop for PlistNode {
    fn drop(&mut self) {
        // SAFETY: this node owns a live plist_t not transferred into a container.
        unsafe { plist_free(self.as_ptr()) };
    }
}

unsafe extern "C" {
    fn plist_new_dict() -> *mut c_void;
    fn plist_new_array() -> *mut c_void;
    fn plist_new_string(value: *const c_char) -> *mut c_void;
    fn plist_new_bool(value: u8) -> *mut c_void;
    fn plist_new_uint(value: u64) -> *mut c_void;
    fn plist_new_real(value: f64) -> *mut c_void;
    fn plist_new_data(value: *const c_char, length: u64) -> *mut c_void;
    fn plist_free(plist: *mut c_void);
    fn plist_dict_set_item(plist: *mut c_void, key: *const c_char, item: *mut c_void);
    fn plist_array_append_item(plist: *mut c_void, item: *mut c_void);
    fn plist_to_bin(plist: *mut c_void, output: *mut *mut c_char, length: *mut u32) -> c_int;
    fn plist_mem_free(pointer: *mut c_void);
}

fn airplay_hardware_address(raw: &[u8]) -> String {
    raw.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn decode_hex(value: &[u8]) -> Option<Vec<u8>> {
    let (pairs, remainder) = value.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn c_string_prefix(value: &[u8]) -> &[u8] {
    &value[..value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len())]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_ORACLE: &str = include_str!("../../record-shims/raop_info_oracle.expected.txt");

    fn context() -> InfoContext<'static> {
        InfoContext {
            raw_hardware_address: &[0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8],
            airplay_txt: &[0x03, b'f', b'o', b'o', 0x03, b'b', b'a', b'r'],
            raop_txt: &[0x04, b'r', b'a', b'o', b'p', 0x01, b'x'],
            public_key_hex: b"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            airplay_features: 0x5a7ffee6,
            name: "UxPlay Test",
            initial_volume: -0.125,
            width: 1920,
            height: 1080,
            refresh_rate: 60,
            max_fps: 30,
            overscanned: 1,
        }
    }

    fn fixture(name: &str) -> Value {
        let normalized = C_ORACLE.replace("\r\n", "\n");
        let marker = format!("--{name}--\n");
        let section = normalized.split_once(&marker).expect("C oracle section").1;
        let xml = section.split_once("\n--").map_or(section, |(xml, _)| xml);
        Value::from_reader_xml(Cursor::new(xml.as_bytes())).expect("valid libplist XML fixture")
    }

    fn binary_fixture(name: &str) -> Vec<u8> {
        let normalized = C_ORACLE.replace("\r\n", "\n");
        let marker = format!("--{name}-bin--\n");
        let encoded = normalized
            .split_once(&marker)
            .expect("C oracle binary section")
            .1
            .lines()
            .next()
            .expect("C oracle binary bytes");
        let (pairs, remainder) = encoded.as_bytes().as_chunks::<2>();
        assert!(remainder.is_empty(), "even length fixture hex");
        pairs
            .iter()
            .map(|pair| {
                let hex = std::str::from_utf8(pair).expect("ASCII hex");
                u8::from_str_radix(hex, 16).expect("valid hex byte")
            })
            .collect()
    }

    fn assert_binary_fixture(name: &str, request: InfoRequest<'_>) {
        assert_eq!(
            response_body(request, context()).expect("valid response plist"),
            binary_fixture(name),
            "serialized bytes for {name}"
        );
    }

    fn decoded_response(request: InfoRequest<'_>) -> Value {
        let bytes = response_body(request, context()).expect("valid response plist");
        Value::from_reader(Cursor::new(bytes)).expect("valid binary plist response")
    }

    fn qualifier_body(value: &str) -> Vec<u8> {
        let mut root = Dictionary::new();
        root.insert(
            "qualifier".to_owned(),
            Value::Array(vec![Value::from(value)]),
        );
        let mut bytes = Vec::new();
        Value::Dictionary(root)
            .to_writer_binary(&mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn unsequenced_ble_urls_select_the_source_txt_fields_and_identity() {
        for (url, expected) in [
            (b"/info?txtAirPlay".as_slice(), "ble-airplay"),
            (b"/info?txtRAOP".as_slice(), "ble-raop"),
            (b"/info?txtAirPlay?txtRAOP".as_slice(), "ble-both"),
            (b"/info".as_slice(), "ble-plain"),
        ] {
            assert_eq!(
                decoded_response(InfoRequest {
                    url,
                    content_type: None,
                    cseq: None,
                    body: None,
                }),
                fixture(expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn binary_plist_qualifier_selects_the_requested_dns_sd_record() {
        for (qualifier, expected) in [
            ("txtAirPlay", "qualifier-airplay"),
            ("txtRAOP", "qualifier-raop"),
            ("unknown", "qualifier-invalid"),
        ] {
            let body = qualifier_body(qualifier);
            assert_eq!(
                decoded_response(InfoRequest {
                    url: b"/info",
                    content_type: Some(BINARY_PLIST_CONTENT_TYPE),
                    cseq: Some(b"9"),
                    body: Some(&body),
                }),
                fixture(expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn any_content_type_skips_normal_info_fields_after_url_txt_selection() {
        assert_eq!(
            decoded_response(InfoRequest {
                url: b"/info?txtAirPlay",
                content_type: Some(b"application/json"),
                cseq: None,
                body: None,
            }),
            fixture("content-type-other")
        );
    }

    #[test]
    fn cseq_response_matches_dns_sd_crypto_audio_and_display_fields() {
        let response = decoded_response(InfoRequest {
            url: b"/info",
            content_type: None,
            cseq: Some(b"10"),
            body: None,
        });
        assert_eq!(response, fixture("cseq"));
        let dictionary = response.as_dictionary().expect("root plist dictionary");
        assert_eq!(
            dictionary.get("pk").and_then(Value::as_data),
            Some((0..=31).collect::<Vec<u8>>().as_slice())
        );
        assert_eq!(
            dictionary
                .get("audioLatencies")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            dictionary
                .get("displays")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn source_matching_uses_c_string_nul_boundaries() {
        assert_eq!(
            decoded_response(InfoRequest {
                url: b"/info?txtAirPlay\0?txtRAOP",
                content_type: None,
                cseq: None,
                body: None,
            }),
            fixture("ble-airplay")
        );
    }

    #[test]
    fn binary_content_type_does_not_accept_an_xml_encoded_request_body() {
        let response = decoded_response(InfoRequest {
            url: b"/info",
            content_type: Some(BINARY_PLIST_CONTENT_TYPE),
            cseq: Some(b"11"),
            body: Some(b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>qualifier</key><array><string>txtAirPlay</string></array></dict></plist>"),
        });
        assert!(response.as_dictionary().is_some_and(Dictionary::is_empty));
    }

    #[test]
    fn serialized_bytes_match_the_libplist_c_oracle_for_all_response_paths() {
        for (url, name) in [
            (b"/info?txtAirPlay".as_slice(), "ble-airplay"),
            (b"/info?txtRAOP".as_slice(), "ble-raop"),
            (b"/info?txtAirPlay?txtRAOP".as_slice(), "ble-both"),
            (b"/info".as_slice(), "ble-plain"),
        ] {
            assert_binary_fixture(
                name,
                InfoRequest {
                    url,
                    content_type: None,
                    cseq: None,
                    body: None,
                },
            );
        }

        assert_binary_fixture(
            "content-type-other",
            InfoRequest {
                url: b"/info?txtAirPlay",
                content_type: Some(b"application/json"),
                cseq: None,
                body: None,
            },
        );

        for (qualifier, name, cseq) in [
            ("txtAirPlay", "qualifier-airplay", b"7".as_slice()),
            ("txtRAOP", "qualifier-raop", b"8".as_slice()),
            ("unknown", "qualifier-invalid", b"9".as_slice()),
        ] {
            let body = qualifier_body(qualifier);
            assert_binary_fixture(
                name,
                InfoRequest {
                    url: b"/info",
                    content_type: Some(BINARY_PLIST_CONTENT_TYPE),
                    cseq: Some(cseq),
                    body: Some(&body),
                },
            );
        }

        assert_binary_fixture(
            "cseq",
            InfoRequest {
                url: b"/info",
                content_type: None,
                cseq: Some(b"10"),
                body: None,
            },
        );
    }
}
