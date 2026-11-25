//! Safe, incremental HTTP/1 request state used by the Rust server.
//!
//! This ports the request-facing portion of `lib/http_request.c`. `httparse`
//! supplies the HTTP/1 header grammar; body framing, header lookup, reverse
//! EVENT buffering and the source's limits stay in this module.

use crate::http_event::{EventFrame, parse_event_frame};
use std::fmt;

const MAX_EVENT_DATA: usize = 4096;
const HTTP2_PREFACE_SUFFIX: &[u8] = b"\r\n\r\nSM\r\n\r\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestHeader {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParserFailure {
    pub name: &'static str,
    pub description: String,
}

impl fmt::Display for ParserFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.name, self.description)
    }
}

impl std::error::Error for ParserFailure {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventMessage {
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct HttpRequest {
    input: Vec<u8>,
    pending_data: Vec<u8>,
    header_scan_pos: usize,
    headers_parsed: bool,
    body_parser: Option<BodyParser>,
    method: Option<String>,
    url: Option<Vec<u8>>,
    protocol: Option<String>,
    headers: Vec<RequestHeader>,
    header_values_present: Vec<bool>,
    header_lines_complete: Vec<bool>,
    body: Option<Vec<u8>>,
    complete: bool,
    is_reverse: bool,
    event_mode: bool,
    discard_read_ahead: bool,
    http2_preface: bool,
    error: Option<ParserFailure>,
}

impl HttpRequest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a request fragment and update the parsed request state.
    pub fn add_data(&mut self, data: &[u8]) -> Result<(), ParserFailure> {
        if self.event_mode {
            return Err(ParserFailure {
                name: "HPE_INVALID_EOF_STATE",
                description: "request parser has been upgraded to reverse HTTP".into(),
            });
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.http2_preface {
            return Ok(());
        }
        if self.complete {
            if !self.discard_read_ahead {
                self.pending_data
                    .try_reserve(data.len())
                    .map_err(allocation_error)?;
                self.pending_data.extend_from_slice(data);
            }
            return Ok(());
        }
        if self.headers_parsed {
            return self.record_body(data);
        }

        let search_start = self.header_scan_pos.saturating_sub(3);
        let line_ending_scan_start = self.header_scan_pos.saturating_sub(1);
        self.input.extend_from_slice(data);
        if let Err((error, valid_prefix_len)) = validate_partial_request_line(&self.input) {
            capture_partial_request_line(
                &self.input[..valid_prefix_len],
                &mut self.url,
                &mut self.protocol,
            );
            self.error = Some(error.clone());
            return Err(error);
        }
        capture_partial_request_line(&self.input, &mut self.url, &mut self.protocol);

        if let Some(line) = parse_http_version_request_line(&self.input)
            && &self.input[..line.method_end] == b"PRI"
        {
            let suffix = &self.input[line.protocol_end..];
            let compared_len = suffix.len().min(HTTP2_PREFACE_SUFFIX.len());
            if suffix[..compared_len] != HTTP2_PREFACE_SUFFIX[..compared_len] {
                let error = ParserFailure {
                    name: "HPE_INVALID_VERSION",
                    description: "Expected HTTP/2 Connection Preface".into(),
                };
                self.error = Some(error.clone());
                return Err(error);
            }
            if suffix.len() < HTTP2_PREFACE_SUFFIX.len() {
                return Ok(());
            }

            // llhttp pauses on the PRI preface rather than completing an HTTP request.
            self.input.clear();
            self.header_scan_pos = 0;
            self.http2_preface = true;
            return Ok(());
        }

        let header_end = find_bytes(&self.input[search_start..], b"\r\n\r\n")
            .map(|relative_end| search_start + relative_end + 4);
        let inspect_end = header_end.unwrap_or(self.input.len());
        if let Err(error) =
            validate_header_line_endings(&self.input[..inspect_end], line_ending_scan_start)
        {
            self.error = Some(error.clone());
            return Err(error);
        }
        capture_partial_headers(
            &self.input[..inspect_end],
            &mut self.headers,
            &mut self.header_values_present,
            &mut self.header_lines_complete,
        );

        let Some(header_end) = header_end else {
            self.header_scan_pos = self.input.len();
            return Ok(());
        };

        let parsed = match parse_request_head(&self.input[..header_end]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error = Some(error.clone());
                return Err(error);
            }
        };
        self.method = parsed.method;
        self.url = parsed.url;
        self.protocol = parsed.protocol;
        self.headers = parsed.headers;
        self.header_values_present = vec![true; self.headers.len()];
        self.header_lines_complete = vec![true; self.headers.len()];
        self.headers_parsed = true;

        let parsed_body_parser = match body_framing(&self.headers) {
            Ok(parser) => parser,
            Err(error) => {
                self.error = Some(error.clone());
                return Err(error);
            }
        };
        let is_connect = self.method.as_deref() == Some("CONNECT");
        self.discard_read_ahead = is_connect || has_upgrade_headers(&self.headers);
        let body_parser = if is_connect {
            // llhttp treats CONNECT payload bytes as the upgraded protocol and pauses
            // at the header boundary, even when Content-Length or Transfer-Encoding exists.
            BodyParser::Complete
        } else {
            parsed_body_parser
        };
        self.complete = matches!(body_parser, BodyParser::Complete);
        self.body_parser = (!self.complete).then_some(body_parser);

        let tail = self.input.split_off(header_end);
        self.input.clear();
        self.header_scan_pos = 0;
        if self.complete {
            if !self.discard_read_ahead {
                self.pending_data
                    .try_reserve(tail.len())
                    .map_err(allocation_error)?;
                self.pending_data.extend_from_slice(&tail);
            }
        } else {
            self.record_body(&tail)?;
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    pub fn error_name(&self) -> Option<&str> {
        Some(self.error.as_ref().map_or("HPE_OK", |error| error.name))
    }

    pub fn error_description(&self) -> Option<&str> {
        self.error.as_ref().map(|error| error.description.as_str())
    }

    pub fn method(&self) -> Option<&str> {
        (!self.is_reverse && self.complete)
            .then_some(self.method.as_deref())
            .flatten()
    }

    pub fn url(&self) -> Option<&[u8]> {
        (!self.is_reverse).then_some(self.url.as_deref()).flatten()
    }

    pub fn protocol(&self) -> Option<&str> {
        (!self.is_reverse).then_some(self.protocol.as_deref().unwrap_or(""))
    }

    /// Return the first exact-case header name, matching the C API's `strcmp` lookup.
    pub fn header(&self, name: &[u8]) -> Option<&[u8]> {
        if self.is_reverse {
            return None;
        }
        let index = self.headers.iter().position(|header| header.name == name)?;
        if !self
            .header_values_present
            .get(index)
            .copied()
            .unwrap_or(true)
        {
            return None;
        }
        Some(self.headers[index].value.as_slice())
    }

    pub fn headers(&self) -> &[RequestHeader] {
        &self.headers
    }

    pub fn body(&self) -> Option<&[u8]> {
        if self.is_reverse {
            return None;
        }
        self.body.as_deref()
    }

    /// Return the C API's total field/value byte count and length statistics.
    pub fn header_stats(&self) -> (usize, usize, usize, usize) {
        if self.is_reverse {
            return (0, 0, 0, 0);
        }
        let mut total = 0;
        let mut max_name = 0;
        let mut max_value = 0;
        for header in &self.headers {
            total += header.name.len() + header.value.len();
            max_name = max_name.max(header.name.len());
            max_value = max_value.max(header.value.len());
        }
        (total, self.headers.len(), max_name, max_value)
    }

    /// Return headers in the source's `Name: value\n` format.
    pub fn header_string(&self) -> Option<Vec<u8>> {
        if self.is_reverse || self.headers.is_empty() {
            return None;
        }
        let mut output = Vec::new();
        for (index, header) in self.headers.iter().enumerate() {
            output.extend_from_slice(&header.name);
            output.extend_from_slice(b": ");
            if self
                .header_values_present
                .get(index)
                .copied()
                .unwrap_or(true)
            {
                output.extend_from_slice(&header.value);
                if self
                    .header_lines_complete
                    .get(index)
                    .copied()
                    .unwrap_or(true)
                {
                    output.push(b'\n');
                }
            }
        }
        Some(output)
    }

    pub fn is_reverse(&self) -> bool {
        self.is_reverse
    }

    pub fn set_reverse(&mut self) {
        self.is_reverse = true;
    }

    /// Reset body storage and switch this request object to EVENT framing mode.
    pub fn upgrade_data(&mut self) {
        self.input.clear();
        self.pending_data.clear();
        self.header_scan_pos = 0;
        self.body = None;
        self.discard_read_ahead = false;
        self.event_mode = true;
    }

    /// Append one reverse HTTP/EVENT fragment. The source rejects buffers of 4096 bytes.
    pub fn add_event_data(&mut self, data: &[u8]) -> Result<(), ParserFailure> {
        if !self.event_mode {
            return Err(ParserFailure {
                name: "HPE_INVALID_EOF_STATE",
                description: "reverse HTTP mode has not been enabled".into(),
            });
        }
        if self.input.len().saturating_add(data.len()) >= MAX_EVENT_DATA {
            return Err(ParserFailure {
                name: "HPE_USER",
                description: "reverse HTTP event buffer limit exceeded".into(),
            });
        }
        self.input.extend_from_slice(data);
        Ok(())
    }

    /// Extract and consume one complete reverse HTTP/EVENT frame.
    pub fn parse_event_data(&mut self) -> Option<EventMessage> {
        if !self.event_mode {
            return None;
        }
        let EventFrame {
            leading_junk_len: _,
            header_len,
            body_len,
            consumed_len,
        } = parse_event_frame(&self.input)?;
        // C computes leading junk into the consumed length, but copies the frame
        // header/body from request->data without applying that offset.
        let header_start: usize = 0;
        let header_end = header_start.checked_add(header_len)?;
        let body_end = header_end.checked_add(body_len)?;
        let header = self.input.get(header_start..header_end)?.to_vec();
        let body = self.input.get(header_end..body_end)?.to_vec();
        self.input.drain(..consumed_len);
        Some(EventMessage { header, body })
    }

    pub fn buffered_len(&self) -> usize {
        self.input.len() + self.pending_data.len()
    }

    pub(crate) fn take_pending_data(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_data)
    }

    /// Reproduce the C parser's callbacks when a recv buffer continues past the
    /// first completed request. The C server executes the whole recv against
    /// one llhttp instance, then checks its error before dispatching the
    /// request. Its request callbacks append URL bytes and reject a second
    /// protocol token because the protocol storage is already occupied.
    pub(crate) fn apply_source_read_ahead(
        &mut self,
        read_ahead: &[u8],
    ) -> Result<(), ParserFailure> {
        if read_ahead.is_empty() {
            return Ok(());
        }
        if !self.source_should_keep_alive() {
            return Err(ParserFailure {
                name: "HPE_CLOSED_CONNECTION",
                description: "Data after `Connection: close`".into(),
            });
        }

        let mut following_request = Self::new();
        let parse_result = following_request.add_data(read_ahead);
        if following_request
            .protocol
            .as_deref()
            .is_some_and(|protocol| !protocol.is_empty())
        {
            // The C on_protocol callback returns HPE_USER after the first
            // request has filled its four-byte protocol buffer. llhttp leaves
            // the callback error reason null in this case.
            return Err(ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            });
        }
        parse_result?;

        if let Some(url) = following_request.url.take() {
            self.url
                .get_or_insert_with(Vec::new)
                .extend_from_slice(&url);
        }
        // C reuses one `http_request_t` for every message parsed from a recv.
        // A complete HTTP/0.9 read-ahead message has no protocol callback to
        // reject it, so its other callbacks mutate the first request in place:
        // URL and header callbacks append, body bytes append, and message
        // completion overwrites the method.
        self.headers.extend(following_request.headers);
        self.header_values_present
            .extend(following_request.header_values_present);
        self.header_lines_complete
            .extend(following_request.header_lines_complete);
        if following_request.complete {
            self.method = following_request.method;
        }
        if let Some(body) = following_request.body {
            self.body.get_or_insert_with(Vec::new).extend(body);
        }
        Ok(())
    }

    fn source_should_keep_alive(&self) -> bool {
        let (major, minor) = self
            .protocol
            .as_deref()
            .and_then(|protocol| protocol.split_once('/'))
            .and_then(|(_, version)| version.split_once('.'))
            .and_then(|(major, minor)| {
                Some((major.parse::<u16>().ok()?, minor.parse::<u16>().ok()?))
            })
            .unwrap_or((0, 0));
        let mut connection_close = false;
        let mut connection_keep_alive = false;
        for header in self
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(b"Connection"))
        {
            for token in header.value.split(|byte| *byte == b',') {
                let token = trim_http_ows(token);
                connection_close |= token.eq_ignore_ascii_case(b"close");
                connection_keep_alive |= token.eq_ignore_ascii_case(b"keep-alive");
            }
        }
        if major > 0 && minor > 0 {
            !connection_close
        } else {
            connection_keep_alive
        }
    }

    fn process_body(&mut self, data: &[u8]) -> Result<usize, ParserFailure> {
        let (complete, consumed) = match self.body_parser.as_mut() {
            Some(BodyParser::Fixed { remaining }) => {
                let consumed = (*remaining).min(data.len());
                if consumed != 0 {
                    append_body(&mut self.body, &data[..consumed])?;
                    *remaining -= consumed;
                }
                (*remaining == 0, consumed)
            }
            Some(BodyParser::Chunked(parser)) => {
                parser.consume(data, &mut self.body, &mut self.headers)?
            }
            Some(BodyParser::Complete) | None => (true, 0),
        };
        if complete {
            self.complete = true;
            self.body_parser = Some(BodyParser::Complete);
        }
        Ok(consumed)
    }

    fn record_body(&mut self, data: &[u8]) -> Result<(), ParserFailure> {
        match self.process_body(data) {
            Ok(consumed) => {
                if self.complete && consumed < data.len() && !self.discard_read_ahead {
                    self.pending_data
                        .try_reserve(data.len() - consumed)
                        .map_err(allocation_error)?;
                    self.pending_data.extend_from_slice(&data[consumed..]);
                }
                Ok(())
            }
            Err(error) => {
                self.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

#[derive(Debug)]
enum BodyParser {
    Fixed { remaining: usize },
    Chunked(ChunkedBody),
    Complete,
}

#[derive(Debug)]
struct ChunkedBody {
    state: ChunkState,
}

#[derive(Debug)]
enum ChunkState {
    SizeLine(Vec<u8>),
    Data { remaining: usize },
    DataCrlf { saw_cr: bool },
    Trailers(Vec<u8>),
    Complete,
}

struct ParsedRequest {
    method: Option<String>,
    url: Option<Vec<u8>>,
    protocol: Option<String>,
    headers: Vec<RequestHeader>,
}

fn validate_partial_request_line(input: &[u8]) -> Result<(), (ParserFailure, usize)> {
    let line_end = input.iter().position(|byte| matches!(*byte, b'\r' | b'\n'));
    let line = &input[..line_end.unwrap_or(input.len())];
    let mut method_end = line.iter().position(|byte| *byte == b' ');
    let method_prefix = &line[..method_end.unwrap_or(line.len())];
    if let Some(offset) = llhttp_method_prefix_failure(method_prefix) {
        return Err((invalid_method(), offset));
    }
    let Some(method_end) = method_end.take() else {
        return Ok(());
    };
    let method = &line[..method_end];
    if !is_llhttp_method(method) {
        return Err((invalid_method(), method_end));
    }

    let mut target_start = method_end + 1;
    while line.get(target_start) == Some(&b' ') {
        target_start += 1;
    }
    if target_start == line.len() {
        return Ok(());
    }
    let target_end = line[target_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| target_start + offset);
    let target_is_complete = target_end < line.len() || line_end.is_some();
    let target = &line[target_start..target_end];
    if let Some((failure, offset)) =
        request_target_prefix_failure(method, target, target_is_complete)
    {
        return Err((failure, target_start + offset));
    }
    if target_end == line.len() {
        return Ok(());
    }

    let mut protocol_start = target_end;
    while line.get(protocol_start) == Some(&b' ') {
        protocol_start += 1;
    }
    let protocol_end = line[protocol_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| protocol_start + offset);
    let protocol = &line[protocol_start..protocol_end];
    let has_protocol_boundary = line_end.is_some() || protocol_end < line.len();
    if let Some((failure, offset)) = request_protocol_prefix_failure(
        method,
        protocol,
        protocol_start,
        has_protocol_boundary,
        protocol_end < line.len(),
    ) {
        return Err((failure, offset));
    }
    Ok(())
}

fn request_target_prefix_failure(
    method: &[u8],
    target: &[u8],
    is_complete: bool,
) -> Option<(ParserFailure, usize)> {
    if method == b"CONNECT" {
        return request_target_url_failure_at(method, target);
    }
    if target.first().is_some_and(u8::is_ascii_alphabetic) {
        let Some(scheme_end) = target.iter().position(|byte| *byte == b':') else {
            if let Some(offset) = target.iter().position(|byte| {
                !(byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'-' | b'.'))
            }) {
                return Some((invalid_url("Unexpected char in url schema"), offset));
            }
            return if is_complete {
                request_target_url_failure_at(method, target)
            } else {
                None
            };
        };
        if let Some(offset) = target[..scheme_end]
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'-' | b'.')))
        {
            return Some((invalid_url("Unexpected char in url schema"), offset));
        }

        let scheme_suffix = &target[scheme_end..];
        for (offset, (actual, expected)) in scheme_suffix.iter().zip(b"://").enumerate() {
            if actual != expected {
                return Some((
                    invalid_url("Unexpected char in url schema"),
                    scheme_end + offset,
                ));
            }
        }
        if scheme_suffix.len() < 3 {
            return if is_complete {
                request_target_url_failure_at(method, target)
            } else {
                None
            };
        }
    }

    request_target_url_failure_at(method, target)
}

fn request_protocol_prefix_failure(
    method: &[u8],
    protocol: &[u8],
    protocol_start: usize,
    has_boundary: bool,
    has_trailing_space: bool,
) -> Option<(ParserFailure, usize)> {
    if protocol.is_empty() {
        return None;
    }
    if protocol.starts_with(b"RTSP") && !is_rtsp_method(method) {
        return Some((
            unsupported_rtsp_method(method),
            protocol_start + b"RTSP".len(),
        ));
    }
    if protocol.starts_with(b"ICE") && method != b"SOURCE" {
        return Some((
            ParserFailure {
                name: "HPE_INVALID_CONSTANT",
                description: "Expected SOURCE method for ICE/x.x request".into(),
            },
            protocol_start + b"ICE".len(),
        ));
    }
    const PREFIXES: [&[u8]; 3] = [b"HTTP/", b"RTSP/", b"ICE/"];
    for end in 1..=protocol.len() {
        let prefix = &protocol[..end];
        let could_be_supported = PREFIXES
            .iter()
            .any(|supported| supported.starts_with(prefix) || prefix.starts_with(supported));
        if !could_be_supported {
            return Some((
                ParserFailure {
                    name: "HPE_INVALID_CONSTANT",
                    description: "Expected HTTP/, RTSP/ or ICE/".into(),
                },
                protocol_start + end - 1,
            ));
        }
    }

    let (kind, version) = if let Some(version) = protocol.strip_prefix(b"HTTP/") {
        (SpecialProtocol::Http, version)
    } else if let Some(version) = protocol.strip_prefix(b"RTSP/") {
        (SpecialProtocol::Rtsp, version)
    } else if let Some(version) = protocol.strip_prefix(b"ICE/") {
        (SpecialProtocol::Ice, version)
    } else {
        if has_boundary {
            return Some((
                ParserFailure {
                    name: "HPE_INVALID_CONSTANT",
                    description: "Expected HTTP/, RTSP/ or ICE/".into(),
                },
                protocol_start + protocol.len(),
            ));
        }
        return None;
    };

    request_version_prefix_failure(version, kind, has_boundary, has_trailing_space)
        .map(|(failure, offset)| (failure, protocol_start + 5 + offset))
}

fn request_version_prefix_failure(
    version: &[u8],
    kind: SpecialProtocol,
    has_boundary: bool,
    has_trailing_space: bool,
) -> Option<(ParserFailure, usize)> {
    let failure = |description: &str, offset| {
        (
            ParserFailure {
                name: "HPE_INVALID_VERSION",
                description: description.into(),
            },
            offset,
        )
    };
    if version.is_empty() {
        return has_boundary.then(|| failure("Invalid major version", 0));
    }
    if !version[0].is_ascii_digit() {
        return Some(failure("Invalid major version", 0));
    }
    if version.len() == 1 {
        return has_boundary.then(|| failure("Expected dot", 1));
    }
    if version[1] != b'.' {
        return Some(failure("Expected dot", 1));
    }
    if version.len() == 2 {
        return has_boundary.then(|| failure("Invalid minor version", 2));
    }
    if !version[2].is_ascii_digit() {
        return Some(failure("Invalid minor version", 2));
    }
    if version.len() > 3 {
        return Some(failure("Expected CRLF after version", 3));
    }
    if matches!(kind, SpecialProtocol::Rtsp) && !matches!(version, b"1.0" | b"1.1" | b"2.0") {
        return Some(failure("Invalid HTTP version", 2));
    }
    if has_trailing_space {
        return Some(failure("Expected CRLF after version", 3));
    }
    None
}

fn parse_request_head(input: &[u8]) -> Result<ParsedRequest, ParserFailure> {
    if let Some(method) = raw_request_method(input)
        && !is_llhttp_method(method)
    {
        return Err(invalid_method());
    }
    if let Some(failure) = request_target_url_failure(input) {
        return Err(failure);
    }
    if let Some(failure) = request_line_protocol_failure(input) {
        return Err(failure);
    }
    if let Some(failure) = request_line_version_failure(input) {
        return Err(failure);
    }
    let http_09_line = parse_http_09_request_line(input);
    let rtsp_line = parse_rtsp_request_line(input);
    let ice_line = parse_ice_request_line(input);
    let http_version_line = parse_http_version_request_line(input);
    let special_line = rtsp_line.or(ice_line).or(http_version_line);
    let mut normalized_input = Vec::new();
    if let Some(line) = http_09_line {
        let normalized_len = input
            .len()
            .checked_add(b" HTTP/1.1".len())
            .ok_or(ParserFailure {
                name: "HPE_USER",
                description: "request line length overflow".into(),
            })?;
        normalized_input
            .try_reserve_exact(normalized_len)
            .map_err(allocation_error)?;
        normalized_input.extend_from_slice(&input[..line.target_end]);
        normalized_input.extend_from_slice(b" HTTP/1.1");
        normalized_input.extend_from_slice(&input[line.line_end..]);
    } else if let Some(line) = special_line {
        let normalized_len = input
            .len()
            .checked_sub(line.protocol_end - line.protocol_start)
            .and_then(|len| len.checked_add(b"HTTP/1.1".len()))
            .ok_or(ParserFailure {
                name: "HPE_USER",
                description: "request line length overflow".into(),
            })?;
        normalized_input
            .try_reserve_exact(normalized_len)
            .map_err(allocation_error)?;
        normalized_input.extend_from_slice(&input[..line.protocol_start]);
        normalized_input.extend_from_slice(b"HTTP/1.1");
        normalized_input.extend_from_slice(&input[line.protocol_end..]);
    }
    let parse_input = if http_09_line.is_some() || special_line.is_some() {
        normalized_input.as_slice()
    } else {
        input
    };
    let mut parser_config = httparse::ParserConfig::default();
    parser_config.allow_multiple_spaces_in_request_line_delimiters(true);

    let mut header_capacity = 32;
    loop {
        let mut header_slots = Vec::new();
        header_slots
            .try_reserve_exact(header_capacity)
            .map_err(|_| ParserFailure {
                name: "HPE_USER",
                description: "unable to allocate HTTP header slots".into(),
            })?;
        header_slots.resize(header_capacity, httparse::EMPTY_HEADER);
        let mut request = httparse::Request::new(&mut header_slots);
        match parser_config.parse_request(&mut request, parse_input) {
            Ok(httparse::Status::Partial) => {
                return Err(ParserFailure {
                    name: "HPE_INVALID_EOF_STATE",
                    description: "header delimiter found before request headers parsed".into(),
                });
            }
            Ok(httparse::Status::Complete(_)) => {
                if let Some(line) = special_line {
                    let method = &input[..line.method_end];
                    match line.kind {
                        SpecialProtocol::Rtsp if !is_rtsp_method(method) => {
                            return Err(unsupported_rtsp_method(method));
                        }
                        SpecialProtocol::Ice if method != b"SOURCE" => {
                            return Err(ParserFailure {
                                name: "HPE_INVALID_CONSTANT",
                                description: "Expected SOURCE method for ICE/x.x request".into(),
                            });
                        }
                        _ => {}
                    }
                }
                let protocol_override = special_line.map(|line| match line.kind {
                    SpecialProtocol::Rtsp => &input[line.protocol_start..line.protocol_end],
                    // lib/http_request.c's fixed-width protocol callback stores the three ICE
                    // bytes before its NUL-filled slash slot, so the public C getter returns ICE.
                    SpecialProtocol::Ice => &b"ICE"[..],
                    SpecialProtocol::Http => &input[line.protocol_start..line.protocol_end],
                });
                let mut parsed = parsed_snapshot(&request, protocol_override, input);
                if http_09_line.is_some() {
                    parsed.protocol = None;
                }
                return Ok(parsed);
            }
            Err(httparse::Error::TooManyHeaders) => {
                header_capacity = header_capacity.checked_mul(2).ok_or(ParserFailure {
                    name: "HPE_USER",
                    description: "HTTP header count overflow".into(),
                })?;
            }
            Err(error) => return Err(parse_request_error(error, input)),
        }
    }
}

#[derive(Clone, Copy)]
struct Http09RequestLine {
    target_end: usize,
    line_end: usize,
}

fn parse_http_09_request_line(input: &[u8]) -> Option<Http09RequestLine> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let mut target_start = method_end + 1;
    while line.get(target_start) == Some(&b' ') {
        target_start += 1;
    }
    let target_end = line[target_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| target_start + offset);
    if target_end != line.len() || target_start == target_end {
        return None;
    }
    Some(Http09RequestLine {
        target_end,
        line_end,
    })
}

#[derive(Clone, Copy)]
struct SpecialRequestLine {
    method_end: usize,
    protocol_start: usize,
    protocol_end: usize,
    kind: SpecialProtocol,
}

#[derive(Clone, Copy)]
enum SpecialProtocol {
    Rtsp,
    Ice,
    Http,
}

/// Find the RTSP request-line forms accepted by this llhttp build.
fn parse_rtsp_request_line(input: &[u8]) -> Option<SpecialRequestLine> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let protocol_start = line.iter().rposition(|byte| *byte == b' ')? + 1;
    let protocol = line.get(protocol_start..)?;
    if method_end == 0
        || protocol_start <= method_end + 1
        || (protocol != b"RTSP/1.0" && protocol != b"RTSP/1.1" && protocol != b"RTSP/2.0")
    {
        return None;
    }
    Some(SpecialRequestLine {
        method_end,
        protocol_start,
        protocol_end: line_end,
        kind: SpecialProtocol::Rtsp,
    })
}

/// llhttp accepts ICE only with its SOURCE request method.
fn parse_ice_request_line(input: &[u8]) -> Option<SpecialRequestLine> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let protocol_start = line.iter().rposition(|byte| *byte == b' ')? + 1;
    let protocol = line.get(protocol_start..)?;
    let version = protocol.strip_prefix(b"ICE/")?;
    if method_end == 0
        || protocol_start <= method_end + 1
        || version.len() != 3
        || !version[0].is_ascii_digit()
        || version[1] != b'.'
        || !version[2].is_ascii_digit()
    {
        return None;
    }
    Some(SpecialRequestLine {
        method_end,
        protocol_start,
        protocol_end: line_end,
        kind: SpecialProtocol::Ice,
    })
}

/// llhttp accepts any single-digit HTTP major/minor pair, while `httparse` only accepts 1.0/1.1.
fn parse_http_version_request_line(input: &[u8]) -> Option<SpecialRequestLine> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let protocol_start = line.iter().rposition(|byte| *byte == b' ')? + 1;
    let protocol = line.get(protocol_start..)?;
    let version = protocol.strip_prefix(b"HTTP/")?;
    if method_end == 0
        || protocol_start <= method_end + 1
        || version.len() != 3
        || !version[0].is_ascii_digit()
        || version[1] != b'.'
        || !version[2].is_ascii_digit()
    {
        return None;
    }
    Some(SpecialRequestLine {
        method_end,
        protocol_start,
        protocol_end: line_end,
        kind: SpecialProtocol::Http,
    })
}

fn is_rtsp_method(method: &[u8]) -> bool {
    matches!(
        method,
        b"GET"
            | b"POST"
            | b"OPTIONS"
            | b"DESCRIBE"
            | b"ANNOUNCE"
            | b"SETUP"
            | b"PLAY"
            | b"PAUSE"
            | b"TEARDOWN"
            | b"GET_PARAMETER"
            | b"SET_PARAMETER"
            | b"REDIRECT"
            | b"RECORD"
            | b"FLUSH"
    )
}

fn capture_partial_request_line(
    input: &[u8],
    url: &mut Option<Vec<u8>>,
    protocol: &mut Option<String>,
) {
    let line_end = input
        .iter()
        .position(|byte| matches!(*byte, b'\r' | b'\n'))
        .unwrap_or(input.len());
    let line = &input[..line_end];
    let Some(method_end) = line.iter().position(|byte| *byte == b' ') else {
        return;
    };

    let mut target_start = method_end + 1;
    while line.get(target_start) == Some(&b' ') {
        target_start += 1;
    }
    let target_end = line[target_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| target_start + offset);
    if target_start < target_end {
        *url = Some(line[target_start..target_end].to_vec());
    }
    if target_end == line.len() {
        return;
    }

    let mut protocol_start = target_end;
    while line.get(protocol_start) == Some(&b' ') {
        protocol_start += 1;
    }
    let raw_protocol = &line[protocol_start..];
    if raw_protocol.is_empty() {
        return;
    }
    let prefix_end = raw_protocol
        .iter()
        .position(|byte| *byte == b'/')
        .unwrap_or(raw_protocol.len())
        .min(4);
    let mut parsed_protocol = raw_protocol[..prefix_end].to_vec();
    if prefix_end == 4 {
        parsed_protocol.push(b'/');
        if raw_protocol.get(prefix_end) == Some(&b'/') {
            let version_end = raw_protocol[prefix_end + 1..]
                .iter()
                .position(|byte| !byte.is_ascii_digit() && *byte != b'.')
                .map_or(raw_protocol.len(), |offset| prefix_end + 1 + offset)
                .min(prefix_end + 4);
            parsed_protocol.extend_from_slice(&raw_protocol[prefix_end + 1..version_end]);
        }
    }
    if let Ok(parsed_protocol) = String::from_utf8(parsed_protocol) {
        *protocol = Some(parsed_protocol);
    }
}

fn raw_request_method(input: &[u8]) -> Option<&[u8]> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    Some(&line[..method_end])
}

fn raw_request_target(input: &[u8]) -> Option<&[u8]> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let mut target_start = method_end + 1;
    while line.get(target_start) == Some(&b' ') {
        target_start += 1;
    }
    let target_end = line[target_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| target_start + offset);
    (target_start < target_end).then_some(&line[target_start..target_end])
}

fn invalid_method() -> ParserFailure {
    ParserFailure {
        name: "HPE_INVALID_METHOD",
        description: "Invalid method encountered".into(),
    }
}

fn unsupported_rtsp_method(method: &[u8]) -> ParserFailure {
    if !is_llhttp_method(method) {
        return invalid_method();
    }
    ParserFailure {
        name: "HPE_INVALID_CONSTANT",
        description: "Invalid method for RTSP/x.x request".into(),
    }
}

const LLHTTP_METHODS: &[&[u8]] = &[
    b"DELETE",
    b"GET",
    b"HEAD",
    b"POST",
    b"PUT",
    b"CONNECT",
    b"OPTIONS",
    b"TRACE",
    b"COPY",
    b"LOCK",
    b"MKCOL",
    b"MOVE",
    b"PROPFIND",
    b"PROPPATCH",
    b"SEARCH",
    b"UNLOCK",
    b"BIND",
    b"REBIND",
    b"UNBIND",
    b"ACL",
    b"REPORT",
    b"MKACTIVITY",
    b"CHECKOUT",
    b"MERGE",
    b"M-SEARCH",
    b"NOTIFY",
    b"SUBSCRIBE",
    b"UNSUBSCRIBE",
    b"PATCH",
    b"PURGE",
    b"MKCALENDAR",
    b"LINK",
    b"UNLINK",
    b"SOURCE",
    b"PRI",
    b"DESCRIBE",
    b"ANNOUNCE",
    b"SETUP",
    b"PLAY",
    b"PAUSE",
    b"TEARDOWN",
    b"GET_PARAMETER",
    b"SET_PARAMETER",
    b"REDIRECT",
    b"RECORD",
    b"FLUSH",
    b"QUERY",
];

fn is_llhttp_method(method: &[u8]) -> bool {
    LLHTTP_METHODS.contains(&method)
}

fn llhttp_method_prefix_failure(method: &[u8]) -> Option<usize> {
    (1..=method.len())
        .find(|end| {
            !LLHTTP_METHODS
                .iter()
                .any(|candidate| candidate.starts_with(&method[..*end]))
        })
        .map(|end| end - 1)
}

fn parsed_snapshot(
    request: &httparse::Request<'_, '_>,
    protocol_override: Option<&[u8]>,
    raw_input: &[u8],
) -> ParsedRequest {
    let raw_values = raw_request_header_values(raw_input);
    ParsedRequest {
        method: request.method.map(str::to_owned),
        url: request.path.map(|path| path.as_bytes().to_vec()),
        protocol: protocol_override
            .and_then(|protocol| std::str::from_utf8(protocol).ok())
            .map(str::to_owned)
            .or_else(|| request.version.map(|version| format!("HTTP/1.{version}"))),
        headers: request
            .headers
            .iter()
            .filter(|header| !header.name.is_empty())
            .enumerate()
            .map(|(index, header)| RequestHeader {
                name: header.name.as_bytes().to_vec(),
                value: raw_values
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .copied()
                    .unwrap_or(header.value)
                    .to_vec(),
            })
            .collect(),
    }
}

/// Reproduce llhttp's header-value spans: leading OWS is skipped, trailing OWS is included.
fn raw_request_header_values(input: &[u8]) -> Option<Vec<&[u8]>> {
    let request_line_end = find_bytes(input, b"\r\n")?;
    raw_header_value_spans(input, request_line_end + 2)
}

fn capture_partial_headers(
    input: &[u8],
    headers: &mut Vec<RequestHeader>,
    values_present: &mut Vec<bool>,
    lines_complete: &mut Vec<bool>,
) {
    let Some(request_line_end) = find_bytes(input, b"\r\n") else {
        return;
    };
    let mut cursor = request_line_end + 2;
    let mut parsed_headers = Vec::new();
    let mut parsed_values_present = Vec::new();
    let mut parsed_lines_complete = Vec::new();

    while cursor < input.len() {
        if input[cursor..].starts_with(b"\r\n") {
            break;
        }
        let line_end = find_bytes(&input[cursor..], b"\r\n").map(|offset| cursor + offset);
        let end = line_end.unwrap_or(input.len());
        let line = &input[cursor..end];
        if line.is_empty() {
            break;
        }

        if let Some(colon) = line.iter().position(|byte| *byte == b':') {
            let raw_value = &line[colon + 1..];
            let value_start = raw_value
                .iter()
                .position(|byte| !matches!(*byte, b' ' | b'\t'))
                .unwrap_or(raw_value.len());
            parsed_headers.push(RequestHeader {
                name: line[..colon].to_vec(),
                value: raw_value[value_start..].to_vec(),
            });
            parsed_values_present.push(line_end.is_some() || value_start < raw_value.len());
            parsed_lines_complete.push(line_end.is_some());
        } else {
            parsed_headers.push(RequestHeader {
                name: line.to_vec(),
                value: Vec::new(),
            });
            parsed_values_present.push(false);
            parsed_lines_complete.push(line_end.is_some());
        }

        let Some(line_end) = line_end else {
            break;
        };
        cursor = line_end + 2;
    }

    *headers = parsed_headers;
    *values_present = parsed_values_present;
    *lines_complete = parsed_lines_complete;
}

fn raw_header_value_spans(input: &[u8], mut cursor: usize) -> Option<Vec<&[u8]>> {
    let mut values = Vec::new();
    while cursor < input.len() {
        let line_end = cursor + find_bytes(&input[cursor..], b"\r\n")?;
        if line_end == cursor {
            break;
        }
        let line = &input[cursor..line_end];
        let colon = line.iter().position(|byte| *byte == b':')?;
        let value = &line[colon + 1..];
        let value_start = value
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t'))
            .unwrap_or(value.len());
        values.push(&value[value_start..]);
        cursor = line_end + 2;
    }
    Some(values)
}

fn body_framing(headers: &[RequestHeader]) -> Result<BodyParser, ParserFailure> {
    let mut content_length = None;
    let mut has_transfer_encoding = false;
    let mut chunked_seen = false;
    for header in headers {
        if header.name.eq_ignore_ascii_case(b"content-length") {
            if has_transfer_encoding {
                return Err(ParserFailure {
                    name: "HPE_INVALID_CONTENT_LENGTH",
                    description: "Content-Length can't be present with Transfer-Encoding".into(),
                });
            }
            let value = parse_content_length(&header.value)?;
            if content_length.is_some() {
                return Err(ParserFailure {
                    name: "HPE_UNEXPECTED_CONTENT_LENGTH",
                    description: "duplicate Content-Length header".into(),
                });
            }
            content_length = Some(value);
        } else if header.name.eq_ignore_ascii_case(b"transfer-encoding") {
            if content_length.is_some() {
                return Err(ParserFailure {
                    name: "HPE_INVALID_TRANSFER_ENCODING",
                    description: "Transfer-Encoding can't be present with Content-Length".into(),
                });
            }
            has_transfer_encoding = true;
            for part in header.value.split(|byte| *byte == b',') {
                if chunked_seen {
                    return Err(ParserFailure {
                        name: "HPE_INVALID_TRANSFER_ENCODING",
                        description: "Invalid `Transfer-Encoding` header value".into(),
                    });
                }
                let is_chunked = trim_ascii(part).eq_ignore_ascii_case(b"chunked");
                chunked_seen |= is_chunked;
            }
        }
    }

    if has_transfer_encoding && !chunked_seen {
        return Err(ParserFailure {
            name: "HPE_INVALID_TRANSFER_ENCODING",
            description: "Request has invalid `Transfer-Encoding`".into(),
        });
    }

    let chunked = has_transfer_encoding;
    if chunked {
        Ok(BodyParser::Chunked(ChunkedBody {
            state: ChunkState::SizeLine(Vec::new()),
        }))
    } else {
        match content_length.unwrap_or(0) {
            0 => Ok(BodyParser::Complete),
            remaining => Ok(BodyParser::Fixed { remaining }),
        }
    }
}

fn has_upgrade_headers(headers: &[RequestHeader]) -> bool {
    let connection_allows_upgrade = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(b"connection"))
        .any(|header| {
            header
                .value
                .split(|byte| *byte == b',')
                .any(|token| trim_http_ows(token).eq_ignore_ascii_case(b"upgrade"))
        });
    let upgrade_has_value = headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case(b"upgrade") && !trim_http_ows(&header.value).is_empty()
    });
    connection_allows_upgrade && upgrade_has_value
}

fn trim_http_ows(mut value: &[u8]) -> &[u8] {
    while value
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[1..];
    }
    while value
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[..value.len() - 1];
    }
    value
}

fn parse_content_length(value: &[u8]) -> Result<usize, ParserFailure> {
    let value = trim_ascii(value);
    if value.is_empty() {
        return Err(ParserFailure {
            name: "HPE_INVALID_CONTENT_LENGTH",
            description: "Empty Content-Length".into(),
        });
    }
    if !value.iter().all(u8::is_ascii_digit) {
        return Err(ParserFailure {
            name: "HPE_INVALID_CONTENT_LENGTH",
            description: "Invalid character in Content-Length".into(),
        });
    }
    let text = std::str::from_utf8(value).map_err(|_| ParserFailure {
        name: "HPE_INVALID_CONTENT_LENGTH",
        description: "Invalid character in Content-Length".into(),
    })?;
    text.parse().map_err(|_| ParserFailure {
        name: "HPE_INVALID_CONTENT_LENGTH",
        description: "Content-Length overflow".into(),
    })
}

impl ChunkedBody {
    fn consume(
        &mut self,
        input: &[u8],
        body: &mut Option<Vec<u8>>,
        headers: &mut Vec<RequestHeader>,
    ) -> Result<(bool, usize), ParserFailure> {
        let mut offset = 0;
        while offset < input.len() {
            match &mut self.state {
                ChunkState::SizeLine(line) => {
                    let rest = &input[offset..];
                    if let Some(line_end) = rest.iter().position(|byte| *byte == b'\n') {
                        line.try_reserve(line_end + 1).map_err(allocation_error)?;
                        line.extend_from_slice(&rest[..=line_end]);
                        offset += line_end + 1;
                        if !line.ends_with(b"\r\n") {
                            return Err(missing_chunk_cr_error(
                                line.strip_suffix(b"\n").unwrap_or(line),
                            ));
                        }
                        line.truncate(line.len() - 2);
                        let size = parse_chunk_size_line(line)?;
                        self.state = if size == 0 {
                            ChunkState::Trailers(Vec::new())
                        } else {
                            ChunkState::Data { remaining: size }
                        };
                    } else {
                        line.try_reserve(rest.len()).map_err(allocation_error)?;
                        line.extend_from_slice(rest);
                        return Ok((false, input.len()));
                    }
                }
                ChunkState::Data { remaining } => {
                    let consumed = (*remaining).min(input.len() - offset);
                    append_body(body, &input[offset..offset + consumed])?;
                    *remaining -= consumed;
                    offset += consumed;
                    if *remaining == 0 {
                        self.state = ChunkState::DataCrlf { saw_cr: false };
                    }
                }
                ChunkState::DataCrlf { saw_cr } => {
                    let byte = input[offset];
                    offset += 1;
                    if !*saw_cr {
                        if byte != b'\r' {
                            return Err(chunk_error("chunk data is not followed by CRLF"));
                        }
                        *saw_cr = true;
                    } else if byte == b'\n' {
                        self.state = ChunkState::SizeLine(Vec::new());
                    } else {
                        return Err(chunk_error("chunk data is not followed by CRLF"));
                    }
                }
                ChunkState::Trailers(trailers) => {
                    trailers.try_reserve(1).map_err(allocation_error)?;
                    trailers.push(input[offset]);
                    offset += 1;
                    if trailers == b"\r\n" {
                        self.state = ChunkState::Complete;
                        return Ok((true, offset));
                    }
                    if trailers.ends_with(b"\r\n\r\n") {
                        headers.extend(parse_trailer_headers(trailers)?);
                        self.state = ChunkState::Complete;
                        return Ok((true, offset));
                    }
                }
                ChunkState::Complete => return Ok((true, offset)),
            }
        }
        Ok((matches!(self.state, ChunkState::Complete), offset))
    }
}

fn parse_trailer_headers(input: &[u8]) -> Result<Vec<RequestHeader>, ParserFailure> {
    let raw_values = raw_header_value_spans(input, 0);
    let mut capacity = 16;
    loop {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| allocation_error(()))?;
        slots.resize(capacity, httparse::EMPTY_HEADER);
        match httparse::parse_headers(input, &mut slots) {
            Ok(httparse::Status::Complete((_consumed, headers))) => {
                return Ok(headers
                    .iter()
                    .enumerate()
                    .map(|(index, header)| RequestHeader {
                        name: header.name.as_bytes().to_vec(),
                        value: raw_values
                            .as_ref()
                            .and_then(|values| values.get(index))
                            .copied()
                            .unwrap_or(header.value)
                            .to_vec(),
                    })
                    .collect());
            }
            Ok(httparse::Status::Partial) => {
                return Err(ParserFailure {
                    name: "HPE_INVALID_EOF_STATE",
                    description: "trailer delimiter found before trailer headers parsed".into(),
                });
            }
            Err(httparse::Error::TooManyHeaders) => {
                capacity = capacity.checked_mul(2).ok_or(ParserFailure {
                    name: "HPE_USER",
                    description: "trailer header count overflow".into(),
                })?;
            }
            Err(error) => return Err(parse_error(error)),
        }
    }
}

fn append_body(body: &mut Option<Vec<u8>>, data: &[u8]) -> Result<(), ParserFailure> {
    if data.is_empty() {
        return Ok(());
    }
    let body = body.get_or_insert_with(Vec::new);
    body.try_reserve(data.len()).map_err(allocation_error)?;
    body.extend_from_slice(data);
    Ok(())
}

fn allocation_error(_: impl fmt::Debug) -> ParserFailure {
    ParserFailure {
        name: "HPE_USER",
        description: "unable to allocate request data".into(),
    }
}

fn chunk_error(description: &str) -> ParserFailure {
    ParserFailure {
        name: "HPE_INVALID_CHUNK_SIZE",
        description: description.into(),
    }
}

fn parse_chunk_size_line(line: &[u8]) -> Result<usize, ParserFailure> {
    let size_end = line
        .iter()
        .position(|byte| !byte.is_ascii_hexdigit())
        .unwrap_or(line.len());
    if size_end == 0 {
        return Err(chunk_error("invalid hexadecimal chunk size"));
    }
    let size = usize::from_str_radix(std::str::from_utf8(&line[..size_end]).unwrap_or(""), 16)
        .map_err(|_| chunk_error("chunk size overflows the platform size"))?;
    let remainder = &line[size_end..];
    if !remainder.is_empty() && remainder[0] != b';' {
        return Err(chunk_error("Invalid character in chunk size"));
    }
    validate_chunk_extensions(remainder)?;
    Ok(size)
}

#[derive(Clone, Copy)]
enum ChunkExtensionEnd {
    Name,
    Value,
}

fn validate_chunk_extensions(input: &[u8]) -> Result<Option<ChunkExtensionEnd>, ParserFailure> {
    let mut cursor = 0;
    let mut end = None;
    while cursor < input.len() {
        if input[cursor] != b';' {
            return Err(chunk_extension_error(
                "Invalid character in chunk extensions",
            ));
        }
        cursor += 1;

        let name_start = cursor;
        while input
            .get(cursor)
            .is_some_and(|byte| is_http_token_byte(*byte))
        {
            cursor += 1;
        }
        if cursor == name_start {
            return Err(chunk_extension_error(
                "Invalid character in chunk extensions",
            ));
        }

        if input.get(cursor) == Some(&b'=') {
            cursor += 1;
            if input.get(cursor) == Some(&b'"') {
                cursor = parse_quoted_chunk_extension(input, cursor)?;
                if cursor < input.len() && input[cursor] != b';' {
                    return Err(chunk_extension_error(
                        "Invalid character in chunk extensions quote value",
                    ));
                }
            } else {
                while input
                    .get(cursor)
                    .is_some_and(|byte| is_http_token_byte(*byte))
                {
                    cursor += 1;
                }
                if cursor < input.len() && input[cursor] != b';' {
                    return Err(chunk_extension_error(
                        "Invalid character in chunk extensions value",
                    ));
                }
            }
            end = Some(ChunkExtensionEnd::Value);
        } else {
            if cursor < input.len() && input[cursor] != b';' {
                return Err(chunk_extension_error(
                    "Invalid character in chunk extensions name",
                ));
            }
            end = Some(ChunkExtensionEnd::Name);
        }
    }
    Ok(end)
}

fn parse_quoted_chunk_extension(input: &[u8], quote: usize) -> Result<usize, ParserFailure> {
    let mut cursor = quote + 1;
    while let Some(byte) = input.get(cursor).copied() {
        match byte {
            b'"' => return Ok(cursor + 1),
            b'\\' => {
                cursor += 1;
                if !input
                    .get(cursor)
                    .is_some_and(|escaped| matches!(*escaped, b'\t' | b' '..=b'~' | 0x80..=0xff))
                {
                    return Err(chunk_extension_error(
                        "Invalid quoted-pair in chunk extensions quoted value",
                    ));
                }
            }
            b'\t' | b' '..=b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff => {}
            _ => {
                return Err(chunk_extension_error(
                    "Invalid character in chunk extensions quoted value",
                ));
            }
        }
        cursor += 1;
    }
    Err(chunk_extension_error(
        "Invalid character in chunk extensions quoted value",
    ))
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn chunk_extension_error(description: &str) -> ParserFailure {
    ParserFailure {
        name: "HPE_STRICT",
        description: description.into(),
    }
}

fn missing_chunk_cr_error(line: &[u8]) -> ParserFailure {
    let size_end = line
        .iter()
        .position(|byte| !byte.is_ascii_hexdigit())
        .unwrap_or(line.len());
    if size_end == 0 {
        return chunk_error("invalid hexadecimal chunk size");
    }
    if size_end == line.len() {
        return ParserFailure {
            name: "HPE_CR_EXPECTED",
            description: "Missing expected CR after chunk size".into(),
        };
    }
    match validate_chunk_extensions(&line[size_end..]) {
        Ok(Some(ChunkExtensionEnd::Name)) => ParserFailure {
            name: "HPE_CR_EXPECTED",
            description: "Missing expected CR after chunk extension name".into(),
        },
        Ok(Some(ChunkExtensionEnd::Value)) => ParserFailure {
            name: "HPE_CR_EXPECTED",
            description: "Missing expected CR after chunk extension value".into(),
        },
        Ok(None) => ParserFailure {
            name: "HPE_CR_EXPECTED",
            description: "Missing expected CR after chunk size".into(),
        },
        Err(error) => error,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn validate_header_line_endings(input: &[u8], start: usize) -> Result<(), ParserFailure> {
    for index in start.min(input.len())..input.len() {
        match input[index] {
            b'\n' if index == 0 || input[index - 1] != b'\r' => {
                return Err(line_ending_failure(input, index, false));
            }
            b'\r' if index + 1 < input.len() && input[index + 1] != b'\n' => {
                return Err(line_ending_failure(input, index, true));
            }
            _ => {}
        }
    }
    Ok(())
}

fn line_ending_failure(input: &[u8], index: usize, saw_cr: bool) -> ParserFailure {
    let request_line_end = find_bytes(input, b"\r\n");
    if request_line_end.is_none_or(|line_end| index < line_end) {
        return if saw_cr {
            ParserFailure {
                name: "HPE_STRICT",
                description: "Expected CRLF after version".into(),
            }
        } else {
            ParserFailure {
                name: "HPE_INVALID_VERSION",
                description: "Expected CRLF after version".into(),
            }
        };
    }

    let Some(request_line_end) = request_line_end else {
        unreachable!("request-line failure returned above")
    };
    let header_start = request_line_end + 2;
    let last_line_start = input[header_start..index]
        .windows(2)
        .rposition(|window| window == b"\r\n")
        .map_or(header_start, |offset| header_start + offset + 2);
    let line = &input[last_line_start..index];

    if line.is_empty() {
        ParserFailure {
            name: if saw_cr {
                "HPE_STRICT"
            } else {
                "HPE_INVALID_HEADER_TOKEN"
            },
            description: if saw_cr {
                "Expected LF after headers"
            } else {
                "Invalid header field char"
            }
            .into(),
        }
    } else if let Some(colon) = line.iter().position(|byte| *byte == b':') {
        let has_value = line[colon + 1..]
            .iter()
            .any(|byte| !matches!(*byte, b' ' | b'\t'));
        if saw_cr && has_value {
            ParserFailure {
                name: "HPE_LF_EXPECTED",
                description: "Missing expected LF after header value".into(),
            }
        } else if !saw_cr && has_value {
            ParserFailure {
                name: "HPE_CR_EXPECTED",
                description: "Missing expected CR after header value".into(),
            }
        } else if saw_cr {
            ParserFailure {
                name: "HPE_STRICT",
                description: "Expected LF after CR".into(),
            }
        } else {
            ParserFailure {
                name: "HPE_INVALID_HEADER_TOKEN",
                description: "Invalid header value char".into(),
            }
        }
    } else {
        ParserFailure {
            name: "HPE_INVALID_HEADER_TOKEN",
            description: "Invalid header token".into(),
        }
    }
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &value[start..end]
}

fn parse_request_error(error: httparse::Error, input: &[u8]) -> ParserFailure {
    if error == httparse::Error::Token
        && let Some(failure) = request_target_url_failure(input)
    {
        return failure;
    }
    if error == httparse::Error::Version
        && let Some(failure) = request_line_version_failure(input)
    {
        return failure;
    }
    parse_error(error)
}

fn request_target_url_failure(input: &[u8]) -> Option<ParserFailure> {
    let method = raw_request_method(input)?;
    let target = raw_request_target(input)?;
    request_target_url_failure_at(method, target).map(|(failure, _)| failure)
}

fn request_target_url_failure_at(method: &[u8], target: &[u8]) -> Option<(ParserFailure, usize)> {
    let mut url = target;
    let mut url_offset = 0;

    // llhttp treats CONNECT's target as an authority, and recognizes absolute
    // URI authorities before its path/query states.
    if method == b"CONNECT" {
        if let Some((offset, reason)) = url.iter().enumerate().find_map(|(offset, byte)| {
            url_server_char_failure(*byte).map(|reason| (offset, reason))
        }) {
            return Some((invalid_url(reason), offset));
        }
    } else if let Some(scheme_end) = absolute_uri_scheme_end(url) {
        let authority_start = scheme_end + 3;
        let authority_end = url[authority_start..]
            .iter()
            .position(|byte| matches!(*byte, b'/' | b'?' | b'#'))
            .map_or(url.len(), |offset| authority_start + offset);
        if let Some((offset, reason)) = url[authority_start..authority_end]
            .iter()
            .enumerate()
            .find_map(|(offset, byte)| {
                url_server_char_failure(*byte).map(|reason| (offset, reason))
            })
        {
            return Some((invalid_url(reason), authority_start + offset));
        }
        url_offset = authority_end;
        url = &url[authority_end..];
    } else if target.first().is_some_and(u8::is_ascii_alphabetic) {
        let scheme_end = target.iter().position(|byte| *byte == b':');
        let scheme = scheme_end.map_or(target, |end| &target[..end]);
        if let Some(offset) = scheme
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'-' | b'.')))
        {
            return Some((invalid_url("Unexpected char in url schema"), offset));
        }
        let Some(scheme_end) = scheme_end else {
            if let Some(offset) = target
                .iter()
                .position(|byte| matches!(*byte, b'/' | b'?' | b'#'))
            {
                return Some((invalid_url("Unexpected char in url schema"), offset));
            }
            return Some((invalid_url("Invalid characters in url"), target.len()));
        };
        if !target[scheme_end..].starts_with(b"://") {
            let mismatch = target[scheme_end..]
                .iter()
                .zip(b"://")
                .position(|(actual, expected)| actual != expected)
                .unwrap_or(target[scheme_end..].len());
            return Some((
                invalid_url("Unexpected char in url schema"),
                scheme_end + mismatch,
            ));
        }
        let authority_start = scheme_end + 3;
        let authority_end = target[authority_start..]
            .iter()
            .position(|byte| matches!(*byte, b'/' | b'?' | b'#'))
            .map_or(target.len(), |offset| authority_start + offset);
        if let Some((offset, reason)) = target[authority_start..authority_end]
            .iter()
            .enumerate()
            .find_map(|(offset, byte)| {
                url_server_char_failure(*byte).map(|reason| (offset, reason))
            })
        {
            return Some((invalid_url(reason), authority_start + offset));
        }
        url_offset = authority_end;
        url = &target[authority_end..];
    } else if !target
        .first()
        .is_some_and(|byte| matches!(*byte, b'/' | b'*'))
    {
        return Some((invalid_url("Unexpected start char in url"), 0));
    }

    let mut component = UrlComponent::Path;
    for (offset, byte) in url.iter().enumerate() {
        match (component, *byte) {
            (UrlComponent::Path, b'?') => {
                component = UrlComponent::Query;
                continue;
            }
            (UrlComponent::Path | UrlComponent::Query, b'#') => {
                component = UrlComponent::Fragment;
                continue;
            }
            _ => {}
        }

        let reason = match *byte {
            b'\t' | 0x0c => Some("Invalid characters in url"),
            0x00..=0x1f | 0x7f..=0xff => Some(match component {
                UrlComponent::Path => "Invalid char in url path",
                UrlComponent::Query => "Invalid char in url query",
                UrlComponent::Fragment => "Invalid char in url fragment start",
            }),
            _ => None,
        };
        if let Some(reason) = reason {
            return Some((invalid_url(reason), url_offset + offset));
        }
    }
    None
}

fn url_server_char_failure(byte: u8) -> Option<&'static str> {
    match byte {
        b'\t' | 0x0c => Some("Invalid characters in url"),
        0x00..=0x08
        | 0x0b
        | 0x0e..=0x1f
        | b'"'
        | b'<'
        | b'>'
        | b'\\'
        | b'^'
        | b'`'
        | b'{'
        | b'|'
        | b'}'
        | 0x7f..=0xff => Some("Unexpected char in url server"),
        _ => None,
    }
}

fn request_line_protocol_failure(input: &[u8]) -> Option<ParserFailure> {
    let protocol = raw_request_protocol(input)?;
    if protocol.is_empty()
        || protocol.starts_with(b"HTTP/")
        || protocol.starts_with(b"RTSP/")
        || protocol.starts_with(b"ICE/")
    {
        return None;
    }
    Some(ParserFailure {
        name: "HPE_INVALID_CONSTANT",
        description: "Expected HTTP/, RTSP/ or ICE/".into(),
    })
}

fn request_line_version_failure(input: &[u8]) -> Option<ParserFailure> {
    let protocol = raw_request_protocol(input)?;
    let (kind, version) = if let Some(version) = protocol.strip_prefix(b"HTTP/") {
        (SpecialProtocol::Http, version)
    } else if let Some(version) = protocol.strip_prefix(b"RTSP/") {
        (SpecialProtocol::Rtsp, version)
    } else {
        (SpecialProtocol::Ice, protocol.strip_prefix(b"ICE/")?)
    };

    let failure = |description: &str| ParserFailure {
        name: "HPE_INVALID_VERSION",
        description: description.into(),
    };
    if !version.first().is_some_and(u8::is_ascii_digit) {
        return Some(failure("Invalid major version"));
    }
    if version.get(1) != Some(&b'.') {
        return Some(failure("Expected dot"));
    }
    if !version.get(2).is_some_and(u8::is_ascii_digit) {
        return Some(failure("Invalid minor version"));
    }
    if version.len() > 3 {
        return Some(failure("Expected CRLF after version"));
    }
    if matches!(kind, SpecialProtocol::Rtsp) && !matches!(version, b"1.0" | b"1.1" | b"2.0") {
        return Some(failure("Invalid HTTP version"));
    }
    None
}

fn raw_request_protocol(input: &[u8]) -> Option<&[u8]> {
    let line_end = find_bytes(input, b"\r\n")?;
    let line = &input[..line_end];
    let method_end = line.iter().position(|byte| *byte == b' ')?;
    let mut target_start = method_end + 1;
    while line.get(target_start) == Some(&b' ') {
        target_start += 1;
    }
    let target_end = line[target_start..]
        .iter()
        .position(|byte| *byte == b' ')
        .map_or(line.len(), |offset| target_start + offset);
    let mut protocol_start = target_end;
    while line.get(protocol_start) == Some(&b' ') {
        protocol_start += 1;
    }
    Some(&line[protocol_start..])
}

#[derive(Clone, Copy)]
enum UrlComponent {
    Path,
    Query,
    Fragment,
}

fn absolute_uri_scheme_end(target: &[u8]) -> Option<usize> {
    let delimiter = target.windows(3).position(|part| part == b"://")?;
    let scheme = target.get(..delimiter)?;
    let mut bytes = scheme.iter();
    let first = *bytes.next()?;
    if !first.is_ascii_alphabetic()
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return None;
    }
    Some(delimiter)
}

fn invalid_url(description: &str) -> ParserFailure {
    ParserFailure {
        name: "HPE_INVALID_URL",
        description: description.into(),
    }
}

fn parse_error(error: httparse::Error) -> ParserFailure {
    let (name, description) = match error {
        httparse::Error::HeaderName => ("HPE_INVALID_HEADER_TOKEN", "Invalid header token"),
        httparse::Error::HeaderValue => ("HPE_INVALID_HEADER_TOKEN", "Invalid header value char"),
        httparse::Error::NewLine => ("HPE_STRICT", "Expected LF after CR"),
        httparse::Error::Status => ("HPE_INVALID_METHOD", "Invalid method encountered"),
        httparse::Error::Token => ("HPE_INVALID_METHOD", "Invalid method encountered"),
        httparse::Error::TooManyHeaders => ("HPE_HEADER_OVERFLOW", "too many headers"),
        httparse::Error::Version => ("HPE_INVALID_VERSION", "Invalid HTTP version"),
    };
    ParserFailure {
        name,
        description: description.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_request_and_body() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"POST /play HTTP/1.1\r\nCSeq: 8\r\nContent-Length: 4\r\n\r\nbo")
            .unwrap();
        assert!(!request.is_complete());
        assert_eq!(request.method(), None);
        assert_eq!(request.url(), Some(&b"/play"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.header(b"CSeq"), Some(&b"8"[..]));
        assert_eq!(request.body(), Some(&b"bo"[..]));
        request.add_data(b"dy").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("POST"));
        assert_eq!(request.url(), Some(&b"/play"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.header(b"CSeq"), Some(&b"8"[..]));
        assert_eq!(request.body(), Some(&b"body"[..]));
        assert_eq!(request.buffered_len(), 0);
    }

    #[test]
    fn exposes_request_url_and_protocol_as_llhttp_receives_them() {
        let mut request = HttpRequest::new();
        assert_eq!(request.method(), None);
        assert_eq!(request.url(), None);
        assert_eq!(request.protocol(), Some(""));

        request.add_data(b"GET").unwrap();
        assert_eq!(request.url(), None);
        request.add_data(b" ").unwrap();
        assert_eq!(request.url(), None);
        request.add_data(b"/abc").unwrap();
        assert_eq!(request.url(), Some(&b"/abc"[..]));
        assert_eq!(request.method(), None);
        assert_eq!(request.protocol(), Some(""));

        request.add_data(b"/def").unwrap();
        assert_eq!(request.url(), Some(&b"/abc/def"[..]));
        request.add_data(b" HTTP/1.").unwrap();
        assert_eq!(request.protocol(), Some("HTTP/1."));
        request.add_data(b"1\r\n").unwrap();
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.method(), None);
        assert!(!request.is_complete());

        request.add_data(b"\r\n").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("GET"));
        assert_eq!(request.url(), Some(&b"/abc/def"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
    }

    #[test]
    fn preserves_the_source_empty_protocol_for_http_09_requests() {
        let mut request = HttpRequest::new();
        request.add_data(b"GET /legacy\r\n").unwrap();
        assert!(!request.is_complete());
        assert_eq!(request.method(), None);
        assert_eq!(request.url(), Some(&b"/legacy"[..]));
        assert_eq!(request.protocol(), Some(""));

        request.add_data(b"\r\n").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("GET"));
        assert_eq!(request.url(), Some(&b"/legacy"[..]));
        assert_eq!(request.protocol(), Some(""));
    }

    #[test]
    fn coalesced_http_09_callbacks_append_headers_body_and_completed_method() {
        let mut request = HttpRequest::new();
        request.add_data(b"GET /one HTTP/1.1\r\n\r\n").unwrap();
        request
            .apply_source_read_ahead(b"POST /two\r\nHost: x\r\nContent-Length: 3\r\n\r\nabc")
            .unwrap();

        assert_eq!(request.method(), Some("POST"));
        assert_eq!(request.url(), Some(&b"/one/two"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.header(b"Host"), Some(&b"x"[..]));
        assert_eq!(request.header(b"Content-Length"), Some(&b"3"[..]));
        assert_eq!(request.body(), Some(&b"abc"[..]));
    }

    #[test]
    fn coalesced_incomplete_http_09_callbacks_append_header_without_method_update() {
        let mut request = HttpRequest::new();
        request.add_data(b"GET /one HTTP/1.1\r\n\r\n").unwrap();
        request
            .apply_source_read_ahead(b"POST /two\r\nHost: x\r\n")
            .unwrap();

        assert_eq!(request.method(), Some("GET"));
        assert_eq!(request.url(), Some(&b"/one/two"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.header(b"Host"), Some(&b"x"[..]));
    }

    #[test]
    fn exposes_partial_header_callbacks_like_llhttp() {
        let mut request = HttpRequest::new();
        request.add_data(b"GET / HTTP/1.1\r\n").unwrap();
        assert_eq!(request.header(b"Host"), None);
        assert_eq!(request.header_stats(), (0, 0, 0, 0));

        request.add_data(b"Host").unwrap();
        assert_eq!(request.header(b"Host"), None);
        assert_eq!(request.header_stats(), (4, 1, 4, 0));
        assert_eq!(request.header_string().as_deref(), Some(&b"Host: "[..]));

        request.add_data(b": ").unwrap();
        assert_eq!(request.header(b"Host"), None);
        assert_eq!(request.header_string().as_deref(), Some(&b"Host: "[..]));
        request.add_data(b" example").unwrap();
        assert_eq!(request.header(b"Host"), Some(&b"example"[..]));
        assert_eq!(request.header_stats(), (11, 1, 4, 7));
        assert_eq!(
            request.header_string().as_deref(),
            Some(&b"Host: example"[..])
        );

        request.add_data(b".com\r\nCSeq: 7\r\n\r\n").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.header(b"Host"), Some(&b"example.com"[..]));
        assert_eq!(request.header(b"CSeq"), Some(&b"7"[..]));
        assert_eq!(
            request.header_string().as_deref(),
            Some(&b"Host: example.com\nCSeq: 7\n"[..])
        );
    }

    #[test]
    fn rejects_bare_lf_and_malformed_crlf_before_the_header_terminator() {
        for (input, expected_name, expected_description) in [
            (
                &b"GET / HTTP/1.1\n"[..],
                "HPE_INVALID_VERSION",
                "Expected CRLF after version",
            ),
            (
                &b"GET / HTTP/1.1\rX"[..],
                "HPE_STRICT",
                "Expected CRLF after version",
            ),
            (
                &b"GET / HTTP/1.1\r\nX-Test: value\n"[..],
                "HPE_CR_EXPECTED",
                "Missing expected CR after header value",
            ),
            (
                &b"GET / HTTP/1.1\r\nX-Test: value\rX"[..],
                "HPE_LF_EXPECTED",
                "Missing expected LF after header value",
            ),
            (
                &b"GET / HTTP/1.1\r\nX-Test: \rX"[..],
                "HPE_STRICT",
                "Expected LF after CR",
            ),
            (
                &b"GET / HTTP/1.1\r\n\rX"[..],
                "HPE_STRICT",
                "Expected LF after headers",
            ),
            (
                &b"GET / HTTP/1.1\r\nX-Test\n"[..],
                "HPE_INVALID_HEADER_TOKEN",
                "Invalid header token",
            ),
            (
                &b"GET / HTTP/1.1\r\n\n"[..],
                "HPE_INVALID_HEADER_TOKEN",
                "Invalid header field char",
            ),
        ] {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, expected_name, "{input:?}");
            assert_eq!(error.description, expected_description, "{input:?}");
            assert_eq!(request.error_name(), Some(expected_name), "{input:?}");
        }
    }

    #[test]
    fn malformed_protocol_versions_match_llhttp_error_reasons() {
        for (input, expected_description) in [
            (&b"GET / HTTP/x.1\r\n\r\n"[..], "Invalid major version"),
            (&b"GET / HTTP/10.1\r\n\r\n"[..], "Expected dot"),
            (&b"GET / HTTP/1\r\n\r\n"[..], "Expected dot"),
            (&b"GET / HTTP/1.x\r\n\r\n"[..], "Invalid minor version"),
            (
                &b"GET / HTTP/1.10\r\n\r\n"[..],
                "Expected CRLF after version",
            ),
            (
                &b"GET / HTTP/1.1x\r\n\r\n"[..],
                "Expected CRLF after version",
            ),
            (&b"OPTIONS * RTSP/1.2\r\n\r\n"[..], "Invalid HTTP version"),
            (&b"OPTIONS * RTSP/10.0\r\n\r\n"[..], "Expected dot"),
            (&b"OPTIONS * RTSP/1.x\r\n\r\n"[..], "Invalid minor version"),
            (
                &b"SOURCE /stream ICE/1.x\r\n\r\n"[..],
                "Invalid minor version",
            ),
        ] {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_VERSION", "{input:?}");
            assert_eq!(error.description, expected_description, "{input:?}");
            assert_eq!(request.error_name(), Some("HPE_INVALID_VERSION"));
        }
    }

    #[test]
    fn malformed_header_bytes_match_llhttp_error_reasons() {
        for (input, expected_description) in [
            (
                &b"GET / HTTP/1.1\r\nBad Name: value\r\n\r\n"[..],
                "Invalid header token",
            ),
            (
                &b"GET / HTTP/1.1\r\nX-Test: value\x01bad\r\n\r\n"[..],
                "Invalid header value char",
            ),
        ] {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_HEADER_TOKEN", "{input:?}");
            assert_eq!(error.description, expected_description, "{input:?}");
            assert_eq!(request.error_name(), Some("HPE_INVALID_HEADER_TOKEN"));
        }
    }

    #[test]
    fn rejects_request_line_prefixes_when_llhttp_first_can_identify_the_error() {
        let mut method = HttpRequest::new();
        method.add_data(b"F").unwrap();
        let error = method.add_data(b"O").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_METHOD");
        assert_eq!(error.description, "Invalid method encountered");
        assert_eq!(method.url(), None);

        let mut target_start = HttpRequest::new();
        target_start.add_data(b"GET ").unwrap();
        let error = target_start.add_data(b"%").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_URL");
        assert_eq!(error.description, "Unexpected start char in url");
        assert_eq!(target_start.url(), None);

        let mut target_path = HttpRequest::new();
        target_path.add_data(b"GET /path").unwrap();
        let error = target_path.add_data(b"\x7f").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_URL");
        assert_eq!(error.description, "Invalid char in url path");
        assert_eq!(target_path.url(), Some(&b"/path"[..]));

        let mut target_authority = HttpRequest::new();
        target_authority.add_data(b"GET http://host").unwrap();
        let error = target_authority.add_data(b"\\").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_URL");
        assert_eq!(error.description, "Unexpected char in url server");
        assert_eq!(target_authority.url(), Some(&b"http://host"[..]));

        let mut target_schema = HttpRequest::new();
        target_schema.add_data(b"GET relative").unwrap();
        let error = target_schema.add_data(b"/").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_URL");
        assert_eq!(error.description, "Unexpected char in url schema");
        assert_eq!(target_schema.url(), Some(&b"relative"[..]));

        let mut protocol = HttpRequest::new();
        protocol.add_data(b"GET / ").unwrap();
        let error = protocol.add_data(b"S").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_CONSTANT");
        assert_eq!(error.description, "Expected HTTP/, RTSP/ or ICE/");
        assert_eq!(protocol.url(), Some(&b"/"[..]));

        for (prefix, next, expected_description, expected_protocol) in [
            (
                &b"GET / HTTP/"[..],
                &b"x"[..],
                "Invalid major version",
                "HTTP/",
            ),
            (&b"GET / HTTP/1"[..], &b"0"[..], "Expected dot", "HTTP/1"),
            (
                &b"GET / HTTP/1."[..],
                &b"x"[..],
                "Invalid minor version",
                "HTTP/1.",
            ),
            (
                &b"GET / HTTP/1.1"[..],
                &b"0"[..],
                "Expected CRLF after version",
                "HTTP/1.1",
            ),
            (
                &b"OPTIONS * RTSP/1."[..],
                &b"2"[..],
                "Invalid HTTP version",
                "RTSP/1.",
            ),
        ] {
            let mut request = HttpRequest::new();
            request.add_data(prefix).unwrap();
            let error = request.add_data(next).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_VERSION", "{prefix:?}");
            assert_eq!(error.description, expected_description, "{prefix:?}");
            assert_eq!(request.protocol(), Some(expected_protocol), "{prefix:?}");
        }
    }

    #[test]
    fn rtsp_and_ice_method_restrictions_begin_when_the_protocol_is_identified() {
        let mut unsupported_rtsp = HttpRequest::new();
        let error = unsupported_rtsp.add_data(b"PATCH * RTSP").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_CONSTANT");
        assert_eq!(error.description, "Invalid method for RTSP/x.x request");
        assert_eq!(unsupported_rtsp.url(), Some(&b"*"[..]));
        assert_eq!(unsupported_rtsp.protocol(), Some("RTSP/"));

        let mut unsupported_ice = HttpRequest::new();
        let error = unsupported_ice.add_data(b"GET / ICE").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_CONSTANT");
        assert_eq!(
            error.description,
            "Expected SOURCE method for ICE/x.x request"
        );
        assert_eq!(unsupported_ice.url(), Some(&b"/"[..]));
        assert_eq!(unsupported_ice.protocol(), Some("ICE"));

        let mut valid_rtsp_method = HttpRequest::new();
        valid_rtsp_method.add_data(b"GET / RTSP").unwrap();
        assert_eq!(valid_rtsp_method.protocol(), Some("RTSP/"));

        let mut valid_ice_method = HttpRequest::new();
        valid_ice_method.add_data(b"SOURCE / ICE").unwrap();
        assert_eq!(valid_ice_method.protocol(), Some("ICE"));
    }

    #[test]
    fn waits_for_split_crlf_but_rejects_a_bad_continuation() {
        let mut valid = HttpRequest::new();
        valid.add_data(b"GET / HTTP/1.1\r").unwrap();
        valid.add_data(b"\n\r\n").unwrap();
        assert!(valid.is_complete());

        let mut invalid = HttpRequest::new();
        invalid.add_data(b"GET / HTTP/1.1\r").unwrap();
        assert_eq!(invalid.add_data(b"X").unwrap_err().name, "HPE_STRICT");
    }

    #[test]
    fn arbitrary_body_bytes_are_not_checked_as_header_line_endings() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"POST /body HTTP/1.1\r\nContent-Length: 4\r\n\r\na\n\rb")
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"a\n\rb"[..]));
    }

    #[test]
    fn accepts_llhttp_rtsp_methods_and_preserves_protocol() {
        let methods = [
            "GET",
            "POST",
            "OPTIONS",
            "DESCRIBE",
            "ANNOUNCE",
            "SETUP",
            "PLAY",
            "PAUSE",
            "TEARDOWN",
            "GET_PARAMETER",
            "SET_PARAMETER",
            "REDIRECT",
            "RECORD",
            "FLUSH",
        ];

        for method in methods {
            let input = format!("{method} * RTSP/1.0\r\nCSeq: 7\r\n\r\n");
            let mut request = HttpRequest::new();
            request.add_data(input.as_bytes()).unwrap();
            assert!(request.is_complete(), "{method}");
            assert_eq!(request.method(), Some(method), "{method}");
            assert_eq!(request.url(), Some(&b"*"[..]), "{method}");
            assert_eq!(request.protocol(), Some("RTSP/1.0"), "{method}");
            assert_eq!(request.header(b"CSeq"), Some(&b"7"[..]), "{method}");
        }
    }

    #[test]
    fn accepts_ice_source_request_and_matches_the_c_protocol_getter() {
        let mut request = HttpRequest::new();
        request.add_data(b"SOURCE /stream ICE/1.0\r\n\r\n").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("SOURCE"));
        assert_eq!(request.url(), Some(&b"/stream"[..]));
        // The reference callback leaves ICE's slash slot NUL, so C returns "ICE".
        assert_eq!(request.protocol(), Some("ICE"));
    }

    #[test]
    fn accepts_llhttp_single_digit_http_major_and_minor_versions() {
        for version in ["0.9", "2.0", "9.9"] {
            let input = format!("GET /version HTTP/{version}\r\n\r\n");
            let expected = format!("HTTP/{version}");
            let mut request = HttpRequest::new();
            request.add_data(input.as_bytes()).unwrap();
            assert!(request.is_complete(), "HTTP/{version}");
            assert_eq!(request.protocol(), Some(expected.as_str()));
        }
    }

    #[test]
    fn accepts_multiple_space_request_line_delimiters_like_llhttp() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"GET   /spaced   HTTP/1.1\r\n\r\n")
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("GET"));
        assert_eq!(request.url(), Some(&b"/spaced"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
    }

    #[test]
    fn llhttp_pri_preface_waits_for_the_http2_magic_without_completing_a_request() {
        let mut request = HttpRequest::new();
        request.add_data(b"PRI * HTTP/2.0\r\n").unwrap();
        assert!(!request.is_complete());
        request.add_data(b"\r\nSM\r\n").unwrap();
        assert!(!request.is_complete());
        request.add_data(b"\r\n").unwrap();
        assert!(!request.is_complete());
        assert_eq!(request.error_name(), Some("HPE_OK"));
        assert_eq!(request.method(), None);

        // The reference pauses and resumes llhttp after the preface, but does not invoke
        // its message-complete callback or expose a complete HTTP request.
        request.add_data(b"\x00\x01binary-frame").unwrap();
        assert!(!request.is_complete());
    }

    #[test]
    fn llhttp_pri_preface_rejects_a_nonmatching_http2_magic_suffix() {
        let mut request = HttpRequest::new();
        request.add_data(b"PRI * HTTP/2.0\r\n\r\nS").unwrap();
        let error = request.add_data(b"X").unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_VERSION");
        assert_eq!(error.description, "Expected HTTP/2 Connection Preface");
        assert_eq!(request.error_name(), Some("HPE_INVALID_VERSION"));
    }

    #[test]
    fn rejects_non_source_ice_requests() {
        let mut request = HttpRequest::new();
        let error = request
            .add_data(b"GET /stream ICE/1.0\r\n\r\n")
            .unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_CONSTANT");
        assert_eq!(
            error.description,
            "Expected SOURCE method for ICE/x.x request"
        );
    }

    #[test]
    fn request_methods_match_the_llhttp_method_map() {
        for input in [
            &b"FOO / HTTP/1.1\r\n\r\n"[..],
            &b"CUSTOM * RTSP/1.0\r\n\r\n"[..],
        ] {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_METHOD");
            assert_eq!(error.description, "Invalid method encountered");
        }

        let mut query = HttpRequest::new();
        query.add_data(b"QUERY / HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(query.method(), Some("QUERY"));
    }

    #[test]
    fn maps_invalid_request_target_bytes_to_llhttp_url_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"GET /path\x7f HTTP/1.1\r\n\r\n",
                "Invalid char in url path",
            ),
            (
                b"GET /path\x80 HTTP/1.1\r\n\r\n",
                "Invalid char in url path",
            ),
            (
                b"GET /path\x01 HTTP/1.1\r\n\r\n",
                "Invalid char in url path",
            ),
            (b"GET /path\t HTTP/1.1\r\n\r\n", "Invalid characters in url"),
            (
                b"GET /?q=\x01 HTTP/1.1\r\n\r\n",
                "Invalid char in url query",
            ),
            (
                b"GET /?q=\x80 HTTP/1.1\r\n\r\n",
                "Invalid char in url query",
            ),
            (b"GET /?q=\t HTTP/1.1\r\n\r\n", "Invalid characters in url"),
            (
                b"GET /?q=#\x80 HTTP/1.1\r\n\r\n",
                "Invalid char in url fragment start",
            ),
            (
                b"GET /path#\x01 HTTP/1.1\r\n\r\n",
                "Invalid char in url fragment start",
            ),
            (
                b"GET http://host/path\x80 HTTP/1.1\r\n\r\n",
                "Invalid char in url path",
            ),
            (
                b"GET http://ho\x80st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
            (
                b"GET http://ho\x01st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
            (
                b"GET http://ho\tst/path HTTP/1.1\r\n\r\n",
                "Invalid characters in url",
            ),
            (
                b"GET http://ho\x0cst/path HTTP/1.1\r\n\r\n",
                "Invalid characters in url",
            ),
            (
                b"GET http://ho\"st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
            (
                b"GET http://ho\\st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
            (
                b"GET http://ho<st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
            (
                b"GET relative HTTP/1.1\r\n\r\n",
                "Invalid characters in url",
            ),
            (
                b"GET relative/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url schema",
            ),
            (b"GET % HTTP/1.1\r\n\r\n", "Unexpected start char in url"),
            (
                b"GET h@ttp://host/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url schema",
            ),
            (
                b"GET http:/host/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url schema",
            ),
            (
                b"GET http://ho|st/path HTTP/1.1\r\n\r\n",
                "Unexpected char in url server",
            ),
        ];

        for (input, description) in cases {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_URL", "{input:?}");
            assert_eq!(error.description, *description, "{input:?}");
        }
    }

    #[test]
    fn maps_url_separator_tokens_to_llhttp_protocol_errors() {
        for input in [
            &b"GET /bad space HTTP/1.1\r\n\r\n"[..],
            &b"GET /?q= x HTTP/1.1\r\n\r\n"[..],
            &b"GET /path SIP/2.0\r\n\r\n"[..],
            &b"GET /path HTTP\r\n\r\n"[..],
        ] {
            let mut request = HttpRequest::new();
            let error = request.add_data(input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_CONSTANT", "{input:?}");
            assert_eq!(
                error.description, "Expected HTTP/, RTSP/ or ICE/",
                "{input:?}"
            );
        }
    }

    #[test]
    fn leaves_source_accepted_percent_and_path_hash_bytes_valid() {
        for input in [
            &b"GET /% HTTP/1.1\r\n\r\n"[..],
            &b"GET /path#anchor HTTP/1.1\r\n\r\n"[..],
            &b"GET http://host/path HTTP/1.1\r\n\r\n"[..],
            &b"OPTIONS * HTTP/1.1\r\n\r\n"[..],
        ] {
            let mut request = HttpRequest::new();
            request.add_data(input).unwrap();
            assert!(request.is_complete(), "{input:?}");
        }
    }

    #[test]
    fn connect_authority_uses_llhttp_url_server_character_rules() {
        let mut valid = HttpRequest::new();
        valid
            .add_data(b"CONNECT host:443 HTTP/1.1\r\n\r\n")
            .unwrap();
        assert!(valid.is_complete());

        for (target, expected_description) in [
            (&b"host\x01name:443"[..], "Unexpected char in url server"),
            (&b"host\tname:443"[..], "Invalid characters in url"),
            (&b"host|name:443"[..], "Unexpected char in url server"),
            (&b"host\\name:443"[..], "Unexpected char in url server"),
        ] {
            let mut input = b"CONNECT ".to_vec();
            input.extend_from_slice(target);
            input.extend_from_slice(b" HTTP/1.1\r\n\r\n");
            let mut request = HttpRequest::new();
            let error = request.add_data(&input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_URL", "{target:?}");
            assert_eq!(error.description, expected_description, "{target:?}");
        }
    }

    #[test]
    fn connect_is_an_upgrade_even_with_a_declared_request_body() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"CONNECT host:443 HTTP/1.1\r\nContent-Length: 4\r\n\r\nDATATAIL")
            .unwrap();

        assert!(request.is_complete());
        assert_eq!(request.method(), Some("CONNECT"));
        assert_eq!(request.body(), None);
        assert!(request.discard_read_ahead, "headers={:?}", request.headers);
        assert!(request.take_pending_data().is_empty());
    }

    #[test]
    fn connection_upgrade_requires_both_flags_and_a_nonempty_protocol_value() {
        let next = b"GET /next HTTP/1.1\r\n\r\n";
        for (headers, pauses) in [
            ("Connection: Upgrade\r\nUpgrade: PTTH/1.0\r\n", true),
            (
                "Connection: keep-alive, Upgrade\r\nUpgrade: PTTH/1.0\r\n",
                true,
            ),
            ("Connection: Upgrade\r\nUpgrade: x\r\n", true),
            ("Connection: Upgrade\r\nUpgrade:\r\n", false),
            ("Connection: Upgrade\r\nUpgrade: \t \r\n", false),
            ("Connection: Upgrade\r\n", false),
            ("Upgrade: PTTH/1.0\r\n", false),
            ("Connection: xUpgrade\r\nUpgrade: PTTH/1.0\r\n", false),
        ] {
            let input = [
                b"GET / HTTP/1.1\r\n".as_slice(),
                headers.as_bytes(),
                b"\r\n",
                next,
            ]
            .concat();
            let mut request = HttpRequest::new();
            request.add_data(&input).unwrap();
            assert!(request.is_complete(), "{headers:?}");
            assert_eq!(
                request.discard_read_ahead, pauses,
                "{headers:?}; parsed={:?}; method={:?}",
                request.headers, request.method
            );
            assert_eq!(
                request.take_pending_data().is_empty(),
                pauses,
                "{headers:?}"
            );
        }
    }

    #[test]
    fn upgrade_read_ahead_is_discarded_after_a_fixed_body_completes() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /reverse HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: PTTH/1.0\r\nContent-Length: 1\r\n\r\nxGET /next HTTP/1.1\r\n\r\n",
            )
            .unwrap();

        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"x"[..]));
        assert!(request.take_pending_data().is_empty());
    }

    #[test]
    fn upgrade_read_ahead_is_discarded_after_a_chunked_body_completes() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /reverse HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: PTTH/1.0\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\n\r\nGET /next HTTP/1.1\r\n\r\n",
            )
            .unwrap();

        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"x"[..]));
        assert!(request.take_pending_data().is_empty());
    }

    #[test]
    fn rtsp_body_and_http_11_protocol_version_are_preserved() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"ANNOUNCE /announce RTSP/1.1\r\nCSeq: 8\r\nContent-Length: 4\r\n\r\nbody")
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.method(), Some("ANNOUNCE"));
        assert_eq!(request.protocol(), Some("RTSP/1.1"));
        assert_eq!(request.header(b"CSeq"), Some(&b"8"[..]));
        assert_eq!(request.body(), Some(&b"body"[..]));
    }

    #[test]
    fn accepts_only_rtsp_versions_supported_by_the_source_parser() {
        for (version, expected) in [
            ("1.0", "RTSP/1.0"),
            ("1.1", "RTSP/1.1"),
            ("2.0", "RTSP/2.0"),
        ] {
            let input = format!("OPTIONS * RTSP/{version}\r\nCSeq: 1\r\n\r\n");
            let mut request = HttpRequest::new();
            request.add_data(input.as_bytes()).unwrap();
            assert!(request.is_complete(), "RTSP/{version}");
            assert_eq!(request.protocol(), Some(expected));
        }
    }

    #[test]
    fn rejects_rtsp_methods_outside_llhttp_method_map() {
        let mut known_http_method = HttpRequest::new();
        let error = known_http_method
            .add_data(b"PATCH * RTSP/1.0\r\n\r\n")
            .unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_CONSTANT");
        assert_eq!(error.description, "Invalid method for RTSP/x.x request");

        let mut unknown_method = HttpRequest::new();
        let error = unknown_method
            .add_data(b"CUSTOM * RTSP/1.0\r\n\r\n")
            .unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_METHOD");
        assert_eq!(error.description, "Invalid method encountered");
    }

    #[test]
    fn rejects_malformed_rtsp_versions() {
        for input in [
            &b"OPTIONS * RTSP/1.x\r\n\r\n"[..],
            &b"OPTIONS * RTSP/1.2\r\n\r\n"[..],
            &b"OPTIONS * RTSP/2.1\r\n\r\n"[..],
            &b"OPTIONS * RTSP/10.0\r\n\r\n"[..],
            &b"OPTIONS * RTSP/1.00\r\n\r\n"[..],
        ] {
            let mut request = HttpRequest::new();
            assert!(request.add_data(input).is_err());
            assert_eq!(request.error_name(), Some("HPE_INVALID_VERSION"));
        }
    }

    #[test]
    fn keeps_first_exact_case_header_lookup_and_source_string_shape() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"GET / HTTP/1.0\r\nCSeq: 1\r\ncseq: 2\r\n\r\n")
            .unwrap();
        assert_eq!(request.header(b"CSeq"), Some(&b"1"[..]));
        assert_eq!(request.header(b"cseq"), Some(&b"2"[..]));
        assert_eq!(request.header_stats(), (10, 2, 4, 1));
        assert_eq!(request.header_string().unwrap(), b"CSeq: 1\ncseq: 2\n");
    }

    #[test]
    fn decodes_chunked_body() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n")
            .unwrap();
        assert_eq!(request.body(), Some(&b"abcde"[..]));
    }

    #[test]
    fn validates_chunk_extension_tokens_and_quoted_values_like_llhttp() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3;name=value;quoted=\"a b\"\r\nabc\r\n0;done=yes\r\n\r\n",
            )
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"abc"[..]));

        let mut empty_value = HttpRequest::new();
        empty_value
            .add_data(
                b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3;name=\r\nabc\r\n0\r\n\r\n",
            )
            .unwrap();
        assert!(empty_value.is_complete());
        assert_eq!(empty_value.body(), Some(&b"abc"[..]));
    }

    #[test]
    fn rejects_invalid_chunk_extensions_like_llhttp() {
        let invalid_lines: &[(&[u8], &str, &str)] = &[
            (
                b"3;\r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions",
            ),
            (
                b"3;name \r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions name",
            ),
            (
                b"3;bad/name\r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions name",
            ),
            (
                b"3;name=value other\r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions value",
            ),
            (
                b"3;name=\"ok\" \r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions quote value",
            ),
            (
                b"3;name=\"unterminated\r\n",
                "HPE_STRICT",
                "Invalid character in chunk extensions quoted value",
            ),
            (
                b"3;name=\"a\\\x01b\"\r\n",
                "HPE_STRICT",
                "Invalid quoted-pair in chunk extensions quoted value",
            ),
            (
                b"3;name\n",
                "HPE_CR_EXPECTED",
                "Missing expected CR after chunk extension name",
            ),
            (
                b"3;name=value\n",
                "HPE_CR_EXPECTED",
                "Missing expected CR after chunk extension value",
            ),
        ];
        for (size_line, expected_name, expected_reason) in invalid_lines {
            let mut input = b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            input.extend_from_slice(size_line);
            let mut request = HttpRequest::new();
            let error = request.add_data(&input).unwrap_err();
            assert_eq!(error.name, *expected_name, "{size_line:?}");
            assert_eq!(error.description, *expected_reason, "{size_line:?}");
        }
    }

    #[test]
    fn transfer_encoding_requires_chunked_as_the_final_coding() {
        for (value, expected_reason) in [
            ("gzip", "Request has invalid `Transfer-Encoding`"),
            ("chunked, gzip", "Invalid `Transfer-Encoding` header value"),
            (
                "gzip, chunked, gzip",
                "Invalid `Transfer-Encoding` header value",
            ),
            ("chunked;foo", "Request has invalid `Transfer-Encoding`"),
            ("gzip,,", "Request has invalid `Transfer-Encoding`"),
            ("chunked,", "Invalid `Transfer-Encoding` header value"),
        ] {
            let input = format!("POST /event HTTP/1.1\r\nTransfer-Encoding: {value}\r\n\r\n");
            let mut request = HttpRequest::new();
            let error = request.add_data(input.as_bytes()).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_TRANSFER_ENCODING", "{value}");
            assert_eq!(error.description, expected_reason, "{value}");
            assert_eq!(request.error_name(), Some("HPE_INVALID_TRANSFER_ENCODING"));
        }
    }

    #[test]
    fn content_length_transfer_encoding_conflicts_match_llhttp_header_order() {
        for (headers, expected_name, expected_reason) in [
            (
                "Content-Length: 0\r\nTransfer-Encoding: chunked\r\n",
                "HPE_INVALID_TRANSFER_ENCODING",
                "Transfer-Encoding can't be present with Content-Length",
            ),
            (
                "Transfer-Encoding: chunked\r\nContent-Length: 0\r\n",
                "HPE_INVALID_CONTENT_LENGTH",
                "Content-Length can't be present with Transfer-Encoding",
            ),
        ] {
            let input = format!("POST /event HTTP/1.1\r\n{headers}\r\n");
            let mut request = HttpRequest::new();
            let error = request.add_data(input.as_bytes()).unwrap_err();
            assert_eq!(error.name, expected_name, "{headers}");
            assert_eq!(error.description, expected_reason, "{headers}");
        }
    }

    #[test]
    fn accepts_prior_transfer_codings_when_chunked_is_final() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /event HTTP/1.1\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
            )
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"abc"[..]));
    }

    #[test]
    fn rejects_duplicate_content_length_even_when_values_match() {
        for (first, second) in [("4", "4"), ("4", "5")] {
            let input = format!(
                "POST /event HTTP/1.1\r\nContent-Length: {first}\r\ncontent-length: {second}\r\n\r\n"
            );
            let mut request = HttpRequest::new();
            let error = request.add_data(input.as_bytes()).unwrap_err();
            assert_eq!(error.name, "HPE_UNEXPECTED_CONTENT_LENGTH");
            assert_eq!(request.error_name(), Some("HPE_UNEXPECTED_CONTENT_LENGTH"));
        }
    }

    #[test]
    fn content_length_errors_match_llhttp_reasons() {
        for (value, expected_reason) in [
            (&b""[..], "Empty Content-Length"),
            (&b"not-a-number"[..], "Invalid character in Content-Length"),
            (&b"18446744073709551616"[..], "Content-Length overflow"),
        ] {
            let mut request = HttpRequest::new();
            let mut input = b"POST /length HTTP/1.1\r\nContent-Length: ".to_vec();
            input.extend_from_slice(value);
            input.extend_from_slice(b"\r\n\r\n");
            let error = request.add_data(&input).unwrap_err();
            assert_eq!(error.name, "HPE_INVALID_CONTENT_LENGTH", "{value:?}");
            assert_eq!(error.description, expected_reason, "{value:?}");
        }
    }

    #[test]
    fn rejects_duplicate_chunked_transfer_encoding_fields() {
        let mut request = HttpRequest::new();
        let error = request
            .add_data(
                b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n",
            )
            .unwrap_err();
        assert_eq!(error.name, "HPE_INVALID_TRANSFER_ENCODING");
        assert_eq!(
            error.description,
            "Invalid `Transfer-Encoding` header value"
        );
        assert_eq!(request.error_name(), Some("HPE_INVALID_TRANSFER_ENCODING"));
    }

    #[test]
    fn header_lookup_discards_leading_ows_and_retains_trailing_llhttp_span_bytes() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /event HTTP/1.1\r\ncOnTeNt-LeNgTh:\t 3 \t\r\nX-Custom:\t value \t\r\n\r\nabc",
            )
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.header(b"cOnTeNt-LeNgTh"), Some(&b"3 \t"[..]));
        assert_eq!(request.header(b"X-Custom"), Some(&b"value \t"[..]));
        assert_eq!(request.header(b"Content-Length"), None);
        assert_eq!(
            request.header_string().unwrap(),
            b"cOnTeNt-LeNgTh: 3 \t\nX-Custom: value \t\n"
        );
        assert_eq!(request.body(), Some(&b"abc"[..]));
    }

    #[test]
    fn empty_header_values_remain_present_like_zero_length_llhttp_spans() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"GET /empty HTTP/1.1\r\nEmpty:\r\n\r\n")
            .unwrap();
        assert_eq!(request.header(b"Empty"), Some(&b""[..]));
        assert_eq!(request.header_stats(), (5, 1, 5, 0));
        assert_eq!(request.header_string().unwrap(), b"Empty: \n");
    }

    #[test]
    fn body_framing_does_not_special_case_head_requests() {
        let mut head_without_body = HttpRequest::new();
        head_without_body
            .add_data(b"HEAD /status HTTP/1.1\r\n\r\n")
            .unwrap();
        assert!(head_without_body.is_complete());
        assert_eq!(head_without_body.body(), None);

        let mut head_with_body = HttpRequest::new();
        head_with_body
            .add_data(b"HEAD /status HTTP/1.1\r\nContent-Length: 4\r\n\r\nbo")
            .unwrap();
        assert!(!head_with_body.is_complete());
        head_with_body.add_data(b"dy").unwrap();
        assert!(head_with_body.is_complete());
        assert_eq!(head_with_body.body(), Some(&b"body"[..]));
    }

    #[test]
    fn exposes_chunked_body_as_fragments_arrive() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
            .unwrap();
        request.add_data(b"3\r\nabc\r\n2\r\nd").unwrap();
        assert!(!request.is_complete());
        assert_eq!(request.body(), Some(&b"abcd"[..]));
        request.add_data(b"e\r\n0\r\n\r\n").unwrap();
        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"abcde"[..]));
    }

    #[test]
    fn stores_chunk_trailer_headers() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Trailer: yes \t\r\nY-Trailer:\t value\t\r\n\r\n",
            )
            .unwrap();
        assert!(request.is_complete());
        assert_eq!(request.header(b"X-Trailer"), Some(&b"yes \t"[..]));
        assert_eq!(request.header(b"Y-Trailer"), Some(&b"value\t"[..]));
        assert_eq!(
            request.header_string().unwrap(),
            b"Transfer-Encoding: chunked\nX-Trailer: yes \t\nY-Trailer: value\t\n"
        );
    }

    #[test]
    fn oversized_chunk_size_errors_without_integer_overflow() {
        let mut request = HttpRequest::new();
        request
            .add_data(b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
            .unwrap();
        assert!(
            request
                .add_data(b"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF\r\n")
                .is_err()
        );
        assert_eq!(request.error_name(), Some("HPE_INVALID_CHUNK_SIZE"));
    }

    #[test]
    fn grows_header_slots_past_8192_fields() {
        let mut request = HttpRequest::new();
        let mut bytes = b"GET / HTTP/1.1\r\n".to_vec();
        for index in 0..8200 {
            bytes.extend_from_slice(format!("X-{index}: v\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        request.add_data(&bytes).unwrap();
        assert!(request.is_complete());
        assert_eq!(request.header_stats().1, 8200);
    }

    #[test]
    fn completed_request_retains_only_bytes_after_its_body() {
        let body = vec![b'x'; 1024 * 1024];
        let mut request = HttpRequest::new();
        request
            .add_data(
                format!(
                    "POST /play HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        for chunk in body.chunks(4096) {
            request.add_data(chunk).unwrap();
        }
        assert!(request.is_complete());
        assert_eq!(request.buffered_len(), 0);
        assert_eq!(request.body().map(<[u8]>::len), Some(body.len()));
        let next_request = b"GET /next HTTP/1.1\r\n\r\n";
        request.add_data(next_request).unwrap();
        assert_eq!(request.take_pending_data(), next_request);
        assert_eq!(request.buffered_len(), 0);
        assert_eq!(request.body().map(<[u8]>::len), Some(body.len()));
    }

    #[test]
    fn chunked_body_preserves_coalesced_next_request() {
        let mut request = HttpRequest::new();
        request
            .add_data(
                b"POST /chunked RTSP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nOPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\n",
            )
            .unwrap();

        assert!(request.is_complete());
        assert_eq!(request.body(), Some(&b"abc"[..]));
        assert_eq!(
            request.take_pending_data(),
            b"OPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\n"
        );
    }

    #[test]
    fn clean_parser_reports_hpe_ok() {
        let request = HttpRequest::new();
        assert_eq!(request.error_name(), Some("HPE_OK"));
    }

    #[test]
    fn reports_invalid_header_and_retains_error() {
        let mut request = HttpRequest::new();
        let first = request.add_data(b"GET / HTTP/1.1\r\nBad Header: x\r\n\r\n");
        assert!(first.is_err());
        assert!(request.has_error());
        assert_eq!(request.error_name(), Some("HPE_INVALID_HEADER_TOKEN"));
        assert_eq!(request.add_data(b"later"), first);
    }

    #[test]
    fn upgrades_to_reverse_event_mode_and_consumes_one_frame() {
        let mut request = HttpRequest::new();
        request.upgrade_data();
        request
            .add_event_data(
                b"EVENT /x HTTP/1.1\r\nContent-Length: 2\r\n\r\nokEVENT /y HTTP/1.1\r\n\r\n",
            )
            .unwrap();
        let event = request.parse_event_data().unwrap();
        assert_eq!(
            event.header,
            b"EVENT /x HTTP/1.1\r\nContent-Length: 2\r\n\r\n"
        );
        assert_eq!(event.body, b"ok");
        assert_eq!(request.buffered_len(), b"EVENT /y HTTP/1.1\r\n\r\n".len());
    }

    #[test]
    fn reverse_event_copy_offsets_include_leading_junk_like_c_release() {
        let mut request = HttpRequest::new();
        request.upgrade_data();
        let header = b"EVENT /x HTTP/1.1\r\nContent-Length: 2\r\n\r\n";
        let input = [b"\0".as_slice(), header, b"ab"].concat();
        request.add_event_data(&input).unwrap();

        let event = request.parse_event_data().unwrap();
        assert_eq!(event.header, input[..header.len()]);
        assert_eq!(event.header.first(), Some(&0));
        assert_eq!(event.body, b"\na");
    }

    #[test]
    fn reverse_event_content_length_uses_only_the_current_read() {
        let mut fragmented = HttpRequest::new();
        fragmented.upgrade_data();
        let header = b"EVENT /x HTTP/1.1\r\nContent-Length:\r\n\r\n";
        fragmented.add_event_data(header).unwrap();
        assert_eq!(fragmented.parse_event_data().unwrap().body, b"");
        fragmented.add_event_data(b"2x").unwrap();
        assert!(fragmented.parse_event_data().is_none());

        let mut coalesced = HttpRequest::new();
        coalesced.upgrade_data();
        coalesced
            .add_event_data(&[header.as_slice(), b"2x"].concat())
            .unwrap();
        assert_eq!(coalesced.parse_event_data().unwrap().body, b"2x");
    }

    #[test]
    fn rejects_event_buffer_at_source_limit() {
        let mut request = HttpRequest::new();
        request.upgrade_data();
        assert!(request.add_event_data(&vec![b'x'; MAX_EVENT_DATA]).is_err());
    }

    #[test]
    fn reverse_flag_hides_request_accessors() {
        let mut request = HttpRequest::new();
        request.add_data(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        request.set_reverse();
        assert!(request.method().is_none());
        assert!(request.url().is_none());
        assert!(request.protocol().is_none());
        assert!(request.header(b"CSeq").is_none());
        assert!(request.body().is_none());
    }
}
