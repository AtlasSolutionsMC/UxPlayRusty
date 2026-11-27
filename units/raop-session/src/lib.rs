//! Stateful RAOP SETUP, RECORD, FLUSH, and TEARDOWN transitions.
//!
//! The module parses Apple binary plists, owns the negotiated connection state,
//! and returns explicit effects for the app to execute against its sockets and
//! renderer. The effects use the staged audio and mirror crates' public types.

use fairplay_playfair::port::FairPlay;
use plist::{Dictionary, Value};
use sha2::{Digest, Sha512};
use std::io::{self, Cursor};
use std::sync::{Arc, Mutex};
use uxplay_pairing::PairingSession;
use uxplay_raop_rtp::AudioPorts;
use uxplay_raop_rtp_mirror::NtpClock;
use uxplay_raop_rtp_mirror::runtime::{MirrorReceiver, MirrorRuntimeCallbacks};
use zeroize::Zeroize;

pub const AUDIO_SAMPLE_RATE: u32 = 44_100;
pub const SECOND_IN_USECS: u64 = 1_000_000;
pub const AES_KEY_SIZE: usize = 16;
pub const AES_IV_SIZE: usize = 16;
pub const EKEY_SIZE: usize = 72;
const OLD_PROTOCOL_CLIENT_USER_AGENT_LIST: &str = "AirMyPC/2.0;xxx";
#[cfg(windows)]
const C_LONG_MAX: i64 = i32::MAX as i64;
#[cfg(not(windows))]
const C_LONG_MAX: i64 = i64::MAX;
#[cfg(windows)]
const C_LONG_MIN: i64 = i32::MIN as i64;
#[cfg(not(windows))]
const C_LONG_MIN: i64 = i64::MIN;
#[cfg(windows)]
const C_LONG_MIN_MAGNITUDE: u64 = (i32::MAX as u64) + 1;
#[cfg(not(windows))]
const C_LONG_MIN_MAGNITUDE: u64 = (i64::MAX as u64) + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimingProtocol {
    Ntp,
    None,
    Other,
    Unspecified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoResetReason {
    RtpToHlsTeardown,
    RtpShutdown,
    HlsShutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLogLevel {
    Debug,
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    pub codec_type: u8,
    pub samples_per_frame: u16,
    pub using_screen: bool,
    pub is_media: bool,
    pub audio_format: u64,
}

/// Parameters needed to create the staged mirroring runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirrorSetup {
    pub remote_address: String,
    pub remote_address_length: usize,
    audio_key: [u8; AES_KEY_SIZE],
}

impl MirrorSetup {
    pub fn new(
        remote_address: String,
        remote_address_length: usize,
        audio_key: [u8; AES_KEY_SIZE],
    ) -> Self {
        Self {
            remote_address,
            remote_address_length,
            audio_key,
        }
    }

    /// Construct the staged TCP mirror listener with app-owned clock and callbacks.
    pub fn create_receiver(
        &self,
        clock: Arc<Mutex<Box<dyn NtpClock + Send>>>,
        callbacks: Arc<dyn MirrorRuntimeCallbacks>,
    ) -> io::Result<MirrorReceiver> {
        MirrorReceiver::new(
            &self.remote_address,
            self.remote_address_length,
            &self.audio_key,
            clock,
            callbacks,
        )
    }
}

impl Drop for MirrorSetup {
    fn drop(&mut self) {
        self.audio_key.zeroize();
    }
}

/// Effects that the connection/app layer performs after a transition.
///
/// Variants that correspond to `raop_callbacks_t` are kept explicit, so the
/// handler never reaches into the renderer or invokes callbacks through FFI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionEffect {
    Log {
        level: SessionLogLevel,
        message: String,
    },
    SetRemoteControlId {
        dacp_id: String,
        active_remote: String,
    },
    ReportClientRequest {
        device_id: Option<String>,
        model: Option<String>,
        name: Option<String>,
    },
    RegisterClient {
        device_id: String,
        public_key_base64: String,
        name: String,
    },
    LogSetupPorts,
    StartTiming {
        remote_address: String,
        remote_port: u16,
        local_port: u16,
        protocol: TimingProtocol,
    },
    InitializeAudio {
        remote_address: String,
        key: [u8; AES_KEY_SIZE],
        iv: [u8; AES_IV_SIZE],
    },
    InitializeMirror {
        config: MirrorSetup,
    },
    ClearHlsVideoData,
    InitializeMirrorAes {
        stream_connection_id: u64,
    },
    StartMirror {
        requested_port: u16,
        show_client_fps_data: u8,
    },
    GetAudioFormat {
        format: AudioFormat,
    },
    /// The ordered `GetAudioFormat` callback effect immediately before this
    /// command may change the format fields before the app starts audio.
    StartAudio {
        remote_address: String,
        ports: AudioPorts,
        format: AudioFormat,
        sample_rate: u32,
    },
    FlushAudio {
        next_sequence_number: i32,
    },
    StopAudio,
    StopCoverArtRendering,
    StopMirror,
    DestroyAudio,
    DestroyMirror,
    VideoReset(VideoResetReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionResponse {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub close_connection: bool,
    pub disconnect: bool,
}

impl Default for SessionResponse {
    fn default() -> Self {
        Self {
            status: 200,
            reason: "OK",
            headers: Vec::new(),
            body: None,
            close_connection: false,
            disconnect: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServerPorts {
    pub timing_local: u16,
    pub audio_control_local: u16,
    pub audio_data_local: u16,
    pub mirror_data_local: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallbackAvailability {
    pub report_client_request: bool,
    pub register_client: bool,
    pub audio_get_format: bool,
}

/// Results of outer callbacks/authentication that the RAOP session handler's
/// state transition needs in order to choose its response.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetupDecisions {
    pub authorized: bool,
    pub admit_client: bool,
    pub callbacks: CallbackAvailability,
    pub authorization_challenge: Option<String>,
}

impl SetupDecisions {
    /// Useful for applications without password/client admission callbacks.
    pub fn allow_all() -> Self {
        Self {
            authorized: true,
            admit_client: true,
            callbacks: CallbackAvailability::default(),
            authorization_challenge: None,
        }
    }
}

/// Persistent per-connection state needed by these handlers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RaopSessionState {
    pub timing_exists: bool,
    pub audio_exists: bool,
    pub audio_running: bool,
    pub mirror_exists: bool,
    pub mirror_running: bool,
    pub audio_key: Option<[u8; AES_KEY_SIZE]>,
    pub audio_iv: Option<[u8; AES_IV_SIZE]>,
    pub timing_remote_port: Option<u16>,
    pub timing_local_port: Option<u16>,
    pub audio_ports: Option<AudioPorts>,
    pub mirror_data_port: Option<u16>,
}

impl Drop for RaopSessionState {
    fn drop(&mut self) {
        if let Some(key) = self.audio_key.as_mut() {
            key.zeroize();
        }
        if let Some(iv) = self.audio_iv.as_mut() {
            iv.zeroize();
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionTransition {
    pub response: SessionResponse,
    pub effects: Vec<SessionEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    InvalidPlist,
    PlistEncode(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlist => formatter.write_str("invalid binary plist"),
            Self::PlistEncode(message) => {
                write!(formatter, "failed to encode binary plist: {message}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// Setup environment supplied by the persistent HTTP connection layer.
pub struct SetupContext<'a> {
    pub remote_address: &'a str,
    pub remote_address_length: usize,
    pub ports: ServerPorts,
    pub decisions: SetupDecisions,
    pub fairplay: &'a FairPlay,
    pub pairing_session: Option<&'a PairingSession>,
    pub client_fps_data: u8,
}

/// Parse and handle one RTSP SETUP request.
pub fn setup(
    state: &mut RaopSessionState,
    body: &[u8],
    dacp_id: Option<&str>,
    active_remote: Option<&str>,
    user_agent: Option<&str>,
    context: SetupContext<'_>,
) -> Result<SessionTransition, SessionError> {
    let request = parse_binary_plist_or_empty(body);
    let request_dict = request.as_dictionary();
    let mut transition = SessionTransition::default();
    let mut response_plist = Dictionary::new();

    if state.audio_exists
        && let (Some(dacp_id), Some(active_remote)) = (dacp_id, active_remote)
    {
        transition.effects.push(SessionEffect::SetRemoteControlId {
            dacp_id: dacp_id.to_owned(),
            active_remote: active_remote.to_owned(),
        });
    }

    let eiv = data_value(request_dict, "eiv");
    let ekey = data_value(request_dict, "ekey");
    if let (Some(eiv), Some(ekey)) = (eiv, ekey) {
        let device_id = string_value(request_dict, "deviceID");
        let model = string_value(request_dict, "model");
        let name = string_value(request_dict, "name");

        if !context.decisions.authorized {
            set_unauthorized(
                &mut transition,
                context.decisions.authorization_challenge.as_deref(),
            );
            return Ok(transition);
        }

        if context.decisions.callbacks.report_client_request {
            transition.effects.push(SessionEffect::ReportClientRequest {
                device_id: device_id.clone(),
                model: model.clone(),
                name: name.clone(),
            });
        }

        if context.decisions.admit_client
            && context.decisions.callbacks.register_client
            && let (Some(device_id), Some(name), Some(pairing_session)) = (
                device_id.as_deref(),
                name.as_deref(),
                context.pairing_session,
            )
        {
            let (paired_device_id, public_key) = pairing_session.client_data();
            if let Some(public_key) = public_key
                && paired_device_id == device_id.as_bytes()
            {
                transition.effects.push(SessionEffect::RegisterClient {
                    device_id: device_id.to_owned(),
                    public_key_base64: public_key,
                    name: name.to_owned(),
                });
            }
        }

        if !context.decisions.admit_client {
            set_unauthorized(&mut transition, None);
            return Ok(transition);
        }

        if eiv.len() != AES_IV_SIZE || ekey.len() != EKEY_SIZE {
            transition.response.status = 400;
            transition.response.reason = "Bad Request";
            return Ok(transition);
        }

        let encrypted_key: [u8; EKEY_SIZE] =
            ekey.try_into().map_err(|_| SessionError::InvalidPlist)?;
        let mut iv = [0; AES_IV_SIZE];
        iv.copy_from_slice(eiv);
        let mut decrypted_key = [0; AES_KEY_SIZE];
        let fairplay_status = context.fairplay.decrypt(&encrypted_key, &mut decrypted_key);
        transition.effects.push(SessionEffect::Log {
            level: SessionLogLevel::Debug,
            message: format!("fairplay_decrypt ret = {fairplay_status}"),
        });
        if let Some(user_agent) = user_agent {
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Info,
                message: format!("Client identified as User-Agent: {user_agent}"),
            });
        }
        let old_protocol = is_old_protocol_client(user_agent);
        if old_protocol {
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Info,
                message: "Client identifed as using old protocol (unhashed) AES audio key)".into(),
            });
        }
        if !old_protocol && let Some(pairing_session) = context.pairing_session {
            let mut ecdh_secret = [0; 32];
            if pairing_session.get_ecdh_secret_key(&mut ecdh_secret) > 0 {
                // lib/crypto.c's sha_init() is SHA-512; the C handler copies
                // the first 16 digest bytes back as the negotiated AES key.
                let mut hasher = Sha512::new();
                hasher.update(decrypted_key);
                hasher.update(ecdh_secret);
                decrypted_key.copy_from_slice(&hasher.finalize()[..AES_KEY_SIZE]);
            }
            ecdh_secret.zeroize();
        }

        if bool_value(request_dict, "isRemoteControlOnly") {
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Error,
                message: "Client specified AirPlay2 \"Remote Control\" protocol\n Only AirPlay v1 protocol (using NTP and timing port) is supported".into(),
            });
        }

        let timing_protocol_value = string_value(request_dict, "timingProtocol");
        let timing_protocol = timing_protocol(timing_protocol_value.as_deref());
        match timing_protocol_value.as_deref() {
            None => transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Debug,
                message: "Client did not specify timingProtocol, old protocol without offset will be used".into(),
            }),
            Some(value) if timing_protocol != TimingProtocol::Ntp => {
                transition.effects.push(SessionEffect::Log {
                    level: SessionLogLevel::Error,
                    message: format!("Client specified timingProtocol={value}, but timingProtocol= NTP is required here"),
                });
            }
            Some(_) => {}
        }

        let timing_remote_port_raw = unsigned_value(request_dict, "timingPort");
        let timing_remote_port = timing_remote_port_raw as u16;
        if timing_remote_port_raw != 0 {
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Debug,
                message: format!("timing_rport = {timing_remote_port_raw}"),
            });
        } else {
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Error,
                message: "Client did not supply timing_rport, may be using unsupported AirPlay2 \"Remote Control\" protocol".into(),
            });
        }
        transition.effects.push(SessionEffect::StartTiming {
            remote_address: context.remote_address.to_owned(),
            remote_port: timing_remote_port,
            local_port: context.ports.timing_local,
            protocol: timing_protocol,
        });
        transition.effects.push(SessionEffect::InitializeAudio {
            remote_address: context.remote_address.to_owned(),
            key: decrypted_key,
            iv,
        });
        transition.effects.push(SessionEffect::InitializeMirror {
            config: MirrorSetup::new(
                context.remote_address.to_owned(),
                context.remote_address_length,
                decrypted_key,
            ),
        });

        clear_stored_audio_keys(state);
        state.timing_exists = true;
        state.audio_exists = true;
        state.audio_running = false;
        state.mirror_exists = true;
        state.mirror_running = false;
        state.audio_key = Some(decrypted_key);
        state.audio_iv = Some(iv);
        state.timing_remote_port = Some(timing_remote_port);
        state.timing_local_port = Some(context.ports.timing_local);
        decrypted_key.zeroize();
        iv.zeroize();

        response_plist.insert(
            "timingPort".into(),
            Value::from(u64::from(context.ports.timing_local)),
        );
        response_plist.insert("eventPort".into(), Value::from(0));
        transition.effects.push(SessionEffect::LogSetupPorts);
    }

    if let Some(streams) = request_dict
        .and_then(|dictionary| dictionary.get("streams"))
        .and_then(Value::as_array)
    {
        let mut response_streams = Vec::new();
        for stream in streams {
            let Some(stream_dict) = stream.as_dictionary() else {
                transition.response.disconnect = true;
                continue;
            };
            let stream_type = unsigned_value(Some(stream_dict), "type");
            transition.effects.push(SessionEffect::Log {
                level: SessionLogLevel::Debug,
                message: format!("type = {stream_type}"),
            });
            match stream_type {
                110 => {
                    transition.effects.push(SessionEffect::ClearHlsVideoData);
                    let stream_connection_id =
                        unsigned_value(Some(stream_dict), "streamConnectionID");
                    let stream_connection_id_value = stream_dict
                        .get("streamConnectionID")
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "<missing>".into());
                    let top_level_stream_connection_id = request_dict
                        .and_then(|dictionary| dictionary.get("streamConnectionID"))
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "<missing>".into());
                    transition.effects.push(SessionEffect::Log {
                        level: SessionLogLevel::Debug,
                        message: format!("streamConnectionID (needed for AES-CTR video decryption key and iv): {stream_connection_id}; stream value={stream_connection_id_value}; top-level value={top_level_stream_connection_id}"),
                    });
                    if state.mirror_exists {
                        transition.effects.push(SessionEffect::InitializeMirrorAes {
                            stream_connection_id,
                        });
                        transition.effects.push(SessionEffect::StartMirror {
                            requested_port: context.ports.mirror_data_local,
                            show_client_fps_data: context.client_fps_data,
                        });
                        transition.effects.push(SessionEffect::Log {
                            level: SessionLogLevel::Debug,
                            message: "Mirroring initialized successfully".into(),
                        });
                        state.mirror_running = true;
                    } else {
                        transition.effects.push(SessionEffect::Log {
                            level: SessionLogLevel::Error,
                            message: "Mirroring not initialized at SETUP, playing will fail!"
                                .into(),
                        });
                        transition.response.disconnect = true;
                    }
                    state.mirror_data_port = Some(context.ports.mirror_data_local);
                    response_streams.push(plist_dict([
                        (
                            "dataPort",
                            Value::from(u64::from(context.ports.mirror_data_local)),
                        ),
                        ("type", Value::from(110)),
                    ]));
                }
                96 => {
                    let format = AudioFormat {
                        codec_type: unsigned_value(Some(stream_dict), "ct") as u8,
                        samples_per_frame: unsigned_value(Some(stream_dict), "spf") as u16,
                        using_screen: bool_value(Some(stream_dict), "usingScreen"),
                        is_media: bool_value(Some(stream_dict), "isMedia"),
                        audio_format: unsigned_value(Some(stream_dict), "audioFormat"),
                    };
                    let remote_control_port =
                        unsigned_value(Some(stream_dict), "controlPort") as u16;
                    if context.decisions.callbacks.audio_get_format {
                        transition
                            .effects
                            .push(SessionEffect::GetAudioFormat { format });
                    }
                    if state.audio_exists {
                        transition.effects.push(SessionEffect::StartAudio {
                            remote_address: context.remote_address.to_owned(),
                            ports: AudioPorts {
                                control_remote: remote_control_port,
                                control_local: context.ports.audio_control_local,
                                data_local: context.ports.audio_data_local,
                            },
                            format,
                            sample_rate: AUDIO_SAMPLE_RATE,
                        });
                        transition.effects.push(SessionEffect::Log {
                            level: SessionLogLevel::Debug,
                            message: "RAOP initialized success".into(),
                        });
                        state.audio_running = true;
                        state.audio_ports = Some(AudioPorts {
                            control_remote: remote_control_port,
                            control_local: context.ports.audio_control_local,
                            data_local: context.ports.audio_data_local,
                        });
                    } else {
                        transition.effects.push(SessionEffect::Log {
                            level: SessionLogLevel::Error,
                            message: "RAOP not initialized at SETUP, playing will fail!".into(),
                        });
                        transition.response.disconnect = true;
                    }
                    response_streams.push(plist_dict([
                        (
                            "dataPort",
                            Value::from(u64::from(context.ports.audio_data_local)),
                        ),
                        (
                            "controlPort",
                            Value::from(u64::from(context.ports.audio_control_local)),
                        ),
                        ("type", Value::from(96)),
                    ]));
                }
                _ => {
                    transition.effects.push(SessionEffect::Log {
                        level: SessionLogLevel::Error,
                        message: format!(
                            "SETUP tries to setup stream of unknown type {stream_type}"
                        ),
                    });
                    transition.response.disconnect = true;
                }
            }
        }
        response_plist.insert("streams".into(), Value::Array(response_streams));
    }

    transition.response.headers.push((
        "Content-Type".into(),
        "application/x-apple-binary-plist".into(),
    ));
    let mut body = Vec::new();
    plist::to_writer_binary(&mut body, &Value::Dictionary(response_plist))
        .map_err(|error| SessionError::PlistEncode(error.to_string()))?;
    transition.response.body = Some(body);
    Ok(transition)
}

/// Match both legacy User-Agent checks in `raop_handlers.h`.
pub fn is_old_protocol_client(user_agent: Option<&str>) -> bool {
    user_agent.is_some_and(|agent| {
        OLD_PROTOCOL_CLIENT_USER_AGENT_LIST.contains(agent) || agent.contains("AirMyPC")
    })
}

/// Handle RECORD's latency and analog-jack response headers.
pub fn record(audio_delay_micros: i32) -> SessionResponse {
    let delay = audio_delay_micros as u64;
    let samples = delay.wrapping_mul(u64::from(AUDIO_SAMPLE_RATE)) / SECOND_IN_USECS;
    let latency = samples as u32;
    SessionResponse {
        headers: vec![
            ("Audio-Latency".into(), latency.to_string()),
            ("Audio-Jack-Status".into(), "connected; type=analog".into()),
        ],
        ..SessionResponse::default()
    }
}

/// Parse FLUSH's `RTP-Info` header and emit an audio flush effect when present.
pub fn flush(state: &RaopSessionState, rtp_info: Option<&str>) -> SessionTransition {
    let mut transition = SessionTransition::default();
    if state.audio_exists {
        transition.effects.push(SessionEffect::FlushAudio {
            next_sequence_number: parse_flush_sequence(rtp_info),
        });
    }
    transition
}

/// Handle TEARDOWN with the source's type-96 precedence when both stream types occur.
pub fn teardown(
    state: &mut RaopSessionState,
    body: &[u8],
    hls_connection_count: usize,
    hls_pending: bool,
) -> SessionTransition {
    let request = parse_binary_plist_or_empty(body);
    let mut teardown_audio = false;
    let mut teardown_mirror = false;
    if let Some(streams) = request
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("streams"))
        .and_then(Value::as_array)
    {
        for stream in streams {
            if let Some(stream) = stream.as_dictionary() {
                match unsigned_value(Some(stream), "type") {
                    96 => teardown_audio = true,
                    110 => teardown_mirror = true,
                    _ => {}
                }
            }
        }
    }

    let mut transition = SessionTransition::default();
    transition
        .response
        .headers
        .push(("Connection".into(), "close".into()));
    transition.response.close_connection = true;
    if teardown_audio {
        if state.audio_exists {
            transition.effects.push(SessionEffect::StopAudio);
            transition
                .effects
                .push(SessionEffect::StopCoverArtRendering);
            state.audio_running = false;
        }
    } else if teardown_mirror {
        transition
            .effects
            .push(SessionEffect::VideoReset(if hls_pending {
                VideoResetReason::RtpToHlsTeardown
            } else {
                VideoResetReason::RtpShutdown
            }));
        if state.mirror_exists {
            transition.effects.push(SessionEffect::StopMirror);
            state.mirror_running = false;
        }
    } else {
        if state.audio_exists {
            transition.effects.push(SessionEffect::DestroyAudio);
            state.audio_exists = false;
            state.audio_running = false;
            state.audio_ports = None;
        }
        if state.mirror_exists {
            transition.effects.push(SessionEffect::DestroyMirror);
            state.mirror_exists = false;
            state.mirror_running = false;
            state.mirror_data_port = None;
            transition
                .effects
                .push(SessionEffect::VideoReset(VideoResetReason::RtpShutdown));
        }
        if hls_connection_count != 0 {
            transition
                .effects
                .push(SessionEffect::VideoReset(VideoResetReason::HlsShutdown));
        }
        clear_stored_audio_keys(state);
    }
    transition
}

/// Return the C handler's RTP flush sequence value (`-1` when absent/prefix mismatch).
pub fn parse_flush_sequence(rtp_info: Option<&str>) -> i32 {
    let Some(value) = rtp_info.filter(|value| value.as_bytes().starts_with(b"seq=")) else {
        return -1;
    };
    parse_c_long(&value.as_bytes()[4..]) as i32
}

fn parse_c_long(value: &[u8]) -> i64 {
    let mut index = 0;
    while value.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let negative = match value.get(index) {
        Some(b'-') => {
            index += 1;
            true
        }
        Some(b'+') => {
            index += 1;
            false
        }
        _ => false,
    };
    let start = index;
    let mut magnitude = 0_u64;
    while let Some(byte @ b'0'..=b'9') = value.get(index) {
        magnitude = magnitude
            .saturating_mul(10)
            .saturating_add(u64::from(*byte - b'0'));
        index += 1;
    }
    if index == start {
        return 0;
    }
    if negative {
        if magnitude >= C_LONG_MIN_MAGNITUDE {
            C_LONG_MIN
        } else {
            -(magnitude as i64)
        }
    } else if magnitude > C_LONG_MAX as u64 {
        C_LONG_MAX
    } else {
        magnitude as i64
    }
}

fn timing_protocol(value: Option<&str>) -> TimingProtocol {
    match value {
        None => TimingProtocol::Unspecified,
        Some(value) if "NTP".starts_with(value) => TimingProtocol::Ntp,
        Some(value) if "None".starts_with(value) => TimingProtocol::None,
        Some(_) => TimingProtocol::Other,
    }
}

fn parse_binary_plist_or_empty(bytes: &[u8]) -> Value {
    if bytes.starts_with(b"bplist00")
        && let Ok(value) = Value::from_reader(Cursor::new(bytes))
    {
        return value;
    }
    Value::Dictionary(Dictionary::new())
}

fn data_value<'a>(dictionary: Option<&'a Dictionary>, key: &str) -> Option<&'a [u8]> {
    dictionary?.get(key)?.as_data()
}

fn string_value(dictionary: Option<&Dictionary>, key: &str) -> Option<String> {
    dictionary?.get(key)?.as_string().map(str::to_owned)
}

fn unsigned_value(dictionary: Option<&Dictionary>, key: &str) -> u64 {
    dictionary
        .and_then(|dictionary| dictionary.get(key))
        .and_then(|value| {
            value
                .as_unsigned_integer()
                // AirPlay IDs are uint64 values. The plist reader can expose
                // high-bit binary-plist integers as signed i64 values, while
                // libplist's plist_get_uint_val reads the same bits as u64.
                .or_else(|| value.as_signed_integer().map(|signed| signed as u64))
        })
        .unwrap_or_default()
}

fn bool_value(dictionary: Option<&Dictionary>, key: &str) -> bool {
    dictionary
        .and_then(|dictionary| dictionary.get(key))
        .and_then(Value::as_boolean)
        .unwrap_or(false)
}

fn clear_stored_audio_keys(state: &mut RaopSessionState) {
    if let Some(key) = state.audio_key.as_mut() {
        key.zeroize();
    }
    if let Some(iv) = state.audio_iv.as_mut() {
        iv.zeroize();
    }
    state.audio_key = None;
    state.audio_iv = None;
}

fn set_unauthorized(transition: &mut SessionTransition, challenge: Option<&str>) {
    transition.response.status = 401;
    transition.response.reason = "Unauthorized";
    if let Some(challenge) = challenge {
        transition
            .response
            .headers
            .push(("WWW-Authenticate".into(), challenge.to_owned()));
    }
}

fn plist_dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        dictionary.insert(key.into(), value);
    }
    Value::Dictionary(dictionary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_protocol_user_agent_matches_both_source_strstr_checks() {
        assert!(!is_old_protocol_client(None));
        assert!(is_old_protocol_client(Some("AirMyPC/2.0")));
        assert!(is_old_protocol_client(Some("AirMyPC")));
        assert!(is_old_protocol_client(Some("")));
        assert!(is_old_protocol_client(Some("AirMyPC/7200")));
        assert!(is_old_protocol_client(Some("Player AirMyPC/2.0")));
        assert!(is_old_protocol_client(Some("prefixAirMyPC/2.0suffix")));
        assert!(!is_old_protocol_client(Some("AirPlay/870.14.1")));
        assert!(!is_old_protocol_client(Some("AirPlay/2.0")));
    }

    fn binary_plist(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &value).expect("test plist encodes");
        bytes
    }

    #[test]
    fn unsigned_value_preserves_high_bit_uint64_from_binary_plist() {
        let stream_connection_id = 0xf5f0_1234_89ab_cdef;
        let body = binary_plist(plist_dict([(
            "streamConnectionID",
            Value::from(stream_connection_id),
        )]));
        let decoded = Value::from_reader(Cursor::new(body)).expect("binary plist decodes");
        let dictionary = decoded.as_dictionary().expect("root is a dictionary");

        assert_eq!(
            unsigned_value(Some(dictionary), "streamConnectionID"),
            stream_connection_id
        );
    }

    fn streams_body(types: &[u64]) -> Vec<u8> {
        let streams = types
            .iter()
            .map(|stream_type| plist_dict([("type", Value::from(*stream_type))]))
            .collect();
        binary_plist(plist_dict([("streams", Value::Array(streams))]))
    }

    fn setup_body_with_audio() -> Vec<u8> {
        let mut stream = Dictionary::new();
        stream.insert("type".into(), Value::from(96));
        stream.insert("controlPort".into(), Value::from(6001));
        stream.insert("ct".into(), Value::from(2));
        stream.insert("spf".into(), Value::from(352));
        stream.insert("audioFormat".into(), Value::from(0x100));
        stream.insert("isMedia".into(), Value::Boolean(true));
        stream.insert("usingScreen".into(), Value::Boolean(false));
        let mut request = Dictionary::new();
        request.insert("eiv".into(), Value::Data(vec![7; AES_IV_SIZE]));
        request.insert("ekey".into(), Value::Data(vec![9; EKEY_SIZE]));
        request.insert("timingPort".into(), Value::from(7000));
        request.insert("timingProtocol".into(), Value::String("N".into()));
        request.insert(
            "streams".into(),
            Value::Array(vec![Value::Dictionary(stream)]),
        );
        binary_plist(Value::Dictionary(request))
    }

    #[test]
    fn record_latency_matches_the_c_unsigned_calculation() {
        assert_eq!(record(500_000).headers[0].1, "22050");
        assert_eq!(record(-1).headers[0].1, "4154504685");
    }

    #[test]
    fn flush_matches_strtol_prefix_and_absent_header_behavior() {
        assert_eq!(parse_flush_sequence(None), -1);
        assert_eq!(parse_flush_sequence(Some("RTP-Info: seq=5")), -1);
        assert_eq!(parse_flush_sequence(Some("seq=123tail")), 123);
        assert_eq!(parse_flush_sequence(Some("seq= +7tail")), 7);
        assert_eq!(parse_flush_sequence(Some("seq=tail")), 0);
    }

    #[test]
    fn teardown_audio_has_precedence_and_full_teardown_resets_sessions() {
        let mut state = RaopSessionState {
            audio_exists: true,
            audio_running: true,
            mirror_exists: true,
            mirror_running: true,
            ..RaopSessionState::default()
        };
        let audio_first = teardown(&mut state, &streams_body(&[110, 96]), 0, false);
        assert!(audio_first.effects.contains(&SessionEffect::StopAudio));
        assert!(!audio_first.effects.contains(&SessionEffect::StopMirror));

        let full = teardown(&mut state, &[], 1, false);
        assert!(full.effects.contains(&SessionEffect::DestroyAudio));
        assert!(full.effects.contains(&SessionEffect::DestroyMirror));
        assert!(
            full.effects
                .contains(&SessionEffect::VideoReset(VideoResetReason::HlsShutdown))
        );
        assert!(!state.audio_exists && !state.mirror_exists);
    }

    #[test]
    fn teardown_mirror_resets_video_before_stopping_the_listener() {
        let mut state = RaopSessionState {
            mirror_exists: true,
            mirror_running: true,
            ..RaopSessionState::default()
        };
        let transition = teardown(&mut state, &streams_body(&[110]), 0, true);
        assert_eq!(
            transition.effects,
            vec![
                SessionEffect::VideoReset(VideoResetReason::RtpToHlsTeardown),
                SessionEffect::StopMirror,
            ]
        );
        assert!(!state.mirror_running);
    }

    #[test]
    fn plist_parser_only_accepts_the_binary_format_used_by_raop_handlers() {
        let mut dictionary = Dictionary::new();
        dictionary.insert("timingPort".into(), Value::from(7010));
        let parsed = parse_binary_plist_or_empty(&binary_plist(Value::Dictionary(dictionary)));
        assert_eq!(unsigned_value(parsed.as_dictionary(), "timingPort"), 7010);
        assert_eq!(
            parse_binary_plist_or_empty(b"<?xml version=\"1.0\"?>")
                .as_dictionary()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn first_setup_negotiates_keys_ports_and_returns_binary_plist() {
        let fairplay = FairPlay::new(std::ptr::null_mut());
        let mut state = RaopSessionState::default();
        let transition = setup(
            &mut state,
            &setup_body_with_audio(),
            Some("dacp"),
            Some("remote"),
            Some("AirMyPC/2.0"),
            SetupContext {
                remote_address: "192.0.2.1",
                remote_address_length: 4,
                ports: ServerPorts {
                    timing_local: 7001,
                    audio_control_local: 7002,
                    audio_data_local: 7003,
                    mirror_data_local: 7004,
                },
                decisions: SetupDecisions {
                    authorized: true,
                    admit_client: true,
                    callbacks: CallbackAvailability {
                        report_client_request: true,
                        register_client: true,
                        audio_get_format: true,
                    },
                    authorization_challenge: None,
                },
                fairplay: &fairplay,
                pairing_session: None,
                client_fps_data: 1,
            },
        )
        .expect("valid setup request");

        assert_eq!(transition.response.status, 200);
        assert_eq!(
            transition.response.headers,
            vec![(
                "Content-Type".into(),
                "application/x-apple-binary-plist".into()
            )]
        );
        let response = Value::from_reader(Cursor::new(
            transition
                .response
                .body
                .as_deref()
                .expect("setup plist body"),
        ))
        .expect("response plist decodes");
        let response = response.as_dictionary().expect("dictionary response");
        assert_eq!(unsigned_value(Some(response), "timingPort"), 7001);
        assert_eq!(unsigned_value(Some(response), "eventPort"), 0);
        let response_stream = &response["streams"].as_array().expect("streams")[0];
        assert_eq!(
            unsigned_value(response_stream.as_dictionary(), "dataPort"),
            7003
        );
        assert!(state.timing_exists && state.audio_exists && state.mirror_exists);
        assert_eq!(state.audio_key, Some([0; AES_KEY_SIZE]));
        assert!(transition.effects.contains(&SessionEffect::StartTiming {
            remote_address: "192.0.2.1".into(),
            remote_port: 7000,
            local_port: 7001,
            protocol: TimingProtocol::Ntp,
        }));
        assert!(transition.effects.contains(&SessionEffect::Log {
            level: SessionLogLevel::Debug,
            message: "timing_rport = 7000".into(),
        }));
        assert!(transition.effects.contains(&SessionEffect::LogSetupPorts));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Log {
                level: SessionLogLevel::Debug,
                message,
            } if message.starts_with("fairplay_decrypt ret = ")
        )));
        assert!(transition.effects.contains(&SessionEffect::Log {
            level: SessionLogLevel::Info,
            message: "Client identified as User-Agent: AirMyPC/2.0".into(),
        }));
        assert!(transition.effects.contains(&SessionEffect::Log {
            level: SessionLogLevel::Info,
            message: "Client identifed as using old protocol (unhashed) AES audio key)".into(),
        }));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::StartAudio {
                ports: AudioPorts {
                    control_remote: 6001,
                    control_local: 7002,
                    data_local: 7003,
                },
                format: AudioFormat { codec_type: 2, .. },
                ..
            }
        )));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::GetAudioFormat {
                format: AudioFormat { codec_type: 2, .. }
            }
        )));
    }

    #[test]
    fn setup_emits_source_timing_diagnostics_in_order() {
        let fairplay = FairPlay::new(std::ptr::null_mut());
        let mut request = parse_binary_plist_or_empty(&setup_body_with_audio());
        let dictionary = request
            .as_dictionary_mut()
            .expect("setup request is a dictionary");
        dictionary.insert("isRemoteControlOnly".into(), Value::Boolean(true));
        dictionary.insert("timingProtocol".into(), Value::String("None".into()));
        dictionary.remove("timingPort");
        let audio_stream = dictionary["streams"].as_array().unwrap()[0].clone();
        dictionary.insert(
            "streams".into(),
            Value::Array(vec![
                plist_dict([
                    ("type", Value::from(110)),
                    ("streamConnectionID", Value::from(123)),
                ]),
                audio_stream,
                plist_dict([("type", Value::from(42))]),
            ]),
        );

        let transition = setup(
            &mut RaopSessionState::default(),
            &binary_plist(request),
            None,
            None,
            None,
            SetupContext {
                remote_address: "192.0.2.1",
                remote_address_length: 4,
                ports: ServerPorts {
                    timing_local: 7001,
                    audio_control_local: 7002,
                    audio_data_local: 7003,
                    mirror_data_local: 7004,
                },
                decisions: SetupDecisions {
                    authorized: true,
                    admit_client: true,
                    callbacks: CallbackAvailability::default(),
                    authorization_challenge: None,
                },
                fairplay: &fairplay,
                pairing_session: None,
                client_fps_data: 0,
            },
        )
        .expect("valid SETUP request");

        let logs = transition
            .effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::Log { level, message } => Some((*level, message.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            logs,
            vec![
                (SessionLogLevel::Debug, "fairplay_decrypt ret = -1",),
                (
                    SessionLogLevel::Error,
                    "Client specified AirPlay2 \"Remote Control\" protocol\n Only AirPlay v1 protocol (using NTP and timing port) is supported",
                ),
                (
                    SessionLogLevel::Error,
                    "Client specified timingProtocol=None, but timingProtocol= NTP is required here",
                ),
                (
                    SessionLogLevel::Error,
                    "Client did not supply timing_rport, may be using unsupported AirPlay2 \"Remote Control\" protocol",
                ),
                (SessionLogLevel::Debug, "type = 110"),
                (
                    SessionLogLevel::Debug,
                    "streamConnectionID (needed for AES-CTR video decryption key and iv): 123",
                ),
                (SessionLogLevel::Debug, "Mirroring initialized successfully"),
                (SessionLogLevel::Debug, "type = 96"),
                (SessionLogLevel::Debug, "RAOP initialized success"),
                (SessionLogLevel::Debug, "type = 42"),
                (
                    SessionLogLevel::Error,
                    "SETUP tries to setup stream of unknown type 42",
                ),
            ]
        );
        assert!(transition.response.disconnect);
        assert!(transition.effects.contains(&SessionEffect::StartTiming {
            remote_address: "192.0.2.1".into(),
            remote_port: 0,
            local_port: 7001,
            protocol: TimingProtocol::None,
        }));
    }

    #[test]
    fn setup_forwards_fairplay_decrypted_key_into_audio_and_mirror_runtimes() {
        let mut fairplay = FairPlay::new(std::ptr::null_mut());
        let mut handshake = [0_u8; 164];
        for (index, byte) in handshake.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17).wrapping_add(6);
        }
        handshake[4] = 3;
        handshake[12] = 2;
        let mut handshake_response = [0_u8; 32];
        assert_eq!(fairplay.handshake(&handshake, &mut handshake_response), 0);

        let mut ekey = [0_u8; EKEY_SIZE];
        for (index, byte) in ekey.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(29).wrapping_add(14);
        }
        let mut expected_key = [0_u8; AES_KEY_SIZE];
        assert_eq!(fairplay.decrypt(&ekey, &mut expected_key), 0);

        let iv = (0..AES_IV_SIZE as u8).collect::<Vec<_>>();
        let mut request = Dictionary::new();
        request.insert("eiv".into(), Value::Data(iv.clone()));
        request.insert("ekey".into(), Value::Data(ekey.to_vec()));
        request.insert("timingPort".into(), Value::from(7000));
        let body = binary_plist(Value::Dictionary(request));
        let mut state = RaopSessionState::default();
        let transition = setup(
            &mut state,
            &body,
            None,
            None,
            Some("AirMyPC/2.0"),
            SetupContext {
                remote_address: "192.0.2.1",
                remote_address_length: 4,
                ports: ServerPorts::default(),
                decisions: SetupDecisions::allow_all(),
                fairplay: &fairplay,
                pairing_session: None,
                client_fps_data: 0,
            },
        )
        .expect("binary SETUP plist is valid");

        assert_eq!(state.audio_key, Some(expected_key));
        assert_eq!(state.audio_iv, Some(iv.as_slice().try_into().unwrap()));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::InitializeAudio { key, iv: effect_iv, .. }
                if key == &expected_key && effect_iv == iv.as_slice()
        )));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::InitializeMirror { config }
                if config.audio_key == expected_key
        )));
    }

    #[test]
    fn setup_sha512_hashes_new_protocol_audio_key_with_pairing_secret() {
        let (pairing, _) =
            uxplay_pairing::Pairing::init_generate(b"raop-session-test", None).unwrap();
        let mut pairing_session = pairing.session().unwrap();
        pairing_session.set_setup_status();
        assert_eq!(pairing_session.handshake(&[9; 32], &[1; 32]).unwrap(), 0);

        let mut ecdh_secret = [0; 32];
        assert_eq!(pairing_session.get_ecdh_secret_key(&mut ecdh_secret), 1);

        let mut fairplay = FairPlay::new(std::ptr::null_mut());
        let mut handshake = [0_u8; 164];
        for (index, byte) in handshake.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17).wrapping_add(6);
        }
        handshake[4] = 3;
        handshake[12] = 2;
        let mut handshake_response = [0_u8; 32];
        assert_eq!(fairplay.handshake(&handshake, &mut handshake_response), 0);

        let mut ekey = [0_u8; EKEY_SIZE];
        for (index, byte) in ekey.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(29).wrapping_add(14);
        }
        let mut fairplay_key = [0_u8; AES_KEY_SIZE];
        assert_eq!(fairplay.decrypt(&ekey, &mut fairplay_key), 0);
        let mut hasher = Sha512::new();
        hasher.update(fairplay_key);
        hasher.update(ecdh_secret);
        let expected_key: [u8; AES_KEY_SIZE] = hasher.finalize()[..AES_KEY_SIZE]
            .try_into()
            .expect("SHA-256 output has enough bytes");

        let iv = (0..AES_IV_SIZE as u8).collect::<Vec<_>>();
        let mut request = Dictionary::new();
        request.insert("eiv".into(), Value::Data(iv.clone()));
        request.insert("ekey".into(), Value::Data(ekey.to_vec()));
        let body = binary_plist(Value::Dictionary(request));
        let mut state = RaopSessionState::default();
        let transition = setup(
            &mut state,
            &body,
            None,
            None,
            Some("AppleTV6,2"),
            SetupContext {
                remote_address: "192.0.2.1",
                remote_address_length: 4,
                ports: ServerPorts::default(),
                decisions: SetupDecisions::allow_all(),
                fairplay: &fairplay,
                pairing_session: Some(&pairing_session),
                client_fps_data: 0,
            },
        )
        .expect("binary SETUP plist is valid");

        assert_eq!(state.audio_key, Some(expected_key));
        assert_eq!(state.audio_iv, Some(iv.as_slice().try_into().unwrap()));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::InitializeAudio { key, iv: effect_iv, .. }
                if key == &expected_key && effect_iv == iv.as_slice()
        )));
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::InitializeMirror { config }
                if config.audio_key == expected_key
        )));
        ecdh_secret.zeroize();
        fairplay_key.zeroize();
    }

    #[test]
    fn invalid_setup_key_lengths_return_bad_request_without_starting_sessions() {
        let fairplay = FairPlay::new(std::ptr::null_mut());
        let mut request = Dictionary::new();
        request.insert("eiv".into(), Value::Data(vec![0; 15]));
        request.insert("ekey".into(), Value::Data(vec![0; EKEY_SIZE]));
        let mut state = RaopSessionState::default();
        let transition = setup(
            &mut state,
            &binary_plist(Value::Dictionary(request)),
            None,
            None,
            None,
            SetupContext {
                remote_address: "192.0.2.1",
                remote_address_length: 4,
                ports: ServerPorts::default(),
                decisions: SetupDecisions::allow_all(),
                fairplay: &fairplay,
                pairing_session: None,
                client_fps_data: 0,
            },
        )
        .expect("invalid keys are an RTSP 400 response");
        assert_eq!(transition.response.status, 400);
        assert!(!state.timing_exists && !state.audio_exists && !state.mirror_exists);
        assert!(transition.effects.is_empty());
    }
}
