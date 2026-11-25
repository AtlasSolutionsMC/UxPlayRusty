//! Stateless protocol dispatch built from the C route table and common response path.
//!
//! Routes needing unavailable pairing/session state or renderer behavior are
//! returned as `Unported`; they must not be answered with a fabricated 200.

use crate::airplay_controls::{self, AirPlayControl};
use crate::airplay_info::{self, PlaybackInfoReply, PlaybackSnapshot};
use crate::http_request::HttpRequest;
use crate::http_transport::HttpReply;
use crate::protocol_response::{HandlerOutput, RequestMetadata, assemble_response};
use crate::protocol_routes::{self, Handler, RouteDecision};
use crate::raop_info::{self, InfoContext, InfoRequest};
use crate::raop_parameters::{self, ParameterEffect, SetParameterContext};
use std::io;
use uxplay_airplay_http_handlers::{self as http_handlers, HandlerResponse};

#[derive(Clone, Debug, PartialEq)]
pub enum DispatchOutcome {
    Reply(HttpReply),
    Effect {
        reply: HttpReply,
        effect: HandlerEffect,
    },
    Unported(Handler),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandlerEffect {
    AirPlayControl(AirPlayControl),
    RaopFeedback,
    RaopParameter(ParameterEffect),
    ResetHlsPlayback,
    StartHlsPlayback {
        location: String,
        start_position_seconds: f32,
    },
    DisplayPairingPin(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DispatchContext<'a> {
    pub hardware_address: Option<&'a [u8]>,
    pub playback_snapshot: Option<PlaybackSnapshot>,
    pub client_volume: Option<f32>,
    pub parameter_state: SetParameterContext,
    pub raop_info: Option<InfoContext<'a>>,
}

/// Handle common response behavior with an empty server context.
///
/// Routes requiring DNS-SD or renderer state remain `Unported` here. Call
/// [`dispatch_with_context`] when those values are available.
pub fn dispatch(request: &HttpRequest, hls_request: bool) -> io::Result<DispatchOutcome> {
    dispatch_with_context(request, hls_request, DispatchContext::default())
}

pub fn dispatch_with_context(
    request: &HttpRequest,
    hls_request: bool,
    context: DispatchContext<'_>,
) -> io::Result<DispatchOutcome> {
    let method = request.method().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed method")
    })?;
    let protocol = request.protocol().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "request has no parsed protocol",
        )
    })?;
    let url = request
        .url()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed URL"))?;
    let cseq = request.header(b"CSeq");
    let metadata = RequestMetadata {
        protocol: protocol.as_bytes(),
        method: method.as_bytes(),
        cseq,
        is_hls: hls_request,
    };

    let route = protocol_routes::classify(method.as_bytes(), url, protocol.as_bytes(), hls_request);
    match route {
        RouteDecision::Handler(Handler::RaopFeedback) => Ok(DispatchOutcome::Effect {
            reply: assemble_response(metadata, HandlerOutput::default())?,
            effect: HandlerEffect::RaopFeedback,
        }),
        RouteDecision::Handler(Handler::RaopOptions) => {
            let output = HandlerOutput {
                headers: vec![(
                    b"Public".to_vec(),
                    b"SETUP, RECORD, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER"
                        .to_vec(),
                )],
                ..HandlerOutput::default()
            };
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(handler @ (Handler::Scrub | Handler::Rate | Handler::Stop)) => {
            let Some(effect) = airplay_controls::parse_control(method.as_bytes(), url) else {
                return Ok(DispatchOutcome::Unported(handler));
            };
            Ok(DispatchOutcome::Effect {
                reply: assemble_response(metadata, HandlerOutput::default())?,
                effect: HandlerEffect::AirPlayControl(effect),
            })
        }
        RouteDecision::Handler(Handler::GetParameter) => {
            let output = raop_parameters::get_parameter(
                request.header(b"Content-Type"),
                request.body().unwrap_or_default(),
                context.client_volume,
            );
            dispatch_parameter_output(metadata, output)
        }
        RouteDecision::Handler(Handler::SetParameter) => {
            let output = raop_parameters::set_parameter(
                request.header(b"Content-Type"),
                request.body().unwrap_or_default(),
                context.parameter_state,
            );
            dispatch_parameter_output(metadata, output)
        }
        RouteDecision::Handler(Handler::RaopInfo) => {
            let Some(info_context) = context.raop_info else {
                return Ok(DispatchOutcome::Unported(Handler::RaopInfo));
            };
            let body = raop_info::response_body(
                InfoRequest {
                    url,
                    content_type: request.header(b"Content-Type"),
                    cseq,
                    body: request.body(),
                },
                info_context,
            )
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let output = HandlerOutput {
                headers: vec![(
                    b"Content-Type".to_vec(),
                    raop_info::BINARY_PLIST_CONTENT_TYPE.to_vec(),
                )],
                body: Some(body),
                ..HandlerOutput::default()
            };
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(Handler::ServerInfo) => {
            let Some(hardware_address) = context.hardware_address else {
                return Ok(DispatchOutcome::Unported(Handler::ServerInfo));
            };
            let output = HandlerOutput {
                headers: vec![(
                    b"Content-Type".to_vec(),
                    airplay_info::PLIST_CONTENT_TYPE.to_vec(),
                )],
                body: Some(airplay_info::server_info_plist_xml(hardware_address)),
                ..HandlerOutput::default()
            };
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(Handler::PlaybackInfo) => {
            let Some(snapshot) = context.playback_snapshot else {
                return Ok(DispatchOutcome::Unported(Handler::PlaybackInfo));
            };
            match airplay_info::playback_info_reply(snapshot) {
                PlaybackInfoReply::Finished => Ok(DispatchOutcome::Effect {
                    reply: assemble_response(
                        metadata,
                        HandlerOutput {
                            disconnect: true,
                            ..HandlerOutput::default()
                        },
                    )?,
                    effect: HandlerEffect::ResetHlsPlayback,
                }),
                PlaybackInfoReply::Unavailable => Ok(DispatchOutcome::Reply(assemble_response(
                    metadata,
                    HandlerOutput::default(),
                )?)),
                PlaybackInfoReply::Available(body) => {
                    let output = HandlerOutput {
                        headers: vec![(
                            b"Content-Type".to_vec(),
                            airplay_info::PLIST_CONTENT_TYPE.to_vec(),
                        )],
                        body: Some(body),
                        ..HandlerOutput::default()
                    };
                    Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
                }
            }
        }
        RouteDecision::Handler(Handler::GetProperty) => {
            let output = http_handler_output(http_handlers::get_property(""));
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(Handler::SetProperty) => {
            let output = http_handler_output(http_handlers::set_property_bytes(url));
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(Handler::FairPlaySetup2) => {
            let output = http_handler_output(http_handlers::fairplay_setup2());
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
        RouteDecision::Handler(handler) => Ok(DispatchOutcome::Unported(handler)),
        RouteDecision::DefaultOk => Ok(DispatchOutcome::Reply(assemble_response(
            metadata,
            HandlerOutput::default(),
        )?)),
        RouteDecision::NotFound => Ok(DispatchOutcome::Reply(assemble_response(
            metadata,
            HandlerOutput {
                status_code: 404,
                status_message: b"Not Found".to_vec(),
                ..HandlerOutput::default()
            },
        )?)),
        RouteDecision::NotImplemented => {
            let output = HandlerOutput {
                status_code: 501,
                status_message: b"Not Implemented".to_vec(),
                ..HandlerOutput::default()
            };
            Ok(DispatchOutcome::Reply(assemble_response(metadata, output)?))
        }
    }
}

fn dispatch_parameter_output(
    metadata: RequestMetadata<'_>,
    output: raop_parameters::ParameterOutput,
) -> io::Result<DispatchOutcome> {
    let headers = output
        .content_type
        .map(|value| vec![(b"Content-Type".to_vec(), value.to_vec())])
        .unwrap_or_default();
    let handler_output = HandlerOutput {
        status_code: i32::from(output.status_code),
        status_message: output.status_message.to_vec(),
        headers,
        body: output.body,
        ..HandlerOutput::default()
    };
    let reply = assemble_response(metadata, handler_output)?;
    Ok(match output.effect {
        Some(effect) => DispatchOutcome::Effect {
            reply,
            effect: HandlerEffect::RaopParameter(effect),
        },
        None => DispatchOutcome::Reply(reply),
    })
}

fn http_handler_output(response: HandlerResponse) -> HandlerOutput {
    let (status_code, status_message) = response
        .status
        .map_or((200, "OK"), |(code, message)| (i32::from(code), message));
    HandlerOutput {
        status_code,
        status_message: status_message.as_bytes().to_vec(),
        headers: response
            .headers
            .into_iter()
            .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
            .collect(),
        body: (!response.body.is_empty()).then_some(response.body),
        disconnect: response.disconnect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> HttpRequest {
        let mut request = HttpRequest::new();
        request.add_data(bytes).unwrap();
        assert!(request.is_complete());
        request
    }

    #[test]
    fn rtsp_options_returns_public_and_common_headers_in_source_order() {
        let request = parse(b"OPTIONS * RTSP/1.0\r\nCSeq: 7\r\n\r\n");
        let DispatchOutcome::Reply(reply) = dispatch(&request, false).unwrap() else {
            panic!("OPTIONS is implemented")
        };
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nPublic: SETUP, RECORD, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER\r\nServer: AirTunes/220.68\r\nCSeq: 7\r\n\r\n"
        );
    }

    #[test]
    fn raop_feedback_returns_source_response_and_emits_watchdog_effect() {
        let request = parse(b"POST /feedback RTSP/1.0\r\nCSeq: 11\r\n\r\n");
        let DispatchOutcome::Effect { reply, effect } = dispatch(&request, false).unwrap() else {
            panic!("RAOP feedback should reset the client watchdog")
        };
        assert_eq!(effect, HandlerEffect::RaopFeedback);
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 11\r\n\r\n"
        );
    }

    #[test]
    fn unknown_rtsp_method_returns_source_501_and_common_headers() {
        let request = parse(b"ANNOUNCE * RTSP/1.0\r\nCSeq: 8\r\n\r\n");
        let DispatchOutcome::Reply(reply) = dispatch(&request, false).unwrap() else {
            panic!("unknown RTSP methods return a response")
        };
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 501 Not Implemented\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 8\r\n\r\n"
        );
    }

    #[test]
    fn stateful_routes_are_not_misreported_as_default_success() {
        let request = parse(b"GET /server-info HTTP/1.1\r\n\r\n");
        assert_eq!(
            dispatch(&request, false).unwrap(),
            DispatchOutcome::Unported(Handler::ServerInfo)
        );
    }

    #[test]
    fn unmatched_http_route_returns_source_not_found_status() {
        let request = parse(b"POST /unknown HTTP/1.1\r\n\r\n");
        let DispatchOutcome::Reply(reply) = dispatch(&request, false).unwrap() else {
            panic!("unmatched HTTP routes return a not-found response")
        };
        assert_eq!(
            reply.bytes,
            b"HTTP/1.1 404 Not Found\r\nServer: AirTunes/220.68\r\n\r\n"
        );
    }

    #[test]
    fn hls_route_stays_unported_until_the_playlist_and_segment_handlers_exist() {
        let request = parse(b"GET /playlist.m3u8 HTTP/1.1\r\n\r\n");
        assert_eq!(
            dispatch(&request, true).unwrap(),
            DispatchOutcome::Unported(Handler::Hls)
        );
    }

    #[test]
    fn http_control_routes_return_common_reply_and_renderer_effect() {
        let cases = [
            (
                b"POST /scrub?position=0.75 HTTP/1.1\r\n\r\n".as_slice(),
                AirPlayControl::Scrub { position: 0.75 },
            ),
            (
                b"POST /rate?value=-1 HTTP/1.1\r\n\r\n".as_slice(),
                AirPlayControl::Rate { rate: -1.0 },
            ),
            (
                b"POST /stop HTTP/1.1\r\n\r\n".as_slice(),
                AirPlayControl::Stop,
            ),
        ];

        for (wire, expected_effect) in cases {
            let request = parse(wire);
            let DispatchOutcome::Effect { reply, effect } = dispatch(&request, false).unwrap()
            else {
                panic!("control route should return its renderer effect")
            };
            assert_eq!(effect, HandlerEffect::AirPlayControl(expected_effect));
            assert_eq!(
                reply.bytes,
                b"HTTP/1.1 200 OK\r\nServer: AirTunes/220.68\r\n\r\n"
            );
        }
    }

    #[test]
    fn server_info_uses_supplied_hardware_address_and_source_body_order() {
        let request = parse(b"GET /server-info HTTP/1.1\r\n\r\n");
        let raw_address = [0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8];
        let DispatchOutcome::Reply(reply) = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                hardware_address: Some(&raw_address),
                playback_snapshot: None,
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("server-info is implemented when the DNS-SD address is supplied")
        };
        let body = airplay_info::server_info_plist_xml(&raw_address);
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/x-apple-plist+xml\r\nServer: AirTunes/220.68\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        assert_eq!(&reply.bytes[..headers.len()], headers.as_bytes());
        assert_eq!(&reply.bytes[headers.len()..], body);
    }

    #[test]
    fn playback_info_uses_snapshot_and_returns_finished_reset_effect() {
        let request = parse(b"GET /playback-info HTTP/1.1\r\n\r\n");
        let snapshot = PlaybackSnapshot {
            duration: 60.0,
            position: 12.5,
            seek_start: 0.0,
            seek_duration: 60.0,
            rate: 1.0,
            ready_to_play: true,
            playback_buffer_empty: false,
            playback_buffer_full: false,
            playback_likely_to_keep_up: true,
        };
        let DispatchOutcome::Reply(reply) = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                hardware_address: None,
                playback_snapshot: Some(snapshot),
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("available playback information is a response")
        };
        let body = airplay_info::playback_info_plist_xml(snapshot);
        let content_type = b"Content-Type: text/x-apple-plist+xml";
        assert!(
            reply
                .bytes
                .windows(content_type.len())
                .any(|part| part == content_type)
        );
        assert!(reply.bytes.ends_with(&body));

        let finished = PlaybackSnapshot {
            duration: -1.0,
            ..snapshot
        };
        let DispatchOutcome::Effect { reply, effect } = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                hardware_address: None,
                playback_snapshot: Some(finished),
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("finished playback returns its reset effect")
        };
        assert!(reply.disconnect);
        assert_eq!(effect, HandlerEffect::ResetHlsPlayback);
    }

    #[test]
    fn get_parameter_preserves_missing_header_status_and_volume_body() {
        let missing = parse(b"GET_PARAMETER / RTSP/1.0\r\nCSeq: 3\r\n\r\n");
        let DispatchOutcome::Reply(reply) = dispatch(&missing, false).unwrap() else {
            panic!("missing Content-Type is a 451 response")
        };
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 451 Parameter not understood\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 3\r\n\r\n"
        );

        let request = parse(
            b"GET_PARAMETER / RTSP/1.0\r\nCSeq: 4\r\nContent-Type: text/parameters\r\nContent-Length: 8\r\n\r\nvolume\r\n",
        );
        let DispatchOutcome::Reply(reply) = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                client_volume: Some(0.5),
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("volume query returns a body, not a callback effect")
        };
        assert!(
            reply
                .bytes
                .windows(29)
                .any(|part| part == b"Content-Type: text/parameters")
        );
        assert!(reply.bytes.ends_with(b"volume:  0.500000\r\n"));
    }

    #[test]
    fn set_parameter_returns_typed_rtp_and_hls_effects() {
        let request = parse(
            b"SET_PARAMETER / RTSP/1.0\r\nCSeq: 5\r\nContent-Type: text/parameters\r\nContent-Length: 17\r\n\r\nvolume: -2.25tail",
        );
        let DispatchOutcome::Effect { reply, effect } = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                parameter_state: SetParameterContext {
                    rtp_present: true,
                    rtp_running: true,
                    hls_volume_callback: false,
                },
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("SET_PARAMETER volume returns an RTP callback effect")
        };
        assert_eq!(
            effect,
            HandlerEffect::RaopParameter(ParameterEffect::Volume {
                target: crate::raop_parameters::VolumeTarget::RaopRtp,
                value: -2.25,
            })
        );
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 5\r\n\r\n"
        );
    }

    #[test]
    fn stateless_http_handlers_match_the_source_response_path() {
        let get_property = dispatch(
            &parse(b"POST /getProperty?anything HTTP/1.1\r\nCSeq: 7\r\n\r\n"),
            false,
        )
        .unwrap();
        let DispatchOutcome::Reply(get_property) = get_property else {
            panic!("GET property is a direct response");
        };
        assert_eq!(
            get_property.bytes,
            b"HTTP/1.1 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 7\r\n\r\n"
        );

        let set_property =
            dispatch(&parse(b"PUT /setProperty?unknown HTTP/1.1\r\n\r\n"), false).unwrap();
        let DispatchOutcome::Reply(set_property) = set_property else {
            panic!("unknown property is a direct response");
        };
        assert_eq!(
            set_property.bytes,
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nServer: AirTunes/220.68\r\n\r\n"
        );

        let fairplay = dispatch(
            &parse(b"POST /fp-setup2 HTTP/1.1\r\nCSeq: 9\r\nContent-Length: 0\r\n\r\n"),
            false,
        )
        .unwrap();
        let DispatchOutcome::Reply(fairplay) = fairplay else {
            panic!("fp-setup2 is a direct response");
        };
        assert_eq!(
            fairplay.bytes,
            b"HTTP/1.1 421 Misdirected Request\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 9\r\n\r\n"
        );
    }

    #[test]
    fn rtsp_info_uses_explicit_dns_sd_context() {
        let request = parse(b"GET /info?txtAirPlay RTSP/1.0\r\n\r\n");
        let hardware_address = [0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8];
        let airplay_txt = b"deviceid=02:00:00:a5:c4:b8";
        let raop_txt = b"ch=2";
        let public_key_hex = b"00112233";
        let info_context = InfoContext {
            raw_hardware_address: &hardware_address,
            airplay_txt,
            raop_txt,
            public_key_hex,
            airplay_features: 0x27f,
            name: "UxPlay",
            initial_volume: 0.0,
            width: 1920,
            height: 1080,
            refresh_rate: 60,
            max_fps: 30,
            overscanned: 0,
        };
        let DispatchOutcome::Reply(reply) = dispatch_with_context(
            &request,
            false,
            DispatchContext {
                raop_info: Some(info_context),
                ..DispatchContext::default()
            },
        )
        .unwrap() else {
            panic!("GET /info is handled when DNS-SD and renderer context is supplied")
        };
        let expected_body = raop_info::response_body(
            InfoRequest {
                url: b"/info?txtAirPlay",
                content_type: None,
                cseq: None,
                body: None,
            },
            info_context,
        )
        .unwrap();
        assert!(reply.bytes.starts_with(
            b"RTSP/1.0 200 OK\r\nContent-Type: application/x-apple-binary-plist\r\nServer: AirTunes/220.68\r\nContent-Length: "
        ));
        assert!(reply.bytes.ends_with(&expected_body));
    }
}
