//! Per-connection adapter from the TCP transport to the staged protocol dispatcher.
//!
//! Session and renderer callbacks are injected as effects. An unported route
//! closes its connection with an explicit `Unsupported` error instead of
//! returning a fabricated success response.

use crate::airplay_event_channel::AirPlayEventChannel;
use crate::airplay_info::PlaybackSnapshot;
use crate::airplay_video::{AirPlayVideoStore, HlsAction};
use crate::http_request::{EventMessage, HttpRequest, ParserFailure};
use crate::http_transport::{
    ConnectionClass, ConnectionHandler, ConnectionInfo, ConnectionRegistry, HttpReply,
    RaopMediaControl, RaopMediaRegistration, ReceiveErrorStage, ServerConfig, serve_with_registry,
};
use crate::password_auth::{AuthorizationDecision, PasswordAuth, initial_setup_device_id};
use crate::protocol_dispatch::{self, DispatchContext, DispatchOutcome, HandlerEffect};
use crate::protocol_response::{HandlerOutput, RequestMetadata, assemble_response};
use crate::protocol_routes::{self, Handler, RouteDecision};
use crate::raop_info::InfoContext;
use crate::raop_media_runtime::{SessionMedia, SessionMediaConfig};
use crate::raop_parameters::SetParameterContext;
use fairplay_playfair::port::{FairPlay, INVALID as FAIRPLAY_INVALID};
use http_response_rs::port::HttpResponse;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use uxplay_pairing::port::{Pairing, PairingSession, ed25519_pk_to_base64, random_pin};
use uxplay_raop_session::{
    self as raop_session, AudioFormat, RaopSessionState, ServerPorts, SessionEffect,
    SessionResponse, SessionTransition, SetupContext, SetupDecisions,
};

#[derive(Clone, Debug, Default)]
pub struct OwnedInfoContext {
    pub raw_hardware_address: Vec<u8>,
    pub airplay_txt: Vec<u8>,
    pub raop_txt: Vec<u8>,
    pub public_key_hex: Vec<u8>,
    pub airplay_features: u64,
    pub name: String,
    pub initial_volume: f64,
    pub width: u16,
    pub height: u16,
    pub refresh_rate: u8,
    pub max_fps: u8,
    pub overscanned: u8,
}

impl OwnedInfoContext {
    fn borrowed(&self) -> InfoContext<'_> {
        InfoContext {
            raw_hardware_address: &self.raw_hardware_address,
            airplay_txt: &self.airplay_txt,
            raop_txt: &self.raop_txt,
            public_key_hex: &self.public_key_hex,
            airplay_features: self.airplay_features,
            name: &self.name,
            initial_volume: self.initial_volume,
            width: self.width,
            height: self.height,
            refresh_rate: self.refresh_rate,
            max_fps: self.max_fps,
            overscanned: self.overscanned,
        }
    }
}

#[derive(Clone)]
pub struct ProtocolContext {
    pub event_channel: Arc<AirPlayEventChannel>,
    pub video_store: Arc<Mutex<AirPlayVideoStore>>,
    pub hls_support: bool,
    pub local_uri_prefix: String,
    pub hardware_address: Option<Vec<u8>>,
    pub playback_snapshot: Option<PlaybackSnapshot>,
    pub client_volume: Arc<Mutex<f32>>,
    pub parameter_state: SetParameterContext,
    pub media_config: SessionMediaConfig,
    pub raop_info: Option<OwnedInfoContext>,
    pub server_ports: ServerPorts,
    pub setup_decisions: SetupDecisions,
    pub password_auth: PasswordAuth,
    pub pairing: Option<Arc<Pairing>>,
    pub registration: Option<Arc<crate::pairing_registry::PairingRegistry>>,
    pub pin_mode: bool,
    pub pairing_pin: Arc<Mutex<u16>>,
    pub client_fps_data: u8,
    pub allowed_clients: Vec<String>,
    pub blocked_clients: Vec<String>,
    pub restrict_clients: bool,
    pub nohold: bool,
    pub connection_registry: Option<Arc<ConnectionRegistry>>,
    pub audio_delay_micros: i32,
    pub hls_pending: Arc<AtomicBool>,
}

impl Default for ProtocolContext {
    fn default() -> Self {
        Self {
            event_channel: Arc::new(AirPlayEventChannel::default()),
            video_store: Arc::new(Mutex::new(AirPlayVideoStore::default())),
            hls_support: false,
            local_uri_prefix: String::new(),
            hardware_address: None,
            playback_snapshot: None,
            client_volume: Arc::new(Mutex::new(0.0)),
            parameter_state: SetParameterContext::default(),
            media_config: SessionMediaConfig::default(),
            raop_info: None,
            server_ports: ServerPorts::default(),
            setup_decisions: SetupDecisions::allow_all(),
            password_auth: PasswordAuth::default(),
            pairing: None,
            registration: None,
            pin_mode: false,
            pairing_pin: Arc::new(Mutex::new(0)),
            client_fps_data: 0,
            allowed_clients: Vec::new(),
            blocked_clients: Vec::new(),
            restrict_clients: false,
            nohold: false,
            connection_registry: None,
            audio_delay_micros: 250_000,
            hls_pending: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Receives session, renderer, and reverse-event work produced by dispatch.
pub trait ProtocolEffectSink: Send + Sync + 'static {
    fn apply(&self, effect: HandlerEffect) -> io::Result<()>;

    fn playback_snapshot(&self) -> io::Result<Option<PlaybackSnapshot>> {
        Ok(None)
    }

    fn reset_server(&self) {}

    /// The source's nohold callback resets video without restarting the RAOP server.
    fn nohold_video_reset(&self) {}

    fn apply_session(&self, _effect: SessionEffect) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "RAOP session effects have no app executor",
        ))
    }

    fn get_audio_format(&self, _format: AudioFormat) -> io::Result<AudioFormat> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "RAOP audio format callback is unavailable",
        ))
    }

    fn event(&self, _event: EventMessage) -> io::Result<()> {
        Ok(())
    }

    fn hls_playlist_removed(&self) -> io::Result<f32> {
        Ok(0.0)
    }

    fn disconnected(&self) {}
}

pub struct ProtocolConnection {
    context: ProtocolContext,
    connection_class: ConnectionClass,
    event_channel: Arc<AirPlayEventChannel>,
    reverse_registered: bool,
    reverse_upgraded: bool,
    effects: Arc<dyn ProtocolEffectSink>,
    peer_address: String,
    peer_address_length: usize,
    fairplay: FairPlay,
    pairing_session: Option<PairingSession>,
    raop_state: RaopSessionState,
    password_authenticated: bool,
    media: Arc<SharedSessionMedia>,
    raop_media_registration: Option<RaopMediaRegistration>,
}

struct SharedSessionMedia(Mutex<SessionMedia>);

impl RaopMediaControl for SharedSessionMedia {
    fn stop_services_for_airplay(&self) -> io::Result<()> {
        lock(&self.0).stop_services_for_airplay()
    }
}

impl ProtocolConnection {
    pub fn new(context: ProtocolContext, effects: Arc<dyn ProtocolEffectSink>) -> io::Result<Self> {
        Self::with_peer(context, effects, SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    fn with_peer(
        context: ProtocolContext,
        effects: Arc<dyn ProtocolEffectSink>,
        peer: SocketAddr,
    ) -> io::Result<Self> {
        Self::construct(context, effects, peer)
    }

    fn with_connection_info(
        context: ProtocolContext,
        effects: Arc<dyn ProtocolEffectSink>,
        connection: ConnectionInfo,
    ) -> io::Result<Self> {
        log_connection_addresses(
            &context.media_config.logger,
            connection.local_addr,
            connection.peer_addr,
        );
        Self::construct(context, effects, connection.peer_addr)
    }

    fn construct(
        context: ProtocolContext,
        effects: Arc<dyn ProtocolEffectSink>,
        peer: SocketAddr,
    ) -> io::Result<Self> {
        let peer_address = peer.ip().to_string();
        let peer_address_length = match peer.ip() {
            IpAddr::V4(_) => 4,
            IpAddr::V6(_) => 16,
        };
        let pairing_session = context
            .pairing
            .as_ref()
            .map(|pairing| {
                pairing
                    .session()
                    .map_err(|error| io::Error::other(format!("pairing session failed: {error:?}")))
            })
            .transpose()?;
        let media = Arc::new(SharedSessionMedia(Mutex::new(SessionMedia::new(
            context.media_config.clone(),
        ))));
        let event_channel = Arc::clone(&context.event_channel);
        Ok(Self {
            context,
            connection_class: ConnectionClass::Unknown,
            event_channel,
            reverse_registered: false,
            reverse_upgraded: false,
            effects,
            peer_address,
            peer_address_length,
            fairplay: FairPlay::new(std::ptr::null_mut()),
            pairing_session,
            raop_state: RaopSessionState::default(),
            password_authenticated: false,
            media,
            raop_media_registration: None,
        })
    }

    fn dispatch(&self, request: &HttpRequest) -> io::Result<DispatchOutcome> {
        let raop_info = self
            .context
            .raop_info
            .as_ref()
            .map(OwnedInfoContext::borrowed);
        let (rtp_present, rtp_running) = lock(&self.media.0).parameter_state();
        let parameter_state = SetParameterContext {
            rtp_present,
            rtp_running,
            ..self.context.parameter_state
        };
        let playback_snapshot = match self.context.playback_snapshot {
            Some(snapshot) => Some(snapshot),
            None => self.effects.playback_snapshot()?,
        };
        let context = DispatchContext {
            hardware_address: self.context.hardware_address.as_deref(),
            playback_snapshot,
            client_volume: Some(*lock(&self.context.client_volume)),
            parameter_state,
            raop_info,
        };
        if let (Some(method), Some(url), Some(protocol)) =
            (request.method(), request.url(), request.protocol())
        {
            match protocol_routes::classify(
                method.as_bytes(),
                url,
                protocol.as_bytes(),
                is_hls_request(request),
            ) {
                RouteDecision::Handler(Handler::GetParameter) => {
                    log_get_parameter_warnings(
                        &self.context.media_config.logger,
                        request.body().unwrap_or_default(),
                    );
                }
                RouteDecision::Handler(Handler::SetParameter) => {
                    log_set_parameter_details(
                        &self.context.media_config.logger,
                        request.header(b"Content-Type"),
                        request.body().unwrap_or_default(),
                        rtp_present,
                    );
                }
                RouteDecision::Handler(Handler::RaopFeedback) => {
                    if let Some(logger) = &self.context.media_config.logger {
                        logger.log(crate::logger::DEBUG, "raop_handler_feedback");
                    }
                }
                RouteDecision::Handler(
                    handler @ (Handler::Scrub
                    | Handler::Rate
                    | Handler::Stop
                    | Handler::SetProperty
                    | Handler::GetProperty
                    | Handler::FairPlaySetup2
                    | Handler::PlaybackInfo),
                ) => log_http_handler_diagnostics(
                    &self.context.media_config.logger,
                    handler,
                    method.as_bytes(),
                    url,
                    request.body().unwrap_or_default(),
                    playback_snapshot,
                ),
                _ => {}
            }
        }
        protocol_dispatch::dispatch_with_context(request, is_hls_request(request), context)
    }

    fn dispatch_session(
        &mut self,
        handler: Handler,
        request: &HttpRequest,
    ) -> io::Result<HttpReply> {
        let (rtp_present, _) = lock(&self.media.0).parameter_state();
        log_simple_session_handler(
            &self.context.media_config.logger,
            handler,
            request,
            rtp_present,
        );
        if handler == Handler::Setup {
            log_setup_request(&self.context.media_config.logger, request);
        }
        if handler == Handler::Reverse {
            return self.dispatch_reverse(request);
        }
        if handler == Handler::FairPlaySetup {
            return self.dispatch_fairplay_setup(request);
        }
        if handler == Handler::PairSetup {
            return self.dispatch_pair_setup(request);
        }
        if handler == Handler::PairVerify {
            return self.dispatch_pair_verify(request);
        }
        if handler == Handler::PairPinStart {
            return self.dispatch_pair_pin_start(request);
        }
        if handler == Handler::PairSetupPin {
            return self.dispatch_pair_setup_pin(request);
        }
        if handler == Handler::AudioMode {
            return self.dispatch_audio_mode(request);
        }

        let transition = match handler {
            Handler::Setup => {
                let mut decisions = self.context.setup_decisions.clone();
                if self.context.password_auth.required() {
                    if let Some(device_id) =
                        initial_setup_device_id(request.body().unwrap_or_default())
                    {
                        let method = request.method().map_or(&[][..], |method| method.as_bytes());
                        match self.context.password_auth.authorize(
                            method,
                            request.header(b"Authorization"),
                            &device_id,
                            &mut self.password_authenticated,
                        )? {
                            AuthorizationDecision::Authorized => {
                                decisions.authorized = true;
                            }
                            AuthorizationDecision::Challenge { value, display_pin } => {
                                decisions.authorized = false;
                                decisions.authorization_challenge = Some(value);
                                if let Some(pin) = display_pin {
                                    self.effects.apply(HandlerEffect::DisplayPairingPin(pin))?;
                                }
                            }
                        }
                    } else {
                        // The C handler challenges only the first SETUP shape with ekey/eiv.
                        decisions.authorized = true;
                    }
                }
                decisions.admit_client &= self.client_admitted(request.body().unwrap_or_default());
                let context = SetupContext {
                    remote_address: &self.peer_address,
                    remote_address_length: self.peer_address_length,
                    ports: self.context.server_ports,
                    decisions,
                    fairplay: &self.fairplay,
                    pairing_session: self.pairing_session.as_ref(),
                    client_fps_data: self.context.client_fps_data,
                };
                raop_session::setup(
                    &mut self.raop_state,
                    request.body().unwrap_or_default(),
                    header_text(request, b"DACP-ID"),
                    header_text(request, b"Active-Remote"),
                    header_text(request, b"User-Agent"),
                    context,
                )
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            }
            Handler::Record => SessionTransition {
                response: raop_session::record(self.context.audio_delay_micros),
                effects: Vec::new(),
            },
            Handler::Flush => {
                raop_session::flush(&self.raop_state, header_text(request, b"RTP-Info"))
            }
            Handler::Teardown => raop_session::teardown(
                &mut self.raop_state,
                request.body().unwrap_or_default(),
                self.context
                    .connection_registry
                    .as_ref()
                    .map_or(0, |registry| registry.count_class(ConnectionClass::Hls)),
                self.context.hls_pending.load(Ordering::Acquire),
            ),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "non-RAOP-session route passed to session dispatcher",
                ));
            }
        };

        let removes_hls_connections = transition.effects.iter().any(|effect| {
            matches!(
                effect,
                SessionEffect::VideoReset(raop_session::VideoResetReason::HlsShutdown)
            )
        });
        let mut response = transition.response;
        if let Err(_error) = self.apply_session_effects(transition.effects) {
            response.disconnect = true;
        }
        if handler == Handler::Setup {
            update_setup_ports(&mut response.body, lock(&self.media.0).setup_ports())?;
        }
        let mut reply = assemble_session_response(request, response, false)?;
        if removes_hls_connections {
            reply = reply.remove_classes_after_write([
                ConnectionClass::Hls,
                ConnectionClass::Event,
                ConnectionClass::AirPlay,
            ]);
        }
        Ok(reply)
    }

    fn dispatch_reverse(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let count = self.event_channel.register();
        self.reverse_registered = true;
        let response = uxplay_airplay_http_handlers::reverse(count).response;
        let (status_code, status_message) = response
            .status
            .map_or((200, "OK"), |(code, message)| (i32::from(code), message));
        let output = HandlerOutput {
            status_code,
            status_message: status_message.as_bytes().to_vec(),
            headers: response
                .headers
                .into_iter()
                .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
                .collect(),
            body: (!response.body.is_empty()).then_some(response.body),
            disconnect: response.disconnect,
        };
        let protocol = request.protocol().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "request has no parsed protocol",
            )
        })?;
        let method = request.method().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed method")
        })?;
        assemble_response(
            RequestMetadata {
                protocol: protocol.as_bytes(),
                method: method.as_bytes(),
                cseq: request.header(b"CSeq"),
                is_hls: false,
            },
            output,
        )
    }

    fn dispatch_audio_mode(&self, request: &HttpRequest) -> io::Result<HttpReply> {
        if let Some(body) = request.body().filter(|body| body.starts_with(b"bplist00"))
            && let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(body))
            && let Some(mode) = value
                .as_dictionary()
                .and_then(|dict| dict.get("audioMode"))
                .and_then(plist::Value::as_string)
        {
            let mode = c_string_prefix(mode.as_bytes());
            let mode = String::from_utf8_lossy(mode);
            let level = if mode.contains("default") {
                crate::logger::DEBUG
            } else {
                crate::logger::INFO
            };
            if let Some(logger) = &self.context.media_config.logger {
                logger.log(
                    level,
                    format!("Unhandled RTSP request \"audioMode: {mode}\""),
                );
            }
        }
        assemble_session_response(request, SessionResponse::default(), false)
    }

    fn dispatch_play(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        if let Some(logger) = &self.context.media_config.logger {
            logger.log(crate::logger::DEBUG, "http_handler_play");
        }
        let headers = request.header_string().unwrap_or_default();
        let body = request.body().unwrap_or_default();
        if !body.is_empty()
            && let (Some(logger), Some(headers)) =
                (&self.context.media_config.logger, request.header_string())
        {
            let mut message = b"request header:\n".to_vec();
            message.extend_from_slice(&headers);
            logger.log_bytes(crate::logger::DEBUG, &message);
        }
        let play = uxplay_airplay_http_handlers::parse_play(
            header_text(request, b"X-Apple-Session-ID"),
            &headers,
            body,
        );
        let play = match play {
            Ok(play) => play,
            Err(error) => {
                if let Some(logger) = &self.context.media_config.logger {
                    match error {
                        uxplay_airplay_http_handlers::PlayParseError::MissingSessionId => {
                            logger.log(crate::logger::ERR, "Play request had no X-Apple-Session-ID")
                        }
                        uxplay_airplay_http_handlers::PlayParseError::UnsupportedContentType => {
                            logger.log(
                                crate::logger::ERR,
                                "Play request Content is not binary_plist (unsupported)",
                            );
                        }
                        uxplay_airplay_http_handlers::PlayParseError::InvalidBinaryPlist
                        | uxplay_airplay_http_handlers::PlayParseError::MissingUuid => {}
                    }
                    logger.log(
                        crate::logger::ERR,
                        "Could not find valid Plist Data for POST/play request, Unhandled",
                    );
                }
                self.effects.reset_server();
                return assemble_http_handler_response(
                    request,
                    uxplay_airplay_http_handlers::HandlerResponse {
                        status: Some((400, "Bad Request")),
                        disconnect: true,
                        ..uxplay_airplay_http_handlers::HandlerResponse::default()
                    },
                    false,
                );
            }
        };
        let reusable = lock(&self.context.video_store).has_reusable_playlist(&play.playback_uuid);
        let has_start_position = plist::Value::from_reader(std::io::Cursor::new(body))
            .ok()
            .and_then(|value| {
                value
                    .as_dictionary()
                    .map(|dictionary| dictionary.contains_key("Start-Position-Seconds"))
            })
            .unwrap_or(false);
        let has_content_location = play.content_location.is_some();
        let client_proc_name = play.client_proc_name.clone();
        let content_location = play.content_location.clone();
        let unsupported_location = content_location
            .as_deref()
            .filter(|location| {
                !location.starts_with("http://")
                    && !location.starts_with("https://")
                    && !location.contains("/master.m3u8")
            })
            .map(str::to_owned);
        let (action, cleaned_playlists) = lock(&self.context.video_store)
            .begin_play_with_diagnostics(play, &self.context.local_uri_prefix);
        if let Some(logger) = &self.context.media_config.logger {
            for playlist in cleaned_playlists {
                logger.log(
                    crate::logger::INFO,
                    format!(
                        "deleting playlist playback_uuid {} duration (seconds) {:.6}",
                        playlist.playback_uuid, playlist.duration_seconds
                    ),
                );
            }
        }
        if !reusable
            && has_content_location
            && !has_start_position
            && let Some(logger) = &self.context.media_config.logger
        {
            logger.log(
                crate::logger::INFO,
                "No Start-Position-Seconds in Play request",
            );
        }
        if let (Some(logger), Some(client_proc_name), Some(location)) = (
            &self.context.media_config.logger,
            client_proc_name.as_deref(),
            content_location.as_deref(),
        ) && !reusable
            && location.contains("/master.m3u8")
            && !"YouTube;".contains(client_proc_name)
        {
            logger.log(
                crate::logger::WARNING,
                format!(
                    "Unsupported m3u8 HLS streaming format: clientProcName {client_proc_name} not found in supported list: YouTube;"
                ),
            );
        }
        if let (Some(logger), Some(location)) = (
            &self.context.media_config.logger,
            unsupported_location.as_deref().filter(|_| !reusable),
        ) {
            logger.log(
                crate::logger::ERR,
                format!("Content-Location has unsupported form:\n{location}\n"),
            );
        }
        if action == HlsAction::BadRequest {
            if unsupported_location.is_none()
                && let Some(logger) = &self.context.media_config.logger
            {
                logger.log(
                    crate::logger::ERR,
                    "Could not find valid Plist Data for POST/play request, Unhandled",
                );
            }
            self.effects.reset_server();
            return assemble_http_handler_response(
                request,
                uxplay_airplay_http_handlers::HandlerResponse {
                    status: Some((400, "Bad Request")),
                    disconnect: true,
                    ..uxplay_airplay_http_handlers::HandlerResponse::default()
                },
                false,
            );
        }
        self.apply_hls_action(action)?;
        assemble_http_handler_response(
            request,
            uxplay_airplay_http_handlers::HandlerResponse::default(),
            false,
        )
    }

    fn dispatch_action(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let headers = request.header_string().unwrap_or_default();
        let body = request.body().unwrap_or_default();
        let (current_session, current_uuid) = {
            let video_store = lock(&self.context.video_store);
            (
                video_store.current_session_id().map(str::to_owned),
                video_store.current_playback_uuid().map(str::to_owned),
            )
        };
        let session_header = header_text(request, b"X-Apple-Session-ID");
        if let Some(logger) = &self.context.media_config.logger {
            if current_session.is_none() {
                logger.log(
                    crate::logger::ERR,
                    "hls_get_current_video: failed to identify current_playlist",
                );
            } else if let Some(session_id) = session_header {
                if current_session.as_deref() != Some(session_id) {
                    logger.log(
                        crate::logger::ERR,
                        format!(
                            "X-Apple-Session-ID has changed:\n  was:\"{}\"\n  now:\"{session_id}\"",
                            current_session.as_deref().unwrap_or_default()
                        ),
                    );
                }
            } else {
                logger.log(crate::logger::ERR, "Play request had no X-Apple-Session-ID");
            }
        }
        let session_valid =
            current_session.is_some() && current_session.as_deref() == session_header;
        let has_binary_plist_marker = headers
            .windows(b"apple-binary-plist".len())
            .any(|window| window == b"apple-binary-plist");
        if session_valid && let Some(logger) = &self.context.media_config.logger {
            let mut message = b"request header: ".to_vec();
            message.extend_from_slice(&headers);
            logger.log_bytes(crate::logger::DEBUG, &message);
            if !has_binary_plist_marker || body.is_empty() {
                logger.log(
                    crate::logger::INFO,
                    "POST /action: did not receive expected plist from client",
                );
            }
        }
        let action_plan = {
            uxplay_airplay_http_handlers::action(
                current_session.as_deref(),
                session_header,
                &headers,
                body,
            )
        };
        if session_valid
            && has_binary_plist_marker
            && !body.is_empty()
            && let Some(logger) = &self.context.media_config.logger
            && let Ok(plist::Value::Dictionary(root)) =
                plist::Value::from_reader(std::io::Cursor::new(body))
            && let Some(action_type) = root.get("type").and_then(plist::Value::as_string)
        {
            logger.log(
                crate::logger::DEBUG,
                format!("action type is {action_type}"),
            );
        }
        if let Some(logger) = &self.context.media_config.logger {
            match &action_plan {
                uxplay_airplay_http_handlers::ActionPlan::RemovePlaylist { uuid } => {
                    if current_uuid.as_deref() != Some(uuid) {
                        logger.log(
                            crate::logger::WARNING,
                            format!("playlistRemove uuid {uuid} does not match current_video\n"),
                        );
                    }
                }
                uxplay_airplay_http_handlers::ActionPlan::InsertPlaylist { uuid } => {
                    logger.log(
                        crate::logger::INFO,
                        "action type playlistInsert (start playback)",
                    );
                    if let Some(uuid) = uuid {
                        let index = lock(&self.context.video_store).playlist_index_for_uuid(uuid);
                        if let Some(index) = index {
                            logger.log(
                                crate::logger::INFO,
                                format!(
                                    "playlistInsert uuid {uuid} is stored at airplay_video[{index}]"
                                ),
                            );
                        } else {
                            logger.log(
                                crate::logger::INFO,
                                format!("playlistInsert uuid {uuid} is not a stored playlist"),
                            );
                        }
                    }
                    logger.log(
                        crate::logger::ERR,
                        "FIXME: playlistInsert is not yet implemented",
                    );
                }
                uxplay_airplay_http_handlers::ActionPlan::FcupPlaylist {
                    url,
                    is_master,
                    playlist,
                } => {
                    let details = plist::Value::from_reader(std::io::Cursor::new(body)).ok();
                    let params = details
                        .as_ref()
                        .and_then(plist::Value::as_dictionary)
                        .and_then(|root| root.get("params"))
                        .and_then(plist::Value::as_dictionary);
                    if let Some(status) = params
                        .and_then(|params| params.get("FCUP_Response_StatusCode"))
                        .and_then(plist::Value::as_unsigned_integer)
                    {
                        logger.log(
                            crate::logger::DEBUG,
                            format!("FCUP_Response_StatusCode = {}", status as i32),
                        );
                    }
                    if let Some(request_id) = params
                        .and_then(|params| params.get("FCUP_Response_RequestID"))
                        .and_then(plist::Value::as_unsigned_integer)
                    {
                        logger.log(
                            crate::logger::DEBUG,
                            format!("FCUP_Response_RequestID =  {}", request_id as i32),
                        );
                    }
                    logger.log(crate::logger::DEBUG, format!("FCUP_Response_URL =  {url}"));
                    let mut message = b"begin FCUP Response data:\n".to_vec();
                    message.extend_from_slice(playlist);
                    message.extend_from_slice(b"\nend FCUP Response data");
                    logger.log_bytes(crate::logger::DEBUG, &message);
                    if !is_master {
                        let duplicate = lock(&self.context.video_store)
                            .next_media_playlist_is_duplicate(playlist);
                        if duplicate {
                            logger.log(
                                crate::logger::DEBUG,
                                "media_playlist is a duplicate: do not store",
                            );
                        } else {
                            let stats =
                                uxplay_airplay_http_handlers::analyze_media_playlist(playlist);
                            if stats.chunks > 0 {
                                logger.log(
                                    crate::logger::DEBUG,
                                    format!(
                                        "\n{url}:\nreceived media playlist has {:5} chunks, total duration {:9.3} secs\n",
                                        stats.chunks, stats.duration_seconds
                                    ),
                                );
                            }
                        }
                    }
                }
                uxplay_airplay_http_handlers::ActionPlan::IgnoreUnknown { .. } => {
                    logger.log(crate::logger::INFO, "unknown action type (unhandled)");
                }
                uxplay_airplay_http_handlers::ActionPlan::BadRequest => {
                    if session_valid
                        && has_binary_plist_marker
                        && let Ok(plist::Value::Dictionary(root)) =
                            plist::Value::from_reader(std::io::Cursor::new(body))
                        && let Some(data) = root
                            .get("params")
                            .and_then(plist::Value::as_dictionary)
                            .and_then(|params| params.get("FCUP_Response_Data"))
                            .and_then(plist::Value::as_data)
                        && !data.is_empty()
                        && !data.starts_with(b"#EXTM3U\n")
                    {
                        logger.log(crate::logger::ERR, "playlist is not a valid M3U8 playlist");
                    }
                }
            }
        }
        let action = lock(&self.context.video_store)
            .apply_action(action_plan, &self.context.local_uri_prefix);
        if action == HlsAction::BadRequest {
            return assemble_http_handler_response(
                request,
                uxplay_airplay_http_handlers::HandlerResponse {
                    status: Some((400, "Bad Request")),
                    ..uxplay_airplay_http_handlers::HandlerResponse::default()
                },
                false,
            );
        }
        self.apply_hls_action(action)?;
        assemble_http_handler_response(
            request,
            uxplay_airplay_http_handlers::HandlerResponse::default(),
            false,
        )
    }

    fn dispatch_hls(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let url = request
            .url()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "HLS request has no URL"))?;
        let url = std::str::from_utf8(url)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let (index, has_playlist, body, media_stats) = {
            let video_store = lock(&self.context.video_store);
            let index = video_store.current_video_index();
            let has_playlist = video_store.current_has_playlist();
            let plan = uxplay_airplay_http_handlers::plan_hls(
                index,
                has_playlist,
                header_text(request, b"Upgrade"),
                url,
            );
            let body = match plan {
                uxplay_airplay_http_handlers::HlsPlan::FetchMaster
                | uxplay_airplay_http_handlers::HlsPlan::FetchMedia { .. } => {
                    video_store.playlist_for_request(url)
                }
                _ => None,
            };
            let media_stats = if matches!(
                plan,
                uxplay_airplay_http_handlers::HlsPlan::FetchMedia { .. }
            ) {
                video_store.media_playlist_stats_for_request(url)
            } else {
                None
            };
            (index, has_playlist, body, media_stats)
        };
        let plan = uxplay_airplay_http_handlers::plan_hls(
            index,
            has_playlist,
            header_text(request, b"Upgrade"),
            url,
        );
        if let Some(logger) = &self.context.media_config.logger {
            match &plan {
                uxplay_airplay_http_handlers::HlsPlan::MissingCurrentVideo => {
                    logger.log(crate::logger::ERR, "airplay_video playlist  not found");
                }
                uxplay_airplay_http_handlers::HlsPlan::DeclineUpgrade => {
                    let mut message = request.header_string().unwrap_or_default();
                    message.extend_from_slice(b"\nhls upgrade request declined");
                    logger.log_bytes(crate::logger::INFO, &message);
                }
                uxplay_airplay_http_handlers::HlsPlan::MissingPlaylist
                | uxplay_airplay_http_handlers::HlsPlan::FetchMaster
                    if body.as_ref().is_none_or(Vec::is_empty) =>
                {
                    let message = if url == "/master.m3u8" {
                        format!("requested master playlist {url} not found")
                    } else {
                        format!("requested media playlist {url} not found")
                    };
                    logger.log(crate::logger::ERR, message);
                }
                uxplay_airplay_http_handlers::HlsPlan::FetchMedia { .. }
                    if body.as_ref().is_none_or(Vec::is_empty) =>
                {
                    let message = if media_stats.is_some() {
                        format!(
                            "requested media playlist {url} rejected (invalid YT-EXT-CONDENSED-URL)"
                        )
                    } else {
                        format!("requested media playlist {url} not found")
                    };
                    logger.log(crate::logger::ERR, message);
                }
                uxplay_airplay_http_handlers::HlsPlan::FetchMedia { .. } => {
                    if let Some(stats) = media_stats {
                        logger.log(
                            crate::logger::INFO,
                            format!(
                                "Requested media_playlist {url} has {:5} chunks, total duration {:9.3} secs",
                                stats.chunks, stats.duration_seconds
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
        let response = uxplay_airplay_http_handlers::finish_hls(
            &plan,
            body,
            &source_date_string(std::time::SystemTime::now()),
        );
        assemble_http_handler_response(request, response, true)
    }

    fn apply_hls_action(&mut self, action: HlsAction) -> io::Result<()> {
        match action {
            HlsAction::Fetch {
                url,
                session_id,
                request_id,
            } => {
                if let Err(error) = self.event_channel.send_fcup_request_with_logger(
                    &url,
                    &session_id,
                    request_id,
                    self.context.media_config.logger.as_deref(),
                ) && let Some(logger) = &self.context.media_config.logger
                {
                    logger.log(
                        crate::logger::ERR,
                        format!(
                            "fcup_request: send  error {}:{error}\n",
                            error.raw_os_error().unwrap_or_default()
                        ),
                    );
                }
            }
            HlsAction::StartPlayback {
                location,
                start_position_seconds,
            } => self.effects.apply(HandlerEffect::StartHlsPlayback {
                location,
                start_position_seconds,
            })?,
            HlsAction::RemoveCurrent { uuid } => {
                let position = self.effects.hls_playlist_removed()?;
                lock(&self.context.video_store).set_resume_position(&uuid, position);
            }
            HlsAction::BadRequest | HlsAction::Ignore => {}
        }
        Ok(())
    }

    fn dispatch_fairplay_setup(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let body = request.body().unwrap_or_default();
        let mut response = SessionResponse::default();
        if body.len() != 16 && body.len() != 164 {
            response.status = 400;
            response.reason = "Bad Request";
        } else if body[4] != 2 && body[4] != 3 {
            response.status = 501;
            response.reason = "Not Implemented";
        } else if body.len() == 16 {
            let request_data: &[u8; 16] = body.try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid FairPlay setup request")
            })?;
            let mut reply = [0_u8; 142];
            let status = self.fairplay.setup(request_data, &mut reply);
            if status == 0 {
                response
                    .headers
                    .push(("Content-Type".into(), "application/octet-stream".into()));
                response.body = Some(reply.to_vec());
            } else {
                response.status = if status == FAIRPLAY_INVALID { 400 } else { 500 };
                response.reason = if response.status == 400 {
                    "Bad Request"
                } else {
                    "Internal Server Error"
                };
            }
        } else {
            let request_data: &[u8; 164] = body.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid FairPlay handshake request",
                )
            })?;
            let mut reply = [0_u8; 32];
            let status = self.fairplay.handshake(request_data, &mut reply);
            if status == 0 {
                response
                    .headers
                    .push(("Content-Type".into(), "application/octet-stream".into()));
                response.body = Some(reply.to_vec());
            } else {
                response.status = if status == FAIRPLAY_INVALID { 400 } else { 500 };
                response.reason = if response.status == 400 {
                    "Bad Request"
                } else {
                    "Internal Server Error"
                };
            }
        }
        assemble_session_response(request, response, false)
    }

    fn dispatch_pair_setup(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let Some(pairing) = self.context.pairing.as_ref() else {
            return Err(unported_handler(Handler::PairSetup));
        };
        let Some(session) = self.pairing_session.as_mut() else {
            return Err(unported_handler(Handler::PairSetup));
        };
        let body = request.body().unwrap_or_default();
        let mut response = SessionResponse::default();
        if body.len() == 32 {
            let public_key = pairing.get_public_key().map_err(|error| {
                io::Error::other(format!("pairing public key failed: {error:?}"))
            })?;
            session.set_setup_status();
            response
                .headers
                .push(("Content-Type".into(), "application/octet-stream".into()));
            response.body = Some(public_key.to_vec());
        }
        assemble_session_response(request, response, false)
    }

    fn dispatch_pair_verify(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let pin_mode = self.context.pin_mode;
        let registration = self.context.registration.clone();
        let Some(session) = self.pairing_session.as_mut() else {
            return Err(unported_handler(Handler::PairVerify));
        };
        let register_check = session.check_handshake_status() != 0;
        let mut response = SessionResponse::default();
        if register_check {
            if !pin_mode {
                return assemble_session_response(request, response, false);
            }
            session.set_setup_status();
        }

        let body = request.body().unwrap_or_default();
        if body.len() < 4 {
            return assemble_session_response(request, response, false);
        }
        match body[0] {
            1 => {
                if body.len() != 4 + 32 + 32 {
                    return assemble_session_response(request, response, false);
                }
                let remote_ecdh: &[u8; 32] = body[4..36].try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid pair-verify ECDH key")
                })?;
                let remote_ed: &[u8; 32] = body[36..68].try_into().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid pair-verify Ed25519 key",
                    )
                })?;
                session.handshake(remote_ecdh, remote_ed).map_err(|error| {
                    io::Error::other(format!("pair-verify handshake failed: {error:?}"))
                })?;
                let public_key = session
                    .get_public_key()
                    .map_err(|error| {
                        io::Error::other(format!("pair-verify public key failed: {error:?}"))
                    })?
                    .ok_or_else(|| {
                        io::Error::other("pair-verify handshake produced no public key")
                    })?;
                let signature = session
                    .get_signature()
                    .map_err(|error| {
                        io::Error::other(format!("pair-verify signature failed: {error:?}"))
                    })?
                    .ok_or_else(|| {
                        io::Error::other("pair-verify handshake produced no signature")
                    })?;
                if register_check {
                    let client_public_key = ed25519_pk_to_base64(remote_ed);
                    if registration
                        .as_ref()
                        .is_some_and(|registry| !registry.contains(&client_public_key))
                    {
                        return assemble_session_response(request, response, false);
                    }
                }
                let mut body = Vec::with_capacity(public_key.len() + signature.len());
                body.extend_from_slice(&public_key);
                body.extend_from_slice(&signature);
                response
                    .headers
                    .push(("Content-Type".into(), "application/octet-stream".into()));
                response.body = Some(body);
            }
            0 => {
                if body.len() != 4 + 64 {
                    return assemble_session_response(request, response, false);
                }
                let signature: &[u8; 64] = body[4..68].try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid pair-verify signature")
                })?;
                let status = session.finish(signature).map_err(|error| {
                    io::Error::other(format!("pair-verify finish failed: {error:?}"))
                })?;
                if status != 0 {
                    response.disconnect = true;
                } else {
                    response
                        .headers
                        .push(("Content-Type".into(), "application/octet-stream".into()));
                }
            }
            _ => {}
        }
        assemble_session_response(request, response, false)
    }

    fn dispatch_pair_pin_start(&self, request: &HttpRequest) -> io::Result<HttpReply> {
        let mut stored_pin = self
            .context
            .pairing_pin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pin = if *stored_pin > 9999 {
            *stored_pin % 10000
        } else {
            let pin = random_pin().map_err(|error| {
                io::Error::other(format!("pairing PIN generation failed: {error:?}"))
            })?;
            *stored_pin = pin;
            pin
        };
        drop(stored_pin);
        self.effects
            .apply(HandlerEffect::DisplayPairingPin(format!("{pin:04}")))?;
        assemble_session_response(request, SessionResponse::default(), false)
    }

    fn dispatch_pair_setup_pin(&mut self, request: &HttpRequest) -> io::Result<HttpReply> {
        let Some(pairing) = self.context.pairing.as_ref() else {
            return Err(unported_handler(Handler::PairSetupPin));
        };
        let Some(session) = self.pairing_session.as_mut() else {
            return Err(unported_handler(Handler::PairSetupPin));
        };
        let body = request.body().unwrap_or_default();
        // The C handler searches its complete serialized header string with
        // `strstr`, rather than checking only Content-Type. Preserve that
        // source-visible behavior for clients that place the marker elsewhere.
        let has_plist_marker = request
            .header_string()
            .is_some_and(|headers| contains_bytes(&headers, b"apple-binary-plist"));
        if !has_plist_marker || !body.starts_with(b"bplist00") {
            return pairing_authentication_failure(request);
        }
        let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(body)) else {
            return pairing_authentication_failure(request);
        };
        let Some(dict) = value.as_dictionary() else {
            return pairing_authentication_failure(request);
        };

        let method = dict.get("method").and_then(plist::Value::as_string);
        let user = dict.get("user").and_then(plist::Value::as_string);
        if let (Some(method), Some(user)) = (method, user) {
            let method = c_string_prefix(method.as_bytes());
            if !b"pin".starts_with(method) {
                return assemble_session_response(request, SessionResponse::default(), false);
            }
            let pin = {
                let mut stored_pin = self
                    .context
                    .pairing_pin
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let pin = *stored_pin % 10000;
                if *stored_pin < 10000 {
                    *stored_pin = 0;
                }
                pin
            };
            let pin = format!("{pin:04}");
            let Some(offer) = session
                .srp_new_user(user.as_bytes(), pin.as_bytes())
                .ok()
                .flatten()
            else {
                return pairing_authentication_failure(request);
            };
            return pairing_plist_response(
                request,
                [("pk", offer.server_public), ("salt", offer.salt)],
            );
        }

        if let (Some(client_public), Some(client_proof)) = (
            dict.get("pk").and_then(plist::Value::as_data),
            dict.get("proof").and_then(plist::Value::as_data),
        ) {
            if client_proof.len() != uxplay_pairing::port::SRP_PROOF_SIZE {
                return pairing_authentication_failure(request);
            }
            let mut proof = client_proof.to_vec();
            if session.srp_validate_proof(client_public, &mut proof).ok() != Some(0) {
                return pairing_authentication_failure(request);
            }
            return pairing_plist_response(request, [("proof", proof)]);
        }

        if let (Some(client_ephemeral), Some(auth_tag)) = (
            dict.get("epk").and_then(plist::Value::as_data),
            dict.get("authTag").and_then(plist::Value::as_data),
        ) {
            let (Ok(client_ephemeral), Ok(auth_tag)) = (
                <&[u8; 32]>::try_from(client_ephemeral),
                <&[u8; 16]>::try_from(auth_tag),
            ) else {
                return pairing_authentication_failure(request);
            };
            let Some((server_ephemeral, server_tag)) = session
                .confirm_pair_setup(client_ephemeral, auth_tag, pairing)
                .ok()
                .flatten()
            else {
                return pairing_authentication_failure(request);
            };
            session.set_setup_status();
            return pairing_plist_response(
                request,
                [
                    ("epk", server_ephemeral.to_vec()),
                    ("authTag", server_tag.to_vec()),
                ],
            );
        }

        pairing_authentication_failure(request)
    }

    fn apply_session_effects(&mut self, effects: Vec<SessionEffect>) -> io::Result<()> {
        let mut adjusted_audio_format = None;
        for effect in effects {
            match effect {
                SessionEffect::GetAudioFormat { format } => {
                    adjusted_audio_format = Some(self.effects.get_audio_format(format)?);
                }
                SessionEffect::StartAudio {
                    remote_address,
                    ports,
                    format,
                    sample_rate,
                } => {
                    lock(&self.media.0).apply(SessionEffect::StartAudio {
                        remote_address,
                        ports,
                        format: adjusted_audio_format.take().unwrap_or(format),
                        sample_rate,
                    })?;
                }
                effect @ (SessionEffect::RegisterClient { .. }
                | SessionEffect::ReportClientRequest { .. }) => {
                    self.effects.apply_session(effect)?;
                }
                SessionEffect::ClearHlsVideoData => {
                    lock(&self.context.video_store).clear_all();
                    lock(&self.media.0).apply(SessionEffect::ClearHlsVideoData)?;
                }
                effect @ SessionEffect::VideoReset(
                    uxplay_raop_session::VideoResetReason::HlsShutdown,
                ) => {
                    lock(&self.context.video_store).clear_all();
                    self.event_channel.clear();
                    lock(&self.media.0).apply(effect)?;
                }
                effect => lock(&self.media.0).apply(effect)?,
            }
        }
        Ok(())
    }

    fn client_admitted(&self, body: &[u8]) -> bool {
        let device_id = if body.starts_with(b"bplist00") {
            plist::Value::from_reader(std::io::Cursor::new(body))
                .ok()
                .and_then(|value| {
                    value
                        .as_dictionary()?
                        .get("deviceID")?
                        .as_string()
                        .map(str::to_owned)
                })
        } else {
            None
        };
        let Some(device_id) = device_id else {
            // Match uxplay.cpp's report_client_request: it can only apply the allow/block lists
            // when the source callback receives a non-null DeviceID.
            return true;
        };
        !self
            .context
            .blocked_clients
            .iter()
            .any(|item| item == &device_id)
            && (!self.context.restrict_clients
                || self
                    .context
                    .allowed_clients
                    .iter()
                    .any(|item| item == &device_id))
    }
}

impl ConnectionHandler for ProtocolConnection {
    fn request(&mut self, request: &HttpRequest) -> io::Result<Option<HttpReply>> {
        log_request_start(&self.context.media_config.logger);
        let method = request.method().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed method")
        })?;
        let protocol = request.protocol().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "request has no parsed protocol",
            )
        })?;
        let url = request.url().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed URL")
        })?;
        let hls_request = is_hls_request(request);
        let ble_request = protocol.as_bytes() == b"RTSP/1.0"
            && request.header(b"CSeq").is_none()
            && (contains_bytes(url, b"txtAirPlay") || contains_bytes(url, b"txtRAOP"));
        if ble_request {
            log_at(
                &self.context.media_config.logger,
                crate::logger::INFO,
                "response to Bluetooth LE beacon advertisement received)",
            );
        }
        if !self.context.hls_support && request.header(b"CSeq").is_none() && !ble_request {
            log_at(
                &self.context.media_config.logger,
                crate::logger::INFO,
                "ignoring AirPlay video streaming request (use option -hls to activate HLS support)",
            );
            return Ok(None);
        }
        if self.connection_class == ConnectionClass::Unknown {
            let incoming_class = if request.header(b"CSeq").is_some() || ble_request {
                Some(ConnectionClass::Raop)
            } else if request.header(b"X-Apple-Session-ID").is_some() {
                Some(ConnectionClass::AirPlay)
            } else if request.header(b"Host").is_some() {
                Some(ConnectionClass::Hls)
            } else {
                None
            };
            if incoming_class == Some(ConnectionClass::Raop)
                && let Some(registry) = self.context.connection_registry.as_ref()
                && registry.has_class(ConnectionClass::Raop)
            {
                if !self.context.nohold {
                    log_at(
                        &self.context.media_config.logger,
                        crate::logger::WARNING,
                        format!(
                            "rejecting new connection request from {}",
                            self.peer_address
                        ),
                    );
                    let reply = raop_connection_conflict_response(request)?;
                    log_response_headers(&self.context.media_config.logger, url, &reply);
                    return Ok(Some(reply));
                }
                log_at(
                    &self.context.media_config.logger,
                    crate::logger::INFO,
                    format!(
                        "*****\"nohold\" feature: switch to new connection request from {}",
                        self.peer_address
                    ),
                );
                registry.remove_known_connections();
                self.effects.nohold_video_reset();
            }
            if let Some(incoming_class) = incoming_class {
                log_connection_classification(
                    &self.context.media_config.logger,
                    self,
                    incoming_class,
                );
                self.connection_class = incoming_class;
                match incoming_class {
                    ConnectionClass::Raop => {
                        if let Some(registry) = self.context.connection_registry.as_ref() {
                            self.raop_media_registration =
                                Some(registry.register_raop_media(&self.media));
                        }
                    }
                    ConnectionClass::AirPlay => {
                        if let Some(registry) = self.context.connection_registry.as_ref()
                            && let Err(error) = registry.stop_raop_media_for_airplay()
                        {
                            log_at(
                                &self.context.media_config.logger,
                                crate::logger::ERR,
                                format!(
                                    "New AirPlay connection: failed to stop RAOP media services: {error}"
                                ),
                            );
                        }
                    }
                    _ => {}
                }
            } else {
                log_at(
                    &self.context.media_config.logger,
                    crate::logger::WARNING,
                    "connection from unknown connection type",
                );
            }
        }
        log_request_details(
            &self.context.media_config.logger,
            request,
            method,
            url,
            protocol,
        );
        let route =
            protocol_routes::classify(method.as_bytes(), url, protocol.as_bytes(), hls_request);
        match route {
            RouteDecision::Handler(Handler::ServerInfo) => {
                self.context.hls_pending.store(true, Ordering::Release);
            }
            RouteDecision::Handler(Handler::Setup) => {
                self.context.hls_pending.store(false, Ordering::Release);
            }
            _ => {}
        }
        if let RouteDecision::Handler(
            handler @ (Handler::PairSetup
            | Handler::PairVerify
            | Handler::PairPinStart
            | Handler::PairSetupPin
            | Handler::FairPlaySetup
            | Handler::AudioMode
            | Handler::Reverse
            | Handler::Play
            | Handler::Action
            | Handler::Hls
            | Handler::Setup
            | Handler::Record
            | Handler::Flush
            | Handler::Teardown),
        ) = route
        {
            let reply = match handler {
                Handler::Play => self.dispatch_play(request)?,
                Handler::Action => self.dispatch_action(request)?,
                Handler::Hls => self.dispatch_hls(request)?,
                _ => self.dispatch_session(handler, request)?,
            };
            log_response_headers(&self.context.media_config.logger, url, &reply);
            return Ok(Some(reply));
        }
        match self.dispatch(request)? {
            DispatchOutcome::Reply(reply) => {
                log_response_headers(&self.context.media_config.logger, url, &reply);
                Ok(Some(reply))
            }
            DispatchOutcome::Effect {
                reply,
                effect: HandlerEffect::RaopParameter(parameter),
            } => {
                match parameter {
                    parameter @ crate::raop_parameters::ParameterEffect::Volume {
                        target: crate::raop_parameters::VolumeTarget::HlsRenderer,
                        ..
                    } => self
                        .effects
                        .apply(HandlerEffect::RaopParameter(parameter))?,
                    parameter => lock(&self.media.0).apply_parameter(parameter)?,
                }
                log_response_headers(&self.context.media_config.logger, url, &reply);
                Ok(Some(reply))
            }
            DispatchOutcome::Effect { reply, effect } => {
                let closes_media_connections = effect == HandlerEffect::ResetHlsPlayback;
                self.effects.apply(effect)?;
                let reply = if closes_media_connections {
                    reply.remove_classes_after_write([
                        ConnectionClass::Hls,
                        ConnectionClass::Event,
                        ConnectionClass::AirPlay,
                    ])
                } else {
                    reply
                };
                log_response_headers(&self.context.media_config.logger, url, &reply);
                Ok(Some(reply))
            }
            DispatchOutcome::Unported(handler) => Err(unported_handler(handler)),
        }
    }

    fn idle_timeout(&mut self, socket_id: i32, idle_timeout_seconds: u64) {
        if let Some(logger) = self.context.media_config.logger.as_ref() {
            logger.log(
                crate::logger::WARNING,
                format!(
                    "httpd closing dead connection on socket {socket_id} after timeout_limit = {idle_timeout_seconds} seconds of inactivity"
                ),
            );
        }
    }

    fn receive_error(&mut self, error: &ParserFailure, recv_data: &[u8], stage: ReceiveErrorStage) {
        let Some(logger) = self.context.media_config.logger.as_ref() else {
            return;
        };
        match stage {
            ReceiveErrorStage::Request => {
                // llhttp leaves the callback reason null when the source's
                // second-protocol-token callback returns HPE_USER. The C
                // logger prints that null `%s` as `(null)` on this platform.
                let reason = if error.description.is_empty() {
                    "(null)"
                } else {
                    error.description.as_str()
                };
                let mut message =
                    format!("httpd error in parsing: {}\n{}\n", error.name, reason).into_bytes();
                message.extend(
                    recv_data
                        .iter()
                        .map(|byte| if *byte == b'\r' { b' ' } else { *byte }),
                );
                logger.log_bytes(crate::logger::ERR, &message);
            }
            ReceiveErrorStage::ReverseEvent => {
                logger.log(
                    crate::logger::ERR,
                    format!(
                        "httd error in adding event data (too large?)): size = {}",
                        recv_data.len()
                    ),
                );
            }
        }
    }

    fn connection_class(&self) -> ConnectionClass {
        self.connection_class
    }

    fn event(&mut self, event: EventMessage) -> io::Result<()> {
        log_event_message(&self.context.media_config.logger, &event);
        self.effects.event(event)
    }

    fn upgraded(&mut self, stream: std::net::TcpStream) -> io::Result<()> {
        if self.reverse_registered {
            self.connection_class = ConnectionClass::Event;
            self.event_channel.install(stream);
            self.reverse_upgraded = true;
        }
        Ok(())
    }

    fn disconnected(&mut self) {
        lock(&self.media.0).stop();
        self.raop_media_registration.take();
        if self.reverse_registered {
            if self.reverse_upgraded {
                self.event_channel.clear();
            }
            self.event_channel.unregister();
            self.reverse_registered = false;
        }
        self.effects.disconnected();
    }
}

fn log_event_message(logger: &Option<Arc<crate::logger::Logger>>, event: &EventMessage) {
    let Some(logger) = logger else {
        return;
    };
    let header = event
        .header
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default();
    let (level, prefix, body) = if header.len() > 8 && header.starts_with(b"HTTP/1.1") {
        (
            crate::logger::DEBUG,
            b"<<<<<< reverse-HTTP response:\n".as_slice(),
            true,
        )
    } else if header.len() > 9 && header.starts_with(b"EVENT/1.0") {
        if event.body.is_empty() {
            (
                crate::logger::DEBUG,
                b"<<<<<< event message without Body:\n".as_slice(),
                true,
            )
        } else {
            (
                crate::logger::INFO,
                b"<<<<<< event message with Body:\n".as_slice(),
                true,
            )
        }
    } else {
        (
            crate::logger::ERR,
            b"<<<<<< unrecognized message on EVENT channel:\n".as_slice(),
            false,
        )
    };
    let logged_header = if body {
        header
    } else {
        &header[..header.len().min(32)]
    };
    let mut message = Vec::with_capacity(prefix.len() + logged_header.len() + 1);
    message.extend_from_slice(prefix);
    message.extend_from_slice(logged_header);
    message.push(b'\n');
    logger.log_bytes(level, &message);
}

fn log_request_start(logger: &Option<Arc<crate::logger::Logger>>) {
    if let Some(logger) = logger {
        logger.log(crate::logger::DEBUG, "conn_request");
    }
}

fn log_request_details(
    logger: &Option<Arc<crate::logger::Logger>>,
    request: &HttpRequest,
    method: &str,
    url: &[u8],
    protocol: &str,
) {
    let Some(logger) = logger else {
        return;
    };
    let url = c_string_prefix(url);
    let mut request_line = Vec::with_capacity(method.len() + url.len() + protocol.len() + 3);
    request_line.push(b'\n');
    request_line.extend_from_slice(method.as_bytes());
    request_line.push(b' ');
    request_line.extend_from_slice(url);
    request_line.push(b' ');
    request_line.extend_from_slice(protocol.as_bytes());
    logger.log_bytes(crate::logger::DEBUG, &request_line);
    if let Some(headers) = request.header_string()
        && (url != b"/playback-info" || logger.level() >= crate::logger::DEBUG_DATA)
    {
        logger.log_bytes(crate::logger::DEBUG, &headers);
    }
    let mut handling = b"Handling request ".to_vec();
    handling.extend_from_slice(method.as_bytes());
    handling.extend_from_slice(b" with URL ");
    handling.extend_from_slice(url);
    logger.log_bytes(crate::logger::DEBUG, &handling);
}

fn log_response_headers(
    logger: &Option<Arc<crate::logger::Logger>>,
    url: &[u8],
    reply: &HttpReply,
) {
    let Some(logger) = logger else {
        return;
    };
    if c_string_prefix(url) == b"/playback-info" && logger.level() < crate::logger::DEBUG_DATA {
        return;
    }
    let Some(separator) = reply
        .bytes
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
    else {
        return;
    };
    let header_end = if reply.bytes.len() > separator + 4 {
        separator + 4
    } else {
        separator + 2
    };
    let header = reply.bytes[..header_end]
        .iter()
        .map(|byte| if *byte == b'\r' { b' ' } else { *byte })
        .collect::<Vec<_>>();
    logger.log_bytes(crate::logger::DEBUG, &header);
}

fn log_http_handler_diagnostics(
    logger: &Option<Arc<crate::logger::Logger>>,
    handler: Handler,
    method: &[u8],
    url: &[u8],
    body: &[u8],
    playback_snapshot: Option<PlaybackSnapshot>,
) {
    let Some(logger) = logger else {
        return;
    };
    match handler {
        Handler::Scrub | Handler::Rate => {
            let Some((control, parsed_numeric)) =
                crate::airplay_controls::parse_control_with_numeric_status(method, url)
            else {
                return;
            };
            match control {
                crate::airplay_controls::AirPlayControl::Scrub { position } => {
                    if parsed_numeric {
                        logger.log(
                            crate::logger::DEBUG,
                            format!("http_handler_scrub: got position = {position:.6}"),
                        );
                    }
                    logger.log(
                        crate::logger::DEBUG,
                        format!(
                            "**********************SCRUB {position:.6} ***********************"
                        ),
                    );
                }
                crate::airplay_controls::AirPlayControl::Rate { rate } => {
                    if parsed_numeric {
                        logger.log(
                            crate::logger::DEBUG,
                            format!("http_handler_rate: got rate = {rate:.6}"),
                        );
                    }
                }
                crate::airplay_controls::AirPlayControl::Stop => {}
            }
        }
        Handler::Stop => logger.log(crate::logger::INFO, "client HTTP request POST stop"),
        Handler::SetProperty => {
            let url = url.split(|byte| *byte == 0).next().unwrap_or_default();
            let property = url
                .strip_prefix(b"/setProperty?")
                .unwrap_or_else(|| url.get(b"/setProperty?".len()..).unwrap_or_default());
            let mut message = b"http_handler_set_property: ".to_vec();
            message.extend_from_slice(property);
            logger.log_bytes(crate::logger::DEBUG, &message);

            let known = uxplay_airplay_http_handlers::set_property_bytes(url)
                .headers
                .iter()
                .any(|(name, _)| name == "Content-Type");
            let mut message = b"property ".to_vec();
            message.extend_from_slice(property);
            if known {
                message.extend_from_slice(b" is known but unhandled");
            } else {
                message.extend_from_slice(b" is unknown, unhandled");
            }
            logger.log_bytes(crate::logger::DEBUG, &message);
        }
        Handler::GetProperty => {
            let url = url.split(|byte| *byte == 0).next().unwrap_or_default();
            // The source advances by strlen("getProperty?") without the leading
            // slash, so the logged property starts with the question mark.
            let property = url.get(b"getProperty?".len()..).unwrap_or_default();
            let mut message = b"http_handler_get_property: ".to_vec();
            message.extend_from_slice(property);
            message.extend_from_slice(b" (unhandled)");
            logger.log_bytes(crate::logger::DEBUG, &message);
        }
        Handler::FairPlaySetup2 => {
            logger.log(
                crate::logger::WARNING,
                "client HTTP request POST fp-setup2 is unhandled",
            );
            if let Some(version) = body.get(4) {
                logger.log(
                    crate::logger::ERR,
                    format!(
                        "only FairPlay version 0x03 is implemented, version is 0x{version:02x}"
                    ),
                );
            }
        }
        Handler::PlaybackInfo => {
            if let Some(snapshot) = playback_snapshot {
                if snapshot.duration == -1.0 {
                    logger.log(
                        crate::logger::DEBUG,
                        "playback_info not available (finishing)",
                    );
                } else if snapshot.position == -1.0 {
                    logger.log(crate::logger::DEBUG, "playback_info not available");
                }
            }
        }
        _ => {}
    }
}

fn log_get_parameter_warnings(logger: &Option<Arc<crate::logger::Logger>>, body: &[u8]) {
    let Some(logger) = logger else {
        return;
    };
    let mut cursor = 0;
    while cursor < body.len() {
        let remaining = &body[cursor..];
        if remaining.len() >= 8 && remaining.starts_with(b"volume\r\n") {
            return;
        }
        let Some(line_end_relative) = remaining.iter().position(|byte| *byte == b'\r') else {
            return;
        };
        let line_end = cursor + line_end_relative;
        if body.len() - line_end < 2 || &body[line_end..line_end + 2] != b"\r\n" {
            return;
        }
        if line_end > cursor {
            let mut message = b"Found an unknown parameter: ".to_vec();
            message.extend_from_slice(&body[cursor..line_end]);
            logger.log_bytes(crate::logger::WARNING, &message);
        }
        cursor = line_end + 2;
    }
}

fn log_set_parameter_details(
    logger: &Option<Arc<crate::logger::Logger>>,
    content_type: Option<&[u8]>,
    body: &[u8],
    rtp_present: bool,
) {
    let Some(logger) = logger else {
        return;
    };
    match content_type {
        Some(b"text/parameters")
            if !rtp_present && !body.is_empty() && !body.starts_with(b"volume: ") =>
        {
            logger.log(
                crate::logger::WARNING,
                "RAOP not initialized at SET_PARAMETER",
            );
        }
        Some(b"image/jpeg" | b"image/png") => {
            logger.log(
                crate::logger::DEBUG,
                format!("Got image data of {} bytes", body.len() as i32),
            );
            if !rtp_present {
                logger.log(
                    crate::logger::WARNING,
                    "RAOP not initialized at SET_PARAMETER coverart",
                );
            }
        }
        Some(b"application/x-dmap-tagged") => {
            logger.log(
                crate::logger::DEBUG,
                format!("Got metadata of {} bytes", body.len() as i32),
            );
            if !rtp_present {
                logger.log(
                    crate::logger::WARNING,
                    "RAOP not initialized at SET_PARAMETER metadata",
                );
            }
        }
        _ => {}
    }
}

fn log_simple_session_handler(
    logger: &Option<Arc<crate::logger::Logger>>,
    handler: Handler,
    request: &HttpRequest,
    rtp_present: bool,
) {
    let Some(logger) = logger else {
        return;
    };
    match handler {
        Handler::Record => logger.log(crate::logger::DEBUG, "raop_handler_record"),
        Handler::Flush => {
            if let Some(rtp_info) = request.header(b"RTP-Info") {
                let mut message = b"Flush with RTP-Info: ".to_vec();
                message.extend_from_slice(c_string_prefix(rtp_info));
                logger.log_bytes(crate::logger::DEBUG, &message);
            }
            if !rtp_present {
                logger.log(crate::logger::WARNING, "RAOP not initialized at FLUSH");
            }
        }
        Handler::Teardown => {
            let (teardown_audio, teardown_mirror) =
                teardown_stream_flags(request.body().unwrap_or_default());
            logger.log(
                crate::logger::DEBUG,
                format!(
                    "TEARDOWN request,  96={}, 110={}",
                    i32::from(teardown_audio),
                    i32::from(teardown_mirror)
                ),
            );
        }
        _ => {}
    }
}

fn log_setup_request(logger: &Option<Arc<crate::logger::Logger>>, request: &HttpRequest) {
    let Some(logger) = logger else {
        return;
    };
    if let (Some(dacp_id), Some(active_remote)) =
        (request.header(b"DACP-ID"), request.header(b"Active-Remote"))
    {
        for (label, value) in [
            (b"DACP-ID: ".as_slice(), dacp_id),
            (b"Active-Remote: ".as_slice(), active_remote),
        ] {
            let mut message = label.to_vec();
            message.extend_from_slice(c_string_prefix(value));
            logger.log_bytes(crate::logger::DEBUG, &message);
        }
    }
    if initial_setup_device_id(request.body().unwrap_or_default()).is_some() {
        logger.log(crate::logger::DEBUG, "SETUP 1");
    }
}

fn teardown_stream_flags(body: &[u8]) -> (bool, bool) {
    if !body.starts_with(b"bplist00") {
        return (false, false);
    }
    let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(body)) else {
        return (false, false);
    };
    let Some(streams) = value
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("streams"))
        .and_then(plist::Value::as_array)
    else {
        return (false, false);
    };
    let mut teardown_audio = false;
    let mut teardown_mirror = false;
    for stream in streams {
        match stream
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("type"))
            .and_then(plist::Value::as_unsigned_integer)
        {
            Some(96) => teardown_audio = true,
            Some(110) => teardown_mirror = true,
            _ => {}
        }
    }
    (teardown_audio, teardown_mirror)
}

fn raop_connection_conflict_response(request: &HttpRequest) -> io::Result<HttpReply> {
    let protocol = request.protocol().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "request has no parsed protocol",
        )
    })?;
    let mut response = HttpResponse::new();
    response.init(
        protocol.as_bytes(),
        409,
        b"Conflict: Server is connected to another client",
    );
    response.add_header(b"Server", crate::protocol_response::SOURCE_SERVER_HEADER);
    if let Some(cseq) = request.header(b"CSeq") {
        if cseq.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty CSeq triggers the source response builder assertion",
            ));
        }
        response.add_header(b"CSeq", cseq);
    }
    response.finish(None, 0);
    Ok(HttpReply::new(response.completed_data().to_vec()))
}

/// Serve the staged protocol dispatcher over the existing persistent transport.
///
/// This is ready for startup/session integration, but callers must still supply
/// DNS-SD identity, callback state, listeners, and the session effect sink.
pub fn serve_protocol(
    listeners: Vec<TcpListener>,
    shutdown: Arc<AtomicBool>,
    server_config: ServerConfig,
    context: ProtocolContext,
    effects: Arc<dyn ProtocolEffectSink>,
) -> io::Result<()> {
    serve_protocol_with_registry(
        listeners,
        shutdown,
        server_config,
        context,
        Arc::new(ConnectionRegistry::default()),
        effects,
    )
}

pub fn serve_protocol_with_registry(
    listeners: Vec<TcpListener>,
    shutdown: Arc<AtomicBool>,
    server_config: ServerConfig,
    context: ProtocolContext,
    registry: Arc<ConnectionRegistry>,
    effects: Arc<dyn ProtocolEffectSink>,
) -> io::Result<()> {
    let mut context = context;
    context.connection_registry = Some(Arc::clone(&registry));
    serve_with_registry(
        listeners,
        shutdown,
        server_config,
        registry,
        move |connection: ConnectionInfo| {
            ProtocolConnection::with_connection_info(
                context.clone(),
                Arc::clone(&effects),
                connection,
            )
        },
    )
}

fn unported_handler(handler: Handler) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("protocol handler {handler:?} is not ported"),
    )
}

fn log_at(logger: &Option<Arc<crate::logger::Logger>>, level: i32, message: impl AsRef<str>) {
    if let Some(logger) = logger {
        logger.log(level, message.as_ref());
    }
}

fn log_connection_addresses(
    logger: &Option<Arc<crate::logger::Logger>>,
    local: SocketAddr,
    peer: SocketAddr,
) {
    log_at(
        logger,
        crate::logger::INFO,
        format!("Local : {}", local.ip()),
    );
    log_at(
        logger,
        crate::logger::INFO,
        format!("Remote: {}", peer.ip()),
    );
}

fn log_connection_classification(
    logger: &Option<Arc<crate::logger::Logger>>,
    connection: &ProtocolConnection,
    class: ConnectionClass,
) {
    let kind = match class {
        ConnectionClass::Raop => "RAOP",
        ConnectionClass::AirPlay => "AirPlay",
        ConnectionClass::Hls => "HLS",
        ConnectionClass::Unknown | ConnectionClass::Event => return,
    };
    log_at(
        logger,
        crate::logger::DEBUG,
        format!("New connection {connection:p} identified as Connection type {kind}"),
    );
}

fn header_text<'a>(request: &'a HttpRequest, name: &[u8]) -> Option<&'a str> {
    let value = request.header(name)?;
    let value = value.split(|byte| *byte == 0).next()?;
    std::str::from_utf8(value).ok()
}

fn is_hls_request(request: &HttpRequest) -> bool {
    request.header(b"Host").is_some()
        && request.header(b"CSeq").is_none()
        && request.header(b"X-Apple-Session-ID").is_none()
}

fn assemble_http_handler_response(
    request: &HttpRequest,
    response: uxplay_airplay_http_handlers::HandlerResponse,
    is_hls: bool,
) -> io::Result<HttpReply> {
    let protocol = request.protocol().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "request has no parsed protocol",
        )
    })?;
    let method = request.method().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed method")
    })?;
    let (status_code, status_message) = response
        .status
        .map_or((200, "OK"), |(code, message)| (i32::from(code), message));
    assemble_response(
        RequestMetadata {
            protocol: protocol.as_bytes(),
            method: method.as_bytes(),
            cseq: request.header(b"CSeq"),
            is_hls,
        },
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
        },
    )
}

fn source_date_string(time: std::time::SystemTime) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Ok(elapsed) = time.duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    let days = (elapsed.as_secs() / 86_400) as i64;
    let day_seconds = elapsed.as_secs() % 86_400;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let weekday = WEEKDAYS[((days + 4).rem_euclid(7)) as usize];
    let month_name = MONTHS[(month - 1) as usize];
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{weekday} {month_name} {day:02} {hour:02}:{minute:02}:{second:02} {year} GMT")
}

fn assemble_session_response(
    request: &HttpRequest,
    response: SessionResponse,
    is_hls: bool,
) -> io::Result<HttpReply> {
    let protocol = request.protocol().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "request has no parsed protocol",
        )
    })?;
    let method = request.method().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "request has no parsed method")
    })?;
    let output = HandlerOutput {
        status_code: i32::from(response.status),
        status_message: response.reason.as_bytes().to_vec(),
        headers: response
            .headers
            .into_iter()
            .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
            .collect(),
        body: response.body,
        disconnect: response.close_connection || response.disconnect,
    };
    assemble_response(
        RequestMetadata {
            protocol: protocol.as_bytes(),
            method: method.as_bytes(),
            cseq: request.header(b"CSeq"),
            is_hls,
        },
        output,
    )
}

fn pairing_plist_response<const N: usize>(
    request: &HttpRequest,
    fields: [(&str, Vec<u8>); N],
) -> io::Result<HttpReply> {
    let mut dictionary = plist::Dictionary::new();
    for (key, value) in fields {
        dictionary.insert(key.to_owned(), plist::Value::Data(value));
    }
    let mut body = Vec::new();
    plist::Value::Dictionary(dictionary)
        .to_writer_binary(&mut body)
        .map_err(|error| {
            io::Error::other(format!("pairing plist serialization failed: {error}"))
        })?;
    let response = SessionResponse {
        headers: vec![(
            "Content-Type".into(),
            "application/x-apple-binary-plist".into(),
        )],
        body: Some(body),
        ..SessionResponse::default()
    };
    assemble_session_response(request, response, false)
}

fn pairing_authentication_failure(request: &HttpRequest) -> io::Result<HttpReply> {
    assemble_session_response(
        request,
        SessionResponse {
            status: 470,
            reason: "Client Authentication Failure",
            ..SessionResponse::default()
        },
        false,
    )
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn c_string_prefix(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    &bytes[..end]
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn update_setup_ports(
    response_body: &mut Option<Vec<u8>>,
    (timing_port, audio_ports, mirror_port): (
        Option<u16>,
        Option<uxplay_raop_rtp::AudioPorts>,
        Option<u16>,
    ),
) -> io::Result<()> {
    if timing_port.is_none() && audio_ports.is_none() && mirror_port.is_none() {
        return Ok(());
    }
    let Some(body) = response_body.as_mut() else {
        return Ok(());
    };
    let value = plist::Value::from_reader(std::io::Cursor::new(body.as_slice()))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let plist::Value::Dictionary(mut dictionary) = value else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RAOP SETUP response plist is not a dictionary",
        ));
    };
    if let Some(port) = timing_port {
        dictionary.insert("timingPort".into(), plist::Value::from(u64::from(port)));
    }
    if let Some(streams) = dictionary
        .get_mut("streams")
        .and_then(plist::Value::as_array_mut)
    {
        for stream in streams {
            let Some(stream) = stream.as_dictionary_mut() else {
                continue;
            };
            match stream
                .get("type")
                .and_then(plist::Value::as_unsigned_integer)
            {
                Some(96) => {
                    if let Some(ports) = audio_ports {
                        stream.insert(
                            "controlPort".into(),
                            plist::Value::from(u64::from(ports.control_local)),
                        );
                        stream.insert(
                            "dataPort".into(),
                            plist::Value::from(u64::from(ports.data_local)),
                        );
                    }
                }
                Some(110) => {
                    if let Some(port) = mirror_port {
                        stream.insert("dataPort".into(), plist::Value::from(u64::from(port)));
                    }
                }
                _ => {}
            }
        }
    }
    let mut serialized = Vec::new();
    plist::Value::Dictionary(dictionary)
        .to_writer_binary(&mut serialized)
        .map_err(|error| io::Error::other(format!("SETUP response plist failed: {error}")))?;
    *body = serialized;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airplay_controls::AirPlayControl;
    use crate::raop_media_runtime::VIDEO_RENDERER_TEST_LOCK;
    use openssl::bn::{BigNum, BigNumContext, BigNumRef};
    use openssl::hash::{MessageDigest, hash};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, Shutdown, TcpStream, UdpSocket};
    use std::sync::{Mutex, atomic::AtomicUsize};
    use std::thread;
    use std::time::Duration;
    use std::{collections::VecDeque, sync::atomic::Ordering};
    use uxplay_crypto::port::{
        AesContext, AesMode, Direction, Ed25519Key, X25519Key, gcm_decrypt, gcm_encrypt, sha512,
    };
    use uxplay_pairing::port::{Pairing, PairingStatus, ed25519_pk_to_base64};

    #[test]
    fn missing_device_id_follows_source_admission_rule_when_clients_are_restricted() {
        let context = ProtocolContext {
            restrict_clients: true,
            allowed_clients: Vec::new(),
            blocked_clients: vec!["blocked-device".into()],
            ..ProtocolContext::default()
        };
        let connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();
        let mut body = Vec::new();
        plist::Value::Dictionary(plist::Dictionary::new())
            .to_writer_binary(&mut body)
            .unwrap();

        assert!(connection.client_admitted(&body));
    }

    #[derive(Default)]
    struct RecordingEffects(
        Mutex<Vec<HandlerEffect>>,
        Mutex<Vec<SessionEffect>>,
        std::sync::atomic::AtomicUsize,
        std::sync::atomic::AtomicUsize,
    );

    impl ProtocolEffectSink for RecordingEffects {
        fn apply(&self, effect: HandlerEffect) -> io::Result<()> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(effect);
            Ok(())
        }

        fn apply_session(&self, effect: SessionEffect) -> io::Result<()> {
            self.1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(effect);
            Ok(())
        }

        fn get_audio_format(&self, format: AudioFormat) -> io::Result<AudioFormat> {
            Ok(format)
        }

        fn reset_server(&self) {
            self.2.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }

        fn nohold_video_reset(&self) {
            self.3.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    #[derive(Default)]
    struct PlaylistRemovalEffects {
        applied: Mutex<Vec<HandlerEffect>>,
        callback_positions: Mutex<VecDeque<f32>>,
        callback_calls: AtomicUsize,
    }

    impl ProtocolEffectSink for PlaylistRemovalEffects {
        fn apply(&self, effect: HandlerEffect) -> io::Result<()> {
            self.applied
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(effect);
            Ok(())
        }

        fn hls_playlist_removed(&self) -> io::Result<f32> {
            self.callback_calls.fetch_add(1, Ordering::AcqRel);
            self.callback_positions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .ok_or_else(|| io::Error::other("missing playlist-removal fixture position"))
        }
    }

    fn parse(wire: &[u8]) -> HttpRequest {
        let mut request = HttpRequest::new();
        request.add_data(wire).unwrap();
        assert!(request.is_complete());
        request
    }

    fn temp_registration_file() -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "uxplay-pair-verify-registration-{}-{nonce}.txt",
            std::process::id()
        ))
    }

    type CapturedLogs = Arc<Mutex<Vec<Vec<u8>>>>;
    type CapturedLogEvents = Arc<Mutex<Vec<(i32, Vec<u8>)>>>;

    fn collecting_logger(level: i32) -> (Arc<crate::logger::Logger>, CapturedLogs) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger = Arc::new(crate::logger::Logger::new());
        logger.set_level(level);
        logger.set_callback(Some(Arc::new(move |_, message| {
            events_for_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message.to_vec());
        })));
        (logger, events)
    }

    fn collecting_logger_events(level: i32) -> (Arc<crate::logger::Logger>, CapturedLogEvents) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger = Arc::new(crate::logger::Logger::new());
        logger.set_level(level);
        logger.set_callback(Some(Arc::new(move |level, message| {
            events_for_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((level, message.to_vec()));
        })));
        (logger, events)
    }

    fn protocol_context_with_logger(logger: Arc<crate::logger::Logger>) -> ProtocolContext {
        ProtocolContext {
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        }
    }

    #[test]
    fn server_info_sets_shared_hls_pending_until_a_raop_setup() {
        let context = ProtocolContext {
            hls_support: true,
            hardware_address: Some(vec![0x02, 0x00, 0x00, 0x12, 0x34, 0x56]),
            ..ProtocolContext::default()
        };
        let hls_pending = Arc::clone(&context.hls_pending);
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut airplay = ProtocolConnection::new(context.clone(), Arc::clone(&effects)).unwrap();

        let server_info =
            parse(b"GET /server-info HTTP/1.1\r\nX-Apple-Session-ID: hls-session\r\n\r\n");
        let reply = airplay.request(&server_info).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(hls_pending.load(Ordering::Acquire));

        let mut raop = ProtocolConnection::new(context, effects).unwrap();
        let setup = setup_rtsp(&[], None, 1);
        let reply = raop.request(&setup).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            !hls_pending.load(Ordering::Acquire),
            "SETUP clears the server-wide pending transition even on another connection"
        );
        airplay.disconnected();
        raop.disconnected();
    }

    #[test]
    fn teardown_uses_live_hls_connections_and_removes_related_classes_after_reply() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let hls_peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (hls_stream, _) = listener.accept().unwrap();
        let registry = Arc::new(ConnectionRegistry::default());
        let hls_registration = registry.register(&hls_stream).unwrap();
        hls_registration.set_class(ConnectionClass::Hls);

        let video_store = Arc::new(Mutex::new(AirPlayVideoStore::default()));
        lock(&video_store).begin_play(
            uxplay_airplay_http_handlers::PlayRequest {
                session_id: "hls-session".into(),
                playback_uuid: "hls-video".into(),
                content_location: Some("https://example.invalid/master.m3u8".into()),
                start_position_seconds: 0.0,
                client_proc_name: None,
            },
            "http://localhost:7000",
        );
        let context = ProtocolContext {
            hls_support: true,
            video_store: Arc::clone(&video_store),
            connection_registry: Some(Arc::clone(&registry)),
            media_config: SessionMediaConfig {
                use_video: false,
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        };
        let mut raop =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        let teardown = parse(b"TEARDOWN / RTSP/1.0\r\nCSeq: 7\r\n\r\n");
        let reply = raop.request(&teardown).unwrap().unwrap();

        assert!(reply.disconnect);
        assert_eq!(
            reply.remove_classes_after_write,
            [
                ConnectionClass::Hls,
                ConnectionClass::Event,
                ConnectionClass::AirPlay
            ]
        );
        assert_eq!(registry.count_class(ConnectionClass::Hls), 1);
        assert_eq!(lock(&video_store).current_video_index(), None);
        assert_eq!(
            lock(&video_store).playlist_index_for_uuid("hls-video"),
            None
        );

        raop.disconnected();
        drop(hls_registration);
        drop(hls_peer);
    }

    #[test]
    fn connection_start_and_raop_classification_logs_match_source_shapes() {
        let (logger, events) = collecting_logger_events(crate::logger::DEBUG);
        let context = protocol_context_with_logger(logger);
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::with_connection_info(
            context,
            effects,
            ConnectionInfo {
                local_addr: "192.0.2.10:7000".parse().unwrap(),
                peer_addr: "198.51.100.4:51000".parse().unwrap(),
            },
        )
        .unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [
                (crate::logger::INFO, b"Local : 192.0.2.10".to_vec()),
                (crate::logger::INFO, b"Remote: 198.51.100.4".to_vec()),
            ]
        );
        events.lock().unwrap().clear();

        connection
            .request(&parse(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n"))
            .unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events[0], (crate::logger::DEBUG, b"conn_request".to_vec()));
        let classification = events
            .iter()
            .position(|(_, message)| message.starts_with(b"New connection 0x"))
            .expect("RAOP identification log");
        assert_eq!(events[classification].0, crate::logger::DEBUG);
        assert!(
            events[classification]
                .1
                .ends_with(b" identified as Connection type RAOP")
        );
        let request = events
            .iter()
            .position(|(_, message)| message == b"\nOPTIONS * RTSP/1.0")
            .expect("request line log");
        assert!(classification < request);
    }

    #[test]
    fn beacon_and_hls_rejection_logs_match_source_levels_and_text() {
        let (logger, events) = collecting_logger_events(crate::logger::INFO);
        let context = protocol_context_with_logger(Arc::clone(&logger));
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(context, effects).unwrap();
        events.lock().unwrap().clear();

        assert!(
            connection
                .request(&parse(b"GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            *events.lock().unwrap(),
            [(
                crate::logger::INFO,
                b"ignoring AirPlay video streaming request (use option -hls to activate HLS support)".to_vec()
            )]
        );

        events.lock().unwrap().clear();
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                raop_info: Some(OwnedInfoContext::default()),
                ..protocol_context_with_logger(logger)
            },
            Arc::new(RecordingEffects::default()),
        )
        .unwrap();
        events.lock().unwrap().clear();
        connection
            .request(&parse(b"GET /info?txtAirPlay RTSP/1.0\r\n\r\n"))
            .unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [(
                crate::logger::INFO,
                b"response to Bluetooth LE beacon advertisement received)".to_vec()
            )]
        );
    }

    #[test]
    fn unclassified_connection_logs_source_warning() {
        let (logger, events) = collecting_logger_events(crate::logger::WARNING);
        let context = ProtocolContext {
            hls_support: true,
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        };
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(context, effects).unwrap();
        events.lock().unwrap().clear();

        connection
            .request(&parse(b"OPTIONS * RTSP/1.0\r\n\r\n"))
            .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            [(
                crate::logger::WARNING,
                b"connection from unknown connection type".to_vec()
            )]
        );
    }

    #[test]
    fn protocol_logger_records_request_and_response_headers_at_debug() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let configured = Some(Arc::clone(&logger));
        let request = parse(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n");
        log_request_start(&configured);
        log_request_details(&configured, &request, "GET", b"/info", "RTSP/1.0");
        let reply = HttpReply::new(
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nServer: AirTunes/220.68\r\n\r\n".to_vec(),
        );
        log_response_headers(&configured, b"/info", &reply);

        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [
                b"conn_request".to_vec(),
                b"\nGET /info RTSP/1.0".to_vec(),
                b"CSeq: 1\n".to_vec(),
                b"Handling request GET with URL /info".to_vec(),
                b"RTSP/1.0 200 OK \nCSeq: 1 \nServer: AirTunes/220.68 \n".to_vec(),
            ]
        );
    }

    #[test]
    fn protocol_logger_preserves_non_utf8_request_and_response_header_bytes() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let configured = Some(Arc::clone(&logger));
        let request = parse(b"GET / RTSP/1.0\r\nX-Test: \xfe\r\n\r\n");
        log_request_details(&configured, &request, "GET", b"/\xff", "RTSP/1.0");
        let reply = HttpReply::new(b"RTSP/1.0 200 OK\r\nX-Value: \xff\r\n\r\n".to_vec());
        log_response_headers(&configured, b"/raw", &reply);

        assert_eq!(
            *events.lock().unwrap(),
            [
                b"\nGET /\xff RTSP/1.0".to_vec(),
                b"X-Test: \xfe\n".to_vec(),
                b"Handling request GET with URL /\xff".to_vec(),
                b"RTSP/1.0 200 OK \nX-Value: \xff \n".to_vec(),
            ]
        );
    }

    #[test]
    fn setup_logging_matches_source_first_request_fields() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let configured = Some(logger);
        let mut setup = plist::Dictionary::new();
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let mut wire = format!(
            "SETUP / RTSP/1.0\r\nDACP-ID: client-dacp\r\nActive-Remote: 42\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        wire.extend_from_slice(&body);
        let request = parse(&wire);

        log_setup_request(&configured, &request);

        assert_eq!(
            *events.lock().unwrap(),
            [
                b"DACP-ID: client-dacp".to_vec(),
                b"Active-Remote: 42".to_vec(),
                b"SETUP 1".to_vec(),
            ]
        );
    }

    #[test]
    fn protocol_logger_requires_debug_data_for_playback_info_headers() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let configured = Some(Arc::clone(&logger));
        let request = parse(b"GET /playback-info RTSP/1.0\r\nCSeq: 1\r\n\r\n");
        log_request_start(&configured);
        log_request_details(&configured, &request, "GET", b"/playback-info", "RTSP/1.0");
        let reply = HttpReply::new(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n".to_vec());
        log_response_headers(&configured, b"/playback-info", &reply);
        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [
                b"conn_request".to_vec(),
                b"\nGET /playback-info RTSP/1.0".to_vec(),
                b"Handling request GET with URL /playback-info".to_vec(),
            ]
        );

        logger.set_level(crate::logger::DEBUG_DATA);
        log_request_details(&configured, &request, "GET", b"/playback-info", "RTSP/1.0");
        log_response_headers(&configured, b"/playback-info", &reply);
        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(events[4], b"CSeq: 1\n");
        assert_eq!(events[6], b"RTSP/1.0 200 OK \nCSeq: 1 \n");
    }

    #[test]
    fn reverse_channel_logger_preserves_source_categories_and_raw_headers() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let configured = Some(Arc::clone(&logger));
        log_event_message(
            &configured,
            &EventMessage {
                header: b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
                body: Vec::new(),
            },
        );
        log_event_message(
            &configured,
            &EventMessage {
                header: b"EVENT/1.0 200 OK\r\nContent-Length: 1\r\n\r\n".to_vec(),
                body: vec![0xff],
            },
        );
        log_event_message(
            &configured,
            &EventMessage {
                header: b"EVENT/1.0 200 OK\r\n\r\n".to_vec(),
                body: Vec::new(),
            },
        );
        log_event_message(
            &configured,
            &EventMessage {
                header: b"INVALID \xff\0hidden bytes beyond C string".to_vec(),
                body: Vec::new(),
            },
        );

        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            &events[..3],
            [
                b"<<<<<< reverse-HTTP response:\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n\n"
                    .to_vec(),
                b"<<<<<< event message with Body:\nEVENT/1.0 200 OK\r\nContent-Length: 1\r\n\r\n\n"
                    .to_vec(),
                b"<<<<<< event message without Body:\nEVENT/1.0 200 OK\r\n\r\n\n".to_vec(),
            ]
        );
        let mut unrecognized = b"<<<<<< unrecognized message on EVENT channel:\nINVALID ".to_vec();
        unrecognized.extend_from_slice(&[0xff, b'\n']);
        assert_eq!(events[3], unrecognized);
    }

    #[test]
    fn reverse_channel_bodyless_events_are_filtered_below_debug() {
        let (logger, events) = collecting_logger(crate::logger::INFO);
        let configured = Some(Arc::clone(&logger));
        log_event_message(
            &configured,
            &EventMessage {
                header: b"EVENT/1.0 200 OK\r\n\r\n".to_vec(),
                body: Vec::new(),
            },
        );
        log_event_message(
            &configured,
            &EventMessage {
                header: b"EVENT/1.0 200 OK\r\nContent-Length: 2\r\n\r\n".to_vec(),
                body: b"ok".to_vec(),
            },
        );
        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [
                b"<<<<<< event message with Body:\nEVENT/1.0 200 OK\r\nContent-Length: 2\r\n\r\n\n"
                    .to_vec()
            ]
        );
    }

    #[test]
    fn request_receive_errors_log_source_reason_and_cr_sanitized_recv_bytes() {
        let (logger, events) = collecting_logger(crate::logger::ERR);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        connection.receive_error(
            &ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            },
            b"GET /two H\r\nBad\r\n",
            ReceiveErrorStage::Request,
        );

        assert_eq!(
            *events.lock().unwrap(),
            [b"httpd error in parsing: HPE_USER\n(null)\nGET /two H \nBad \n".to_vec()]
        );
    }

    #[test]
    fn reverse_event_receive_errors_log_source_overflow_message() {
        let (logger, events) = collecting_logger(crate::logger::ERR);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        connection.receive_error(
            &ParserFailure {
                name: "HPE_USER",
                description: "reverse HTTP event buffer limit exceeded".into(),
            },
            &[b'x'; 1024],
            ReceiveErrorStage::ReverseEvent,
        );

        assert_eq!(
            *events.lock().unwrap(),
            [b"httd error in adding event data (too large?)): size = 1024".to_vec()]
        );
    }

    #[test]
    fn idle_timeout_logs_the_source_socket_warning() {
        let (logger, events) = collecting_logger(crate::logger::WARNING);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        connection.idle_timeout(37, 60);

        assert_eq!(
            *events.lock().unwrap(),
            [b"httpd closing dead connection on socket 37 after timeout_limit = 60 seconds of inactivity".to_vec()]
        );
    }

    fn pairing_context() -> ProtocolContext {
        let pairing = Pairing::init_generate(b"18:3d:2d:c2:cc:c5", None)
            .unwrap()
            .0;
        ProtocolContext {
            pairing: Some(Arc::new(pairing)),
            ..ProtocolContext::default()
        }
    }

    fn post_rtsp(path: &str, body: &[u8], cseq: u8) -> HttpRequest {
        let header = format!(
            "POST {path} RTSP/1.0\r\nCSeq: {cseq}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(body);
        parse(&wire)
    }

    fn registry_with_raop_client() -> (
        Arc<ConnectionRegistry>,
        TcpStream,
        crate::http_transport::ConnectionRegistration,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let peer = TcpStream::connect(address).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let registry = Arc::new(ConnectionRegistry::default());
        let registration = registry.register(&stream).unwrap();
        registration.set_class(ConnectionClass::Raop);
        (registry, peer, registration)
    }

    #[test]
    fn second_raop_client_gets_source_conflict_without_nohold() {
        let (registry, _old_peer, _old_registration) = registry_with_raop_client();
        let (logger, events) = collecting_logger_events(crate::logger::WARNING);
        let context = ProtocolContext {
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            connection_registry: Some(registry),
            ..ProtocolContext::default()
        };
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(context, effects).unwrap();
        events.lock().unwrap().clear();

        let reply = connection
            .request(&parse(b"OPTIONS * RTSP/1.0\r\nCSeq: 12\r\n\r\n"))
            .unwrap()
            .unwrap();

        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 409 Conflict: Server is connected to another client\r\nServer: AirTunes/220.68\r\nCSeq: 12\r\n\r\n"
        );
        assert!(!reply.disconnect);
        assert_eq!(connection.connection_class(), ConnectionClass::Unknown);
        assert_eq!(
            *events.lock().unwrap(),
            [(
                crate::logger::WARNING,
                b"rejecting new connection request from 127.0.0.1".to_vec()
            )]
        );
    }

    #[test]
    fn nohold_switch_closes_old_known_sockets_and_accepts_new_raop_client() {
        let (registry, mut old_peer, old_registration) = registry_with_raop_client();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let old_airplay_peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (old_airplay_stream, _) = listener.accept().unwrap();
        let old_airplay_registration = registry.register(&old_airplay_stream).unwrap();
        old_airplay_registration.set_class(ConnectionClass::AirPlay);
        let address = listener.local_addr().unwrap();
        let mut new_peer = TcpStream::connect(address).unwrap();
        let (new_stream, _) = listener.accept().unwrap();
        let new_registration = registry.register(&new_stream).unwrap();
        let (logger, events) = collecting_logger_events(crate::logger::INFO);
        let context = ProtocolContext {
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            nohold: true,
            connection_registry: Some(Arc::clone(&registry)),
            ..ProtocolContext::default()
        };
        let recording_effects = Arc::new(RecordingEffects::default());
        let effects: Arc<dyn ProtocolEffectSink> = recording_effects.clone();
        let mut connection = ProtocolConnection::new(context, effects).unwrap();
        events.lock().unwrap().clear();

        let reply = connection
            .request(&parse(b"OPTIONS * RTSP/1.0\r\nCSeq: 13\r\n\r\n"))
            .unwrap()
            .unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert_eq!(connection.connection_class(), ConnectionClass::Raop);
        assert_eq!(
            *events.lock().unwrap(),
            [(
                crate::logger::INFO,
                b"*****\"nohold\" feature: switch to new connection request from 127.0.0.1"
                    .to_vec()
            )]
        );

        old_peer
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut closed = Vec::new();
        old_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        let mut old_airplay_peer = old_airplay_peer;
        old_airplay_peer
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut airplay_closed = Vec::new();
        old_airplay_peer.read_to_end(&mut airplay_closed).unwrap();
        assert!(airplay_closed.is_empty());
        drop(old_registration);
        drop(old_airplay_registration);
        assert!(!registry.has_class(ConnectionClass::Raop));
        assert_eq!(recording_effects.3.load(Ordering::Acquire), 1);

        new_registration.set_class(connection.connection_class());
        new_peer.write_all(b"still-open").unwrap();
        let mut received = [0; 10];
        (&new_stream).read_exact(&mut received).unwrap();
        assert_eq!(&received, b"still-open");
        connection.disconnected();
    }

    fn setup_rtsp(body: &[u8], authorization: Option<&[u8]>, cseq: u8) -> HttpRequest {
        let mut header = format!(
            "SETUP / RTSP/1.0\r\nCSeq: {cseq}\r\nContent-Length: {}\r\n",
            body.len()
        )
        .into_bytes();
        if let Some(authorization) = authorization {
            header.extend_from_slice(b"Authorization: ");
            header.extend_from_slice(authorization);
            header.extend_from_slice(b"\r\n");
        }
        header.extend_from_slice(b"\r\n");
        header.extend_from_slice(body);
        parse(&header)
    }

    fn digest_header(method: &[u8], uri: &[u8], password: &[u8], nonce: &[u8]) -> Vec<u8> {
        use uxplay_crypto::port::get_md5_hex;

        let username = b"test-user";
        let realm = b"raop";
        let nc = b"00000001";
        let cnonce = b"c0ffee";
        let qop = b"auth";
        let mut raw = Vec::new();
        raw.extend_from_slice(username);
        raw.push(b':');
        raw.extend_from_slice(realm);
        raw.push(b':');
        raw.extend_from_slice(password);
        let h1 = get_md5_hex(&raw).unwrap();
        raw.clear();
        raw.extend_from_slice(method);
        raw.push(b':');
        raw.extend_from_slice(uri);
        let h2 = get_md5_hex(&raw).unwrap();
        raw.clear();
        raw.extend_from_slice(h1.as_bytes());
        raw.push(b':');
        raw.extend_from_slice(nonce);
        raw.push(b':');
        raw.extend_from_slice(nc);
        raw.push(b':');
        raw.extend_from_slice(cnonce);
        raw.push(b':');
        raw.extend_from_slice(qop);
        raw.push(b':');
        raw.extend_from_slice(h2.as_bytes());
        let response = get_md5_hex(&raw).unwrap();
        format!(
            "Digest username=\"test-user\", realm=\"raop\", nonce=\"{}\", uri=\"/\", qop=auth, nc=00000001, cnonce=\"c0ffee\", response=\"{response}\"",
            String::from_utf8_lossy(nonce)
        )
        .into_bytes()
    }

    fn post_hls_plist(path: &str, session_id: &str, value: plist::Value) -> HttpRequest {
        let mut body = Vec::new();
        value.to_writer_binary(&mut body).unwrap();
        let header = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost:7000\r\nX-Apple-Session-ID: {session_id}\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(&body);
        parse(&wire)
    }

    fn read_fcup_request(stream: &mut TcpStream) -> plist::Value {
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        assert!(header.starts_with(b"POST /event HTTP/1.1\r\n"));
        let content_length = std::str::from_utf8(&header)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut body = vec![0; content_length];
        stream.read_exact(&mut body).unwrap();
        plist::Value::from_reader_xml(std::io::Cursor::new(body)).unwrap()
    }

    fn fcup_reply(url: &str, playlist: &[u8]) -> plist::Value {
        let mut params = plist::Dictionary::new();
        params.insert(
            "FCUP_Response_URL".into(),
            plist::Value::String(url.to_owned()),
        );
        params.insert(
            "FCUP_Response_Data".into(),
            plist::Value::Data(playlist.to_vec()),
        );
        let mut root = plist::Dictionary::new();
        root.insert(
            "type".into(),
            plist::Value::String("unhandledURLResponse".into()),
        );
        root.insert("params".into(), plist::Value::Dictionary(params));
        plist::Value::Dictionary(root)
    }

    fn pairing_plist(fields: [(&str, plist::Value); 2]) -> Vec<u8> {
        let mut dictionary = plist::Dictionary::new();
        for (key, value) in fields {
            dictionary.insert(key.to_owned(), value);
        }
        let mut body = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_binary(&mut body)
            .unwrap();
        body
    }

    fn mirror_wire_packet(kind: u8, timestamp: u64, payload: &[u8]) -> Vec<u8> {
        const HEADER_LEN: usize = 128;
        let mut packet = vec![0; HEADER_LEN + payload.len()];
        packet[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        packet[4] = kind;
        packet[6] = 0x16;
        packet[8..16].copy_from_slice(&timestamp.to_le_bytes());
        for (offset, value) in [(40, 1920.0_f32), (44, 1080.0), (56, 1920.0), (60, 1080.0)] {
            packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        packet[HEADER_LEN..].copy_from_slice(payload);
        packet
    }

    fn annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
        let mut start_codes = Vec::new();
        let mut offset = 0;
        while offset + 3 <= data.len() {
            if offset + 4 <= data.len() && data[offset..offset + 4] == [0, 0, 0, 1] {
                start_codes.push((offset, 4));
                offset += 4;
            } else if data[offset..offset + 3] == [0, 0, 1] {
                start_codes.push((offset, 3));
                offset += 3;
            } else {
                offset += 1;
            }
        }
        start_codes
            .iter()
            .enumerate()
            .filter_map(|(index, &(start, prefix_len))| {
                let payload_start = start + prefix_len;
                let payload_end = start_codes
                    .get(index + 1)
                    .map_or(data.len(), |(next_start, _)| *next_start);
                (payload_start < payload_end).then_some(&data[payload_start..payload_end])
            })
            .collect()
    }

    fn encrypt_mirror_payload(audio_key: &[u8; 16], stream_id: u64, clear: &[u8]) -> Vec<u8> {
        let mut key_hash = sha512().unwrap();
        key_hash
            .update(format!("AirPlayStreamKey{stream_id}").as_bytes())
            .unwrap();
        key_hash.update(audio_key).unwrap();
        let key = key_hash.finish().unwrap();
        let mut iv_hash = sha512().unwrap();
        iv_hash
            .update(format!("AirPlayStreamIV{stream_id}").as_bytes())
            .unwrap();
        iv_hash.update(audio_key).unwrap();
        let iv = iv_hash.finish().unwrap();
        AesContext::new(AesMode::Ctr, Direction::Encrypt, &key[..16], &iv[..16])
            .unwrap()
            .ctr_encrypt(clear)
            .unwrap()
    }

    fn reply_body(reply: &HttpReply) -> &[u8] {
        let offset = reply
            .bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("RTSP reply headers")
            + 4;
        &reply.bytes[offset..]
    }

    fn sha1(bytes: &[u8]) -> Vec<u8> {
        hash(MessageDigest::sha1(), bytes).unwrap().to_vec()
    }

    fn padded_bn(number: &BigNumRef, width: usize) -> Vec<u8> {
        let bytes = number.to_vec();
        assert!(bytes.len() <= width);
        let mut padded = vec![0; width];
        padded[width - bytes.len()..].copy_from_slice(&bytes);
        padded
    }

    // A small test-only SRP-6a client for replaying the PIN handshake against the
    // app's server path. The server parameters and proof layout mirror lib/srp.c.
    fn srp_pin_client_proof(
        user: &[u8],
        pin: &[u8],
        salt: &[u8],
        server_public: &[u8],
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        const N_HEX: &str = concat!(
            "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050",
            "A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50",
            "E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8",
            "55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B",
            "CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748",
            "544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE",
            "6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB",
            "694B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73",
        );
        let n = BigNum::from_hex_str(N_HEX).unwrap();
        let g = BigNum::from_u32(2).unwrap();
        let width = n.num_bytes() as usize;
        let mut context = BigNumContext::new().unwrap();

        let private = BigNum::from_slice(&[0x5a; 32]).unwrap();
        let mut client_public = BigNum::new().unwrap();
        client_public
            .mod_exp(&g, &private, &n, &mut context)
            .unwrap();
        let server_public = BigNum::from_slice(server_public).unwrap();

        let mut k_input = padded_bn(&n, width);
        k_input.extend_from_slice(&padded_bn(&g, width));
        let multiplier = BigNum::from_slice(&sha1(&k_input)).unwrap();

        let mut identity_password = user.to_vec();
        identity_password.push(b':');
        identity_password.extend_from_slice(pin);
        let inner = sha1(&identity_password);
        let mut x_input = salt.to_vec();
        x_input.extend_from_slice(&inner);
        let x = BigNum::from_slice(&sha1(&x_input)).unwrap();

        let mut gx = BigNum::new().unwrap();
        gx.mod_exp(&g, &x, &n, &mut context).unwrap();
        let mut kgx = BigNum::new().unwrap();
        kgx.mod_mul(&multiplier, &gx, &n, &mut context).unwrap();
        let mut base = BigNum::new().unwrap();
        base.mod_sub(&server_public, &kgx, &n, &mut context)
            .unwrap();

        let mut u_input = padded_bn(&client_public, width);
        u_input.extend_from_slice(&padded_bn(&server_public, width));
        let scrambling = BigNum::from_slice(&sha1(&u_input)).unwrap();
        let mut ux = BigNum::new().unwrap();
        ux.checked_mul(&scrambling, &x, &mut context).unwrap();
        let mut exponent = BigNum::new().unwrap();
        exponent.checked_add(&private, &ux).unwrap();
        let mut shared = BigNum::new().unwrap();
        shared.mod_exp(&base, &exponent, &n, &mut context).unwrap();

        let secret = shared.to_vec();
        let mut session_key = Vec::with_capacity(40);
        for suffix in [0_u8, 1] {
            let mut input = secret.clone();
            input.extend_from_slice(&[0, 0, 0, suffix]);
            session_key.extend_from_slice(&sha1(&input));
        }

        let hash_n = sha1(&n.to_vec());
        let hash_g = sha1(&g.to_vec());
        let mut proof_input: Vec<u8> = hash_n
            .iter()
            .zip(hash_g.iter())
            .map(|(left, right)| left ^ right)
            .collect();
        proof_input.extend_from_slice(&sha1(user));
        proof_input.extend_from_slice(salt);
        proof_input.extend_from_slice(&client_public.to_vec());
        proof_input.extend_from_slice(&server_public.to_vec());
        proof_input.extend_from_slice(&session_key);
        let client_proof = sha1(&proof_input);

        let mut expected_hamk_input = client_public.to_vec();
        expected_hamk_input.extend_from_slice(&client_proof);
        expected_hamk_input.extend_from_slice(&session_key);
        let expected_hamk = sha1(&expected_hamk_input);
        (
            client_public.to_vec(),
            client_proof,
            expected_hamk,
            session_key,
        )
    }

    fn pairing_derived_key(label: &[u8], secret: &[u8]) -> [u8; 16] {
        let mut input = label.to_vec();
        input.extend_from_slice(secret);
        hash(MessageDigest::sha512(), &input).unwrap()[..16]
            .try_into()
            .unwrap()
    }

    fn post_pairing_plist(path: &str, body: &[u8], cseq: u8) -> HttpRequest {
        let header = format!(
            "POST {path} RTSP/1.0\r\nCSeq: {cseq}\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(body);
        parse(&wire)
    }

    #[test]
    fn rtsp_options_crosses_the_transport_adapter_byte_for_byte() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext::default(),
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let request = parse(b"OPTIONS * RTSP/1.0\r\nCSeq: 7\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nPublic: SETUP, RECORD, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER\r\nServer: AirTunes/220.68\r\nCSeq: 7\r\n\r\n"
        );
    }

    #[test]
    fn invalid_play_matches_source_disconnect_and_requests_server_reset() {
        let effects = Arc::new(RecordingEffects::default());
        let context = ProtocolContext {
            hls_support: true,
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, effects.clone() as Arc<dyn ProtocolEffectSink>)
                .unwrap();
        let request = parse(
            b"POST /play HTTP/1.1\r\nHost: localhost:7000\r\nX-Apple-Session-ID: session\r\n\r\n",
        );

        let reply = connection.request(&request).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"HTTP/1.1 400 Bad Request\r\n"));
        assert!(reply.disconnect);
        assert_eq!(effects.2.load(std::sync::atomic::Ordering::Acquire), 1);
    }

    #[test]
    fn handler_effects_are_delivered_before_the_reply_is_returned() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                hls_support: true,
                ..ProtocolContext::default()
            },
            effects.clone() as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let request = parse(b"POST /stop HTTP/1.1\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert_eq!(
            reply.bytes,
            b"HTTP/1.1 200 OK\r\nServer: AirTunes/220.68\r\n\r\n"
        );
        assert_eq!(
            *effects
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![HandlerEffect::AirPlayControl(AirPlayControl::Stop)]
        );
    }

    #[test]
    fn unported_stateful_routes_close_with_an_explicit_error() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext::default(),
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let request = parse(b"POST /pair-setup RTSP/1.0\r\nCSeq: 8\r\n\r\n");
        let error = connection.request(&request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("PairSetup"));
    }

    #[test]
    fn audio_mode_returns_success_and_matches_source_log_level() {
        fn run(mode: &str, cseq: u8) -> (Vec<u8>, CapturedLogs) {
            let mut dictionary = plist::Dictionary::new();
            dictionary.insert("audioMode".into(), plist::Value::String(mode.to_owned()));
            let mut body = Vec::new();
            plist::Value::Dictionary(dictionary)
                .to_writer_binary(&mut body)
                .unwrap();
            let request = post_rtsp("/audioMode", &body, cseq);
            let (logger, events) = collecting_logger(crate::logger::INFO);
            let mut context = ProtocolContext::default();
            context.media_config.logger = Some(logger);
            let effects = Arc::new(RecordingEffects::default());
            let mut connection =
                ProtocolConnection::new(context, effects as Arc<dyn ProtocolEffectSink>).unwrap();
            let reply = connection.request(&request).unwrap().unwrap();
            (reply.bytes, events)
        }

        let (default_reply, default_logs) = run("stereo-default-extra", 8);
        assert_eq!(
            default_reply,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 8\r\n\r\n"
        );
        assert!(default_logs.lock().unwrap().is_empty());

        let (other_reply, other_logs) = run("stereo", 9);
        assert_eq!(
            other_reply,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 9\r\n\r\n"
        );
        assert_eq!(
            *other_logs.lock().unwrap(),
            [b"Unhandled RTSP request \"audioMode: stereo\"".to_vec()]
        );
    }

    #[test]
    fn get_parameter_logs_unknown_lines_as_source_warning_bytes() {
        let body = b"artist\xff\r\nvolume\r\n";
        let wire = format!(
            "GET_PARAMETER /anything RTSP/1.0\r\nCSeq: 10\r\nContent-Type: text/parameters\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = wire.into_bytes();
        wire.extend_from_slice(body);
        let request = parse(&wire);
        let (logger, logs) = collecting_logger(crate::logger::WARNING);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection = ProtocolConnection::new(
            context,
            Arc::new(RecordingEffects::default()) as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert_eq!(
            *logs.lock().unwrap(),
            [b"Found an unknown parameter: artist\xff".to_vec()]
        );
    }

    #[test]
    fn set_parameter_logs_cover_art_size_and_missing_rtp_warning() {
        let body = b"abc";
        let wire = b"SET_PARAMETER /anything RTSP/1.0\r\nCSeq: 11\r\nContent-Type: image/jpeg\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(&wire[wire.len() - body.len()..], body);
        let request = parse(wire);
        let (logger, logs) = collecting_logger(crate::logger::DEBUG);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection = ProtocolConnection::new(
            context,
            Arc::new(RecordingEffects::default()) as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        let logs = logs.lock().unwrap();
        assert!(logs.windows(2).any(|messages| {
            messages
                == [
                    b"Got image data of 3 bytes".to_vec(),
                    b"RAOP not initialized at SET_PARAMETER coverart".to_vec(),
                ]
        }));
    }

    #[test]
    fn set_parameter_logs_missing_rtp_for_nonvolume_text_body() {
        let request = parse(
            b"SET_PARAMETER /anything RTSP/1.0\r\nCSeq: 12\r\nContent-Type: text/parameters\r\nContent-Length: 17\r\n\r\nprogress: 1/2/3\r\n",
        );
        let (logger, logs) = collecting_logger(crate::logger::WARNING);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection = ProtocolConnection::new(
            context,
            Arc::new(RecordingEffects::default()) as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert_eq!(
            *logs.lock().unwrap(),
            [b"RAOP not initialized at SET_PARAMETER".to_vec()]
        );
    }

    #[test]
    fn pair_setup_matches_source_length_rules_and_changes_connection_state() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection =
            ProtocolConnection::new(pairing_context(), effects as Arc<dyn ProtocolEffectSink>)
                .unwrap();

        let invalid = post_rtsp("/pair-setup", &[0; 31], 1);
        let invalid_reply = connection.request(&invalid).unwrap().unwrap();
        assert_eq!(
            invalid_reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 1\r\n\r\n"
        );
        assert_eq!(
            connection.pairing_session.as_ref().unwrap().status(),
            PairingStatus::Initial
        );

        let valid = post_rtsp("/pair-setup", &[0; 32], 2);
        let valid_reply = connection.request(&valid).unwrap().unwrap();
        assert!(valid_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            valid_reply
                .bytes
                .windows(b"Content-Type: application/octet-stream".len())
                .any(|w| w == b"Content-Type: application/octet-stream")
        );
        assert!(
            valid_reply
                .bytes
                .windows(b"Content-Length: 32".len())
                .any(|w| w == b"Content-Length: 32")
        );
        let body_offset = valid_reply
            .bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert_eq!(valid_reply.bytes.len() - body_offset, 32);
        assert_eq!(
            connection.pairing_session.as_ref().unwrap().status(),
            PairingStatus::Setup
        );
    }

    #[test]
    fn pair_verify_runs_both_steps_and_closes_on_bad_client_signature() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection =
            ProtocolConnection::new(pairing_context(), effects as Arc<dyn ProtocolEffectSink>)
                .unwrap();
        let setup = post_rtsp("/pair-setup", &[0; 32], 1);
        connection.request(&setup).unwrap().unwrap();

        let client_ecdh = X25519Key::generate().unwrap();
        let client_ed = Ed25519Key::generate_random().unwrap();
        let mut first_body = [0_u8; 68];
        first_body[0] = 1;
        first_body[4..36].copy_from_slice(&client_ecdh.raw_public().unwrap());
        first_body[36..68].copy_from_slice(&client_ed.raw_public().unwrap());
        let first = post_rtsp("/pair-verify", &first_body, 2);
        let first_reply = connection.request(&first).unwrap().unwrap();
        assert!(first_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            first_reply
                .bytes
                .windows(b"Content-Length: 96".len())
                .any(|w| w == b"Content-Length: 96")
        );
        let first_body_offset = first_reply
            .bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert_eq!(first_reply.bytes.len() - first_body_offset, 96);
        assert_eq!(
            connection.pairing_session.as_ref().unwrap().status(),
            PairingStatus::Handshake
        );

        let mut second_body = [0_u8; 68];
        second_body[0] = 0;
        let second = post_rtsp("/pair-verify", &second_body, 3);
        let second_reply = connection.request(&second).unwrap().unwrap();
        assert!(second_reply.disconnect);
        assert_eq!(
            second_reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 3\r\n\r\n"
        );
    }

    #[test]
    fn pin_mode_pair_verify_requires_a_registered_client_key() {
        let effects = Arc::new(RecordingEffects::default());
        let pairing = Arc::new(
            Pairing::init_generate(b"18:3d:2d:c2:cc:c5", None)
                .unwrap()
                .0,
        );
        let client_ecdh = X25519Key::generate().unwrap();
        let client_ed = Ed25519Key::generate_random().unwrap();
        let client_ed_public = client_ed.raw_public().unwrap();
        let mut first_body = [0_u8; 68];
        first_body[0] = 1;
        first_body[4..36].copy_from_slice(&client_ecdh.raw_public().unwrap());
        first_body[36..68].copy_from_slice(&client_ed_public);
        let request = post_rtsp("/pair-verify", &first_body, 4);

        let path = temp_registration_file();
        let registered_key = ed25519_pk_to_base64(&client_ed_public);
        crate::pairing_registry::PairingRegistry::load(true, true, Some(path.clone())).register(
            &registered_key,
            "client-id",
            "Client",
        );
        let registered = Arc::new(crate::pairing_registry::PairingRegistry::load(
            true,
            true,
            Some(path.clone()),
        ));
        assert!(registered.contains(&registered_key));
        let accepted_context = ProtocolContext {
            pairing: Some(Arc::clone(&pairing)),
            registration: Some(registered),
            pin_mode: true,
            ..ProtocolContext::default()
        };
        let mut accepted = ProtocolConnection::new(
            accepted_context,
            effects.clone() as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let accepted_reply = accepted.request(&request).unwrap().unwrap();
        assert!(accepted_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            accepted_reply
                .bytes
                .windows(b"Content-Length: 96".len())
                .any(|window| { window == b"Content-Length: 96" })
        );

        let empty_registry = Arc::new(crate::pairing_registry::PairingRegistry::load(
            true, true, None,
        ));
        let rejected_context = ProtocolContext {
            pairing: Some(pairing),
            registration: Some(empty_registry),
            pin_mode: true,
            ..ProtocolContext::default()
        };
        let mut rejected =
            ProtocolConnection::new(rejected_context, effects as Arc<dyn ProtocolEffectSink>)
                .unwrap();
        let rejected_reply = rejected.request(&request).unwrap().unwrap();
        assert_eq!(
            rejected_reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 4\r\n\r\n"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pin_mode_pair_verify_after_pair_setup_skips_initial_registration_check() {
        let effects = Arc::new(RecordingEffects::default());
        let pairing = Arc::new(
            Pairing::init_generate(b"18:3d:2d:c2:cc:c5", None)
                .unwrap()
                .0,
        );
        let empty_registry = Arc::new(crate::pairing_registry::PairingRegistry::load(
            true, true, None,
        ));
        let context = ProtocolContext {
            pairing: Some(pairing),
            registration: Some(empty_registry),
            pin_mode: true,
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, effects as Arc<dyn ProtocolEffectSink>).unwrap();
        connection
            .pairing_session
            .as_mut()
            .unwrap()
            .set_setup_status();

        let client_ecdh = X25519Key::generate().unwrap();
        let client_ed = Ed25519Key::generate_random().unwrap();
        let mut body = [0_u8; 68];
        body[0] = 1;
        body[4..36].copy_from_slice(&client_ecdh.raw_public().unwrap());
        body[36..68].copy_from_slice(&client_ed.raw_public().unwrap());
        let request = post_rtsp("/pair-verify", &body, 5);

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            reply
                .bytes
                .windows(b"Content-Length: 96".len())
                .any(|window| window == b"Content-Length: 96")
        );
        assert_eq!(
            connection.pairing_session.as_ref().unwrap().status(),
            PairingStatus::Handshake
        );
    }

    #[test]
    fn pair_pin_start_displays_the_configured_four_digit_pin() {
        let effects = Arc::new(RecordingEffects::default());
        let context = ProtocolContext {
            pairing_pin: Arc::new(Mutex::new(10_042)),
            ..pairing_context()
        };
        let mut connection =
            ProtocolConnection::new(context, effects.clone() as Arc<dyn ProtocolEffectSink>)
                .unwrap();
        let request = parse(b"POST /pair-pin-start RTSP/1.0\r\nCSeq: 6\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 6\r\n\r\n"
        );
        assert_eq!(
            *effects
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![HandlerEffect::DisplayPairingPin("0042".into())]
        );
    }

    #[test]
    fn pair_setup_pin_issues_an_srp_offer_from_a_binary_plist() {
        let effects = Arc::new(RecordingEffects::default());
        let context = ProtocolContext {
            pairing_pin: Arc::new(Mutex::new(10_042)),
            ..pairing_context()
        };
        let mut connection =
            ProtocolConnection::new(context, effects as Arc<dyn ProtocolEffectSink>).unwrap();
        let body = pairing_plist([
            ("method", plist::Value::String("pin".into())),
            ("user", plist::Value::String("client-device-id".into())),
        ]);
        let header = format!(
            "POST /pair-setup-pin RTSP/1.0\r\nCSeq: 7\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(&body);
        let request = parse(&wire);
        let reply = connection.request(&request).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            reply
                .bytes
                .windows(b"Content-Type: application/x-apple-binary-plist".len())
                .any(|window| window == b"Content-Type: application/x-apple-binary-plist")
        );
        let body_offset = reply
            .bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let response =
            plist::Value::from_reader(std::io::Cursor::new(&reply.bytes[body_offset..])).unwrap();
        let response = response.as_dictionary().unwrap();
        assert!(!response.get("pk").unwrap().as_data().unwrap().is_empty());
        assert!(!response.get("salt").unwrap().as_data().unwrap().is_empty());
    }

    #[test]
    fn pin_pairing_completes_crypto_setup_and_encrypted_audio_video_delivery() {
        let _renderer_guard = crate::raop_media_runtime::VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        const CLIENT_ID: &str = "client-device-123";
        const CLIENT_NAME: &str = "Rust Pairing Test";
        const PIN: &[u8] = b"0042";

        let effects = Arc::new(RecordingEffects::default());
        let audio_dump_prefix = std::env::temp_dir().join(format!(
            "uxplay-pairing-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let video_dump_prefix = std::env::temp_dir().join(format!(
            "uxplay-pairing-video-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dump_options = crate::options::Options {
            dump_audio: true,
            audio_dump_filename: audio_dump_prefix.to_string_lossy().into_owned(),
            dump_video: true,
            video_dump_filename: video_dump_prefix.to_string_lossy().into_owned(),
            use_audio: false,
            video_sink: "fakesink".into(),
            video_parser: "h264parse".into(),
            video_decoder: "avdec_h264".into(),
            video_converter: "videoconvert".into(),
            video_sync: false,
            ..crate::options::Options::default()
        };
        let logger = Arc::new(crate::logger::Logger::new());
        logger.set_level(crate::logger::DEBUG);
        let renderer_logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_logs = Arc::clone(&renderer_logs);
        logger.set_callback(Some(Arc::new(move |_level, message| {
            captured_logs
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(message).into_owned());
        })));
        let mut renderer_loop =
            crate::raop_media_runtime::RendererEventLoop::start(&dump_options, &logger).unwrap();
        let audio_outputs = Arc::new(Mutex::new(
            crate::raop_media_runtime::MediaOutputRuntime::new(&dump_options).unwrap(),
        ));
        let media_config = SessionMediaConfig {
            use_audio: false,
            use_video: true,
            outputs: Arc::clone(&audio_outputs),
            logger: Some(Arc::clone(&logger)),
            ..SessionMediaConfig::default()
        };
        let pairing = Arc::new(
            Pairing::init_generate(b"18:3d:2d:c2:cc:c5", None)
                .unwrap()
                .0,
        );
        let server_ed_public =
            Ed25519Key::from_raw_public(&pairing.get_public_key().unwrap()).unwrap();
        let pairing_pin = Arc::new(Mutex::new(10_042));
        let context = ProtocolContext {
            pairing: Some(Arc::clone(&pairing)),
            registration: Some(Arc::new(crate::pairing_registry::PairingRegistry::load(
                true, true, None,
            ))),
            pin_mode: true,
            pairing_pin: Arc::clone(&pairing_pin),
            media_config,
            setup_decisions: SetupDecisions {
                callbacks: uxplay_raop_session::CallbackAvailability {
                    report_client_request: true,
                    register_client: true,
                    audio_get_format: false,
                },
                ..SetupDecisions::allow_all()
            },
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, effects.clone() as Arc<dyn ProtocolEffectSink>)
                .unwrap();

        // The C API has a source-compatible oddity when a random salt has a
        // shorter byte width: it returns the leading slice of its padded buffer.
        // Retry the offer in that astronomically rare case so this valid-client
        // transcript exercises the normal sixteen-byte wire salt.
        let mut completed = false;
        for attempt in 0..16 {
            *pairing_pin.lock().unwrap() = 10_042;
            let offer_body = pairing_plist([
                ("method", plist::Value::String("pin".into())),
                ("user", plist::Value::String(CLIENT_ID.into())),
            ]);
            let offer_request = post_pairing_plist("/pair-setup-pin", &offer_body, 20 + attempt);
            let offer_reply = connection.request(&offer_request).unwrap().unwrap();
            assert!(offer_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            let offer =
                plist::Value::from_reader(std::io::Cursor::new(reply_body(&offer_reply))).unwrap();
            let offer = offer.as_dictionary().unwrap();
            let server_public = offer["pk"].as_data().unwrap();
            let salt = offer["salt"].as_data().unwrap();
            if salt.len() != 16 {
                continue;
            }

            let (client_public, client_proof, expected_hamk, session_key) =
                srp_pin_client_proof(CLIENT_ID.as_bytes(), PIN, salt, server_public);
            let proof_body = pairing_plist([
                ("pk", plist::Value::Data(client_public)),
                ("proof", plist::Value::Data(client_proof)),
            ]);
            let proof_reply = connection
                .request(&post_pairing_plist(
                    "/pair-setup-pin",
                    &proof_body,
                    40 + attempt,
                ))
                .unwrap()
                .unwrap();
            assert!(proof_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            let proof_response =
                plist::Value::from_reader(std::io::Cursor::new(reply_body(&proof_reply))).unwrap();
            assert_eq!(
                proof_response.as_dictionary().unwrap()["proof"].as_data(),
                Some(expected_hamk.as_slice())
            );

            let setup_key = pairing_derived_key(b"Pair-Setup-AES-Key", &session_key);
            let mut setup_iv = pairing_derived_key(b"Pair-Setup-AES-IV", &session_key);
            setup_iv[15] = setup_iv[15].wrapping_add(1);
            let client_ed = Ed25519Key::from_raw_private(&[0x11; 32]).unwrap();
            let client_ed_public = client_ed.raw_public().unwrap();
            let (encrypted_client_key, client_tag) =
                gcm_encrypt(&client_ed_public, &setup_key, &setup_iv).unwrap();
            let confirm_body = pairing_plist([
                ("epk", plist::Value::Data(encrypted_client_key)),
                ("authTag", plist::Value::Data(client_tag.to_vec())),
            ]);
            let confirm_reply = connection
                .request(&post_pairing_plist(
                    "/pair-setup-pin",
                    &confirm_body,
                    60 + attempt,
                ))
                .unwrap()
                .unwrap();
            assert!(confirm_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            let confirm =
                plist::Value::from_reader(std::io::Cursor::new(reply_body(&confirm_reply)))
                    .unwrap();
            let confirm = confirm.as_dictionary().unwrap();
            let mut server_iv = setup_iv;
            server_iv[15] = server_iv[15].wrapping_add(1);
            let server_key = gcm_decrypt(
                confirm["epk"].as_data().unwrap(),
                &setup_key,
                &server_iv,
                confirm["authTag"].as_data().unwrap(),
            )
            .unwrap();
            assert!(server_key.authenticated);
            assert_eq!(server_key.plaintext, pairing.get_public_key().unwrap());
            assert_eq!(
                connection.pairing_session.as_ref().unwrap().status(),
                PairingStatus::Setup
            );

            let client_ecdh = X25519Key::generate().unwrap();
            let client_ecdh_public = client_ecdh.raw_public().unwrap();
            let mut verify_start = [0; 68];
            verify_start[0] = 1;
            verify_start[4..36].copy_from_slice(&client_ecdh_public);
            verify_start[36..68].copy_from_slice(&client_ed_public);
            let verify_start_reply = connection
                .request(&post_rtsp("/pair-verify", &verify_start, 80 + attempt))
                .unwrap()
                .unwrap();
            assert!(verify_start_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            let verify_reply = reply_body(&verify_start_reply);
            assert_eq!(verify_reply.len(), 96);
            let server_ecdh_public: [u8; 32] = verify_reply[..32].try_into().unwrap();
            let server_ecdh = X25519Key::from_raw_public(&server_ecdh_public).unwrap();
            let shared = client_ecdh.derive(&server_ecdh).unwrap();
            let verify_key = pairing_derived_key(b"Pair-Verify-AES-Key", &shared);
            let verify_iv = pairing_derived_key(b"Pair-Verify-AES-IV", &shared);

            let mut server_ctr =
                AesContext::new(AesMode::Ctr, Direction::Decrypt, &verify_key, &verify_iv).unwrap();
            let server_signature = server_ctr.ctr_decrypt(&verify_reply[32..]).unwrap();
            let mut server_message = [0; 64];
            server_message[..32].copy_from_slice(&server_ecdh_public);
            server_message[32..].copy_from_slice(&client_ecdh_public);
            assert!(
                server_ed_public
                    .verify(&server_message, &server_signature)
                    .unwrap()
            );

            let mut client_message = [0; 64];
            client_message[..32].copy_from_slice(&client_ecdh_public);
            client_message[32..].copy_from_slice(&server_ecdh_public);
            let client_signature = client_ed.sign(&client_message).unwrap();
            let mut client_ctr =
                AesContext::new(AesMode::Ctr, Direction::Encrypt, &verify_key, &verify_iv).unwrap();
            let _source_fake_round = client_ctr.ctr_encrypt(&[0; 64]).unwrap();
            let encrypted_signature = client_ctr.ctr_encrypt(&client_signature).unwrap();
            let mut verify_finish = [0; 68];
            verify_finish[4..].copy_from_slice(&encrypted_signature);
            let verify_finish_reply = connection
                .request(&post_rtsp("/pair-verify", &verify_finish, 100 + attempt))
                .unwrap()
                .unwrap();
            assert!(!verify_finish_reply.disconnect);
            assert!(
                verify_finish_reply
                    .bytes
                    .starts_with(b"RTSP/1.0 200 OK\r\n")
            );
            assert_eq!(
                connection.pairing_session.as_ref().unwrap().status(),
                PairingStatus::Finished
            );

            let mut fairplay_setup = [0; 16];
            fairplay_setup[4] = 3;
            fairplay_setup[14] = 2;
            let fairplay_setup_reply = connection
                .request(&post_rtsp("/fp-setup", &fairplay_setup, 110 + attempt))
                .unwrap()
                .unwrap();
            assert!(
                fairplay_setup_reply
                    .bytes
                    .starts_with(b"RTSP/1.0 200 OK\r\n")
            );
            assert_eq!(reply_body(&fairplay_setup_reply).len(), 142);

            let mut fairplay_handshake = [0; 164];
            for (index, byte) in fairplay_handshake.iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(17).wrapping_add(6);
            }
            fairplay_handshake[4] = 3;
            fairplay_handshake[12] = 2;
            let fairplay_handshake_reply = connection
                .request(&post_rtsp("/fp-setup", &fairplay_handshake, 111 + attempt))
                .unwrap()
                .unwrap();
            assert!(
                fairplay_handshake_reply
                    .bytes
                    .starts_with(b"RTSP/1.0 200 OK\r\n")
            );
            assert_eq!(reply_body(&fairplay_handshake_reply).len(), 32);

            let mut encrypted_audio_key = [0; 72];
            for (index, byte) in encrypted_audio_key.iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(29).wrapping_add(14);
            }
            let mut fairplay_audio_key = [0; 16];
            assert_eq!(
                connection
                    .fairplay
                    .decrypt(&encrypted_audio_key, &mut fairplay_audio_key),
                0
            );
            assert_eq!(
                fairplay_audio_key,
                [
                    0x8d, 0xcb, 0xe9, 0x5f, 0xd7, 0x58, 0x4a, 0x59, 0xc9, 0xf7, 0xc7, 0x55, 0x02,
                    0x70, 0xd8, 0x53,
                ]
            );
            let mut ecdh_secret = [0; 32];
            assert_eq!(
                connection
                    .pairing_session
                    .as_ref()
                    .unwrap()
                    .get_ecdh_secret_key(&mut ecdh_secret),
                1
            );
            let mut key_material = fairplay_audio_key.to_vec();
            key_material.extend_from_slice(&ecdh_secret);
            let expected_audio_key: [u8; 16] = hash(MessageDigest::sha256(), &key_material)
                .unwrap()[..16]
                .try_into()
                .unwrap();
            let audio_iv: Vec<u8> = (0..16).collect();

            let mut setup = plist::Dictionary::new();
            setup.insert("deviceID".into(), plist::Value::String(CLIENT_ID.into()));
            setup.insert("model".into(), plist::Value::String("Test Model".into()));
            setup.insert("name".into(), plist::Value::String(CLIENT_NAME.into()));
            setup.insert(
                "ekey".into(),
                plist::Value::Data(encrypted_audio_key.to_vec()),
            );
            setup.insert("eiv".into(), plist::Value::Data(audio_iv.clone()));
            let mut audio_stream = plist::Dictionary::new();
            audio_stream.insert("type".into(), plist::Value::from(96));
            audio_stream.insert("ct".into(), plist::Value::from(2));
            audio_stream.insert("spf".into(), plist::Value::from(352));
            audio_stream.insert("controlPort".into(), plist::Value::from(0));
            audio_stream.insert("usingScreen".into(), plist::Value::Boolean(false));
            audio_stream.insert("isMedia".into(), plist::Value::Boolean(true));
            audio_stream.insert("audioFormat".into(), plist::Value::from(0_u64));
            const MIRROR_STREAM_ID: u64 = 0x1020_3040_5060_7080;
            let mut mirror_stream = plist::Dictionary::new();
            mirror_stream.insert("type".into(), plist::Value::from(110));
            mirror_stream.insert(
                "streamConnectionID".into(),
                plist::Value::from(MIRROR_STREAM_ID),
            );
            setup.insert(
                "streams".into(),
                plist::Value::Array(vec![
                    plist::Value::Dictionary(audio_stream),
                    plist::Value::Dictionary(mirror_stream),
                ]),
            );
            let mut setup_body = Vec::new();
            plist::Value::Dictionary(setup)
                .to_writer_binary(&mut setup_body)
                .unwrap();
            let setup_reply = connection
                .request(&setup_rtsp(&setup_body, None, 120 + attempt))
                .unwrap()
                .unwrap();
            assert!(setup_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            assert_eq!(connection.raop_state.audio_key, Some(expected_audio_key));
            assert_eq!(
                connection.raop_state.audio_iv,
                Some(audio_iv.as_slice().try_into().unwrap())
            );
            assert!(connection.raop_state.audio_running);
            assert!(connection.raop_state.mirror_running);
            let setup_response =
                plist::Value::from_reader(std::io::Cursor::new(reply_body(&setup_reply))).unwrap();
            let setup_streams = setup_response.as_dictionary().unwrap()["streams"]
                .as_array()
                .unwrap();
            let audio_setup = setup_streams[0].as_dictionary().unwrap();
            let mirror_setup = setup_streams[1].as_dictionary().unwrap();
            let control_port = audio_setup["controlPort"].as_unsigned_integer().unwrap() as u16;
            let data_port = audio_setup["dataPort"].as_unsigned_integer().unwrap() as u16;
            let mirror_port = mirror_setup["dataPort"].as_unsigned_integer().unwrap() as u16;
            assert_ne!(control_port, 0);
            assert_ne!(data_port, 0);
            assert_ne!(mirror_port, 0);
            assert_eq!(mirror_setup["type"].as_unsigned_integer(), Some(110));
            assert!(effects.1.lock().unwrap().iter().any(|effect| matches!(
                effect,
                SessionEffect::RegisterClient {
                    device_id,
                    public_key_base64,
                    name,
                } if device_id == CLIENT_ID
                    && public_key_base64 == &ed25519_pk_to_base64(&client_ed_public)
                    && name == CLIENT_NAME
            )));

            let record_reply = connection
                .request(&parse(b"RECORD / RTSP/1.0\r\nCSeq: 121\r\n\r\n"))
                .unwrap()
                .unwrap();
            assert!(record_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
            let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let rtp_timestamp = 0x1020_3040_u32;
            let mut sync_packet = [0; 20];
            sync_packet[0] = 0x80;
            sync_packet[1] = 0xd4;
            sync_packet[2..4].copy_from_slice(&4_u16.to_be_bytes());
            sync_packet[4..8].copy_from_slice(&rtp_timestamp.to_be_bytes());
            sync_packet[8..16].copy_from_slice(&2_000_000_000_u64.to_be_bytes());
            sync_packet[16..20].copy_from_slice(&rtp_timestamp.wrapping_add(352).to_be_bytes());
            sender
                .send_to(&sync_packet, (Ipv4Addr::LOCALHOST, control_port))
                .unwrap();
            thread::sleep(Duration::from_millis(10));

            let clear_audio = [
                0x20, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ];
            let mut audio_cipher = AesContext::new(
                AesMode::Cbc,
                Direction::Encrypt,
                &expected_audio_key,
                &audio_iv,
            )
            .unwrap();
            let mut encrypted_payload = audio_cipher
                .cbc_crypt(&clear_audio, Direction::Encrypt)
                .unwrap();
            encrypted_payload.extend_from_slice(b"tail");
            let mut audio_packet = vec![0x80, 0x60];
            audio_packet.extend_from_slice(&29_u16.to_be_bytes());
            audio_packet.extend_from_slice(&rtp_timestamp.to_be_bytes());
            audio_packet.extend_from_slice(&[0; 4]);
            audio_packet.extend_from_slice(&encrypted_payload);
            sender
                .send_to(&audio_packet, (Ipv4Addr::LOCALHOST, data_port))
                .unwrap();

            let dump_path =
                std::path::PathBuf::from(format!("{}.1.alac", audio_dump_prefix.display()));
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !dump_path.exists() && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(
                dump_path.exists(),
                "RAOP RTP callback did not write a frame"
            );

            let mut mirror_client = TcpStream::connect((Ipv4Addr::LOCALHOST, mirror_port)).unwrap();
            let h264_sample =
                include_bytes!("../../render/video-renderer/testdata/hls/mirror-frame.h264");
            let nalus = annex_b_nalus(h264_sample);
            let sps = nalus
                .iter()
                .copied()
                .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 7))
                .expect("H.264 fixture SPS");
            let pps = nalus
                .iter()
                .copied()
                .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 8))
                .expect("H.264 fixture PPS");
            let idr = nalus
                .iter()
                .copied()
                .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 5))
                .expect("H.264 fixture IDR frame");
            let mut codec_payload = vec![0; 6];
            codec_payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            codec_payload.extend_from_slice(sps);
            codec_payload.push(1);
            codec_payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            codec_payload.extend_from_slice(pps);
            let mirror_timestamp = 2_000_000_000_u64;
            let codec_packet = mirror_wire_packet(1, mirror_timestamp, &codec_payload);
            let mut clear_video = (idr.len() as u32).to_be_bytes().to_vec();
            clear_video.extend_from_slice(idr);
            let encrypted_video =
                encrypt_mirror_payload(&expected_audio_key, MIRROR_STREAM_ID, &clear_video);
            let frame_packet = mirror_wire_packet(0, mirror_timestamp, &encrypted_video);
            for packet in [&codec_packet, &frame_packet] {
                for fragment in packet.chunks(37) {
                    mirror_client.write_all(fragment).unwrap();
                }
            }
            let video_dump_path =
                std::path::PathBuf::from(format!("{}.h264", video_dump_prefix.display()));
            let expected_video_frame = [
                &[0, 0, 0, 1][..],
                sps,
                &[0, 0, 0, 1][..],
                pps,
                &[0, 0, 0, 1][..],
                idr,
            ]
            .concat();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if std::fs::read(&video_dump_path)
                    .is_ok_and(|bytes| bytes.len() >= expected_video_frame.len())
                {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "mirror frame callback did not write a video dump"
                );
                thread::sleep(Duration::from_millis(5));
            }
            let renderer_activated = video_renderer_rs::port::video_renderer_has_active_renderer();

            let flush_reply = connection
                .request(&parse(
                    b"FLUSH / RTSP/1.0\r\nCSeq: 122\r\nRTP-Info: seq=30\r\n\r\n",
                ))
                .unwrap()
                .unwrap();
            assert!(flush_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));

            let mut teardown = plist::Dictionary::new();
            let mut teardown_mirror = plist::Dictionary::new();
            teardown_mirror.insert("type".into(), plist::Value::from(110));
            teardown.insert(
                "streams".into(),
                plist::Value::Array(vec![plist::Value::Dictionary(teardown_mirror)]),
            );
            let mut teardown_body = Vec::new();
            plist::Value::Dictionary(teardown)
                .to_writer_binary(&mut teardown_body)
                .unwrap();
            let mut teardown_wire = format!(
                "TEARDOWN / RTSP/1.0\r\nCSeq: 123\r\nContent-Length: {}\r\n\r\n",
                teardown_body.len()
            )
            .into_bytes();
            teardown_wire.extend_from_slice(&teardown_body);
            let teardown_reply = connection.request(&parse(&teardown_wire)).unwrap().unwrap();
            assert!(teardown_reply.disconnect);
            assert!(!connection.raop_state.mirror_running);
            connection.disconnected();
            lock(&audio_outputs).shutdown();
            assert_eq!(
                std::fs::read(&dump_path).unwrap(),
                [clear_audio.as_slice(), b"tail"].concat()
            );
            assert_eq!(
                std::fs::read(&video_dump_path).unwrap(),
                [expected_video_frame.as_slice(), &[0, 0, 0, 1]].concat()
            );
            std::fs::remove_file(dump_path).unwrap();
            std::fs::remove_file(video_dump_path).unwrap();
            renderer_loop.stop();
            let renderer_logs = renderer_logs.lock().unwrap();
            let h264_logs = renderer_logs
                .iter()
                .filter(|message| message.contains("video") || message.contains("GStreamer"))
                .collect::<Vec<_>>();
            assert!(
                renderer_activated,
                "mirror codec did not activate a renderer: {h264_logs:?}"
            );
            assert!(
                h264_logs
                    .iter()
                    .any(|message| message.contains("Begin streaming to GStreamer video pipeline")),
                "mirror frame was not submitted to GStreamer: {h264_logs:?}"
            );
            assert!(
                !h264_logs
                    .iter()
                    .any(|message| message.contains("GStreamer error (video)"))
            );
            completed = true;
            break;
        }

        assert!(completed, "could not get a full-width 128-bit SRP salt");
    }

    #[test]
    fn pair_setup_pin_finds_the_plist_marker_anywhere_in_the_header_string() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection =
            ProtocolConnection::new(pairing_context(), effects as Arc<dyn ProtocolEffectSink>)
                .unwrap();
        let body = pairing_plist([
            ("method", plist::Value::String("pin".into())),
            ("user", plist::Value::String("client-device-id".into())),
        ]);
        let header = format!(
            "POST /pair-setup-pin RTSP/1.0\r\nCSeq: 8\r\nX-Source-Quirk: application/apple-binary-plist\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(&body);
        let request = parse(&wire);
        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        let body_offset = reply
            .bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        let response =
            plist::Value::from_reader(std::io::Cursor::new(&reply.bytes[body_offset..])).unwrap();
        let response = response.as_dictionary().unwrap();
        assert!(response.get("pk").unwrap().as_data().is_some());
        assert!(response.get("salt").unwrap().as_data().is_some());
    }

    #[test]
    fn fairplay_setup_is_connection_stateful_and_matches_response_lengths() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext::default(),
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let invalid = parse(b"POST /fp-setup RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n");
        let invalid_reply = connection.request(&invalid).unwrap().unwrap();
        assert!(
            invalid_reply
                .bytes
                .starts_with(b"RTSP/1.0 400 Bad Request\r\n")
        );

        let mut setup_body = [0_u8; 16];
        setup_body[4] = 3;
        setup_body[14] = 0;
        let mut wire = b"POST /fp-setup RTSP/1.0\r\nCSeq: 2\r\nContent-Length: 16\r\n\r\n".to_vec();
        wire.extend_from_slice(&setup_body);
        let setup = parse(&wire);
        let setup_reply = connection.request(&setup).unwrap().unwrap();
        assert!(setup_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            setup_reply
                .bytes
                .windows(b"Content-Type: application/octet-stream".len())
                .any(|window| window == b"Content-Type: application/octet-stream")
        );
        assert!(
            setup_reply
                .bytes
                .windows(19)
                .any(|window| window == b"Content-Length: 142")
        );
        assert_eq!(
            setup_reply.bytes.len()
                - setup_reply
                    .bytes
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .unwrap()
                - 4,
            142
        );

        let mut handshake_body = [0_u8; 164];
        handshake_body[4] = 3;
        handshake_body[12] = 0;
        let mut wire =
            b"POST /fp-setup RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 164\r\n\r\n".to_vec();
        wire.extend_from_slice(&handshake_body);
        let handshake = parse(&wire);
        let handshake_reply = connection.request(&handshake).unwrap().unwrap();
        assert!(handshake_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(
            handshake_reply
                .bytes
                .windows(18)
                .any(|window| window == b"Content-Length: 32")
        );
        assert_eq!(
            handshake_reply.bytes.len()
                - handshake_reply
                    .bytes
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .unwrap()
                - 4,
            32
        );
    }

    #[test]
    fn raop_record_flush_and_teardown_use_connection_state_and_effect_order() {
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                audio_delay_micros: 1_000_000,
                ..ProtocolContext::default()
            },
            effects.clone() as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let record = parse(b"RECORD / RTSP/1.0\r\nCSeq: 4\r\n\r\n");
        let record_reply = connection.request(&record).unwrap().unwrap();
        assert_eq!(
            record_reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Latency: 44100\r\nAudio-Jack-Status: connected; type=analog\r\nServer: AirTunes/220.68\r\nCSeq: 4\r\n\r\n"
        );

        connection.raop_state.audio_exists = true;
        let flush = parse(b"FLUSH / RTSP/1.0\r\nCSeq: 5\r\nRTP-Info: seq=42\r\n\r\n");
        let flush_reply = connection.request(&flush).unwrap().unwrap();
        assert!(flush_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert_eq!(
            *effects
                .1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Vec::<SessionEffect>::new()
        );

        let teardown = parse(b"TEARDOWN / RTSP/1.0\r\nCSeq: 6\r\nContent-Length: 0\r\n\r\n");
        let teardown_reply = connection.request(&teardown).unwrap().unwrap();
        assert!(teardown_reply.disconnect);
        assert_eq!(
            teardown_reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nConnection: close\r\nServer: AirTunes/220.68\r\nCSeq: 6\r\n\r\n"
        );
        assert_eq!(
            *effects
                .1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Vec::<SessionEffect>::new()
        );
    }

    #[test]
    fn raop_record_uses_the_source_default_quarter_second_audio_delay() {
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(ProtocolContext::default(), effects).unwrap();
        let request = parse(b"RECORD / RTSP/1.0\r\nCSeq: 14\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Latency: 11025\r\nAudio-Jack-Status: connected; type=analog\r\nServer: AirTunes/220.68\r\nCSeq: 14\r\n\r\n"
        );
    }

    #[test]
    fn raop_feedback_dispatch_applies_watchdog_reset_effect() {
        let recording_effects = Arc::new(RecordingEffects::default());
        let effects: Arc<dyn ProtocolEffectSink> = recording_effects.clone();
        let mut connection = ProtocolConnection::new(ProtocolContext::default(), effects).unwrap();
        let request = parse(b"POST /feedback RTSP/1.0\r\nCSeq: 15\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert_eq!(
            reply.bytes,
            b"RTSP/1.0 200 OK\r\nAudio-Jack-Status: connected; type=digital\r\nServer: AirTunes/220.68\r\nCSeq: 15\r\n\r\n"
        );
        assert_eq!(
            *recording_effects
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [HandlerEffect::RaopFeedback]
        );
    }

    #[test]
    fn raop_feedback_record_and_flush_logs_match_source_categories() {
        let (logger, logs) = collecting_logger(crate::logger::DEBUG);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection = ProtocolConnection::new(
            context,
            Arc::new(RecordingEffects::default()) as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        for request in [
            parse(b"RECORD / RTSP/1.0\r\nCSeq: 16\r\n\r\n"),
            parse(b"FLUSH / RTSP/1.0\r\nCSeq: 17\r\nRTP-Info: seq=42\r\n\r\n"),
            parse(b"POST /feedback RTSP/1.0\r\nCSeq: 18\r\n\r\n"),
        ] {
            connection.request(&request).unwrap().unwrap();
        }

        let expected = [
            b"raop_handler_record".to_vec(),
            b"Flush with RTP-Info: seq=42".to_vec(),
            b"RAOP not initialized at FLUSH".to_vec(),
            b"raop_handler_feedback".to_vec(),
        ];
        let logs = logs.lock().unwrap();
        let actual = logs
            .iter()
            .filter(|message| expected.contains(message))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn raop_teardown_logs_audio_and_mirror_stream_flags() {
        let mut root = plist::Dictionary::new();
        root.insert(
            "streams".into(),
            plist::Value::Array(
                [96_u64, 110]
                    .into_iter()
                    .map(|kind| {
                        let mut stream = plist::Dictionary::new();
                        stream.insert("type".into(), plist::Value::from(kind));
                        plist::Value::Dictionary(stream)
                    })
                    .collect(),
            ),
        );
        let mut body = Vec::new();
        plist::Value::Dictionary(root)
            .to_writer_binary(&mut body)
            .unwrap();
        let header = format!(
            "TEARDOWN / RTSP/1.0\r\nCSeq: 19\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut wire = header.into_bytes();
        wire.extend_from_slice(&body);
        let request = parse(&wire);
        let (logger, logs) = collecting_logger(crate::logger::DEBUG);
        let mut context = ProtocolContext::default();
        context.media_config.logger = Some(logger);
        let mut connection = ProtocolConnection::new(
            context,
            Arc::new(RecordingEffects::default()) as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.disconnect);
        assert!(
            logs.lock()
                .unwrap()
                .contains(&b"TEARDOWN request,  96=1, 110=1".to_vec())
        );
    }

    #[test]
    fn live_loopback_server_writes_protocol_response_then_closes_on_peer_eof() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let effects = Arc::new(RecordingEffects::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut connection = ProtocolConnection::new(
            ProtocolContext::default(),
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();
        let server_shutdown = Arc::clone(&shutdown);
        let worker = thread::spawn(move || {
            crate::http_transport::run_connection(
                server,
                &mut connection,
                server_shutdown,
                Duration::from_secs(2),
            )
        });

        let mut client = client;
        client
            .write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 11\r\n\r\n")
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        worker.join().unwrap().unwrap();
        assert!(response.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(response.ends_with(b"CSeq: 11\r\n\r\n"));
    }

    #[test]
    fn hls_host_request_serves_the_stored_local_master_playlist() {
        let context = ProtocolContext {
            hls_support: true,
            local_uri_prefix: "http://localhost:7000".to_owned(),
            ..ProtocolContext::default()
        };
        {
            let mut store = lock(&context.video_store);
            store.begin_play(
                uxplay_airplay_http_handlers::PlayRequest {
                    session_id: "session-hls".to_owned(),
                    playback_uuid: "video-hls".to_owned(),
                    content_location: Some("/video/master.m3u8".to_owned()),
                    start_position_seconds: 0.0,
                    client_proc_name: None,
                },
                &context.local_uri_prefix,
            );
            store.apply_action(
                uxplay_airplay_http_handlers::ActionPlan::FcupPlaylist {
                    url: "/video/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n"
                        .to_vec(),
                },
                &context.local_uri_prefix,
            );
        }
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();
        let request = parse(b"GET /master.m3u8 HTTP/1.1\r\nHost: localhost:7000\r\n\r\n");
        let reply = connection.request(&request).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(
            reply
                .bytes
                .windows(b"Content-Type: application/x-mpegURL; charset=utf-8".len())
                .any(|part| part == b"Content-Type: application/x-mpegURL; charset=utf-8")
        );
        assert!(reply.bytes.ends_with(
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttp://localhost:7000/low.m3u8\n"
        ));
    }

    #[test]
    fn reverse_upgrade_drives_fcup_sequence_and_local_hls_serving() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut peer = TcpStream::connect(address).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (writer, _) = listener.accept().unwrap();
        let event_channel = Arc::new(AirPlayEventChannel::default());
        let context = ProtocolContext {
            event_channel: Arc::clone(&event_channel),
            hls_support: true,
            local_uri_prefix: "http://localhost:7000".to_owned(),
            ..ProtocolContext::default()
        };
        let effects = Arc::new(RecordingEffects::default());
        let mut reverse_connection =
            ProtocolConnection::new(context.clone(), effects.clone()).unwrap();
        let reverse_request = parse(
            b"POST /reverse HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: PTTH/1.0\r\nX-Apple-Purpose: event\r\nContent-Length: 0\r\n\r\n",
        );
        let reverse_reply = reverse_connection
            .request(&reverse_request)
            .unwrap()
            .unwrap();
        assert!(
            reverse_reply
                .bytes
                .starts_with(b"HTTP/1.1 101 Switching Protocols\r\n")
        );
        assert!(
            reverse_reply
                .bytes
                .windows(b"Upgrade: PTTH/1.0\r\n".len())
                .any(|part| part == b"Upgrade: PTTH/1.0\r\n")
        );
        reverse_connection.upgraded(writer).unwrap();
        assert_eq!(event_channel.connection_count(), 1);

        let mut connection = ProtocolConnection::new(context, effects.clone()).unwrap();

        let mut play = plist::Dictionary::new();
        play.insert("uuid".into(), plist::Value::String("video-hls".into()));
        play.insert(
            "Content-Location".into(),
            plist::Value::String("/video/master.m3u8".into()),
        );
        play.insert("Start-Position-Seconds".into(), plist::Value::Real(3.5));
        let reply = connection
            .request(&post_hls_plist(
                "/play",
                "session-hls",
                plist::Value::Dictionary(play),
            ))
            .unwrap()
            .unwrap();
        assert!(reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
        let request = read_fcup_request(&mut peer);
        let request = request.as_dictionary().unwrap()["request"]
            .as_dictionary()
            .unwrap();
        assert_eq!(
            request["FCUP_Response_URL"].as_string(),
            Some("http://localhost:7000/master.m3u8")
        );
        assert_eq!(
            request["FCUP_Response_RequestID"].as_unsigned_integer(),
            Some(1)
        );

        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls",
                fcup_reply(
                    "http://localhost:7000/master.m3u8",
                    b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n",
                ),
            ))
            .unwrap();
        let request = read_fcup_request(&mut peer);
        let request = request.as_dictionary().unwrap()["request"]
            .as_dictionary()
            .unwrap();
        assert_eq!(
            request["FCUP_Response_URL"].as_string(),
            Some("/video/low.m3u8")
        );
        assert_eq!(
            request["FCUP_Response_RequestID"].as_unsigned_integer(),
            Some(2)
        );

        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls",
                fcup_reply(
                    "/video/low.m3u8",
                    b"#EXTM3U\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n",
                ),
            ))
            .unwrap();
        assert!(effects.0.lock().unwrap().iter().any(|effect| matches!(
            effect,
            HandlerEffect::StartHlsPlayback {
                location,
                start_position_seconds
            } if location == "http://localhost:7000/master.m3u8" && *start_position_seconds == 3.5
        )));

        let local_master = connection
            .request(&parse(
                b"GET /master.m3u8 HTTP/1.1\r\nHost: localhost:7000\r\n\r\n",
            ))
            .unwrap()
            .unwrap();
        assert!(local_master.bytes.ends_with(
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttp://localhost:7000/low.m3u8\n"
        ));
        reverse_connection.disconnected();
        assert_eq!(event_channel.connection_count(), 0);
    }

    #[test]
    fn hls_play_action_and_playlist_routes_emit_source_handler_logs() {
        let (logger, events) = collecting_logger(crate::logger::DEBUG);
        let media_config = SessionMediaConfig {
            logger: Some(logger),
            ..SessionMediaConfig::default()
        };
        let context = ProtocolContext {
            hls_support: true,
            local_uri_prefix: "http://localhost:7000".to_owned(),
            media_config,
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        let mut play = plist::Dictionary::new();
        play.insert("uuid".into(), plist::Value::String("video-hls-log".into()));
        play.insert(
            "Content-Location".into(),
            plist::Value::String("/video/master.m3u8".into()),
        );
        play.insert(
            "clientProcName".into(),
            plist::Value::String("OtherPlayer".into()),
        );
        let reply = connection
            .request(&post_hls_plist(
                "/play",
                "session-hls-log",
                plist::Value::Dictionary(play),
            ))
            .unwrap()
            .unwrap();
        assert!(reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));

        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n";
        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls-log",
                fcup_reply("http://localhost:7000/master.m3u8", master),
            ))
            .unwrap();
        let media = b"#EXTM3U\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n";
        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls-log",
                fcup_reply("/video/low.m3u8", media),
            ))
            .unwrap();
        let media_reply = connection
            .request(&parse(
                b"GET /low.m3u8 HTTP/1.1\r\nHost: localhost:7000\r\n\r\n",
            ))
            .unwrap()
            .unwrap();
        assert!(
            media_reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"),
            "{}",
            String::from_utf8_lossy(&media_reply.bytes)
        );

        let remove = |uuid: &str| {
            let mut item = plist::Dictionary::new();
            item.insert("uuid".into(), plist::Value::String(uuid.to_owned()));
            let mut params = plist::Dictionary::new();
            params.insert("item".into(), plist::Value::Dictionary(item));
            let mut action = plist::Dictionary::new();
            action.insert(
                "type".into(),
                plist::Value::String("playlistRemove".to_owned()),
            );
            action.insert("params".into(), plist::Value::Dictionary(params));
            plist::Value::Dictionary(action)
        };
        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls-log",
                remove("not-current"),
            ))
            .unwrap();
        connection
            .request(&post_hls_plist(
                "/action",
                "session-hls-log",
                remove("video-hls-log"),
            ))
            .unwrap();
        let missing = connection
            .request(&parse(
                b"GET /master.m3u8 HTTP/1.1\r\nHost: localhost:7000\r\n\r\n",
            ))
            .unwrap()
            .unwrap();
        assert!(missing.bytes.starts_with(b"HTTP/1.1 404 Not Found\r\n"));

        let logs = events.lock().unwrap();
        let contains = |needle: &[u8]| logs.iter().any(|message| message == needle);
        assert!(contains(b"http_handler_play"));
        assert!(contains(b"No Start-Position-Seconds in Play request"));
        assert!(contains(
            b"Unsupported m3u8 HLS streaming format: clientProcName OtherPlayer not found in supported list: YouTube;"
        ));
        assert!(
            logs.iter()
                .any(|message| { message.starts_with(b"fcup_request: send  error ") })
        );
        assert!(contains(b"action type is unhandledURLResponse"));
        assert!(contains(b"FCUP_Response_URL =  /video/low.m3u8"));
        assert!(
            logs.iter()
                .any(|message| { message.starts_with(b"begin FCUP Response data:\n#EXTM3U\n") })
        );
        assert!(contains(
            b"Requested media_playlist /low.m3u8 has     1 chunks, total duration    10.000 secs"
        ));
        assert!(contains(
            b"playlistRemove uuid not-current does not match current_video\n"
        ));
        assert!(contains(b"airplay_video playlist  not found"));
    }

    #[test]
    fn remaining_http_handlers_emit_source_diagnostics() {
        let events = Arc::new(Mutex::new(Vec::<(i32, Vec<u8>)>::new()));
        let captured_events = Arc::clone(&events);
        let logger = Arc::new(crate::logger::Logger::new());
        logger.set_level(crate::logger::DEBUG);
        logger.set_callback(Some(Arc::new(move |level, message| {
            captured_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((level, message.to_vec()));
        })));
        let context = ProtocolContext {
            hls_support: true,
            playback_snapshot: Some(PlaybackSnapshot {
                duration: 30.0,
                position: -1.0,
                seek_start: 0.0,
                seek_duration: 30.0,
                rate: 1.0,
                ready_to_play: false,
                playback_buffer_empty: false,
                playback_buffer_full: false,
                playback_likely_to_keep_up: false,
            }),
            media_config: SessionMediaConfig {
                logger: Some(Arc::clone(&logger)),
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        for wire in [
            b"POST /scrub?position=1.25 HTTP/1.1\r\n\r\n".as_slice(),
            b"POST /rate?value=-0.5 HTTP/1.1\r\n\r\n",
            b"POST /stop HTTP/1.1\r\n\r\n",
            b"PUT /setProperty?actionAtItemEnd HTTP/1.1\r\n\r\n",
            b"PUT /setProperty?unknown HTTP/1.1\r\n\r\n",
            b"POST /getProperty?speed=1 HTTP/1.1\r\n\r\n",
        ] {
            connection.request(&parse(wire)).unwrap();
        }
        let mut fpsetup2 = b"POST /fp-setup2 HTTP/1.1\r\nContent-Length: 5\r\n\r\n".to_vec();
        fpsetup2.extend_from_slice(&[0, 0, 0, 0, 4]);
        connection.request(&parse(&fpsetup2)).unwrap();
        connection
            .request(&parse(b"GET /playback-info HTTP/1.1\r\n\r\n"))
            .unwrap();
        let finished_context = ProtocolContext {
            hls_support: true,
            playback_snapshot: Some(PlaybackSnapshot {
                duration: -1.0,
                position: 0.0,
                seek_start: 0.0,
                seek_duration: 0.0,
                rate: 0.0,
                ready_to_play: false,
                playback_buffer_empty: false,
                playback_buffer_full: false,
                playback_likely_to_keep_up: false,
            }),
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        };
        let mut finished_connection =
            ProtocolConnection::new(finished_context, Arc::new(RecordingEffects::default()))
                .unwrap();
        finished_connection
            .request(&parse(b"GET /playback-info HTTP/1.1\r\n\r\n"))
            .unwrap();

        let logs = events.lock().unwrap();
        let relevant = logs
            .iter()
            .filter(|(_, message)| {
                message.starts_with(b"http_handler_scrub:")
                    || message.starts_with(b"**********************SCRUB")
                    || message.starts_with(b"http_handler_rate:")
                    || message.as_slice() == b"client HTTP request POST stop"
                    || message.starts_with(b"http_handler_set_property:")
                    || message.starts_with(b"property ")
                    || message.starts_with(b"http_handler_get_property:")
                    || message.as_slice() == b"client HTTP request POST fp-setup2 is unhandled"
                    || message.starts_with(b"only FairPlay version")
                    || message.as_slice() == b"playback_info not available"
                    || message.as_slice() == b"playback_info not available (finishing)"
            })
            .map(|(level, message)| (*level, message.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            relevant,
            [
                (
                    crate::logger::DEBUG,
                    b"http_handler_scrub: got position = 1.250000".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"**********************SCRUB 1.250000 ***********************".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"http_handler_rate: got rate = -0.500000".to_vec()
                ),
                (
                    crate::logger::INFO,
                    b"client HTTP request POST stop".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"http_handler_set_property: actionAtItemEnd".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"property actionAtItemEnd is known but unhandled".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"http_handler_set_property: unknown".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"property unknown is unknown, unhandled".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"http_handler_get_property: ?speed=1 (unhandled)".to_vec()
                ),
                (
                    crate::logger::WARNING,
                    b"client HTTP request POST fp-setup2 is unhandled".to_vec()
                ),
                (
                    crate::logger::ERR,
                    b"only FairPlay version 0x03 is implemented, version is 0x04".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"playback_info not available".to_vec()
                ),
                (
                    crate::logger::DEBUG,
                    b"playback_info not available (finishing)".to_vec()
                ),
            ]
        );
    }

    #[test]
    fn play_cleanup_logs_source_message_at_info_after_playlist_removal() {
        let events = Arc::new(Mutex::new(Vec::<(i32, Vec<u8>)>::new()));
        let captured_events = Arc::clone(&events);
        let logger = Arc::new(crate::logger::Logger::new());
        logger.set_level(crate::logger::DEBUG);
        logger.set_callback(Some(Arc::new(move |level, message| {
            captured_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((level, message.to_vec()));
        })));
        let context = ProtocolContext {
            hls_support: true,
            local_uri_prefix: "http://localhost:7000".to_owned(),
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            ..ProtocolContext::default()
        };
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();

        let mut short_play = plist::Dictionary::new();
        short_play.insert("uuid".into(), plist::Value::String("short-ad".to_owned()));
        short_play.insert(
            "Content-Location".into(),
            plist::Value::String("/video/master.m3u8".to_owned()),
        );
        connection
            .request(&post_hls_plist(
                "/play",
                "session-cleanup",
                plist::Value::Dictionary(short_play),
            ))
            .unwrap();
        connection
            .request(&post_hls_plist(
                "/action",
                "session-cleanup",
                fcup_reply(
                    "http://localhost:7000/master.m3u8",
                    b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n",
                ),
            ))
            .unwrap();
        connection
            .request(&post_hls_plist(
                "/action",
                "session-cleanup",
                fcup_reply(
                    "/video/low.m3u8",
                    b"#EXTM3U\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n",
                ),
            ))
            .unwrap();

        let mut direct_play = plist::Dictionary::new();
        direct_play.insert("uuid".into(), plist::Value::String("next-video".to_owned()));
        direct_play.insert(
            "Content-Location".into(),
            plist::Value::String("https://origin.example/next.mp4".to_owned()),
        );
        connection
            .request(&post_hls_plist(
                "/play",
                "session-next",
                plist::Value::Dictionary(direct_play),
            ))
            .unwrap();

        let logs = events.lock().unwrap();
        let deletion_message =
            b"deleting playlist playback_uuid short-ad duration (seconds) 10.000000";
        let deletion_index = logs
            .iter()
            .position(|(level, message)| {
                *level == crate::logger::INFO && message.as_slice() == deletion_message
            })
            .expect("the expired short playlist is logged at INFO");
        let no_start_position_index = logs
            .iter()
            .rposition(|(_, message)| message == b"No Start-Position-Seconds in Play request")
            .expect("the second Play request has no start position");
        assert!(
            deletion_index < no_start_position_index,
            "source cleanup log precedes the missing start-position log"
        );
    }

    #[test]
    fn playlist_remove_retains_resume_position_for_reused_playback_uuid() {
        let local_prefix = "http://localhost:7000";
        let video_store = Arc::new(Mutex::new(
            crate::airplay_video::AirPlayVideoStore::default(),
        ));
        let first_action = lock(&video_store).begin_play(
            uxplay_airplay_http_handlers::PlayRequest {
                session_id: "session-old".to_owned(),
                playback_uuid: "video-resume".to_owned(),
                content_location: Some("/video/master.m3u8".to_owned()),
                start_position_seconds: 12.0,
                client_proc_name: None,
            },
            local_prefix,
        );
        assert!(matches!(first_action, HlsAction::Fetch { .. }));

        let context = ProtocolContext {
            hls_support: true,
            local_uri_prefix: local_prefix.to_owned(),
            video_store: Arc::clone(&video_store),
            ..ProtocolContext::default()
        };
        let effects = Arc::new(PlaylistRemovalEffects {
            callback_positions: Mutex::new(VecDeque::from([23.5, 8.0])),
            ..PlaylistRemovalEffects::default()
        });
        let mut connection = ProtocolConnection::new(context, effects.clone()).unwrap();
        let mut session_id = "session-old".to_owned();

        for (iteration, (resume_position, expected_start)) in
            [(23.5, 23.5), (8.0, 12.0)].into_iter().enumerate()
        {
            let mut item = plist::Dictionary::new();
            item.insert(
                "uuid".into(),
                plist::Value::String("video-resume".to_owned()),
            );
            let mut params = plist::Dictionary::new();
            params.insert("item".into(), plist::Value::Dictionary(item));
            let mut action = plist::Dictionary::new();
            action.insert(
                "type".into(),
                plist::Value::String("playlistRemove".to_owned()),
            );
            action.insert("params".into(), plist::Value::Dictionary(params));

            let remove_reply = connection
                .request(&post_hls_plist(
                    "/action",
                    &session_id,
                    plist::Value::Dictionary(action),
                ))
                .unwrap()
                .unwrap();
            assert!(remove_reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
            assert_eq!(lock(&video_store).current_video_index(), None);

            let mut reuse = plist::Dictionary::new();
            reuse.insert(
                "uuid".into(),
                plist::Value::String("video-resume".to_owned()),
            );
            session_id = format!("session-reused-{iteration}");
            let play_reply = connection
                .request(&post_hls_plist(
                    "/play",
                    &session_id,
                    plist::Value::Dictionary(reuse),
                ))
                .unwrap()
                .unwrap();
            assert!(play_reply.bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));

            let applied = effects
                .applied
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(
                matches!(
                    applied.last(),
                    Some(HandlerEffect::StartHlsPlayback {
                        start_position_seconds,
                        ..
                    }) if *start_position_seconds == expected_start
                ),
                "removal position {resume_position} should resume at {expected_start}"
            );
        }

        assert_eq!(effects.callback_calls.load(Ordering::Acquire), 2);
        assert!(
            effects
                .callback_positions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[test]
    fn reverse_post_installs_fcup_writer_until_event_disconnect() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let context = ProtocolContext {
            hls_support: true,
            ..ProtocolContext::default()
        };
        let event_channel = Arc::clone(&context.event_channel);
        let mut connection =
            ProtocolConnection::new(context, Arc::new(RecordingEffects::default())).unwrap();
        let worker = thread::spawn(move || {
            crate::http_transport::run_connection(
                server,
                &mut connection,
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(2),
            )
        });

        client
            .write_all(b"POST /reverse HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            client.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
        }
        assert!(response.starts_with(b"HTTP/1.1 101 Switching Protocols\r\n"));
        assert_eq!(event_channel.connection_count(), 1);

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            match event_channel.send_fcup_request("http://localhost/master.m3u8", "session-1", 4) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == io::ErrorKind::NotConnected
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("FCUP writer was not installed: {error}"),
            }
        }

        client.shutdown(Shutdown::Write).unwrap();
        let mut event_bytes = Vec::new();
        client.read_to_end(&mut event_bytes).unwrap();
        worker.join().unwrap().unwrap();
        assert!(event_bytes.starts_with(b"POST /event HTTP/1.1\r\n"));
        assert_eq!(event_channel.connection_count(), 0);
        assert_eq!(
            event_channel
                .send_fcup_request("http://localhost/next.m3u8", "session-1", 5)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
    }

    #[test]
    fn serve_protocol_accepts_connections_and_shuts_down_cleanly() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let server = thread::spawn(move || {
            serve_protocol(
                vec![listener],
                server_shutdown,
                ServerConfig::default(),
                ProtocolContext::default(),
                effects,
            )
        });

        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 23\r\n\r\n")
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        shutdown.store(true, std::sync::atomic::Ordering::Release);
        server.join().unwrap().unwrap();
        assert!(response.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert!(response.ends_with(b"CSeq: 23\r\n\r\n"));
    }

    #[test]
    fn setup_response_uses_the_ports_actually_bound_by_media_workers() {
        let mut audio = plist::Dictionary::new();
        audio.insert("type".into(), plist::Value::from(96));
        audio.insert("dataPort".into(), plist::Value::from(0));
        audio.insert("controlPort".into(), plist::Value::from(0));
        let mut mirror = plist::Dictionary::new();
        mirror.insert("type".into(), plist::Value::from(110));
        mirror.insert("dataPort".into(), plist::Value::from(0));
        let mut response = plist::Dictionary::new();
        response.insert("timingPort".into(), plist::Value::from(0));
        response.insert(
            "streams".into(),
            plist::Value::Array(vec![
                plist::Value::Dictionary(audio),
                plist::Value::Dictionary(mirror),
            ]),
        );
        let mut bytes = Vec::new();
        plist::Value::Dictionary(response)
            .to_writer_binary(&mut bytes)
            .unwrap();

        let mut body = Some(bytes);
        update_setup_ports(
            &mut body,
            (
                Some(5101),
                Some(uxplay_raop_rtp::AudioPorts {
                    control_remote: 7000,
                    control_local: 5102,
                    data_local: 5103,
                }),
                Some(5104),
            ),
        )
        .unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(body.unwrap())).unwrap();
        let dictionary = value.as_dictionary().unwrap();
        assert_eq!(
            dictionary
                .get("timingPort")
                .and_then(plist::Value::as_unsigned_integer),
            Some(5101)
        );
        let streams = dictionary
            .get("streams")
            .and_then(plist::Value::as_array)
            .unwrap();
        let audio = streams[0].as_dictionary().unwrap();
        assert_eq!(
            audio
                .get("controlPort")
                .and_then(plist::Value::as_unsigned_integer),
            Some(5102)
        );
        assert_eq!(
            audio
                .get("dataPort")
                .and_then(plist::Value::as_unsigned_integer),
            Some(5103)
        );
        let mirror = streams[1].as_dictionary().unwrap();
        assert_eq!(
            mirror
                .get("dataPort")
                .and_then(plist::Value::as_unsigned_integer),
            Some(5104)
        );
    }

    #[test]
    fn finished_playback_closes_its_reply_and_schedules_source_connection_removal() {
        let effects = Arc::new(RecordingEffects::default());
        let context = ProtocolContext {
            hls_support: true,
            playback_snapshot: Some(crate::airplay_info::PlaybackSnapshot {
                duration: -1.0,
                position: -1.0,
                seek_start: 0.0,
                seek_duration: 0.0,
                rate: 0.0,
                ready_to_play: false,
                playback_buffer_empty: false,
                playback_buffer_full: false,
                playback_likely_to_keep_up: false,
            }),
            ..ProtocolContext::default()
        };
        let effect_sink: Arc<dyn ProtocolEffectSink> = effects.clone();
        let mut connection = ProtocolConnection::new(context, effect_sink).unwrap();
        let request = parse(b"GET /playback-info HTTP/1.1\r\nX-Apple-Session-ID: session\r\n\r\n");

        let reply = connection.request(&request).unwrap().unwrap();

        assert!(reply.disconnect);
        assert_eq!(
            reply.remove_classes_after_write,
            [
                ConnectionClass::Hls,
                ConnectionClass::Event,
                ConnectionClass::AirPlay
            ]
        );
        assert_eq!(
            *effects
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [HandlerEffect::ResetHlsPlayback]
        );
    }

    #[test]
    fn disconnected_setup_releases_all_ports_and_allows_reconnect() {
        let _video_lock = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut audio = plist::Dictionary::new();
        audio.insert("type".into(), plist::Value::from(96));
        audio.insert("ct".into(), plist::Value::from(1));
        audio.insert("spf".into(), plist::Value::from(352));
        audio.insert("controlPort".into(), plist::Value::from(5001));
        audio.insert("usingScreen".into(), plist::Value::Boolean(false));
        audio.insert("isMedia".into(), plist::Value::Boolean(true));
        audio.insert("audioFormat".into(), plist::Value::from(0_u64));
        let mut mirror = plist::Dictionary::new();
        mirror.insert("type".into(), plist::Value::from(110));
        mirror.insert("streamConnectionID".into(), plist::Value::from(1_u64));
        let mut setup = plist::Dictionary::new();
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        setup.insert("timingPort".into(), plist::Value::from(5002));
        setup.insert("timingProtocol".into(), plist::Value::String("None".into()));
        setup.insert(
            "streams".into(),
            plist::Value::Array(vec![
                plist::Value::Dictionary(audio),
                plist::Value::Dictionary(mirror),
            ]),
        );
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let mut request_wire = format!(
            "SETUP / RTSP/1.0\r\nCSeq: 72\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request_wire.extend_from_slice(&body);
        let request = parse(&request_wire);
        let effects = Arc::new(RecordingEffects::default());
        let context = ProtocolContext {
            server_ports: ServerPorts {
                timing_local: 0,
                audio_control_local: 0,
                audio_data_local: 0,
                mirror_data_local: 0,
            },
            ..ProtocolContext::default()
        };
        let mut connection = ProtocolConnection::new(
            context.clone(),
            effects.clone() as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        let decode_ports = |reply: &HttpReply| {
            let body_offset = reply
                .bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap()
                + 4;
            let response =
                plist::Value::from_reader(std::io::Cursor::new(&reply.bytes[body_offset..]))
                    .unwrap();
            let response = response.as_dictionary().unwrap();
            let port = |value: Option<&plist::Value>| {
                u16::try_from(value.and_then(plist::Value::as_unsigned_integer).unwrap()).unwrap()
            };
            let timing = port(response.get("timingPort"));
            let streams = response
                .get("streams")
                .and_then(plist::Value::as_array)
                .unwrap();
            let stream_port = |index: usize, field: &str| {
                port(streams[index].as_dictionary().unwrap().get(field))
            };
            [
                timing,
                stream_port(0, "controlPort"),
                stream_port(0, "dataPort"),
                stream_port(1, "dataPort"),
            ]
        };
        let assert_released = |ports: [u16; 4]| {
            assert!(ports.into_iter().all(|port| port != 0));
            for port in &ports[..3] {
                UdpSocket::bind((Ipv4Addr::UNSPECIFIED, *port))
                    .unwrap_or_else(|error| panic!("UDP port {port} was not released: {error}"));
            }
            std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, ports[3])).unwrap_or_else(
                |error| panic!("mirror TCP port {} was not released: {error}", ports[3]),
            );
        };
        let first_ports = decode_ports(&reply);
        connection.disconnected();
        assert_released(first_ports);

        let mut reconnected =
            ProtocolConnection::new(context, effects as Arc<dyn ProtocolEffectSink>).unwrap();
        let reconnect_reply = reconnected.request(&request).unwrap().unwrap();
        assert!(reconnect_reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        let reconnect_ports = decode_ports(&reconnect_reply);
        assert!(reconnect_ports.into_iter().all(|port| port != 0));
        reconnected.disconnected();
        assert_released(reconnect_ports);
    }

    #[test]
    fn new_airplay_connection_stops_raop_workers_without_closing_raop_session() {
        let _video_lock = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (logger, log_events) = collecting_logger_events(crate::logger::DEBUG);
        let registry = Arc::new(ConnectionRegistry::default());
        let context = ProtocolContext {
            hls_support: true,
            hardware_address: Some(vec![0x02, 0x00, 0x00, 0x12, 0x34, 0x56]),
            media_config: SessionMediaConfig {
                logger: Some(logger),
                ..SessionMediaConfig::default()
            },
            server_ports: ServerPorts {
                timing_local: 0,
                audio_control_local: 0,
                audio_data_local: 0,
                mirror_data_local: 0,
            },
            connection_registry: Some(Arc::clone(&registry)),
            ..ProtocolContext::default()
        };
        let video_store = Arc::clone(&context.video_store);
        lock(&video_store).begin_play(
            uxplay_airplay_http_handlers::PlayRequest {
                session_id: "hls-session".into(),
                playback_uuid: "hls-video".into(),
                content_location: Some("https://example.invalid/master.m3u8".into()),
                start_position_seconds: 0.0,
                client_proc_name: None,
            },
            &context.local_uri_prefix,
        );
        assert_eq!(lock(&video_store).current_video_index(), Some(0));

        let mut audio = plist::Dictionary::new();
        audio.insert("type".into(), plist::Value::from(96));
        audio.insert("ct".into(), plist::Value::from(1));
        audio.insert("spf".into(), plist::Value::from(352));
        audio.insert("controlPort".into(), plist::Value::from(5001));
        audio.insert("usingScreen".into(), plist::Value::Boolean(false));
        audio.insert("isMedia".into(), plist::Value::Boolean(true));
        audio.insert("audioFormat".into(), plist::Value::from(0_u64));
        let mut mirror = plist::Dictionary::new();
        mirror.insert("type".into(), plist::Value::from(110));
        mirror.insert("streamConnectionID".into(), plist::Value::from(1_u64));
        let mut setup = plist::Dictionary::new();
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        setup.insert("timingPort".into(), plist::Value::from(5002));
        setup.insert("timingProtocol".into(), plist::Value::String("None".into()));
        setup.insert(
            "streams".into(),
            plist::Value::Array(vec![
                plist::Value::Dictionary(audio),
                plist::Value::Dictionary(mirror),
            ]),
        );
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let decode_ports = |reply: &HttpReply| {
            let body_offset = reply
                .bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap()
                + 4;
            let response =
                plist::Value::from_reader(std::io::Cursor::new(&reply.bytes[body_offset..]))
                    .unwrap();
            let response = response.as_dictionary().unwrap();
            let port = |value: Option<&plist::Value>| {
                u16::try_from(value.and_then(plist::Value::as_unsigned_integer).unwrap()).unwrap()
            };
            let streams = response
                .get("streams")
                .and_then(plist::Value::as_array)
                .unwrap();
            [
                port(response.get("timingPort")),
                port(streams[0].as_dictionary().unwrap().get("controlPort")),
                port(streams[0].as_dictionary().unwrap().get("dataPort")),
                port(streams[1].as_dictionary().unwrap().get("dataPort")),
            ]
        };
        let assert_released = |ports: [u16; 4]| {
            assert!(ports.into_iter().all(|port| port != 0));
            for port in &ports[..3] {
                UdpSocket::bind((Ipv4Addr::UNSPECIFIED, *port))
                    .unwrap_or_else(|error| panic!("UDP port {port} was not released: {error}"));
            }
            std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, ports[3])).unwrap_or_else(
                |error| panic!("mirror TCP port {} was not released: {error}", ports[3]),
            );
        };

        let effects: Arc<dyn ProtocolEffectSink> = Arc::new(RecordingEffects::default());
        let mut raop = ProtocolConnection::new(context.clone(), Arc::clone(&effects)).unwrap();
        let first_setup = raop.request(&setup_rtsp(&body, None, 80)).unwrap().unwrap();
        assert!(first_setup.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        assert_eq!(lock(&video_store).current_video_index(), None);
        assert_eq!(
            lock(&video_store).playlist_index_for_uuid("hls-video"),
            None,
            "mirror SETUP destroys the source's cached HLS video objects"
        );
        let first_ports = decode_ports(&first_setup);
        assert_eq!(raop.connection_class, ConnectionClass::Raop);
        assert!(raop.raop_media_registration.is_some());

        // The TCP registry entry represents the RAOP control connection. AirPlay
        // classification must stop its UDP/TCP media workers without removing it.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let raop_socket_registration = registry.register(&server).unwrap();
        raop_socket_registration.set_class(ConnectionClass::Raop);

        let mut airplay = ProtocolConnection::new(context, Arc::clone(&effects)).unwrap();
        let airplay_reply = airplay
            .request(&parse(
                b"GET /server-info HTTP/1.1\r\nX-Apple-Session-ID: video-session\r\n\r\n",
            ))
            .unwrap()
            .unwrap();
        assert!(airplay_reply.bytes.starts_with(b"HTTP/1.1 "));
        assert_eq!(airplay.connection_class, ConnectionClass::AirPlay);
        let repeated_airplay_reply = airplay
            .request(&parse(
                b"GET /server-info HTTP/1.1\r\nX-Apple-Session-ID: video-session\r\n\r\n",
            ))
            .unwrap()
            .unwrap();
        assert!(repeated_airplay_reply.bytes.starts_with(b"HTTP/1.1 "));
        assert_eq!(raop.connection_class, ConnectionClass::Raop);
        assert!(registry.has_class(ConnectionClass::Raop));
        assert_eq!(lock(&raop.media.0).parameter_state(), (true, false));
        assert_released(first_ports);
        let stop_messages: Vec<_> = log_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, message)| message.starts_with(b"New AirPlay connection: stopping"))
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(stop_messages.len(), 3);
        assert!(stop_messages[0].starts_with(
            b"New AirPlay connection: stopping RAOP mirror service on RAOP connection "
        ));
        assert!(stop_messages[1].starts_with(
            b"New AirPlay connection: stopping RAOP audio service on RAOP connection "
        ));
        assert!(
            stop_messages[2].starts_with(
                b"New AirPlay connection: stopping NTP time service on RAOP connection "
            )
        );

        let restarted_setup = raop.request(&setup_rtsp(&body, None, 81)).unwrap().unwrap();
        assert!(restarted_setup.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        let restarted_ports = decode_ports(&restarted_setup);
        raop.disconnected();
        assert_released(restarted_ports);
        drop(airplay);
        drop(peer);
    }

    #[test]
    fn password_mode_challenges_the_initial_encrypted_setup() {
        let mut setup = plist::Dictionary::new();
        setup.insert("deviceID".into(), plist::Value::String("client-id".into()));
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let mut request_wire = format!(
            "SETUP / RTSP/1.0\r\nCSeq: 74\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request_wire.extend_from_slice(&body);
        let request = parse(&request_wire);
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                setup_decisions: SetupDecisions {
                    authorized: false,
                    ..SetupDecisions::allow_all()
                },
                password_auth: PasswordAuth::new(2, Some("swordfish".into())),
                ..ProtocolContext::default()
            },
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection.request(&request).unwrap().unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 401 Unauthorized\r\n"));
        assert!(
            reply
                .bytes
                .windows(b"WWW-Authenticate: Digest realm=\"raop\", nonce=\"".len())
                .any(|window| window == b"WWW-Authenticate: Digest realm=\"raop\", nonce=\"")
        );
        assert!(reply.bytes.ends_with(b"CSeq: 74\r\n\r\n"));
    }

    #[test]
    fn password_mode_accepts_a_digest_and_continues_setup() {
        let mut setup = plist::Dictionary::new();
        setup.insert("deviceID".into(), plist::Value::String("client-id".into()));
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        setup.insert("timingPort".into(), plist::Value::from(5002));
        setup.insert("timingProtocol".into(), plist::Value::String("None".into()));
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                setup_decisions: SetupDecisions {
                    authorized: false,
                    ..SetupDecisions::allow_all()
                },
                password_auth: PasswordAuth::new(2, Some("swordfish".into())),
                ..ProtocolContext::default()
            },
            effects as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let challenge = connection
            .request(&setup_rtsp(&body, None, 75))
            .unwrap()
            .unwrap();
        assert!(
            challenge
                .bytes
                .starts_with(b"RTSP/1.0 401 Unauthorized\r\n")
        );
        let marker = b"nonce=\"";
        let nonce_start = challenge
            .bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap()
            + marker.len();
        let nonce_end = nonce_start + 32;
        let nonce = &challenge.bytes[nonce_start..nonce_end];
        let authorization = digest_header(b"SETUP", b"/", b"swordfish", nonce);

        let reply = connection
            .request(&setup_rtsp(&body, Some(&authorization), 76))
            .unwrap()
            .unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 200 OK\r\n"));
        connection.disconnected();
    }

    #[test]
    fn random_password_mode_displays_the_generated_pin_for_initial_setup() {
        let mut setup = plist::Dictionary::new();
        setup.insert("deviceID".into(), plist::Value::String("client-id".into()));
        setup.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        setup.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        let mut body = Vec::new();
        plist::Value::Dictionary(setup)
            .to_writer_binary(&mut body)
            .unwrap();
        let effects = Arc::new(RecordingEffects::default());
        let mut connection = ProtocolConnection::new(
            ProtocolContext {
                setup_decisions: SetupDecisions {
                    authorized: false,
                    ..SetupDecisions::allow_all()
                },
                password_auth: PasswordAuth::new(3, None),
                ..ProtocolContext::default()
            },
            effects.clone() as Arc<dyn ProtocolEffectSink>,
        )
        .unwrap();

        let reply = connection
            .request(&setup_rtsp(&body, None, 77))
            .unwrap()
            .unwrap();
        assert!(reply.bytes.starts_with(b"RTSP/1.0 401 Unauthorized\r\n"));
        assert!(effects
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|effect| matches!(effect, HandlerEffect::DisplayPairingPin(pin) if pin.len() == 4 && pin.bytes().all(|byte| byte.is_ascii_digit()))));
        connection.disconnected();
    }
}
