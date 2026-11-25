//! Common response assembly from `lib/raop.c` around each protocol handler.
//!
//! This uses the separately C-differential-tested `http-response-rs` byte
//! builder. Route selection and handler-specific state changes remain separate.

use crate::http_transport::HttpReply;
use http_response_rs::port::HttpResponse;
use std::io;

pub const SOURCE_SERVER_HEADER: &[u8] = b"AirTunes/220.68";
pub const SOURCE_AUDIO_JACK_STATUS: &[u8] = b"connected; type=digital";

#[derive(Clone, Copy, Debug)]
pub struct RequestMetadata<'a> {
    pub protocol: &'a [u8],
    pub method: &'a [u8],
    pub cseq: Option<&'a [u8]>,
    pub is_hls: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerOutput {
    pub status_code: i32,
    pub status_message: Vec<u8>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub body: Option<Vec<u8>>,
    pub disconnect: bool,
}

impl Default for HandlerOutput {
    fn default() -> Self {
        Self {
            status_code: 200,
            status_message: b"OK".to_vec(),
            headers: Vec::new(),
            body: None,
            disconnect: false,
        }
    }
}

/// Assemble the status line, handler headers/body, and `raop.c` common headers.
///
/// Header order follows the source: Audio-Jack-Status, handler headers, Server,
/// CSeq, then the response builder's Content-Length and final CRLF.
pub fn assemble_response(
    request: RequestMetadata<'_>,
    output: HandlerOutput,
) -> io::Result<HttpReply> {
    if request.protocol.is_empty()
        || !(100..1000).contains(&output.status_code)
        || output.status_message.is_empty()
        || output
            .headers
            .iter()
            .any(|(name, value)| name.is_empty() || value.is_empty())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid protocol response fields",
        ));
    }

    let body = output.body.as_deref().filter(|bytes| !bytes.is_empty());
    let body_len = body.map_or(0, <[u8]>::len);
    let body_len = i32::try_from(body_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "protocol response body exceeds the source length type",
        )
    })?;

    let mut response = HttpResponse::new();
    response.init(request.protocol, output.status_code, &output.status_message);
    if request.cseq.is_some() && request.method != b"RECORD" {
        response.add_header(b"Audio-Jack-Status", SOURCE_AUDIO_JACK_STATUS);
    }
    for (name, value) in &output.headers {
        response.add_header(name, value);
    }
    if !request.is_hls {
        response.add_header(b"Server", SOURCE_SERVER_HEADER);
        if let Some(cseq) = request.cseq {
            if cseq.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty CSeq triggers the source response builder assertion",
                ));
            }
            response.add_header(b"CSeq", cseq);
        }
    }
    response.finish(body, body_len);
    Ok(HttpReply {
        bytes: response.completed_data().to_vec(),
        disconnect: output.disconnect,
        remove_classes_after_write: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata<'a>(
        protocol: &'a [u8],
        method: &'a [u8],
        cseq: Option<&'a [u8]>,
        is_hls: bool,
    ) -> RequestMetadata<'a> {
        RequestMetadata {
            protocol,
            method,
            cseq,
            is_hls,
        }
    }

    #[test]
    fn rtsp_common_headers_follow_source_order() {
        let reply = assemble_response(
            metadata(b"RTSP/1.0", b"OPTIONS", Some(b"7"), false),
            HandlerOutput {
                headers: vec![(b"Public".to_vec(), b"SETUP, RECORD, OPTIONS".to_vec())],
                ..HandlerOutput::default()
            },
        )
        .unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nPublic: SETUP, RECORD, OPTIONS\r\nServer: AirTunes/220.68\r\nCSeq: 7\r\n\r\n"
        );
    }

    #[test]
    fn record_omits_audio_jack_status_but_keeps_server_and_cseq() {
        let reply = assemble_response(
            metadata(b"RTSP/1.0", b"RECORD", Some(b"15"), false),
            HandlerOutput::default(),
        )
        .unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nServer: AirTunes/220.68\r\nCSeq: 15\r\n\r\n"
        );
    }

    #[test]
    fn hls_response_skips_common_server_and_cseq_headers() {
        let reply = assemble_response(
            metadata(b"HTTP/1.1", b"GET", Some(b"19"), true),
            HandlerOutput {
                headers: vec![(
                    b"Content-Type".to_vec(),
                    b"application/vnd.apple.mpegurl".to_vec(),
                )],
                ..HandlerOutput::default()
            },
        )
        .unwrap();
        assert_eq!(
            reply.bytes,
            b"HTTP/1.1 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: 0\r\n\r\n"
        );
    }

    #[test]
    fn nonempty_binary_body_gets_content_length_after_common_headers() {
        let reply = assemble_response(
            metadata(b"HTTP/1.1", b"POST", None, false),
            HandlerOutput {
                headers: vec![(
                    b"Content-Type".to_vec(),
                    b"application/octet-stream".to_vec(),
                )],
                body: Some(vec![0, 0xff, 0x41]),
                disconnect: true,
                ..HandlerOutput::default()
            },
        )
        .unwrap();
        assert!(reply.disconnect);
        assert_eq!(
            reply.bytes,
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nServer: AirTunes/220.68\r\nContent-Length: 3\r\n\r\n\0\xffA"
        );
    }

    #[test]
    fn default_response_without_cseq_has_no_audio_jack_status() {
        let reply = assemble_response(
            metadata(b"HTTP/1.1", b"GET", None, false),
            HandlerOutput::default(),
        )
        .unwrap();
        assert_eq!(
            reply.bytes,
            b"HTTP/1.1 200 OK\r\nServer: AirTunes/220.68\r\n\r\n"
        );
    }
}
