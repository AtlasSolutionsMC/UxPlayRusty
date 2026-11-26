//! Rust-owned AirPlay/RAOP startup, discovery, and protocol/media runtime.

use crate::discovery_publisher::DiscoveryPublisher;
use crate::discovery_services::{
    DiscoveryServices, ServiceNames, ServiceTxtRecords, build_service_txt, prepare_service_names,
};
use crate::discovery_state::DiscoveryState;
use crate::discovery_txt::TxtRecordInputs;
use crate::host_identity::{self, MacAddress};
use crate::http_transport::ConnectionRegistry;
use crate::http_transport::ServerConfig;
use crate::netutils::{self, AddressFamily, Transport};
use crate::options::{Options, RaspberryPiModel};
use crate::pairing_registry::PairingRegistry;
use crate::password_auth::PasswordAuth;
use crate::protocol_connection::{
    OwnedInfoContext, ProtocolContext, ProtocolEffectSink, serve_protocol_with_registry,
};
use crate::protocol_dispatch::HandlerEffect;
use crate::raop_media_runtime::{
    FeedbackWatchdogTick, MediaOutputRuntime, RendererEventLoop, SessionMediaConfig,
    source_volume_db, source_volume_to_gstreamer,
};
use std::env;
use std::io;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uxplay_pairing::Pairing;
use uxplay_raop_session::{CallbackAvailability, ServerPorts, SetupDecisions};

/// Starts the TCP listener, advertises the two DNS-SD services, and serves the
/// staged protocol adapter.
pub fn run(options: Options) -> ExitCode {
    match run_server(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(ServerRuntimeError::ExitCode(code)) => code,
        Err(ServerRuntimeError::Io(error)) => {
            eprintln!(
                "UxPlay Rust: server startup failed: {}",
                crate::compat::socket_error_description(&error)
            );
            ExitCode::from(1)
        }
    }
}

enum ServerRuntimeError {
    Io(io::Error),
    ExitCode(ExitCode),
}

impl From<io::Error> for ServerRuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn run_server(options: Options) -> Result<(), ServerRuntimeError> {
    crate::logger::set_console_output_utf8();
    let logger_level = if options.debug_log {
        if options.suppress_packet_debug_data {
            crate::logger::DEBUG
        } else {
            crate::logger::DEBUG_DATA
        }
    } else {
        crate::logger::INFO
    };
    let logger = crate::logger::stdout_logger(logger_level);
    logger.log(
        crate::logger::INFO,
        "UxPlay 1.74: An Open-Source AirPlay mirroring and audio-streaming server.",
    );

    let mut renderer_loop = match RendererEventLoop::start(&options, &logger) {
        Ok(renderer_loop) => renderer_loop,
        Err(error)
            if error
                .to_string()
                .contains("required GStreamer plugins are unavailable") =>
        {
            println!("*** ERROR: stopping");
            return Err(ServerRuntimeError::ExitCode(ExitCode::from(1)));
        }
        Err(error) => return Err(error.into()),
    };
    let outputs = Arc::new(Mutex::new(MediaOutputRuntime::new_with_logger(
        &options, &logger,
    )?));
    let current_volume_db = Arc::new(Mutex::new(options.initial_volume_attenuation_db as f32));
    let media_config = SessionMediaConfig {
        use_audio: options.use_audio && options.audio_sink != "0",
        use_video: options.video_sink != "0",
        close_window_on_client_stop: options.close_window_on_client_stop,
        nofreeze: options.nofreeze,
        peer_to_peer: options.peer_to_peer,
        render_cover_art: options.render_cover_art,
        cover_art_filename: options.cover_art_filename.clone(),
        metadata_filename: options.metadata_filename.clone(),
        dacp_filename: options.dacp_file.clone(),
        taper_volume: options.taper_volume,
        db_range: options.db_range,
        audio_delay_alac_ns: options.audio_delay_alac_ns,
        audio_delay_aac_ns: options.audio_delay_aac_ns,
        current_volume_db: Arc::clone(&current_volume_db),
        outputs: Arc::clone(&outputs),
        logger: Some(Arc::clone(&logger)),
    };
    crate::raop_media_runtime::initialize_audio_output_files(&media_config);
    let mac = host_identity::select_mac_address(&options)?;
    let (listeners, server_port) = bind_tcp_listeners(options.tcp_ports[1], options.peer_to_peer)?;
    let identity = ServerIdentity::build(&options, mac)?;
    let registration = pairing_registry(&options);

    let mut publisher = DiscoveryPublisher::new_with_peer_to_peer_and_logger(
        DiscoveryServices::new(identity.names.clone()),
        options.peer_to_peer,
        Arc::clone(&logger),
    );
    if let Some(error_code) = publisher.initialization_error_code() {
        logger.log(
            crate::logger::ERR,
            format!("Could not initialize dnssd library!: error {error_code}"),
        );
        stop_failed_startup(&mut publisher, &outputs, &mut renderer_loop, &options);
        return Err(ServerRuntimeError::ExitCode(ExitCode::from(1)));
    }
    let raop_registration = publisher
        .register_raop(server_port, &identity.txt.raop)
        .map_err(|error| io::Error::other(format!("RAOP DNS-SD registration failed: {error:?}")))?;
    if raop_registration.return_code != 0 {
        if should_abort_discovery_startup(
            raop_registration.return_code,
            options.ble_filename.is_some(),
        ) {
            eprintln!(
                "UxPlay Rust: dnssd_register_raop failed with error code {}",
                raop_registration.return_code
            );
            write_dns_sd_error(&dnssd_registration_error_text(
                raop_registration.return_code,
                uses_external_dns_sd(),
                &current_app_name(),
            ));
            stop_failed_startup(&mut publisher, &outputs, &mut renderer_loop, &options);
            return Err(ServerRuntimeError::ExitCode(ExitCode::from(1)));
        }
        eprintln!(
            "UxPlay Rust: dnssd_register_raop failed; ignoring because Bluetooth LE service discovery may be available"
        );
    }
    let airplay_registration = publisher
        .register_airplay(server_port, &identity.txt.airplay)
        .map_err(|error| {
            io::Error::other(format!("AirPlay DNS-SD registration failed: {error:?}"))
        })?;
    if airplay_registration.return_code != 0 {
        if should_abort_discovery_startup(
            airplay_registration.return_code,
            options.ble_filename.is_some(),
        ) {
            eprintln!(
                "UxPlay Rust: dnssd_register_airplay failed with error code {}",
                airplay_registration.return_code
            );
            write_dns_sd_error(&dnssd_registration_error_text(
                airplay_registration.return_code,
                uses_external_dns_sd(),
                &current_app_name(),
            ));
            stop_failed_startup(&mut publisher, &outputs, &mut renderer_loop, &options);
            return Err(ServerRuntimeError::ExitCode(ExitCode::from(1)));
        }
        eprintln!(
            "UxPlay Rust: dnssd_register_airplay failed; ignoring because Bluetooth LE service discovery may be available"
        );
    }
    if !publisher.is_running() {
        eprintln!("UxPlay Rust: no mDNS responder socket is available; DNS-SD is inactive");
    }

    let hardware_address = mac.octets();
    let custom_profile = hls_custom_profile(&options);
    let video_store = Arc::new(Mutex::new(
        crate::airplay_video::AirPlayVideoStore::with_profile_and_languages(
            custom_profile,
            options.requested_language.clone(),
            options.subtitle_language.clone(),
            system_languages(),
        ),
    ));
    let event_channel = Arc::new(crate::airplay_event_channel::AirPlayEventChannel::default());
    let connection_registry = Arc::new(ConnectionRegistry::default());
    let context = ProtocolContext {
        hls_support: options.hls_support,
        local_uri_prefix: format!("http://localhost:{server_port}"),
        event_channel: Arc::clone(&event_channel),
        video_store: Arc::clone(&video_store),
        hardware_address: Some(hardware_address.to_vec()),
        raop_info: Some(identity.info_context(&options)),
        client_volume: Arc::clone(&current_volume_db),
        parameter_state: crate::raop_parameters::SetParameterContext {
            hls_volume_callback: options.use_audio && options.audio_sink != "0",
            ..crate::raop_parameters::SetParameterContext::default()
        },
        media_config: media_config.clone(),
        pairing: Some(Arc::clone(&identity.pairing)),
        registration: Some(Arc::clone(&registration)),
        pin_mode: options.pin_password_mode == 1,
        pairing_pin: Arc::new(Mutex::new(options.pin.unwrap_or(0))),
        server_ports: ServerPorts {
            timing_local: options.udp_ports[0],
            audio_control_local: options.udp_ports[1],
            audio_data_local: options.udp_ports[2],
            mirror_data_local: options.tcp_ports[0],
        },
        setup_decisions: SetupDecisions {
            authorized: options.pin_password_mode <= 1 && options.password.is_none(),
            admit_client: true,
            callbacks: CallbackAvailability {
                report_client_request: true,
                register_client: options.registration_list,
                audio_get_format: true,
            },
            authorization_challenge: None,
        },
        password_auth: PasswordAuth::new(options.pin_password_mode, options.password.clone()),
        allowed_clients: options.allowed_clients.clone(),
        blocked_clients: options.blocked_clients.clone(),
        restrict_clients: options.restrict_clients,
        nohold: options.nohold,
        connection_registry: Some(Arc::clone(&connection_registry)),
        client_fps_data: u8::from(options.show_client_fps),
        audio_delay_micros: audio_delay_micros(&options),
        ..ProtocolContext::default()
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop_requested = Arc::new(AtomicBool::new(false));
    let restart_requested = Arc::new(AtomicBool::new(false));
    let effects: Arc<dyn ProtocolEffectSink> = Arc::new(StartupEffects {
        registration,
        media_config: media_config.clone(),
        hls_support: options.hls_support,
        event_channel: Arc::clone(&event_channel),
        video_store: Arc::clone(&video_store),
        restart_requested: Arc::clone(&restart_requested),
        shutdown: Arc::clone(&shutdown),
    });

    let shutdown_on_signal = Arc::clone(&shutdown);
    let stop_on_signal = Arc::clone(&stop_requested);
    ctrlc::set_handler(move || {
        stop_on_signal.store(true, std::sync::atomic::Ordering::Release);
        shutdown_on_signal.store(true, std::sync::atomic::Ordering::Release);
    })
    .map_err(|error| io::Error::other(format!("could not install shutdown handler: {error}")))?;

    let watchdog_stop = Arc::new(AtomicBool::new(false));
    let watchdog_thread = start_feedback_watchdog(
        Arc::clone(&outputs),
        Arc::clone(&effects),
        Arc::clone(&restart_requested),
        Arc::clone(&watchdog_stop),
        options.missed_feedback_limit_seconds,
        HlsEosWatchdog {
            enabled: options.hls_support && media_config.use_video,
            video_store: Arc::clone(&video_store),
            event_channel: Arc::clone(&event_channel),
            connection_registry: Arc::clone(&connection_registry),
        },
    )?;

    write_ble_handoff(&options, server_port);
    let mut listeners = Some(listeners);
    let result = loop {
        let current_listeners = match listeners.take() {
            Some(listeners) => listeners,
            None => match bind_tcp_listeners(server_port, options.peer_to_peer) {
                Ok((listeners, _)) => listeners,
                Err(error) => break Err(error),
            },
        };
        let result = serve_protocol_with_registry(
            current_listeners,
            Arc::clone(&shutdown),
            ServerConfig::default(),
            context.clone(),
            Arc::clone(&connection_registry),
            Arc::clone(&effects),
        );
        if result.is_ok()
            && restart_requested.swap(false, std::sync::atomic::Ordering::AcqRel)
            && !stop_requested.load(std::sync::atomic::Ordering::Acquire)
        {
            shutdown.store(false, std::sync::atomic::Ordering::Release);
            if stop_requested.load(std::sync::atomic::Ordering::Acquire) {
                shutdown.store(true, std::sync::atomic::Ordering::Release);
                break Ok(());
            }
            if media_config.use_audio {
                audio_renderer_rs::port::audio_renderer_stop();
            }
            if media_config.use_video
                && let Err(error) = video_renderer_rs::port::video_renderer_restore_mirror()
            {
                eprintln!("UxPlay Rust: could not reset the video renderer: {error}");
            }
            continue;
        }
        break result;
    };
    watchdog_stop.store(true, std::sync::atomic::Ordering::Release);
    let _ = watchdog_thread.join();
    outputs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .shutdown();
    publisher.stop();
    renderer_loop.stop();
    remove_output_files(&options);
    result.map_err(ServerRuntimeError::Io)
}

fn system_languages() -> String {
    let language = env::var("LANGUAGE").ok().filter(|value| !value.is_empty());
    if let Some(language) = language {
        return language;
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .find(|value| !value.is_empty())
        .map(|mut value| {
            if let Some(encoding) = value.find('.') {
                value.truncate(encoding);
            }
            value.replace('_', "-")
        })
        .unwrap_or_default()
}

fn audio_delay_micros(options: &Options) -> i32 {
    options.audio_latency_us.unwrap_or(250_000)
}

fn stop_failed_startup(
    publisher: &mut DiscoveryPublisher,
    outputs: &Arc<Mutex<MediaOutputRuntime>>,
    renderer_loop: &mut RendererEventLoop,
    options: &Options,
) {
    publisher.stop();
    outputs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .shutdown();
    renderer_loop.stop();
    remove_output_files(options);
}

fn uses_external_dns_sd() -> bool {
    cfg!(all(any(unix, windows), feature = "external-dns-sd"))
}

fn should_abort_discovery_startup(return_code: i32, ble_fallback_enabled: bool) -> bool {
    return_code != 0 && !ble_fallback_enabled
}

fn current_app_name() -> String {
    env::args_os()
        .next()
        .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "uxplay".into())
}

fn dnssd_registration_error_text(error: i32, external: bool, app_name: &str) -> String {
    if !external {
        let mut message = String::from(
            "*** dnssd_implementation: internal, mdnsd\nsetup or start of self-contained mDNSResponder (lib/mdnsd) failed; check UDP port 5353 and multicast access",
        );
        if error != -1 {
            let socket_error = error.saturating_neg();
            message.push_str(&format!(
                "Socket error {socket_error}: {}\n",
                crate::compat::socket_error_message(socket_error)
            ));
        }
        return message;
    }

    match error {
        -65_537 => String::from(
            "*** dnssd_implementation: external, dns_sd.h\n    No DNS-SD Server found (DNSServiceRegister call returned kDNSServiceErr_Unknown)\n",
        ),
        -65_548 => format!(
            "*** dnssd_implementation: external, dns_sd.h\n    DNSServiceRegister call returned kDNSServiceErr_NameConflict\n    Is another instance of {app_name} running with the same DeviceID (MAC address) or using same network ports?\n    Use options -m ... and -p ... to allow multiple instances of {app_name} to run concurrently\n"
        ),
        _ => format!(
            "*** dnssd_implementation: external, dns_sd.h\ndnssd_register_raop failed with error code {error}\n mDNS Error codes are in range FFFE FF00 (-65792) to FFFE FFFF (-65537) (see Apple's dns_sd.h)\n"
        ),
    }
}

fn write_dns_sd_error(message: &str) {
    let _ = io::stdout().write_all(message.as_bytes());
}

fn hls_custom_profile(options: &Options) -> Option<String> {
    if let Some(profile) = &options.custom_profile {
        return Some(profile.clone());
    }
    let model = options.raspberry_pi?;
    let h264_hardware = gstreamer::ElementFactory::find("v4l2h264dec").is_some();
    let h265_hardware = gstreamer::ElementFactory::find("v4l2slh265dec").is_some();
    Some(raspberry_pi_custom_profile(
        model,
        h264_hardware,
        h265_hardware,
    ))
}

fn raspberry_pi_custom_profile(
    model: RaspberryPiModel,
    h264_hardware: bool,
    h265_hardware: bool,
) -> String {
    use RaspberryPiModel::{Pi3, Pi3ActiveCooling, Pi4, Pi4ActiveCooling, Pi5, Pi5ActiveCooling};

    let mut profile = String::new();
    match (model, h264_hardware) {
        (Pi3 | Pi3ActiveCooling, _) => profile.push_str("AVC:1080,720"),
        (Pi4 | Pi4ActiveCooling, _) => profile.push_str("AVC:1080"),
        (Pi5 | Pi5ActiveCooling, false) => profile.push_str("AVC:1080"),
        (Pi5 | Pi5ActiveCooling, true) => {}
    }
    match (model, h265_hardware) {
        (Pi4 | Pi4ActiveCooling | Pi5 | Pi5ActiveCooling, true) => profile.push_str(";HEVC:2160"),
        (Pi3 | Pi3ActiveCooling | Pi4 | Pi4ActiveCooling, false) => profile.push_str(";HEVC:480,0"),
        (Pi5, false) => profile.push_str(";HEVC:1080,720"),
        (Pi5ActiveCooling, false) => profile.push_str(";HEVC:2160,1080"),
        (Pi3 | Pi3ActiveCooling, true) => {}
    }
    match model {
        Pi3 | Pi3ActiveCooling => profile.push_str(";VP9:480,0"),
        Pi4 | Pi4ActiveCooling => profile.push_str(";VP9:720,0"),
        Pi5 => profile.push_str(";VP9:1080,720"),
        Pi5ActiveCooling => profile.push_str(";VP9:2160,1080"),
    }
    profile.push_str(";AV1:0");
    profile
}

fn remove_output_files(options: &Options) {
    for path in [
        options.cover_art_filename.as_deref(),
        options.metadata_filename.as_deref(),
        options.dacp_file.as_deref(),
        options.ble_filename.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("UxPlay Rust: failed to remove output file {path}: {error}");
        }
    }
}

fn write_ble_handoff(options: &Options, raop_port: u16) {
    let Some(path) = options.ble_filename.as_deref() else {
        return;
    };
    let pid = std::process::id();
    if pid > 4_194_304 {
        println!("Failed to write BLE beacon data: process ID exceeds 4194304");
        return;
    }
    let process_name = std::env::args_os()
        .next()
        .unwrap_or_else(|| "uxplay".into())
        .to_string_lossy()
        .into_owned();
    if write_ble_handoff_file(Path::new(path), raop_port, pid, process_name.as_bytes()).is_err() {
        println!("Failed to open file {path}");
        return;
    }
    println!("port {raop_port}");
    println!("Bluetooth LE beacon-based service discovery is possible: PID data written to {path}");
}

fn write_ble_handoff_file(
    path: &Path,
    raop_port: u16,
    pid: u32,
    process_name: &[u8],
) -> io::Result<()> {
    if pid > 4_194_304 {
        return Err(io::Error::other("process ID exceeds the source's PID_MAX"));
    }
    std::fs::write(
        path,
        uxplay_core::beacon::encode_beacon_file(raop_port, pid, process_name),
    )
}

fn bind_tcp_listeners(port: u16, peer_to_peer: bool) -> io::Result<(Vec<TcpListener>, u16)> {
    let mut port = port;
    let ipv4 = netutils::bind_socket(&mut port, AddressFamily::Ipv4, Transport::Tcp, peer_to_peer)?;
    ipv4.listen(5)?;
    let mut listeners = vec![TcpListener::from(ipv4)];

    match netutils::bind_socket(&mut port, AddressFamily::Ipv6, Transport::Tcp, peer_to_peer) {
        Ok(ipv6) => {
            ipv6.listen(5)?;
            listeners.push(TcpListener::from(ipv6));
        }
        Err(error) => {
            eprintln!(
                "UxPlay Rust: IPv6 listener unavailable; continuing with IPv4: {}",
                crate::compat::socket_error_description(&error)
            );
        }
    }

    Ok((listeners, port))
}

fn start_feedback_watchdog(
    outputs: Arc<Mutex<MediaOutputRuntime>>,
    effects: Arc<dyn ProtocolEffectSink>,
    restart_requested: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    limit_seconds: u32,
    hls_eos: HlsEosWatchdog,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("uxplay-feedback-watchdog".into())
        .spawn(move || {
            let mut last_feedback_tick = std::time::Instant::now();
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                if hls_eos.enabled {
                    video_renderer_rs::port::waiting_for_x11_window();
                }
                if hls_eos.enabled
                    && let Some(uri) = video_renderer_rs::port::video_renderer_take_hls_eos_uri()
                {
                    eprintln!("UxPlay Rust: hls video has sent EOS");
                    match finish_hls_playback_after_eos(
                        &hls_eos.video_store,
                        &hls_eos.event_channel,
                        &hls_eos.connection_registry,
                        &uri,
                        || {
                            video_renderer_rs::port::video_renderer_hls_ready();
                            let restored = video_renderer_rs::port::video_renderer_restore_mirror();
                            outputs
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .reset_remote_clock_offset();
                            restored
                        },
                    ) {
                        Ok(true) => {}
                        Ok(false) => {}
                        Err(error) => eprintln!("UxPlay Rust: could not restore mirror renderer after HLS EOS: {error}"),
                    }
                }
                if last_feedback_tick.elapsed() < Duration::from_secs(1) {
                    continue;
                }
                last_feedback_tick = std::time::Instant::now();
                if restart_requested.load(std::sync::atomic::Ordering::Acquire) {
                    continue;
                }
                let tick = outputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .feedback_watchdog_tick(limit_seconds);
                match tick {
                    FeedbackWatchdogTick::Warning(missed) => eprintln!(
                        "UxPlay Rust: {missed} seconds since last client feedback request (expected every two seconds); client may be offline"
                    ),
                    FeedbackWatchdogTick::TimedOut(missed) => {
                        eprintln!("UxPlay Rust: lost connection with client (network problem?)");
                        eprintln!("UxPlay Rust: interval since last client feedback request exceeds limit of {limit_seconds} seconds (observed {missed})");
                        eprintln!("UxPlay Rust: sometimes the network connection may recover after a longer delay; change the limit with -reset");
                        effects.reset_server();
                    }
                    FeedbackWatchdogTick::Idle | FeedbackWatchdogTick::Active(_) => {}
                }
            }
        })
}

struct HlsEosWatchdog {
    enabled: bool,
    video_store: Arc<Mutex<crate::airplay_video::AirPlayVideoStore>>,
    event_channel: Arc<crate::airplay_event_channel::AirPlayEventChannel>,
    connection_registry: Arc<ConnectionRegistry>,
}

fn finish_hls_playback_after_eos(
    video_store: &Mutex<crate::airplay_video::AirPlayVideoStore>,
    event_channel: &crate::airplay_event_channel::AirPlayEventChannel,
    connection_registry: &ConnectionRegistry,
    eos_uri: &str,
    reset_renderer: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    let mut store = video_store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if store.current_playback_location() != Some(eos_uri) {
        return Ok(false);
    }

    let reset_result = reset_renderer();
    store.finish_current();
    event_channel.clear();
    connection_registry.remove_known_connections();
    reset_result.map(|()| true)
}

#[cfg(test)]
mod hls_lifecycle_tests {
    use super::*;
    use crate::http_transport::ConnectionClass;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::atomic::AtomicUsize;
    use uxplay_airplay_http_handlers::PlayRequest;

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        (stream, peer)
    }

    fn active_video(location: &str) -> Arc<Mutex<crate::airplay_video::AirPlayVideoStore>> {
        let mut store = crate::airplay_video::AirPlayVideoStore::default();
        let action = store.begin_play(
            PlayRequest {
                session_id: "session-eos".to_owned(),
                playback_uuid: "uuid-eos".to_owned(),
                content_location: Some(location.to_owned()),
                start_position_seconds: 0.0,
                client_proc_name: None,
            },
            "http://localhost:7100",
        );
        assert_eq!(
            action,
            crate::airplay_video::HlsAction::StartPlayback {
                location: location.to_owned(),
                start_position_seconds: 0.0,
            }
        );
        Arc::new(Mutex::new(store))
    }

    #[test]
    fn runtime_hls_effect_decodes_to_eos_then_cleans_up_current_session() {
        let _renderer_guard = crate::raop_media_runtime::VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gstreamer::init().expect("GStreamer is required for the runtime HLS test");
        if [
            "playbin3",
            "hlsdemux2",
            "h264parse",
            "avdec_h264",
            "fakesink",
        ]
        .into_iter()
        .any(|factory| gstreamer::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let playlist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("render")
            .join("video-renderer")
            .join("testdata")
            .join("hls")
            .join("playlist.m3u8");
        let fixture_directory = playlist.parent().expect("HLS playlist parent");
        let playlist_bytes = std::fs::read(&playlist).expect("HLS playlist fixture");
        let segment_bytes =
            std::fs::read(fixture_directory.join("segment00.ts")).expect("HLS segment fixture");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server_stop = Arc::new(AtomicBool::new(false));
        let server_should_stop = Arc::clone(&server_stop);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        let fixture_server = std::thread::spawn(move || {
            while !server_should_stop.load(std::sync::atomic::Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, peer)) => {
                        // On Windows an accepted socket may inherit the listener's
                        // nonblocking mode. Restore blocking reads before waiting
                        // for GStreamer's request bytes, or the segment request can
                        // be mistaken for an empty probe and dropped.
                        stream.set_nonblocking(false).unwrap();
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut request = Vec::new();
                        let mut byte = [0; 1];
                        while !request.ends_with(b"\r\n\r\n") {
                            match stream.read(&mut byte) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => request.push(byte[0]),
                            }
                        }
                        if request.is_empty() {
                            // GStreamer may open and abandon a probe connection
                            // before sending its actual HTTP request.
                            server_requests
                                .lock()
                                .unwrap()
                                .push(format!("<empty connection from {peer}>"));
                            continue;
                        }
                        let request = String::from_utf8_lossy(&request);
                        let request_line = request.lines().next().unwrap_or_default();
                        let (status, content_type, source_body): (&str, &str, &[u8]) =
                            if request_line
                                .lines()
                                .next()
                                .is_some_and(|line| line.starts_with("GET /playlist.m3u8 "))
                            {
                                ("200 OK", "application/vnd.apple.mpegurl", &playlist_bytes)
                            } else if request_line.starts_with("GET /segment00.ts ") {
                                ("200 OK", "video/mp2t", &segment_bytes)
                            } else {
                                ("404 Not Found", "text/plain", b"not found")
                            };
                        let range = request
                            .lines()
                            .find_map(|line| line.strip_prefix("Range: bytes="));
                        let (status, content_range, body) = if status == "200 OK"
                            && content_type == "video/mp2t"
                            && let Some(range) = range
                        {
                            let (start, end) = range.split_once('-').unwrap_or(("0", ""));
                            let start = start.parse::<usize>().unwrap_or(0).min(source_body.len());
                            let end = if end.is_empty() {
                                source_body.len().saturating_sub(1)
                            } else {
                                end.parse::<usize>()
                                    .unwrap_or_else(|_| source_body.len().saturating_sub(1))
                                    .min(source_body.len().saturating_sub(1))
                            };
                            let body = if start <= end {
                                &source_body[start..=end]
                            } else {
                                &source_body[0..0]
                            };
                            (
                                "206 Partial Content",
                                format!(
                                    "Content-Range: bytes {start}-{end}/{}\r\n",
                                    source_body.len()
                                ),
                                body,
                            )
                        } else {
                            (status, String::new(), source_body)
                        };
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\n{content_range}Content-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let response_write = stream
                            .write_all(response.as_bytes())
                            .and_then(|()| stream.write_all(body));
                        let mut request_record = request.to_string();
                        request_record.push_str(&format!(
                            "[fixture response {status}; write {response_write:?}]"
                        ));
                        server_requests.lock().unwrap().push(request_record);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        let uri = format!("http://{address}/playlist.m3u8");

        let logger = Arc::new(crate::logger::Logger::new());
        let options = Options {
            use_audio: false,
            audio_sink: "0".into(),
            video_sink: "fakesink".into(),
            video_parser: "h264parse".into(),
            video_decoder: "avdec_h264".into(),
            video_converter: "videoconvert".into(),
            video_sync: false,
            ..Options::default()
        };
        let mut renderer = crate::raop_media_runtime::RendererEventLoop::start(&options, &logger)
            .expect("Rust server renderer event loop should start");

        let event_channel = Arc::new(crate::airplay_event_channel::AirPlayEventChannel::default());
        let (event_stream, mut event_peer) = tcp_pair();
        event_channel.install(event_stream);
        let video_store = active_video(&uri);
        let registry = Arc::new(ConnectionRegistry::default());
        let (known_stream, mut known_peer) = tcp_pair();
        let known_registration = registry.register(&known_stream).unwrap();
        known_registration.set_class(ConnectionClass::Hls);
        let (unknown_stream, mut unknown_peer) = tcp_pair();
        let _unknown_registration = registry.register(&unknown_stream).unwrap();

        let effects = StartupEffects {
            registration: Arc::new(PairingRegistry::load(true, true, None)),
            media_config: crate::raop_media_runtime::SessionMediaConfig {
                use_audio: false,
                use_video: true,
                outputs: Arc::new(Mutex::new(
                    crate::raop_media_runtime::MediaOutputRuntime::default(),
                )),
                logger: Some(Arc::clone(&logger)),
                ..crate::raop_media_runtime::SessionMediaConfig::default()
            },
            hls_support: true,
            event_channel: Arc::clone(&event_channel),
            video_store: Arc::clone(&video_store),
            restart_requested: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        effects
            .apply(HandlerEffect::StartHlsPlayback {
                location: uri.clone(),
                start_position_seconds: 0.0,
            })
            .expect("StartHlsPlayback should reach the process renderer");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let eos_uri = loop {
            if let Some(uri) = video_renderer_rs::port::video_renderer_take_hls_eos_uri() {
                break Some(uri);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            eos_uri.as_deref(),
            Some(uri.as_str()),
            "local HLS HTTP requests: {:?}",
            *requests.lock().unwrap()
        );
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|request| {
            request
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("GET /playlist.m3u8 "))
        }));
        assert!(requests.iter().any(|request| {
            request
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("GET /segment00.ts "))
        }));
        drop(requests);

        assert!(
            finish_hls_playback_after_eos(&video_store, &event_channel, &registry, &uri, || {
                video_renderer_rs::port::video_renderer_hls_ready();
                video_renderer_rs::port::video_renderer_restore_mirror()
            },)
            .unwrap()
        );
        assert_eq!(video_store.lock().unwrap().current_video_index(), None);
        assert_eq!(event_channel.connection_count(), 0);
        assert_eq!(
            video_renderer_rs::port::video_renderer_choose_codec_and_size(false, false),
            0
        );
        assert!(video_renderer_rs::port::video_renderer_has_active_renderer());

        let mut closed = Vec::new();
        known_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        event_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        unknown_peer.write_all(b"still-open").unwrap();
        let mut received = [0; 10];
        (&unknown_stream).read_exact(&mut received).unwrap();
        assert_eq!(&received, b"still-open");
        unknown_peer.shutdown(Shutdown::Both).unwrap();
        renderer.stop();
        server_stop.store(true, std::sync::atomic::Ordering::Release);
        fixture_server.join().unwrap();
    }

    #[test]
    fn hls_eos_clears_only_the_matching_current_video_and_known_connections() {
        let location = "https://origin.example/video.m3u8";
        let video_store = active_video(location);
        let event_channel = crate::airplay_event_channel::AirPlayEventChannel::default();
        let (event_stream, mut event_peer) = tcp_pair();
        event_channel.install(event_stream);

        let registry = Arc::new(ConnectionRegistry::default());
        let (known_stream, mut known_peer) = tcp_pair();
        let known_registration = registry.register(&known_stream).unwrap();
        known_registration.set_class(ConnectionClass::Hls);
        let (unknown_stream, mut unknown_peer) = tcp_pair();
        let _unknown_registration = registry.register(&unknown_stream).unwrap();
        let reset_count = AtomicUsize::new(0);

        assert!(
            finish_hls_playback_after_eos(
                &video_store,
                &event_channel,
                &registry,
                location,
                || {
                    reset_count.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    Ok(())
                },
            )
            .unwrap()
        );
        assert_eq!(reset_count.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(video_store.lock().unwrap().current_video_index(), None);

        let mut closed = Vec::new();
        known_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        event_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());

        unknown_peer.write_all(b"still-open").unwrap();
        let mut received = [0; 10];
        (&unknown_stream).read_exact(&mut received).unwrap();
        assert_eq!(&received, b"still-open");
        unknown_peer.shutdown(Shutdown::Both).unwrap();
    }

    #[test]
    fn stale_hls_eos_does_not_reset_a_new_current_video() {
        let video_store = active_video("https://origin.example/new.m3u8");
        let event_channel = crate::airplay_event_channel::AirPlayEventChannel::default();
        let registry = ConnectionRegistry::default();
        let reset_called = AtomicBool::new(false);

        assert!(
            !finish_hls_playback_after_eos(
                &video_store,
                &event_channel,
                &registry,
                "https://origin.example/old.m3u8",
                || {
                    reset_called.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                },
            )
            .unwrap()
        );
        assert!(!reset_called.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            video_store.lock().unwrap().current_playback_location(),
            Some("https://origin.example/new.m3u8")
        );
    }
}

#[cfg(test)]
mod pairing_persistence_tests {
    use super::*;
    use uxplay_raop_session::SessionEffect;

    fn temp_registration_file() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "uxplay-runtime-registration-{}-{nonce}.txt",
            std::process::id()
        ))
    }

    #[test]
    fn startup_registration_effect_persists_even_without_pin_mode() {
        let path = temp_registration_file();
        let options = Options {
            registration_list: true,
            registration_file: Some(path.to_string_lossy().into_owned()),
            ..Options::default()
        };
        let effects = StartupEffects {
            registration: pairing_registry(&options),
            media_config: crate::raop_media_runtime::SessionMediaConfig::default(),
            hls_support: false,
            event_channel: Arc::new(crate::airplay_event_channel::AirPlayEventChannel::default()),
            video_store: Arc::new(Mutex::new(
                crate::airplay_video::AirPlayVideoStore::default(),
            )),
            restart_requested: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        };

        effects
            .apply_session(SessionEffect::RegisterClient {
                device_id: "device-id".into(),
                public_key_base64: "Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH".into(),
                name: "Apple TV".into(),
            })
            .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH,"));
        assert!(contents.contains(",device-id,Apple TV"));
        let reloaded = PairingRegistry::load(true, false, Some(path.clone()));
        assert!(reloaded.contains("unknown-client"));
        drop(effects);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod listener_tests {
    use super::*;

    #[test]
    fn external_dns_sd_mode_follows_the_platform_feature_not_the_p2p_option() {
        assert_eq!(
            uses_external_dns_sd(),
            cfg!(all(any(unix, windows), feature = "external-dns-sd"))
        );
    }

    #[test]
    fn primary_listener_binds_ipv4_and_ipv6_to_the_same_selected_port() {
        let (listeners, port) = bind_tcp_listeners(0, false).unwrap();
        assert_ne!(port, 0);
        assert_eq!(
            listeners[0].local_addr().unwrap().ip(),
            "0.0.0.0".parse::<std::net::IpAddr>().unwrap()
        );
        assert_eq!(listeners[0].local_addr().unwrap().port(), port);
        for listener in &listeners[1..] {
            assert!(listener.local_addr().unwrap().is_ipv6());
            assert_eq!(listener.local_addr().unwrap().port(), port);
        }
    }

    #[cfg(all(target_os = "macos", feature = "apple-p2p"))]
    #[test]
    fn peer_to_peer_main_listener_binds_with_receive_any_interface_enabled() {
        let (listeners, port) = bind_tcp_listeners(0, true).unwrap();
        assert_ne!(port, 0);
        assert_eq!(listeners[0].local_addr().unwrap().port(), port);
    }
}

/// Apply process-level protocol effects to the live Rust-owned runtime.
struct StartupEffects {
    registration: Arc<PairingRegistry>,
    media_config: SessionMediaConfig,
    hls_support: bool,
    event_channel: Arc<crate::airplay_event_channel::AirPlayEventChannel>,
    video_store: Arc<Mutex<crate::airplay_video::AirPlayVideoStore>>,
    restart_requested: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl ProtocolEffectSink for StartupEffects {
    fn apply(&self, effect: HandlerEffect) -> io::Result<()> {
        match effect {
            HandlerEffect::DisplayPairingPin(pin) => {
                if let Some(image) = crate::pin_display::render_pin(&pin) {
                    eprintln!("{image}");
                }
                eprintln!("*** CLIENT MUST NOW ENTER PIN = \"{pin}\" AS AIRPLAY PASSWORD");
            }
            HandlerEffect::AirPlayControl(control) => match control {
                crate::airplay_controls::AirPlayControl::Scrub { position } => {
                    video_renderer_rs::port::video_renderer_seek(position);
                }
                crate::airplay_controls::AirPlayControl::Rate { rate } => {
                    if rate == 1.0 {
                        video_renderer_rs::port::video_renderer_resume();
                    } else if rate == 0.0 {
                        video_renderer_rs::port::video_renderer_pause();
                    }
                }
                crate::airplay_controls::AirPlayControl::Stop => {
                    video_renderer_rs::port::video_renderer_hls_ready();
                }
            },
            HandlerEffect::RaopFeedback => {
                self.media_config
                    .outputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .note_client_feedback();
            }
            HandlerEffect::RaopParameter(parameter) => match parameter {
                crate::raop_parameters::ParameterEffect::Volume {
                    target: crate::raop_parameters::VolumeTarget::HlsRenderer,
                    value,
                } => {
                    if self.media_config.use_audio {
                        let value = source_volume_db(value);
                        *self
                            .media_config
                            .current_volume_db
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
                        let volume = source_volume_to_gstreamer(
                            value,
                            self.media_config.db_range,
                            self.media_config.taper_volume,
                        );
                        audio_renderer_rs::port::audio_renderer_set_volume(volume);
                        video_renderer_rs::port::video_renderer_hls_set_volume(volume);
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "connection-scoped RAOP callback reached the process effect sink",
                    ));
                }
            },
            HandlerEffect::ResetHlsPlayback => {
                self.media_config
                    .outputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .reset_remote_clock_offset();
                if self.media_config.use_video {
                    video_renderer_rs::port::video_renderer_stop();
                    video_renderer_rs::port::video_renderer_restore_mirror()
                        .map_err(io::Error::other)?;
                }
                if self.media_config.use_audio {
                    audio_renderer_rs::port::audio_renderer_stop();
                }
                if self.hls_support {
                    self.video_store
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear_all();
                }
                self.event_channel.clear();
            }
            HandlerEffect::StartHlsPlayback {
                location,
                start_position_seconds,
            } => {
                if self.media_config.use_video {
                    video_renderer_rs::port::video_renderer_play_hls(
                        &location,
                        start_position_seconds,
                    )
                    .map_err(io::Error::other)?;
                }
            }
        }
        Ok(())
    }

    fn playback_snapshot(&self) -> io::Result<Option<crate::airplay_info::PlaybackSnapshot>> {
        let info = video_renderer_rs::port::video_get_playback_info();
        if self.hls_support {
            self.media_config
                .outputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .observe_hls_position(info.position);
        }
        Ok(Some(crate::airplay_info::PlaybackSnapshot {
            duration: info.duration,
            position: info.position,
            seek_start: info.seek_start,
            seek_duration: info.seek_duration,
            rate: info.rate,
            ready_to_play: true,
            playback_buffer_empty: info.buffer_empty,
            playback_buffer_full: info.buffer_full,
            playback_likely_to_keep_up: true,
        }))
    }

    fn reset_server(&self) {
        if self
            .restart_requested
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        let shutdown = Arc::clone(&self.shutdown);
        if let Err(error) = std::thread::Builder::new()
            .name("uxplay-protocol-reset".into())
            .spawn(move || {
                // The C main loop polls reset_loop on a 100 ms timer. Let the
                // failed request finish and preserve the same reset window.
                std::thread::sleep(Duration::from_millis(100));
                shutdown.store(true, std::sync::atomic::Ordering::Release);
            })
        {
            eprintln!("UxPlay Rust: could not schedule server reset: {error}");
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn nohold_video_reset(&self) {
        self.media_config
            .outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset_remote_clock_offset();
        if self.hls_support {
            self.video_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear_all();
            self.event_channel.clear();
        }
        if self.media_config.use_video {
            video_renderer_rs::port::video_renderer_stop();
            if let Err(error) = video_renderer_rs::port::video_renderer_restore_mirror() {
                eprintln!("UxPlay Rust: could not reset the video renderer for -nohold: {error}");
            }
            video_renderer_rs::port::video_renderer_start();
        }
    }

    fn apply_session(&self, effect: uxplay_raop_session::SessionEffect) -> io::Result<()> {
        match effect {
            uxplay_raop_session::SessionEffect::RegisterClient {
                device_id,
                public_key_base64,
                name,
            } => {
                self.registration
                    .register(&public_key_base64, &device_id, &name);
                Ok(())
            }
            uxplay_raop_session::SessionEffect::ReportClientRequest {
                device_id,
                model,
                name,
            } => {
                eprintln!(
                    "UxPlay Rust: connection request from {} ({}) with deviceID = {}",
                    name.as_deref().unwrap_or(""),
                    model.as_deref().unwrap_or(""),
                    device_id.as_deref().unwrap_or("(no DeviceI)D")
                );
                // The current C callback returns before updating the renderer when the
                // source does not provide a DeviceID.
                if device_id.is_some() {
                    video_renderer_rs::port::video_renderer_set_device_model(
                        model.as_deref().map(str::as_bytes),
                        name.as_deref().map(str::as_bytes),
                    );
                }
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "RAOP media session effect has no callback adapter",
            )),
        }
    }

    fn get_audio_format(
        &self,
        format: uxplay_raop_session::AudioFormat,
    ) -> io::Result<uxplay_raop_session::AudioFormat> {
        self.media_config
            .outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .select_audio_codec(format.codec_type);
        crate::raop_media_runtime::initialize_audio_output_files(&self.media_config);
        if self.media_config.use_audio {
            audio_renderer_rs::port::audio_renderer_start(format.codec_type);
        }
        Ok(format)
    }

    fn hls_playlist_removed(&self) -> io::Result<f32> {
        video_renderer_rs::port::video_renderer_pause();
        Ok(video_renderer_rs::port::video_get_playback_info().position as f32)
    }
}

fn registration_path(options: &Options) -> Option<PathBuf> {
    if let Some(path) = options
        .registration_file
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    if !options.registration_list || options.pin_password_mode != 1 {
        return None;
    }
    home_directory().map(|directory| directory.join(".uxplay.register"))
}

fn pairing_registry(options: &Options) -> Arc<PairingRegistry> {
    Arc::new(PairingRegistry::load(
        options.registration_list,
        options.pin_password_mode == 1,
        registration_path(options),
    ))
}

fn home_directory() -> Option<PathBuf> {
    if let Some(directory) =
        std::env::var_os("XDG_CONFIG_HOMEDIR").or_else(|| std::env::var_os("HOME"))
    {
        return Some(PathBuf::from(directory));
    }
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: getuid has no pointer arguments; getpwuid returns a process-owned entry.
        let user = unsafe { libc::getpwuid(libc::getuid()) };
        if user.is_null() {
            return None;
        }
        // SAFETY: a non-null passwd entry supplies a NUL-terminated home path.
        let path = unsafe { (*user).pw_dir };
        if path.is_null() {
            return None;
        }
        // SAFETY: pw_dir points to the NUL-terminated path stored in the passwd entry.
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    None
}

struct ServerIdentity {
    names: ServiceNames,
    txt: ServiceTxtRecords,
    instance_name: String,
    public_key_hex: String,
    airplay_features: u64,
    hardware_address: [u8; 6],
    pairing: Arc<Pairing>,
}

impl ServerIdentity {
    fn build(options: &Options, mac: MacAddress) -> io::Result<Self> {
        let hostname = system_hostname();
        let mut instance_name = options.server_name.clone();
        if options.append_hostname {
            instance_name.push('@');
            if let Some(hostname) = hostname.as_deref() {
                instance_name.push_str(
                    hostname
                        .split_once(".local")
                        .map_or(hostname, |part| part.0),
                );
            }
        }

        let hardware_address = mac.octets();
        let device_id = mac.as_lowercase();
        let pairing = Arc::new(
            Pairing::init_generate(device_id.as_bytes(), pairing_key_path(options).as_deref())
                .map_err(|error| io::Error::other(format!("pairing identity failed: {error:?}")))?
                .0,
        );
        let public_key = pairing
            .get_public_key()
            .map_err(|error| io::Error::other(format!("pairing public key failed: {error:?}")))?;
        let public_key_hex = lower_hex(&public_key);

        let mut nul_terminated_name = instance_name.as_bytes().to_vec();
        let name_length = nul_terminated_name.len();
        nul_terminated_name.push(0);
        let mut discovery_state = DiscoveryState::new(
            &nul_terminated_name,
            name_length,
            &hardware_address,
            options.pin_password_mode,
        )
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("DNS-SD identity failed: {error:?}"),
            )
        })?;
        discovery_state.set_feature(0, i32::from(options.hls_support));
        discovery_state.set_feature(4, i32::from(options.hls_support));
        discovery_state.set_feature(27, i32::from(options.legacy_pairing));
        discovery_state.set_feature(42, i32::from(options.h265_support));
        let airplay_features = discovery_state.airplay_features();

        let txt = build_service_txt(TxtRecordInputs {
            model: "AppleTV3,2",
            version: "220.68",
            public_key: &public_key_hex,
            hardware_address: &hardware_address,
            features1: airplay_features as u32,
            features2: (airplay_features >> 32) as u32,
            pin_mode: options.pin_password_mode,
        })
        .map_err(|error| io::Error::other(format!("DNS-SD TXT construction failed: {error:?}")))?;
        let names = prepare_service_names(
            instance_name.as_bytes(),
            &hardware_address,
            hostname.as_deref().map(str::as_bytes).ok_or(()),
        )
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("DNS-SD service name failed: {error:?}"),
            )
        })?;

        Ok(Self {
            names,
            txt,
            instance_name,
            public_key_hex,
            airplay_features,
            hardware_address,
            pairing,
        })
    }

    fn info_context(&self, options: &Options) -> OwnedInfoContext {
        OwnedInfoContext {
            raw_hardware_address: self.hardware_address.to_vec(),
            airplay_txt: self.txt.airplay.clone(),
            raop_txt: self.txt.raop.clone(),
            public_key_hex: self.public_key_hex.as_bytes().to_vec(),
            airplay_features: self.airplay_features,
            name: self.instance_name.clone(),
            initial_volume: options.initial_volume_attenuation_db,
            width: options.display_width.unwrap_or(1920),
            height: options.display_height.unwrap_or(1080),
            refresh_rate: options.display_rate.unwrap_or(60).min(u16::from(u8::MAX)) as u8,
            max_fps: options.max_fps.unwrap_or(30).min(u16::from(u8::MAX)) as u8,
            overscanned: u8::from(options.overscanned),
        }
    }
}

fn pairing_key_path(options: &Options) -> Option<PathBuf> {
    options
        .key_file
        .as_deref()
        .filter(|path| *path != "0")
        .map(PathBuf::from)
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

fn system_hostname() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var_os("COMPUTERNAME").map(|value| value.to_string_lossy().into_owned())
    }
    #[cfg(unix)]
    {
        let mut buffer = [0_i8; 256];
        // SAFETY: `buffer` is writable for the supplied length; convert only after success.
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len()) };
        if result != 0 {
            return None;
        }
        // SAFETY: the pointer spans the initialized fixed-size hostname buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len()) };
        let length = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..length]).into_owned())
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

#[cfg(test)]
mod hls_profile_tests {
    use super::*;

    #[test]
    fn raspberry_pi_profiles_follow_model_and_hardware_decoder_matrix() {
        assert_eq!(
            raspberry_pi_custom_profile(RaspberryPiModel::Pi3, false, false),
            "AVC:1080,720;HEVC:480,0;VP9:480,0;AV1:0"
        );
        assert_eq!(
            raspberry_pi_custom_profile(RaspberryPiModel::Pi4, true, true),
            "AVC:1080;HEVC:2160;VP9:720,0;AV1:0"
        );
        assert_eq!(
            raspberry_pi_custom_profile(RaspberryPiModel::Pi5, false, true),
            "AVC:1080;HEVC:2160;VP9:1080,720;AV1:0"
        );
        assert_eq!(
            raspberry_pi_custom_profile(RaspberryPiModel::Pi5ActiveCooling, false, false),
            "AVC:1080;HEVC:2160,1080;VP9:2160,1080;AV1:0"
        );
    }

    #[test]
    fn raop_record_audio_latency_uses_configured_microseconds_or_source_zero_default() {
        assert_eq!(audio_delay_micros(&Options::default()), 250_000);
        assert_eq!(
            audio_delay_micros(&Options {
                audio_latency_us: Some(4_800_000),
                ..Options::default()
            }),
            4_800_000
        );
    }
}

#[cfg(test)]
mod ble_handoff_tests {
    use super::*;

    #[test]
    fn writes_the_source_shaped_ble_file_for_the_external_beacon_process() {
        let path = std::env::temp_dir().join(format!(
            "uxplayrusty-ble-handoff-{}.tmp",
            std::process::id()
        ));
        write_ble_handoff_file(&path, 7100, 0x12_3456, b"uxplay.exe").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            [
                0xbc, 0x1b, 0x56, 0x34, 0x12, 0x00, b'u', b'x', b'p', b'l', b'a', b'y', b'.', b'e',
                b'x', b'e'
            ]
        );
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod discovery_startup_tests {
    use super::*;

    #[test]
    fn dns_sd_failure_is_fatal_only_without_ble_fallback() {
        assert!(!should_abort_discovery_startup(0, false));
        assert!(!should_abort_discovery_startup(0, true));
        assert!(should_abort_discovery_startup(-97, false));
        assert!(!should_abort_discovery_startup(-97, true));
    }

    #[test]
    fn external_dns_sd_error_text_matches_known_source_codes() {
        assert_eq!(
            dnssd_registration_error_text(-65_537, true, "uxplay"),
            "*** dnssd_implementation: external, dns_sd.h\n    No DNS-SD Server found (DNSServiceRegister call returned kDNSServiceErr_Unknown)\n"
        );
        let conflict = dnssd_registration_error_text(-65_548, true, "receiver");
        assert!(conflict.contains("kDNSServiceErr_NameConflict"));
        assert!(conflict.contains("another instance of receiver"));
        assert!(!dnssd_registration_error_text(-1, false, "uxplay").contains("Socket error"));
    }
}
