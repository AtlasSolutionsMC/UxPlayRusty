//! RAOP audio RTP session from UxPlay's `lib/raop_rtp.c`.
//!
//! This crate owns the UDP control/data sockets, sync clock mapping, resend
//! handling, packet queue, and callbacks for one negotiated audio stream.

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use uxplay_crypto::port::{AesContext, AesMode, CryptoError, Direction};

const RAOP_PACKET_LEN: usize = 32_768;
const RAOP_BUFFER_LENGTH: usize = 256;
const NO_FLUSH: i32 = -42;
const NO_DATA_MARKER: [u8; 4] = [0, 0x68, 0x34, 0];
const AUDIO_SYNC_TOLERANCE_SECONDS: f64 = 0.20;
const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(5);

pub const LOGGER_ERR: i32 = 3;
pub const LOGGER_WARNING: i32 = 4;
pub const LOGGER_INFO: i32 = 6;
pub const LOGGER_DEBUG: i32 = 7;

pub type LogCallback = Arc<dyn Fn(i32, &str) + Send + Sync + 'static>;
pub type UdpSocketBinder =
    Arc<dyn Fn(IpAddr, u16) -> io::Result<(UdpSocket, u16)> + Send + Sync + 'static>;

/// Time services shared with the RAOP timing session.
pub trait TimingClock: Send + Sync + 'static {
    fn local_time_ns(&self) -> u64;
    fn adjust_remote_timestamp_offset(&self, timestamp: u64, add_epoch_seconds: bool) -> u64;
    fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64;
    fn convert_remote_time_ns(&self, remote_time_ns: u64) -> u64;
    fn video_arrival_offset(&self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioFrame {
    pub data: Vec<u8>,
    pub codec_type: u8,
    pub sequence_number: u16,
    pub rtp_timestamp: u32,
    pub remote_time_ns: u64,
    pub local_time_ns: u64,
}

/// Audio-side callbacks corresponding to `raop_callbacks_t` audio members.
pub trait AudioCallbacks: Send + Sync + 'static {
    fn process_audio(&self, _frame: AudioFrame) {}
    fn flush(&self) {}
    fn set_volume(&self, _volume: f32) {}
    fn set_metadata(&self, _metadata: Vec<u8>) {}
    fn set_coverart(&self, _coverart: Vec<u8>) {}
    fn remote_control_id(&self, _dacp_id: String, _active_remote_header: String) {}
    fn set_progress(&self, _start: u32, _current: u32, _end: u32) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPorts {
    pub control_remote: u16,
    pub control_local: u16,
    pub data_local: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartState {
    Started(AudioPorts),
    AlreadyRunning,
}

#[derive(Default)]
struct PendingEvents {
    volume: f32,
    volume_changed: bool,
    flush: Option<i32>,
    metadata: Option<Vec<u8>>,
    coverart: Option<Vec<u8>>,
    remote_control: Option<(String, String)>,
    progress: Option<(u32, u32, u32)>,
}

#[derive(Default)]
struct SyncState {
    initial_sync: bool,
    client_ntp_sync: u64,
    rtp_sync: u32,
    rtp_clock_rate: f64,
    ntp_start_time: u64,
    control_sequence_number: u16,
}

/// One RAOP audio receiver. Start/stop calls bind and release the two UDP ports.
pub struct RaopRtpSession {
    remote_ip: IpAddr,
    audio_key: [u8; 16],
    audio_iv: [u8; 16],
    callbacks: Arc<dyn AudioCallbacks>,
    clock: Arc<dyn TimingClock>,
    logger: Option<LogCallback>,
    debug_data_enabled: bool,
    events: Arc<Mutex<PendingEvents>>,
    sync: Arc<Mutex<SyncState>>,
    running: Arc<AtomicBool>,
    udp_socket_binder: UdpSocketBinder,
    joined: bool,
    worker: Option<JoinHandle<()>>,
}

impl RaopRtpSession {
    pub fn new(
        logger: Option<LogCallback>,
        callbacks: Arc<dyn AudioCallbacks>,
        clock: Arc<dyn TimingClock>,
        remote_address: &str,
        remote_address_length: usize,
        audio_key: &[u8],
        audio_iv: &[u8],
    ) -> io::Result<Self> {
        let audio_key = fixed_key(audio_key, "audio key")?;
        let audio_iv = fixed_key(audio_iv, "audio IV")?;
        AudioBuffer::new(audio_key, audio_iv)
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
        let remote_ip = parse_remote_ip(remote_address, remote_address_length)?;
        log(
            &logger,
            LOGGER_DEBUG,
            format!("raop_rtp parse remote ip = {remote_address}"),
        );
        Ok(Self {
            remote_ip,
            audio_key,
            audio_iv,
            callbacks,
            clock,
            logger,
            debug_data_enabled: false,
            events: Arc::new(Mutex::new(PendingEvents::default())),
            sync: Arc::new(Mutex::new(SyncState::default())),
            running: Arc::new(AtomicBool::new(false)),
            udp_socket_binder: Arc::new(bind_udp),
            joined: true,
            worker: None,
        })
    }

    /// Replace the UDP bind implementation before audio starts. The app uses
    /// this to apply platform socket options while the RTP worker keeps owning
    /// the resulting sockets and port lifecycle.
    pub fn set_udp_socket_binder(&mut self, binder: UdpSocketBinder) {
        self.udp_socket_binder = binder;
    }

    /// Enable the source's detailed per-audio-frame timing diagnostic.
    pub fn set_debug_data_enabled(&mut self, enabled: bool) {
        self.debug_data_enabled = enabled;
    }

    pub fn start_audio(
        &mut self,
        ports: AudioPorts,
        codec_type: u8,
        sample_rate: u32,
    ) -> io::Result<StartState> {
        if self.running.load(Ordering::Acquire) || !self.joined {
            return Ok(StartState::AlreadyRunning);
        }
        if sample_rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audio sample rate must be nonzero",
            ));
        }

        log(
            &self.logger,
            LOGGER_INFO,
            "raop_rtp starting audio".to_owned(),
        );
        let sockets = (|| {
            let control = (self.udp_socket_binder)(self.remote_ip, ports.control_local)?;
            let data = (self.udp_socket_binder)(self.remote_ip, ports.data_local)?;
            Ok::<_, io::Error>((control, data))
        })();
        let ((control_socket, control_local), (data_socket, data_local)) =
            sockets.inspect_err(|_| {
                log(
                    &self.logger,
                    LOGGER_ERR,
                    "raop_rtp initializing sockets failed".to_owned(),
                );
            })?;
        let configure_sockets = control_socket
            .set_nonblocking(true)
            .and_then(|()| data_socket.set_nonblocking(true));
        if let Err(error) = configure_sockets {
            log(
                &self.logger,
                LOGGER_ERR,
                "raop_rtp initializing sockets failed".to_owned(),
            );
            return Err(error);
        }
        log(
            &self.logger,
            LOGGER_DEBUG,
            format!(
                "raop_rtp local control port socket {} port UDP {control_local}",
                socket_descriptor(&control_socket)
            ),
        );
        log(
            &self.logger,
            LOGGER_DEBUG,
            format!(
                "raop_rtp local data port    socket {} port UDP {data_local}",
                socket_descriptor(&data_socket)
            ),
        );

        {
            let mut sync = lock(&self.sync);
            sync.rtp_clock_rate = (1_000_000_000 / sample_rate) as f64;
            sync.ntp_start_time = self.clock.local_time_ns();
        }
        self.running.store(true, Ordering::Release);
        self.joined = false;
        let ports = AudioPorts {
            control_local,
            data_local,
            ..ports
        };
        let running = Arc::clone(&self.running);
        let events = Arc::clone(&self.events);
        let sync = Arc::clone(&self.sync);
        let callbacks = Arc::clone(&self.callbacks);
        let clock = Arc::clone(&self.clock);
        let logger = self.logger.clone();
        let debug_data_enabled = self.debug_data_enabled;
        let audio_key = self.audio_key;
        let audio_iv = self.audio_iv;
        let context = AudioThreadContext {
            control_remote_port: ports.control_remote,
            codec_type,
            running,
            events,
            sync,
            callbacks,
            clock,
            logger,
            debug_data_enabled,
            audio_key,
            audio_iv,
        };

        let worker = thread::Builder::new()
            .name("uxplay-raop-rtp-audio".to_owned())
            .spawn(move || run_audio_thread(control_socket, data_socket, context));
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                self.running.store(false, Ordering::Release);
                self.joined = true;
                return Err(error);
            }
        };
        self.worker = Some(worker);
        Ok(StartState::Started(ports))
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(-144.0, 0.0);
        let mut events = lock(&self.events);
        events.volume = volume;
        events.volume_changed = true;
    }

    pub fn set_metadata(&self, data: &[u8]) {
        if !data.is_empty() {
            lock(&self.events).metadata = Some(data.to_vec());
        }
    }

    pub fn set_coverart(&self, data: &[u8]) {
        if !data.is_empty() {
            lock(&self.events).coverart = Some(data.to_vec());
        }
    }

    pub fn set_remote_control_id(&self, dacp_id: Option<&str>, active_remote_header: Option<&str>) {
        if let (Some(dacp_id), Some(active_remote_header)) = (dacp_id, active_remote_header) {
            lock(&self.events).remote_control = Some((
                dacp_id.split('\0').next().unwrap_or_default().to_owned(),
                active_remote_header
                    .split('\0')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            ));
        }
    }

    pub fn set_progress(&self, start: u32, current: u32, end: u32) {
        lock(&self.events).progress = Some((start, current, end));
    }

    pub fn flush(&self, next_sequence_number: i32) {
        lock(&self.events).flush = Some(next_sequence_number);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn stop(&mut self) {
        let was_running = {
            let _events = lock(&self.events);
            self.running.swap(false, Ordering::AcqRel)
        };
        if was_running && !self.joined {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            self.joined = true;
        } else if let Some(worker) = self.worker.take() {
            // The source leaves a failed thread unjoined; Rust still reaps it to avoid
            // retaining a thread handle when the session is destroyed.
            let _ = worker.join();
        }
    }
}

impl Drop for RaopRtpSession {
    fn drop(&mut self) {
        self.stop();
    }
}

struct AudioThreadContext {
    control_remote_port: u16,
    codec_type: u8,
    running: Arc<AtomicBool>,
    events: Arc<Mutex<PendingEvents>>,
    sync: Arc<Mutex<SyncState>>,
    callbacks: Arc<dyn AudioCallbacks>,
    clock: Arc<dyn TimingClock>,
    logger: Option<LogCallback>,
    debug_data_enabled: bool,
    audio_key: [u8; 16],
    audio_iv: [u8; 16],
}

fn run_audio_thread(
    control_socket: UdpSocket,
    data_socket: UdpSocket,
    context: AudioThreadContext,
) {
    let AudioThreadContext {
        control_remote_port,
        codec_type,
        running,
        events,
        sync,
        callbacks,
        clock,
        logger,
        debug_data_enabled,
        audio_key,
        audio_iv,
    } = context;
    let mut queue = match AudioBuffer::new(audio_key, audio_iv) {
        Ok(queue) => queue,
        Err(error) => {
            log(
                &logger,
                LOGGER_ERR,
                format!("raop_rtp audio buffer initialization failed: {error:?}"),
            );
            running.store(false, Ordering::Release);
            return;
        }
    };
    let mut control_peer = None;
    let mut video_arrival_offset = 0;
    let mut control_packet = [0_u8; RAOP_PACKET_LEN];
    let mut data_packet = [0_u8; RAOP_PACKET_LEN];
    let start_time = lock(&sync).ntp_start_time;
    log(
        &logger,
        LOGGER_DEBUG,
        format!(
            "raop_rtp start_time = {:.6} (raop_rtp audio)",
            start_time as f64 / 1e9
        ),
    );

    while running.load(Ordering::Acquire) {
        if process_events(&events, &callbacks, &running) {
            break;
        }
        let mut received_packet = false;

        match control_socket.recv_from(&mut control_packet) {
            Ok((len, peer)) => {
                received_packet = true;
                if len > 0 && control_peer.is_none() {
                    control_peer = Some(peer);
                }
                log_control_packet(&control_packet[..len], lock(&sync).initial_sync, &logger);
                if let Err(error) = process_control_packet(
                    &control_packet[..len],
                    &mut queue,
                    &mut lock(&sync),
                    clock.as_ref(),
                ) {
                    log(
                        &logger,
                        LOGGER_ERR,
                        format!("raop_rtp control packet failed: {error}"),
                    );
                    running.store(false, Ordering::Release);
                    break;
                }
            }
            Err(error) if is_would_block(&error) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => log_recvfrom_error(&logger, &error),
        }

        match data_socket.recv_from(&mut data_packet) {
            Ok((mut len, _peer)) => {
                received_packet = true;
                if !lock(&sync).initial_sync && video_arrival_offset == 0 {
                    video_arrival_offset = clock.video_arrival_offset();
                }
                if len < 12 {
                    log_short_data_packet(&data_packet[..len], &logger);
                    continue;
                }

                {
                    let mut state = lock(&sync);
                    if !state.initial_sync && codec_type == 8 && video_arrival_offset != 0 {
                        let estimated = clock
                            .local_time_ns()
                            .wrapping_sub(video_arrival_offset)
                            .wrapping_add((AUDIO_SYNC_TOLERANCE_SECONDS * 1e9) as u64);
                        state.client_ntp_sync = estimated;
                        state.rtp_sync = be_u32(&data_packet, 4).unwrap_or(0);
                        state.initial_sync = true;
                    }
                }

                if len == 12 {
                    data_packet[12..16].copy_from_slice(&NO_DATA_MARKER);
                    len = 16;
                }
                if codec_type == 2 && len == 44 {
                    continue;
                }
                if let Err(error) = queue.enqueue(&data_packet[..len], true) {
                    log(
                        &logger,
                        LOGGER_ERR,
                        format!("raop_rtp audio packet failed: {error}"),
                    );
                    running.store(false, Ordering::Release);
                    break;
                }
                if !lock(&sync).initial_sync {
                    continue;
                }

                while let Some(frame) = queue.dequeue(control_remote_port == 0) {
                    if frame.data.as_slice() == NO_DATA_MARKER {
                        continue;
                    }
                    let remote_time_ns = {
                        let state = lock(&sync);
                        rtp_time_to_client_ntp(&state, frame.rtp_timestamp)
                    };
                    let local_time_ns = clock.convert_remote_time_ns(remote_time_ns);
                    if debug_data_enabled {
                        let now_ns = clock.local_time_ns();
                        let latency_ns = if local_time_ns == 0 {
                            0
                        } else {
                            (now_ns as i64).wrapping_sub(local_time_ns as i64)
                        };
                        let codec_name = match codec_type {
                            2 => "ALAC",
                            8 => "AAC_ELD",
                            _ => "TYPE=?",
                        };
                        log(
                            &logger,
                            LOGGER_DEBUG,
                            format!(
                                "raop_rtp audio: now = {:8.6}, ntp = {:8.6}, latency = {:9.6}, ts = {:8.6}, rtp_time={} seqnum = {:5} {} {}",
                                now_ns as f64 / 1e9,
                                local_time_ns as f64 / 1e9,
                                latency_ns as f64 / 1e9,
                                remote_time_ns as f64 / 1e9,
                                frame.rtp_timestamp,
                                frame.sequence_number,
                                codec_name,
                                frame.data.len(),
                            ),
                        );
                    }
                    callbacks.process_audio(AudioFrame {
                        data: frame.data,
                        codec_type,
                        sequence_number: frame.sequence_number,
                        rtp_timestamp: frame.rtp_timestamp,
                        remote_time_ns,
                        local_time_ns,
                    });
                }
                if control_remote_port != 0
                    && let Some((sequence_number, count)) = queue.pending_resend()
                    && let Some(peer) = control_peer
                {
                    send_resend_request(
                        &control_socket,
                        peer,
                        sequence_number,
                        count,
                        &mut lock(&sync).control_sequence_number,
                        &logger,
                    );
                }
            }
            Err(error) if is_would_block(&error) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => log_recvfrom_error(&logger, &error),
        }
        if !received_packet {
            thread::sleep(SOCKET_POLL_INTERVAL);
        }
    }
    log(&logger, LOGGER_DEBUG, "raop_rtp exiting thread".to_owned());
}

fn process_control_packet(
    packet: &[u8],
    queue: &mut AudioBuffer,
    sync: &mut SyncState,
    clock: &dyn TimingClock,
) -> io::Result<()> {
    if packet.len() < 2 {
        return Ok(());
    }
    let packet_type = packet[1] & !0x80;
    if packet_type == 0x56 && packet.len() >= 8 {
        let resent_packet = &packet[4..];
        if resent_packet.len() >= 12 {
            queue.enqueue(resent_packet, true)?;
        }
    } else if packet_type == 0x54 && packet.len() >= 20 {
        sync.initial_sync = true;
        sync.rtp_sync = be_u32(packet, 4).unwrap_or(0);
        let ntp_raw = be_u64(packet, 8).unwrap_or(0);
        let adjusted = clock.adjust_remote_timestamp_offset(ntp_raw, false);
        sync.client_ntp_sync = clock.timestamp_to_nanoseconds(adjusted);
    }
    Ok(())
}

fn log_control_packet(packet: &[u8], initial_sync: bool, logger: &Option<LogCallback>) {
    let Some(packet_type) = packet.get(1).map(|byte| byte & !0x80) else {
        return;
    };
    log(
        logger,
        LOGGER_DEBUG,
        format!(
            "\nraop_rtp type_c 0x{packet_type:02x}, packetlen = {}",
            packet.len()
        ),
    );
    match packet_type {
        0x56 if packet.len() >= 8 => {
            let sequence_number = u16::from_be_bytes([packet[6], packet[7]]);
            if packet.len() >= 16 {
                log(
                    logger,
                    LOGGER_DEBUG,
                    format!("raop_rtp resent audio packet: seqnum={sequence_number}"),
                );
            } else {
                log(
                    logger,
                    LOGGER_DEBUG,
                    format!(
                        "Received empty resent audio packet length {}, seqnum={sequence_number}:\n{}",
                        packet.len(),
                        packet_to_string(packet, 16)
                    ),
                );
            }
        }
        0x54 if packet.len() >= 20 => {
            if !initial_sync {
                log(logger, LOGGER_DEBUG, "first audio rtp sync".to_owned());
            }
        }
        _ => log(
            logger,
            LOGGER_DEBUG,
            format!(
                "raop_rtp unknown udp control packet\n{}",
                packet_to_string(packet, 16)
            ),
        ),
    }
}

fn log_short_data_packet(packet: &[u8], logger: &Option<LogCallback>) {
    let packet_type = packet.get(1).map_or(0, |byte| byte & !0x80);
    log(
        logger,
        LOGGER_DEBUG,
        format!(
            "Received short type_d = 0x{packet_type:2x}  packet with length {}:\n{}",
            packet.len(),
            packet_to_string(packet, 16)
        ),
    );
}

fn packet_to_string(packet: &[u8], bytes_per_line: usize) -> String {
    let mut output = String::with_capacity(packet.len().saturating_mul(3).saturating_add(1));
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, byte) in packet.iter().enumerate() {
        if index != 0 && index % bytes_per_line == 0 {
            output.push('\n');
        }
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        output.push(' ');
    }
    output.push('\n');
    output
}

fn send_resend_request(
    socket: &UdpSocket,
    peer: SocketAddr,
    sequence_number: u16,
    count: u16,
    control_sequence_number: &mut u16,
    logger: &Option<LogCallback>,
) {
    let sequence = *control_sequence_number;
    *control_sequence_number = sequence.wrapping_add(1);
    log(
        logger,
        LOGGER_DEBUG,
        format!("raop_rtp got resend request {sequence_number} {count}"),
    );
    let packet = [
        0x80,
        0xd5,
        (sequence >> 8) as u8,
        sequence as u8,
        (sequence_number >> 8) as u8,
        sequence_number as u8,
        (count >> 8) as u8,
        count as u8,
    ];
    if let Err(error) = socket.send_to(&packet, peer) {
        log_resend_error(logger, error.raw_os_error().unwrap_or(0));
    }
}

fn process_events(
    events: &Mutex<PendingEvents>,
    callbacks: &Arc<dyn AudioCallbacks>,
    running: &AtomicBool,
) -> bool {
    let pending = {
        let mut state = lock(events);
        if !running.load(Ordering::Acquire) {
            return true;
        }
        PendingEvents {
            volume: state.volume,
            volume_changed: std::mem::take(&mut state.volume_changed),
            flush: state.flush.take(),
            metadata: state.metadata.take(),
            coverart: state.coverart.take(),
            remote_control: state.remote_control.take(),
            progress: state.progress.take(),
        }
    };
    if pending.volume_changed {
        callbacks.set_volume(pending.volume);
    }
    if pending.flush.is_some_and(|value| value != NO_FLUSH) {
        callbacks.flush();
    }
    if let Some(metadata) = pending.metadata {
        callbacks.set_metadata(metadata);
    }
    if let Some(coverart) = pending.coverart {
        callbacks.set_coverart(coverart);
    }
    if let Some((dacp_id, active_remote_header)) = pending.remote_control {
        callbacks.remote_control_id(dacp_id, active_remote_header);
    }
    if let Some((start, current, end)) = pending.progress {
        callbacks.set_progress(start, current, end);
    }
    false
}

fn rtp_time_to_client_ntp(sync: &SyncState, rtp_timestamp: u32) -> u64 {
    if !sync.initial_sync {
        return 0;
    }
    let rtp_change = rtp_timestamp.wrapping_sub(sync.rtp_sync) as i32;
    let value = sync.rtp_clock_rate * f64::from(rtp_change) + sync.client_ntp_sync as f64;
    if value < 0.0 { 0 } else { value as u64 }
}

fn parse_remote_ip(remote: &str, length: usize) -> io::Result<IpAddr> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid numeric remote address",
        )
    };
    match length {
        4 => Ipv4Addr::from_str(remote)
            .map(IpAddr::V4)
            .map_err(|_| invalid()),
        16 => {
            let (literal, scope) = remote
                .rsplit_once('%')
                .map_or((remote, None), |(literal, scope)| (literal, Some(scope)));
            if scope.is_some_and(|scope| scope.parse::<u32>().is_err()) {
                return Err(invalid());
            }
            Ipv6Addr::from_str(literal)
                .map(IpAddr::V6)
                .map_err(|_| invalid())
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote address length must be 4 or 16",
        )),
    }
}

fn fixed_key(bytes: &[u8], name: &str) -> io::Result<[u8; 16]> {
    bytes.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must contain exactly 16 bytes"),
        )
    })
}

fn bind_udp(address: IpAddr, port: u16) -> io::Result<(UdpSocket, u16)> {
    let domain = if address.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    let bind_address = if address.is_ipv4() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
    } else {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)
    };
    socket.bind(&SockAddr::from(bind_address))?;
    let actual_port = socket
        .local_addr()?
        .as_socket()
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "socket has no IP address"))?
        .port();
    Ok((socket.into(), actual_port))
}

fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn be_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn is_would_block(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn log(logger: &Option<LogCallback>, level: i32, message: String) {
    if let Some(logger) = logger {
        logger(level, &message);
    }
}

fn log_recvfrom_error(logger: &Option<LogCallback>, error: &io::Error) {
    let (code, text) = error.raw_os_error().map_or_else(
        || (0, error.to_string()),
        |code| (code, socket_error_message(code)),
    );
    log(
        logger,
        LOGGER_ERR,
        format!("raop_rtp error in recvfrom  {code} {text}"),
    );
}

fn socket_error_message(code: i32) -> String {
    #[cfg(windows)]
    {
        String::from_utf8_lossy(&compat_rs::port::wsa_strerror(code)).into_owned()
    }
    #[cfg(not(windows))]
    {
        io::Error::from_raw_os_error(code).to_string()
    }
}

fn log_resend_error(logger: &Option<LogCallback>, code: i32) {
    log(
        logger,
        LOGGER_WARNING,
        format!("raop_rtp resend failed: {code}"),
    );
}

#[cfg(unix)]
fn socket_descriptor(socket: &UdpSocket) -> i64 {
    use std::os::fd::AsRawFd;
    i64::from(socket.as_raw_fd())
}

#[cfg(windows)]
fn socket_descriptor(socket: &UdpSocket) -> i64 {
    use std::os::windows::io::AsRawSocket;
    socket.as_raw_socket() as i64
}

#[cfg(not(any(unix, windows)))]
fn socket_descriptor(_socket: &UdpSocket) -> i64 {
    -1
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct BufferedAudio {
    sequence_number: u16,
    rtp_timestamp: u32,
    data: Vec<u8>,
}

struct AudioBuffer {
    aes: AesContext,
    entries: Vec<Option<BufferedAudio>>,
    first: u16,
    last: u16,
    empty: bool,
}

impl AudioBuffer {
    fn new(key: [u8; 16], iv: [u8; 16]) -> Result<Self, CryptoError> {
        Ok(Self {
            aes: AesContext::new(AesMode::Cbc, Direction::Decrypt, &key, &iv)?,
            entries: (0..RAOP_BUFFER_LENGTH).map(|_| None).collect(),
            first: 0,
            last: 0,
            empty: true,
        })
    }

    fn enqueue(&mut self, datagram: &[u8], use_sequence_number: bool) -> io::Result<bool> {
        if datagram.len() < 12 || datagram.len() > RAOP_PACKET_LEN {
            return Ok(false);
        }
        if datagram.len() == 12 {
            return Ok(false);
        }
        let sequence = if use_sequence_number {
            u16::from_be_bytes([datagram[2], datagram[3]])
        } else {
            self.first
        };
        if !self.empty && sequence_cmp(sequence, self.first) < 0 {
            return Ok(false);
        }
        let end = self.first.wrapping_add(RAOP_BUFFER_LENGTH as u16);
        if sequence_cmp(sequence, end) >= 0 {
            self.flush(Some(sequence));
        }
        let slot = usize::from(sequence) % RAOP_BUFFER_LENGTH;
        if self.entries[slot]
            .as_ref()
            .is_some_and(|entry| entry.sequence_number == sequence)
        {
            return Ok(false);
        }
        let payload = &datagram[12..];
        let data = if datagram.len() == 16 && payload == NO_DATA_MARKER {
            payload.to_vec()
        } else {
            let encrypted_len = payload.len() / 16 * 16;
            let mut output = self
                .aes
                .cbc_crypt(&payload[..encrypted_len], Direction::Decrypt)
                .map_err(|error| io::Error::other(format!("{error:?}")))?;
            self.aes
                .reset()
                .map_err(|error| io::Error::other(format!("{error:?}")))?;
            output.extend_from_slice(&payload[encrypted_len..]);
            output
        };
        self.entries[slot] = Some(BufferedAudio {
            sequence_number: sequence,
            rtp_timestamp: u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]),
            data,
        });
        if self.empty {
            self.first = sequence;
            self.last = sequence;
            self.empty = false;
        }
        if sequence_cmp(sequence, self.last) > 0 {
            self.last = sequence;
        }
        Ok(true)
    }

    fn dequeue(&mut self, no_resend: bool) -> Option<BufferedAudio> {
        let count = sequence_cmp(self.last, self.first).wrapping_add(1);
        if self.empty || count <= 0 {
            return None;
        }
        let slot = usize::from(self.first) % RAOP_BUFFER_LENGTH;
        if !no_resend && self.entries[slot].is_none() && count < RAOP_BUFFER_LENGTH as i16 {
            return None;
        }
        self.first = self.first.wrapping_add(1);
        self.entries[slot].take()
    }

    fn pending_resend(&self) -> Option<(u16, u16)> {
        if sequence_cmp(self.first, self.last) >= 0 {
            return None;
        }
        let mut sequence = self.first;
        let mut count = 0_u16;
        while sequence_cmp(sequence, self.last) < 0 {
            let filled = self.entries[usize::from(sequence) % RAOP_BUFFER_LENGTH]
                .as_ref()
                .is_some_and(|entry| entry.sequence_number == sequence);
            if filled {
                break;
            }
            count = count.wrapping_add(1);
            sequence = sequence.wrapping_add(1);
        }
        (count != 0).then_some((self.first, count))
    }

    fn flush(&mut self, next: Option<u16>) {
        self.entries.fill_with(|| None);
        match next {
            Some(sequence) => {
                self.first = sequence;
                self.last = sequence.wrapping_sub(1);
            }
            None => self.empty = true,
        }
    }
}

fn sequence_cmp(left: u16, right: u16) -> i16 {
    left.wrapping_sub(right) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeClock;

    impl TimingClock for FakeClock {
        fn local_time_ns(&self) -> u64 {
            1_000_000_000
        }
        fn adjust_remote_timestamp_offset(&self, timestamp: u64, _add_epoch: bool) -> u64 {
            timestamp
        }
        fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
            timestamp
        }
        fn convert_remote_time_ns(&self, timestamp: u64) -> u64 {
            timestamp + 1_000
        }
        fn video_arrival_offset(&self) -> u64 {
            0
        }
    }

    #[derive(Default)]
    struct FakeCallbacks {
        events: Mutex<Vec<String>>,
        frames: Mutex<Vec<AudioFrame>>,
    }

    impl AudioCallbacks for FakeCallbacks {
        fn process_audio(&self, frame: AudioFrame) {
            lock(&self.frames).push(frame);
        }
        fn flush(&self) {
            lock(&self.events).push("flush".to_owned());
        }
        fn set_volume(&self, value: f32) {
            lock(&self.events).push(format!("volume:{value}"));
        }
        fn set_metadata(&self, value: Vec<u8>) {
            lock(&self.events).push(format!("metadata:{}", value.len()));
        }
        fn set_coverart(&self, value: Vec<u8>) {
            lock(&self.events).push(format!("cover:{}", value.len()));
        }
        fn remote_control_id(&self, dacp: String, remote: String) {
            lock(&self.events).push(format!("remote:{dacp}:{remote}"));
        }
        fn set_progress(&self, start: u32, current: u32, end: u32) {
            lock(&self.events).push(format!("progress:{start}:{current}:{end}"));
        }
    }

    fn session() -> RaopRtpSession {
        RaopRtpSession::new(
            None,
            Arc::new(FakeCallbacks::default()),
            Arc::new(FakeClock),
            "127.0.0.1",
            4,
            &[0x11; 16],
            &[0x22; 16],
        )
        .unwrap()
    }

    type CapturedRuntimeLogs = Arc<Mutex<Vec<(i32, String)>>>;

    fn collecting_runtime_logger() -> (LogCallback, CapturedRuntimeLogs) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger: LogCallback = Arc::new(move |level, message| {
            lock(&events_for_callback).push((level, message.to_owned()));
        });
        (logger, events)
    }

    #[test]
    fn control_packet_diagnostics_match_raop_source_messages() {
        let (callback, events) = collecting_runtime_logger();
        let logger = Some(callback);

        let empty_resend = [0x80, 0xd6, 0, 0, 0, 0, 0, 9];
        log_control_packet(&empty_resend, false, &logger);
        assert_eq!(
            *lock(&events),
            [
                (
                    LOGGER_DEBUG,
                    "\nraop_rtp type_c 0x56, packetlen = 8".to_owned()
                ),
                (
                    LOGGER_DEBUG,
                    "Received empty resent audio packet length 8, seqnum=9:\n80 d6 00 00 00 00 00 09 \n"
                        .to_owned()
                ),
            ]
        );

        lock(&events).clear();
        let mut received_resend = [0_u8; 16];
        received_resend[1] = 0xd6;
        received_resend[6..8].copy_from_slice(&9_u16.to_be_bytes());
        log_control_packet(&received_resend, true, &logger);
        assert_eq!(
            *lock(&events),
            [
                (
                    LOGGER_DEBUG,
                    "\nraop_rtp type_c 0x56, packetlen = 16".to_owned()
                ),
                (
                    LOGGER_DEBUG,
                    "raop_rtp resent audio packet: seqnum=9".to_owned()
                ),
            ]
        );

        lock(&events).clear();
        let mut sync = [0_u8; 20];
        sync[1] = 0xd4;
        log_control_packet(&sync, false, &logger);
        assert_eq!(
            *lock(&events),
            [
                (
                    LOGGER_DEBUG,
                    "\nraop_rtp type_c 0x54, packetlen = 20".to_owned()
                ),
                (LOGGER_DEBUG, "first audio rtp sync".to_owned()),
            ]
        );

        lock(&events).clear();
        let unknown = [0x80, 0xc1, 0x02];
        log_control_packet(&unknown, true, &logger);
        assert_eq!(
            *lock(&events),
            [
                (
                    LOGGER_DEBUG,
                    "\nraop_rtp type_c 0x41, packetlen = 3".to_owned()
                ),
                (
                    LOGGER_DEBUG,
                    "raop_rtp unknown udp control packet\n80 c1 02 \n".to_owned()
                ),
            ]
        );

        lock(&events).clear();
        log_short_data_packet(&[0x80, 0x60, 0x01], &logger);
        assert_eq!(
            *lock(&events),
            [(
                LOGGER_DEBUG,
                "Received short type_d = 0x60  packet with length 3:\n80 60 01 \n".to_owned()
            )]
        );
    }

    #[test]
    fn socket_error_diagnostics_keep_source_codes_and_wording() {
        let (callback, events) = collecting_runtime_logger();
        let logger = Some(callback);
        let receive_error = io::Error::other("fixture socket failure");
        log_recvfrom_error(&logger, &receive_error);
        log_resend_error(&logger, 10054);
        assert_eq!(
            *lock(&events),
            [
                (
                    LOGGER_ERR,
                    "raop_rtp error in recvfrom  0 fixture socket failure".to_owned(),
                ),
                (LOGGER_WARNING, "raop_rtp resend failed: 10054".to_owned(),),
            ]
        );
        let receive_error = io::Error::from_raw_os_error(10054);
        log_recvfrom_error(&logger, &receive_error);
        let expected_message = format!(
            "raop_rtp error in recvfrom  10054 {}",
            socket_error_message(10054)
        );
        assert_eq!(lock(&events).last().unwrap().1, expected_message);
    }

    #[test]
    fn resend_request_logs_source_sequence_and_preserves_wire_packet() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (callback, events) = collecting_runtime_logger();
        let mut control_sequence = 5;

        send_resend_request(
            &sender,
            receiver.local_addr().unwrap(),
            0x1234,
            2,
            &mut control_sequence,
            &Some(callback),
        );

        let mut packet = [0_u8; 8];
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, 8);
        assert_eq!(packet, [0x80, 0xd5, 0, 5, 0x12, 0x34, 0, 2]);
        assert_eq!(
            *lock(&events),
            [(
                LOGGER_DEBUG,
                "raop_rtp got resend request 4660 2".to_owned()
            )]
        );
        assert_eq!(control_sequence, 6);
    }

    #[test]
    fn remote_address_and_key_lengths_match_source_preconditions() {
        assert!(
            RaopRtpSession::new(
                None,
                Arc::new(FakeCallbacks::default()),
                Arc::new(FakeClock),
                "localhost",
                4,
                &[0; 16],
                &[0; 16]
            )
            .is_err()
        );
        assert!(
            RaopRtpSession::new(
                None,
                Arc::new(FakeCallbacks::default()),
                Arc::new(FakeClock),
                "127.0.0.1",
                16,
                &[0; 16],
                &[0; 16]
            )
            .is_err()
        );
        assert!(
            RaopRtpSession::new(
                None,
                Arc::new(FakeCallbacks::default()),
                Arc::new(FakeClock),
                "127.0.0.1",
                4,
                &[0; 15],
                &[0; 16]
            )
            .is_err()
        );
    }

    #[test]
    fn scoped_ipv6_remote_address_is_accepted_only_with_numeric_zone() {
        assert_eq!(
            parse_remote_ip("fe80::1%7", 16).unwrap(),
            IpAddr::V6("fe80::1".parse().unwrap())
        );
        assert!(parse_remote_ip("fe80::1%en0", 16).is_err());
        assert!(parse_remote_ip("fe80::1%7", 4).is_err());
    }

    #[test]
    fn constructor_forwards_source_debug_level_to_logger_callback() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&messages);
        let logger: LogCallback = Arc::new(move |level, message| {
            lock(&captured).push((level, message.to_owned()));
        });
        let _session = RaopRtpSession::new(
            Some(logger),
            Arc::new(FakeCallbacks::default()),
            Arc::new(FakeClock),
            "127.0.0.1",
            4,
            &[0x11; 16],
            &[0x22; 16],
        )
        .unwrap();
        assert_eq!(
            *lock(&messages),
            [(
                LOGGER_DEBUG,
                "raop_rtp parse remote ip = 127.0.0.1".to_owned()
            )]
        );
    }

    #[test]
    fn setters_coalesce_to_latest_state_and_process_in_source_order() {
        let callbacks = Arc::new(FakeCallbacks::default());
        let events = Mutex::new(PendingEvents::default());
        {
            let mut pending = lock(&events);
            pending.volume = 7.0;
            pending.volume_changed = true;
            pending.flush = Some(9);
            pending.metadata = Some(b"meta".to_vec());
            pending.coverart = Some(b"cover".to_vec());
            pending.remote_control = Some(("dacp".to_owned(), "remote".to_owned()));
            pending.progress = Some((1, 2, 3));
        }
        let running = AtomicBool::new(true);
        assert!(!process_events(
            &events,
            &(callbacks.clone() as Arc<dyn AudioCallbacks>),
            &running
        ));
        assert_eq!(
            *lock(&callbacks.events),
            [
                "volume:7",
                "flush",
                "metadata:4",
                "cover:5",
                "remote:dacp:remote",
                "progress:1:2:3"
            ]
        );
    }

    #[test]
    fn volume_clamps_and_empty_metadata_is_ignored() {
        let session = session();
        session.set_volume(20.0);
        session.set_metadata(&[]);
        assert_eq!(lock(&session.events).volume, 0.0);
        assert!(lock(&session.events).metadata.is_none());
        session.set_volume(-200.0);
        assert_eq!(lock(&session.events).volume, -144.0);
    }

    #[test]
    fn start_uses_ephemeral_udp_ports_and_stops_cleanly() {
        let mut session = session();
        let state = session
            .start_audio(
                AudioPorts {
                    control_remote: 0,
                    control_local: 0,
                    data_local: 0,
                },
                2,
                44_100,
            )
            .unwrap();
        let StartState::Started(ports) = state else {
            panic!("first start should bind")
        };
        assert_ne!(ports.control_local, 0);
        assert_ne!(ports.data_local, 0);
        assert!(session.is_running());
        assert_eq!(
            session
                .start_audio(
                    AudioPorts {
                        control_remote: 0,
                        control_local: 0,
                        data_local: 0
                    },
                    2,
                    44_100
                )
                .unwrap(),
            StartState::AlreadyRunning
        );
        session.stop();
        assert!(!session.is_running());
    }

    #[test]
    fn audio_socket_binding_logs_source_descriptors_and_ports() {
        let (callback, events) = collecting_runtime_logger();
        let mut session = RaopRtpSession::new(
            Some(callback),
            Arc::new(FakeCallbacks::default()),
            Arc::new(FakeClock),
            "127.0.0.1",
            4,
            &[0x11; 16],
            &[0x22; 16],
        )
        .unwrap();
        let StartState::Started(ports) = session
            .start_audio(
                AudioPorts {
                    control_remote: 0,
                    control_local: 0,
                    data_local: 0,
                },
                2,
                44_100,
            )
            .unwrap()
        else {
            panic!("first start should bind");
        };
        session.stop();

        let events = lock(&events);
        assert!(events.iter().any(|(level, message)| {
            *level == LOGGER_DEBUG
                && message.starts_with("raop_rtp local control port socket ")
                && message.ends_with(&format!(" port UDP {}", ports.control_local))
        }));
        assert!(events.iter().any(|(level, message)| {
            *level == LOGGER_DEBUG
                && message.starts_with("raop_rtp local data port    socket ")
                && message.ends_with(&format!(" port UDP {}", ports.data_local))
        }));
    }

    #[test]
    fn configured_udp_binder_creates_both_audio_sockets() {
        let mut session = session();
        let bind_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bind_count_for_factory = Arc::clone(&bind_count);
        session.set_udp_socket_binder(Arc::new(move |address, port| {
            bind_count_for_factory.fetch_add(1, Ordering::AcqRel);
            bind_udp(address, port)
        }));

        session
            .start_audio(
                AudioPorts {
                    control_remote: 0,
                    control_local: 0,
                    data_local: 0,
                },
                2,
                44_100,
            )
            .unwrap();
        session.stop();
        assert_eq!(bind_count.load(Ordering::Acquire), 2);
    }

    #[test]
    fn loopback_sync_and_rtp_datagrams_reach_the_audio_callback() {
        let callbacks = Arc::new(FakeCallbacks::default());
        let clock = Arc::new(FakeClock);
        let (logger, logs) = collecting_runtime_logger();
        let key = [0x11; 16];
        let iv = [0x22; 16];
        let mut session = RaopRtpSession::new(
            Some(logger),
            callbacks.clone(),
            clock,
            "127.0.0.1",
            4,
            &key,
            &iv,
        )
        .unwrap();
        session.set_debug_data_enabled(true);
        let StartState::Started(ports) = session
            .start_audio(
                AudioPorts {
                    control_remote: 0,
                    control_local: 0,
                    data_local: 0,
                },
                2,
                44_100,
            )
            .unwrap()
        else {
            panic!("first start should bind")
        };
        let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let sync_timestamp = 2_000_000_000_u64;
        let rtp_sync = 0x1020_3040_u32;
        let mut sync_packet = vec![0_u8; 20];
        sync_packet[0] = 0x80;
        sync_packet[1] = 0xd4;
        sync_packet[4..8].copy_from_slice(&rtp_sync.to_be_bytes());
        sync_packet[8..16].copy_from_slice(&sync_timestamp.to_be_bytes());
        sender
            .send_to(&sync_packet, (Ipv4Addr::LOCALHOST, ports.control_local))
            .unwrap();

        let plaintext = [0x5a; 16];
        let mut encryptor = AesContext::new(AesMode::Cbc, Direction::Encrypt, &key, &iv).unwrap();
        let mut encrypted_payload = encryptor.cbc_crypt(&plaintext, Direction::Encrypt).unwrap();
        encrypted_payload.extend_from_slice(b"abc");
        let mut expected_payload = plaintext.to_vec();
        expected_payload.extend_from_slice(b"abc");

        for sequence_number in [29_u16, 30] {
            let mut audio_packet = vec![0x80, 0x60];
            audio_packet.extend_from_slice(&sequence_number.to_be_bytes());
            audio_packet.extend_from_slice(&rtp_sync.to_be_bytes());
            audio_packet.extend_from_slice(&[0; 4]);
            audio_packet.extend_from_slice(&encrypted_payload);
            sender
                .send_to(&audio_packet, (Ipv4Addr::LOCALHOST, ports.data_local))
                .unwrap();
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while lock(&callbacks.frames).len() < 2 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let frames = lock(&callbacks.frames).clone();
        assert_eq!(frames.len(), 2);
        for (frame, sequence_number) in frames.iter().zip([29_u16, 30]) {
            assert_eq!(frame.data, expected_payload);
            assert_eq!(frame.codec_type, 2);
            assert_eq!(frame.sequence_number, sequence_number);
            assert_eq!(frame.rtp_timestamp, rtp_sync);
            assert_eq!(frame.remote_time_ns, sync_timestamp);
            assert_eq!(frame.local_time_ns, sync_timestamp + 1_000);
        }
        let audio_timing_logs: Vec<_> = lock(&logs)
            .iter()
            .filter(|(_, message)| message.starts_with("raop_rtp audio: now = "))
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            audio_timing_logs,
            vec![
                "raop_rtp audio: now = 1.000000, ntp = 2.000001, latency = -1.000001, ts = 2.000000, rtp_time=270544960 seqnum =    29 ALAC 19".to_owned(),
                "raop_rtp audio: now = 1.000000, ntp = 2.000001, latency = -1.000001, ts = 2.000000, rtp_time=270544960 seqnum =    30 ALAC 19".to_owned(),
            ]
        );
        session.stop();
    }

    #[test]
    fn rtp_timestamp_wraps_as_a_signed_32_bit_delta() {
        let sync = SyncState {
            initial_sync: true,
            client_ntp_sync: 123_456_789,
            rtp_sync: u32::MAX - 2,
            rtp_clock_rate: (1_000_000_000_u32 / 44_100) as f64,
            ..SyncState::default()
        };
        let mut rows = vec![format!("wrap-forward,{}", rtp_time_to_client_ntp(&sync, 3))];
        let before = SyncState {
            rtp_sync: 3,
            client_ntp_sync: 500,
            ..sync
        };
        rows.push(format!(
            "wrap-negative-clamp,{}",
            rtp_time_to_client_ntp(&before, u32::MAX - 2)
        ));
        let unsynced = SyncState {
            initial_sync: false,
            client_ntp_sync: 77,
            ..sync
        };
        rows.push(format!(
            "before-sync,{}",
            rtp_time_to_client_ntp(&unsynced, 42)
        ));
        assert_eq!(
            rows.join("\n") + "\n",
            include_str!("../reference/rtp_time.expected.txt").replace("\r\n", "\n")
        );
    }

    #[test]
    fn audio_buffer_skips_short_packets_and_decrypts_clear_marker() {
        let mut buffer = AudioBuffer::new([0x11; 16], [0x22; 16]).unwrap();
        assert!(!buffer.enqueue(&[0; 11], true).unwrap());
        let mut packet = vec![0x80, 0x60, 0, 7, 1, 2, 3, 4, 0, 0, 0, 0];
        packet.extend_from_slice(&NO_DATA_MARKER);
        assert!(buffer.enqueue(&packet, true).unwrap());
        assert_eq!(buffer.dequeue(false).unwrap().data, NO_DATA_MARKER);
    }

    #[test]
    fn audio_buffer_decrypts_full_aes_cbc_blocks_and_tracks_resend_gaps() {
        let key = [0x11; 16];
        let iv = [0x22; 16];
        let plaintext = [0x5a; 16];
        let mut encryptor = AesContext::new(AesMode::Cbc, Direction::Encrypt, &key, &iv).unwrap();
        let encrypted = encryptor.cbc_crypt(&plaintext, Direction::Encrypt).unwrap();
        let mut buffer = AudioBuffer::new(key, iv).unwrap();

        let packet = |sequence: u16, timestamp: u32, payload: &[u8]| {
            let mut packet = vec![0x80, 0x60];
            packet.extend_from_slice(&sequence.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&[0; 4]);
            packet.extend_from_slice(payload);
            packet
        };

        assert!(buffer.enqueue(&packet(10, 100, &encrypted), true).unwrap());
        assert!(
            buffer
                .enqueue(&packet(12, 120, &NO_DATA_MARKER), true)
                .unwrap()
        );
        assert_eq!(buffer.pending_resend(), None);
        assert_eq!(buffer.dequeue(false).unwrap().data, plaintext);
        assert_eq!(buffer.pending_resend(), Some((11, 1)));
        assert!(buffer.dequeue(false).is_none());
        assert!(buffer.dequeue(true).is_none());
        assert_eq!(buffer.dequeue(true).unwrap().sequence_number, 12);
    }

    #[test]
    fn sync_control_packet_updates_remote_timing_anchor() {
        let clock = FakeClock;
        let mut state = SyncState::default();
        let mut queue = AudioBuffer::new([0x11; 16], [0x22; 16]).unwrap();
        let mut packet = vec![0_u8; 20];
        packet[0] = 0x80;
        packet[1] = 0xd4;
        packet[4..8].copy_from_slice(&0x7654_3210_u32.to_be_bytes());
        packet[8..16].copy_from_slice(&0x1234_5678_9abc_def0_u64.to_be_bytes());
        process_control_packet(&packet, &mut queue, &mut state, &clock).unwrap();
        assert!(state.initial_sync);
        assert_eq!(state.rtp_sync, 0x7654_3210);
        assert_eq!(state.client_ntp_sync, 0x1234_5678_9abc_def0);
    }
}
