//! TCP listener and worker lifecycle for RAOP video mirroring.

use crate::{MirrorDiagnostic, MirrorEvent, MirrorPacketProcessor, NtpClock, PACKET_HEADER_LEN};
use socket2::{Domain, Protocol, Socket, TcpKeepalive, Type};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const ACCEPT_POLL: Duration = Duration::from_millis(5);
const SOURCE_READ_TIMEOUT: Duration = Duration::from_millis(5);
const MAX_PACKET_PAYLOAD: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum MirrorLogLevel {
    Error = 3,
    Warning = 4,
    Info = 6,
    Debug = 7,
    DebugData = 8,
}

pub trait MirrorRuntimeCallbacks: Send + Sync + 'static {
    /// Return `false` when codec configuration fails, matching the source callback.
    fn event(&self, event: MirrorEvent) -> bool;

    fn log(&self, _level: MirrorLogLevel, _message: &str) {}
    fn mirror_video_running(&self, _running: bool) {}
    fn connection_reset(&self, _reason: u8) {}
    fn video_reset(&self) {}
}

pub type TcpListenerBinder =
    Arc<dyn Fn(IpAddr, u16) -> io::Result<TcpListener> + Send + Sync + 'static>;

pub struct MirrorReceiver {
    remote: IpAddr,
    audio_key: Vec<u8>,
    stream_connection_id: Option<u64>,
    clock: Arc<Mutex<Box<dyn NtpClock + Send>>>,
    callbacks: Arc<dyn MirrorRuntimeCallbacks>,
    listener_binder: TcpListenerBinder,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    port: Option<u16>,
    /// Re-key requests published by `init_aes` for an already-running worker.
    ///
    /// The source keeps one `mirror_buffer_t` inside `raop_rtp_mirror_t`, so
    /// `raop_rtp_mirror_init_aes` re-keys the buffer the live thread is using.
    /// The Rust worker owns its processor privately, so the request is handed
    /// over here and applied by the worker before its next packet.
    pending_stream_id: Arc<Mutex<Option<u64>>>,
}

impl MirrorReceiver {
    pub fn new(
        remote: &str,
        remote_address_len: usize,
        audio_key: &[u8],
        clock: Arc<Mutex<Box<dyn NtpClock + Send>>>,
        callbacks: Arc<dyn MirrorRuntimeCallbacks>,
    ) -> io::Result<Self> {
        if !matches!(remote_address_len, 4 | 16) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote address length must be 4 or 16",
            ));
        }
        let remote: IpAddr = remote.parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "remote address is not numeric")
        })?;
        if remote.is_ipv4() != (remote_address_len == 4) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote address family does not match its source length",
            ));
        }
        if audio_key.len() != 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audio AES key must be 16 bytes",
            ));
        }
        Ok(Self {
            remote,
            audio_key: audio_key.to_vec(),
            stream_connection_id: None,
            clock,
            callbacks,
            listener_binder: Arc::new(bind_listener),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            port: None,
            pending_stream_id: Arc::new(Mutex::new(None)),
        })
    }

    /// Replace listener creation before start so the app can apply its
    /// platform-specific socket options without moving listener ownership.
    pub fn set_listener_binder(&mut self, binder: TcpListenerBinder) {
        self.listener_binder = binder;
    }

    /// Re-key the mirror AES-CTR stream, as `raop_rtp_mirror_init_aes` does.
    ///
    /// The source re-keys the shared buffer immediately, so a repeated SETUP
    /// changes the key/IV used by the already-running worker. Publish the request
    /// for that worker as well as recording it for a future start.
    pub fn init_aes(&mut self, stream_connection_id: u64) {
        self.stream_connection_id = Some(stream_connection_id);
        let mut pending = match self.pending_stream_id.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        *pending = Some(stream_connection_id);
    }

    /// Start the IPv4 or IPv6 listener chosen by the remote client's address family.
    /// A repeated start while the current worker remains unjoined keeps the source port.
    pub fn start(&mut self, port: u16, show_client_fps: bool) -> io::Result<u16> {
        self.callbacks
            .log(MirrorLogLevel::Info, "raop_rtp_mirror starting mirroring");
        if self.worker.is_some() {
            return Ok(self.port.unwrap_or(port));
        }
        let listener = match (self.listener_binder)(self.remote, port) {
            Ok(listener) => listener,
            Err(error) => {
                self.callbacks.log(
                    MirrorLogLevel::Error,
                    "raop_rtp_mirror initializing socket failed",
                );
                return Err(error);
            }
        };
        let local_port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::clone(&self.stop);
        stop.store(false, Ordering::Release);
        let clock = Arc::clone(&self.clock);
        let callbacks = Arc::clone(&self.callbacks);
        let worker_callbacks = Arc::clone(&callbacks);
        let audio_key = self.audio_key.clone();
        let stream_id = self.stream_connection_id;
        // The spawn-time `init_aes` below already applies this value, so the
        // worker must not apply it a second time and restart the CTR stream.
        {
            let mut pending = match self.pending_stream_id.lock() {
                Ok(pending) => pending,
                Err(poisoned) => poisoned.into_inner(),
            };
            *pending = None;
        }
        let pending_stream_id = Arc::clone(&self.pending_stream_id);
        callbacks.mirror_video_running(true);
        let worker = match thread::Builder::new()
            .name("uxplay-raop-mirror".to_owned())
            .spawn(move || {
                let mut processor = match MirrorPacketProcessor::new(&audio_key) {
                    Ok(processor) => processor,
                    Err(_) => {
                        worker_callbacks.log(
                            MirrorLogLevel::Error,
                            "raop_rtp_mirror failed to initialize packet processor",
                        );
                        worker_callbacks.mirror_video_running(false);
                        return;
                    }
                };
                if let Some(stream_id) = stream_id
                    && processor.init_aes(stream_id).is_err()
                {
                    worker_callbacks.log(
                        MirrorLogLevel::Error,
                        "raop_rtp_mirror failed to initialize stream encryption",
                    );
                    worker_callbacks.mirror_video_running(false);
                    return;
                }
                let mut live = LiveProcessor {
                    processor,
                    pending_stream_id,
                };
                let result = serve(
                    listener,
                    &stop,
                    &clock,
                    worker_callbacks.as_ref(),
                    &mut live,
                    show_client_fps,
                );
                worker_callbacks.mirror_video_running(false);
                if result.connection_reset {
                    worker_callbacks.connection_reset(1);
                }
                if result.unsupported_codec {
                    worker_callbacks.video_reset();
                }
                worker_callbacks.log(MirrorLogLevel::Debug, "raop_rtp_mirror exiting TCP thread");
            }) {
            Ok(worker) => worker,
            Err(error) => {
                callbacks.log(
                    MirrorLogLevel::Error,
                    &format!("raop_rtp_mirror failed to start TCP thread: {error}"),
                );
                callbacks.mirror_video_running(false);
                return Err(error);
            }
        };
        self.port = Some(local_port);
        self.worker = Some(worker);
        Ok(local_port)
    }

    pub fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn stop(&mut self) -> io::Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        self.stop.store(true, Ordering::Release);
        worker
            .join()
            .map_err(|_| io::Error::other("RAOP mirror worker panicked"))?;
        self.port = None;
        Ok(())
    }
}

impl Drop for MirrorReceiver {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn bind_listener(remote: IpAddr, port: u16) -> io::Result<TcpListener> {
    let address = SocketAddr::new(
        if remote.is_ipv4() {
            IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
        },
        port,
    );
    let domain = if remote.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if remote.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.bind(&address.into())?;
    socket.listen(1)?;
    Ok(socket.into())
}

#[derive(Default)]
struct WorkerResult {
    connection_reset: bool,
    unsupported_codec: bool,
}

fn serve(
    listener: TcpListener,
    stop: &AtomicBool,
    clock: &Mutex<Box<dyn NtpClock + Send>>,
    callbacks: &dyn MirrorRuntimeCallbacks,
    live: &mut LiveProcessor,
    show_client_fps: bool,
) -> WorkerResult {
    let mut result = WorkerResult::default();
    while !stop.load(Ordering::Acquire) {
        if !live.apply_pending_rekey(callbacks) {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _peer)) => {
                callbacks.log(MirrorLogLevel::Debug, "raop_rtp_mirror accepting client");
                if !configure_stream(&stream, callbacks) {
                    break;
                }
                match serve_client(
                    &mut stream,
                    stop,
                    clock,
                    callbacks,
                    live,
                    show_client_fps,
                    &mut result.connection_reset,
                ) {
                    ClientResult::Disconnected => continue,
                    ClientResult::Ended { connection_reset } => {
                        result.connection_reset |= connection_reset;
                        break;
                    }
                    ClientResult::UnsupportedCodec => {
                        result.unsupported_codec = true;
                        break;
                    }
                    ClientResult::Stopped => break,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                callbacks.log(
                    MirrorLogLevel::Error,
                    &format!("raop_rtp_mirror error in accept: {}", socket_error(&error)),
                );
                break;
            }
        }
    }
    if stop.load(Ordering::Acquire) {
        callbacks.log(
            MirrorLogLevel::Info,
            "raop_rtp_mirror->running is no longer true",
        );
    }
    result
}

fn configure_stream(stream: &TcpStream, callbacks: &dyn MirrorRuntimeCallbacks) -> bool {
    if let Err(error) = stream.set_read_timeout(Some(SOURCE_READ_TIMEOUT)) {
        callbacks.log(
            MirrorLogLevel::Error,
            &format!(
                "raop_rtp_mirror could not set stream socket timeout {}",
                socket_error(&error)
            ),
        );
        return false;
    }
    let socket = match stream.try_clone() {
        Ok(clone) => Socket::from(clone),
        Err(error) => {
            callbacks.log(
                MirrorLogLevel::Warning,
                &format!(
                    "raop_rtp_mirror could not access stream socket for keepalive {}",
                    socket_error(&error)
                ),
            );
            return true;
        }
    };
    if let Err(error) = socket.set_keepalive(true) {
        callbacks.log(
            MirrorLogLevel::Warning,
            &format!(
                "raop_rtp_mirror could not set stream socket keepalive {}",
                socket_error(&error)
            ),
        );
    }
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(60));
    if let Err(error) = socket.set_tcp_keepalive(&keepalive) {
        callbacks.log(
            MirrorLogLevel::Warning,
            &format!(
                "raop_rtp_mirror could not set stream socket keepalive time {}",
                socket_error(&error)
            ),
        );
    }
    true
}

fn socket_error(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{code} {error}"),
        None => error.to_string(),
    }
}

/// The worker's private packet processor plus the re-key slot shared with
/// `MirrorReceiver`. Keeping them together mirrors the source, where the live
/// thread and `raop_rtp_mirror_init_aes` operate on the same `mirror_buffer_t`.
struct LiveProcessor {
    processor: MirrorPacketProcessor,
    pending_stream_id: Arc<Mutex<Option<u64>>>,
}

impl LiveProcessor {
    /// Apply a re-key published by `MirrorReceiver::init_aes`.
    ///
    /// The source's `mirror_buffer_init_aes` restarts the CTR stream and drops
    /// bytes held back from the previous stream; `MirrorPacketProcessor::init_aes`
    /// does the same through `MirrorBuffer`. Only the cipher is reset: C keeps the
    /// worker's codec, parameter-set and first-packet locals across a re-key.
    ///
    /// Returns `false` when the new stream cannot be initialized.
    fn apply_pending_rekey(&mut self, callbacks: &dyn MirrorRuntimeCallbacks) -> bool {
        let stream_id = {
            let mut pending = match self.pending_stream_id.lock() {
                Ok(pending) => pending,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending.take()
        };
        let Some(stream_id) = stream_id else {
            return true;
        };
        if self.processor.init_aes(stream_id).is_err() {
            callbacks.log(
                MirrorLogLevel::Error,
                "raop_rtp_mirror failed to initialize stream encryption",
            );
            return false;
        }
        true
    }
}

enum ClientResult {
    Disconnected,
    Ended { connection_reset: bool },
    UnsupportedCodec,
    Stopped,
}

fn serve_client(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    clock: &Mutex<Box<dyn NtpClock + Send>>,
    callbacks: &dyn MirrorRuntimeCallbacks,
    live: &mut LiveProcessor,
    show_client_fps: bool,
    connection_reset: &mut bool,
) -> ClientResult {
    loop {
        if !live.apply_pending_rekey(callbacks) {
            return ClientResult::Stopped;
        }
        let mut header = [0; PACKET_HEADER_LEN];
        match read_exact_until_stop(stream, &mut header, stop) {
            Ok(()) => {}
            Err(ReadFailure::Disconnected { read }) => {
                callbacks.log(
                    MirrorLogLevel::Debug,
                    &format!(
                        "raop_rtp_mirror tcp socket was closed by client (recv returned 0); got {read} bytes of 128 byte header"
                    ),
                );
                return ClientResult::Disconnected;
            }
            Err(ReadFailure::Stopped) => return ClientResult::Stopped,
            Err(ReadFailure::Fatal { was_reset, error }) => {
                callbacks.log(
                    MirrorLogLevel::Error,
                    &format!(
                        "raop_rtp_mirror error in header recv: {}",
                        socket_error(&error)
                    ),
                );
                *connection_reset |= was_reset;
                return ClientResult::Ended {
                    connection_reset: was_reset,
                };
            }
        }
        let declared_i32 = i32::from_le_bytes(header[..4].try_into().unwrap());
        let Ok(declared) = usize::try_from(declared_i32) else {
            callbacks.log(
                MirrorLogLevel::Error,
                &format!("raop_rtp_mirror: invalid negative payload length {declared_i32}"),
            );
            return ClientResult::Ended {
                connection_reset: false,
            };
        };
        if declared > MAX_PACKET_PAYLOAD {
            callbacks.log(
                MirrorLogLevel::Error,
                &format!("raop_rtp_mirror: payload length {declared} exceeds safety limit"),
            );
            return ClientResult::Ended {
                connection_reset: false,
            };
        }
        let mut payload = vec![0; declared];
        match read_exact_until_stop(stream, &mut payload, stop) {
            Ok(()) => {}
            Err(ReadFailure::Disconnected { .. }) => {
                // The reference ends the listener worker on a payload EOF/error.
                callbacks.log(
                    MirrorLogLevel::Error,
                    "raop_rtp_mirror tcp socket was closed by client (recv returned 0)",
                );
                return ClientResult::Ended {
                    connection_reset: false,
                };
            }
            Err(ReadFailure::Fatal { was_reset, error }) => {
                callbacks.log(
                    MirrorLogLevel::Error,
                    &format!("raop_rtp_mirror error in recv: {}", socket_error(&error)),
                );
                *connection_reset |= was_reset;
                return ClientResult::Ended {
                    connection_reset: was_reset,
                };
            }
            Err(ReadFailure::Stopped) => return ClientResult::Stopped,
        }
        log_packet_summary(callbacks, &header, declared);
        if header[4] == 0x05 && payload.len() > crate::FPS_TRAILER_BYTES {
            let tail = &payload[payload.len() - crate::FPS_TRAILER_BYTES..];
            let first_bytes = tail
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            callbacks.log(
                MirrorLogLevel::Debug,
                &format!("video_info packet had 25kB trailer; first 16 bytes are:\n{first_bytes}"),
            );
        }
        // A SETUP may publish a re-key while this packet's header/payload was
        // being read. Apply it before decryption so the packet uses the current
        // stream, as the source's shared buffer would.
        if !live.apply_pending_rekey(callbacks) {
            return ClientResult::Stopped;
        }
        let processed = {
            let mut clock = match clock.lock() {
                Ok(clock) => clock,
                Err(poisoned) => poisoned.into_inner(),
            };
            live.processor
                .process_packet(&header, &mut payload, clock.as_mut(), show_client_fps)
                .map(|events| (events, live.processor.take_diagnostics()))
        };
        let (events, diagnostics) = match processed {
            Ok(result) => result,
            Err(error) => {
                callbacks.log(
                    MirrorLogLevel::Error,
                    &format!("raop_rtp_mirror failed to process packet: {error:?}"),
                );
                continue;
            }
        };
        for diagnostic in diagnostics {
            match diagnostic {
                MirrorDiagnostic::DiscardedParameterSetsTimestampMismatch {
                    video_timestamp,
                    parameter_set_timestamp,
                } => callbacks.log(
                    MirrorLogLevel::Debug,
                    &format!(
                        "raop_rtp_mirror: prepended sps_pps timestamp does not match timestamp of video payload\n{video_timestamp}\n{parameter_set_timestamp} , discarding"
                    ),
                ),
            }
        }
        let mut unsupported_codec = false;
        for event in events {
            if let MirrorEvent::VideoFrame(frame) = &event {
                let local_now_ns = {
                    let clock = match clock.lock() {
                        Ok(clock) => clock,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    clock.local_time_nanoseconds()
                };
                let latency_ns = local_now_ns as i128 - frame.ntp_time_local as i128;
                let codec = match frame.codec {
                    crate::VideoCodec::H264 => "h264",
                    crate::VideoCodec::H265 => "h265",
                };
                callbacks.log(
                    MirrorLogLevel::DebugData,
                    &format!(
                        "raop_rtp video: now = {:.6}, ntp = {:.6}, latency = {:.6}, ts = {:.6}, {}, {}, size: {}",
                        local_now_ns as f64 / 1_000_000_000.0,
                        frame.ntp_time_local as f64 / 1_000_000_000.0,
                        latency_ns as f64 / 1_000_000_000.0,
                        frame.ntp_time_remote as f64 / 1_000_000_000.0,
                        packet_description(&header),
                        codec,
                        declared
                    ),
                );
            }
            match &event {
                MirrorEvent::CodecChanged { requested, .. } => {
                    let message = match requested {
                        crate::VideoCodec::H264 => {
                            "invalid codec change to H264: codec was set previously"
                        }
                        crate::VideoCodec::H265 => {
                            "invalid video codec change to H265: codec was set previously"
                        }
                    };
                    callbacks.log(MirrorLogLevel::Error, message);
                }
                MirrorEvent::MalformedParameterSets(reason) => callbacks.log(
                    MirrorLogLevel::Error,
                    &format!("raop_rtp_mirror: malformed parameter sets: {reason}"),
                ),
                _ => {}
            }
            if matches!(&event, MirrorEvent::UnsupportedCodec) {
                unsupported_codec = true;
            }
            if matches!(&event, MirrorEvent::CodecChanged { .. }) {
                // The source sets conn_reset but keeps reading this connection.
                *connection_reset = true;
                callbacks.event(event);
                continue;
            }
            if matches!(&event, MirrorEvent::CodecConfigured { .. }) {
                if !callbacks.event(event) {
                    callbacks.log(
                        MirrorLogLevel::Error,
                        &format!(
                            "failed to set video codec as {} ",
                            match live.processor.codec() {
                                Some(crate::VideoCodec::H265) => "H265",
                                _ => "H264",
                            }
                        ),
                    );
                    // The source records the failure, then continues receiving packets.
                    *connection_reset = true;
                }
            } else {
                callbacks.event(event);
            }
        }
        if unsupported_codec {
            return ClientResult::UnsupportedCodec;
        }
    }
}

enum ReadFailure {
    Disconnected { read: usize },
    Stopped,
    Fatal { was_reset: bool, error: io::Error },
}

fn packet_description(header: &[u8; PACKET_HEADER_LEN]) -> String {
    header[4..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn log_packet_summary(
    callbacks: &dyn MirrorRuntimeCallbacks,
    header: &[u8; PACKET_HEADER_LEN],
    payload_size: usize,
) {
    let description = packet_description(header);
    match header[4] {
        0x00 => {}
        0x01 => {
            callbacks.log(
                MirrorLogLevel::Debug,
                &format!(
                    "\nReceived unencrypted codec packet from client: payload_size {payload_size} header {description}"
                ),
            );
            let float = |offset| {
                f32::from_le_bytes(header[offset..offset + 4].try_into().unwrap())
            };
            let width_0 = float(16);
            let height_0 = float(20);
            let width_source = float(40);
            let height_source = float(44);
            let x_offset = float(48);
            let y_offset = float(52);
            let width = float(56);
            let height = float(60);
            if width != width_0 || height != height_0 {
                callbacks.log(
                    MirrorLogLevel::Debug,
                    &format!(
                        "raop_rtp_mirror: Unexpected : data  {width_0:.6}, {height_0:.6} != width_source = {width_source:.6}, height_source = {height_source:.6}"
                    ),
                );
            }
            callbacks.log(
                MirrorLogLevel::Debug,
                &format!(
                    "raop_rtp_mirror width_source = {width_source:.6} height_source = {height_source:.6} x_offset = {x_offset:.6} y_offset = {y_offset:.6} width = {width:.6} height = {height:.6}"
                ),
            );
            if payload_size == 0 {
                callbacks.log(
                    MirrorLogLevel::Error,
                    "raop_rtp_mirror: received type 0x01 packet with no payload:\nthis indicates non-h264 video but Airplay features bit 42 (SupportsScreenMultiCodec) is not set\nuse startup option \"-h265\" to set this bit and support h265 (4K) video",
                );
            }
            if matches!(header[6], 0x56 | 0x5e) {
                callbacks.log(
                    MirrorLogLevel::Debug,
                    "This packet indicates video stream is stopping",
                );
            }
        }
        0x02 => callbacks.log(
            MirrorLogLevel::Debug,
            &format!(
                "\nReceived old-protocol once-per-second packet from client: payload_size {payload_size} header {description}"
            ),
        ),
        0x05 => callbacks.log(
            MirrorLogLevel::Debug,
            &format!(
                "\nReceived video streaming performance info packet from client: payload_size {payload_size} header {description}"
            ),
        ),
        _ => callbacks.log(
            MirrorLogLevel::Warning,
            &format!(
                "\nReceived unexpected TCP packet from client, size {payload_size}, {description}"
            ),
        ),
    }
}

fn read_exact_until_stop(
    stream: &mut TcpStream,
    output: &mut [u8],
    stop: &AtomicBool,
) -> Result<(), ReadFailure> {
    let mut read = 0;
    while read < output.len() {
        if stop.load(Ordering::Acquire) {
            return Err(ReadFailure::Stopped);
        }
        match stream.read(&mut output[read..]) {
            Ok(0) => return Err(ReadFailure::Disconnected { read }),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => {
                return Err(ReadFailure::Fatal {
                    was_reset: error.kind() == io::ErrorKind::ConnectionReset,
                    error,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VideoCodec, VideoFrame};
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::Instant;
    use uxplay_crypto::port::{AesContext, AesMode, Direction, sha512};

    const AUDIO_KEY: [u8; 16] = *b"audio-key-fixtur";
    const STREAM_ID: u64 = 0x0123_4567_89ab_cdef;

    struct TestClock;

    impl NtpClock for TestClock {
        fn adjust_remote_timestamp_offset(&mut self, wire_ntp: u64, _include_epoch: bool) -> u64 {
            wire_ntp
        }
        fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
            timestamp
        }
        fn local_time_nanoseconds(&self) -> u64 {
            100
        }
        fn convert_remote_time(&self, remote_nanoseconds: u64) -> u64 {
            remote_nanoseconds
        }
        fn set_video_arrival_offset(&mut self, _offset_nanoseconds: u64) {}
    }

    struct Recorder {
        events: mpsc::Sender<MirrorEvent>,
        logs: Arc<Mutex<Vec<(MirrorLogLevel, String)>>>,
    }

    impl MirrorRuntimeCallbacks for Recorder {
        fn event(&self, event: MirrorEvent) -> bool {
            self.events.send(event).is_ok()
        }

        fn log(&self, level: MirrorLogLevel, message: &str) {
            self.logs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((level, message.to_owned()));
        }
    }

    fn encrypt(plain: &[u8]) -> Vec<u8> {
        encrypt_for_stream(plain, STREAM_ID)
    }

    fn encrypt_for_stream(plain: &[u8], stream_id: u64) -> Vec<u8> {
        let mut key_hash = sha512().unwrap();
        key_hash
            .update(format!("AirPlayStreamKey{stream_id}").as_bytes())
            .unwrap();
        key_hash.update(&AUDIO_KEY).unwrap();
        let key = key_hash.finish().unwrap();
        let mut iv_hash = sha512().unwrap();
        iv_hash
            .update(format!("AirPlayStreamIV{stream_id}").as_bytes())
            .unwrap();
        iv_hash.update(&AUDIO_KEY).unwrap();
        let iv = iv_hash.finish().unwrap();
        AesContext::new(AesMode::Ctr, Direction::Encrypt, &key[..16], &iv[..16])
            .unwrap()
            .ctr_encrypt(plain)
            .unwrap()
    }

    fn wire_packet(kind: u8, timestamp: u64, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0; PACKET_HEADER_LEN + payload.len()];
        packet[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        packet[4] = kind;
        packet[6] = 0x16;
        packet[8..16].copy_from_slice(&timestamp.to_le_bytes());
        for (offset, value) in [(40, 1920.0f32), (44, 1080.0), (56, 1920.0), (60, 1080.0)] {
            packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        packet[PACKET_HEADER_LEN..].copy_from_slice(payload);
        packet
    }

    fn send_fragmented(stream: &mut TcpStream, bytes: &[u8]) {
        for chunk in bytes.chunks(11) {
            stream.write_all(chunk).unwrap();
        }
    }

    #[test]
    fn listener_accepts_fragmented_codec_and_video_packets_then_stops() {
        let (tx, rx) = mpsc::channel();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let callbacks = Arc::new(Recorder {
            events: tx,
            logs: Arc::clone(&logs),
        });
        let clock: Arc<Mutex<Box<dyn NtpClock + Send>>> = Arc::new(Mutex::new(Box::new(TestClock)));
        let mut receiver =
            MirrorReceiver::new("127.0.0.1", 4, &AUDIO_KEY, clock, callbacks).unwrap();
        receiver.init_aes(STREAM_ID);
        let port = receiver.start(0, false).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let sps = [0x67, 0x42, 0x00, 0x1e];
        let pps = [0x68, 0xce, 0x06, 0xe2];
        let mut codec = vec![0; 6];
        codec.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&sps);
        codec.push(1);
        codec.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&pps);
        send_fragmented(&mut stream, &wire_packet(1, 42, &codec));
        let cipher = encrypt(&[0, 0, 0, 2, 0x65, 0xaa]);
        send_fragmented(&mut stream, &wire_packet(0, 42, &cipher));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen = Vec::new();
        while seen.len() < 2 && Instant::now() < deadline {
            if let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
                seen.push(event);
            }
        }
        receiver.stop().unwrap();
        assert!(matches!(
            seen.first(),
            Some(MirrorEvent::CodecConfigured {
                codec: VideoCodec::H264,
                ..
            })
        ));
        assert!(
            matches!(seen.get(1), Some(MirrorEvent::VideoFrame(VideoFrame { codec: VideoCodec::H264, nal_count: 3, data, .. })) if data.len() == 22)
        );
        assert!(!receiver.is_running());
        let logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(logs.contains(&(
            MirrorLogLevel::Info,
            "raop_rtp_mirror starting mirroring".to_owned()
        )));
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::Debug && message == "raop_rtp_mirror accepting client"
        }));
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::DebugData && message.contains("raop_rtp video:")
        }));
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::Debug && message == "raop_rtp_mirror exiting TCP thread"
        }));
    }

    #[test]
    fn listener_logs_heartbeat_and_partial_header_eof() {
        let (tx, _rx) = mpsc::channel();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let callbacks = Arc::new(Recorder {
            events: tx,
            logs: Arc::clone(&logs),
        });
        let clock: Arc<Mutex<Box<dyn NtpClock + Send>>> = Arc::new(Mutex::new(Box::new(TestClock)));
        let mut receiver =
            MirrorReceiver::new("127.0.0.1", 4, &AUDIO_KEY, clock, callbacks).unwrap();
        let port = receiver.start(0, false).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(&wire_packet(2, 42, &[])).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let seen_eof = logs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|(_, message)| message.contains("got 0 bytes of 128 byte header"));
            if seen_eof {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        receiver.stop().unwrap();
        let logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::Debug
                && message.contains("Received old-protocol once-per-second packet")
                && message.contains("payload_size 0")
        }));
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::Debug && message.contains("got 0 bytes of 128 byte header")
        }));
        assert!(logs.iter().any(|(level, message)| {
            *level == MirrorLogLevel::Info
                && message == "raop_rtp_mirror->running is no longer true"
        }));
    }

    #[test]
    fn listener_logs_discarded_parameter_sets_when_frame_timestamp_changes() {
        let (tx, rx) = mpsc::channel();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let callbacks = Arc::new(Recorder {
            events: tx,
            logs: Arc::clone(&logs),
        });
        let clock: Arc<Mutex<Box<dyn NtpClock + Send>>> = Arc::new(Mutex::new(Box::new(TestClock)));
        let mut receiver =
            MirrorReceiver::new("127.0.0.1", 4, &AUDIO_KEY, clock, callbacks).unwrap();
        receiver.init_aes(STREAM_ID);
        let port = receiver.start(0, false).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let sps = [0x67, 0x42, 0x00, 0x1e];
        let pps = [0x68, 0xce, 0x06, 0xe2];
        let mut codec = vec![0; 6];
        codec.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&sps);
        codec.push(1);
        codec.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&pps);
        send_fragmented(&mut stream, &wire_packet(1, 42, &codec));
        let cipher = encrypt(&[0, 0, 0, 2, 0x65, 0xaa]);
        send_fragmented(&mut stream, &wire_packet(0, 43, &cipher));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen = Vec::new();
        while seen.len() < 2 && Instant::now() < deadline {
            if let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
                seen.push(event);
            }
        }
        receiver.stop().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(
            logs.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|(level, message)| {
                    *level == MirrorLogLevel::Debug
                        && message.contains("prepended sps_pps timestamp does not match")
                        && message.contains("\n43\n42 , discarding")
                })
        );
    }

    #[test]
    fn configured_listener_binder_is_used_before_mirror_worker_starts() {
        let (tx, _rx) = mpsc::channel();
        let callbacks = Arc::new(Recorder {
            events: tx,
            logs: Arc::new(Mutex::new(Vec::new())),
        });
        let clock: Arc<Mutex<Box<dyn NtpClock + Send>>> = Arc::new(Mutex::new(Box::new(TestClock)));
        let mut receiver =
            MirrorReceiver::new("127.0.0.1", 4, &AUDIO_KEY, clock, callbacks).unwrap();
        let bind_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bind_count_for_factory = Arc::clone(&bind_count);
        receiver.set_listener_binder(Arc::new(move |remote, port| {
            bind_count_for_factory.fetch_add(1, Ordering::AcqRel);
            bind_listener(remote, port)
        }));

        receiver.start(0, false).unwrap();
        receiver.stop().unwrap();
        assert_eq!(bind_count.load(Ordering::Acquire), 1);
    }

    fn next_event(rx: &mpsc::Receiver<MirrorEvent>) -> MirrorEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
                return event;
            }
        }
        panic!("mirror worker produced no event before the deadline");
    }

    /// A repeated SETUP must re-key the already-running worker, matching
    /// `raop_rtp_mirror_init_aes` re-keying the source's shared `mirror_buffer_t`.
    #[test]
    fn repeated_setup_rekeys_the_running_mirror_worker() {
        const NEXT_STREAM_ID: u64 = 0x0fed_cba9_8765_4321;
        let (tx, rx) = mpsc::channel();
        let callbacks = Arc::new(Recorder {
            events: tx,
            logs: Arc::new(Mutex::new(Vec::new())),
        });
        let clock: Arc<Mutex<Box<dyn NtpClock + Send>>> = Arc::new(Mutex::new(Box::new(TestClock)));
        let mut receiver =
            MirrorReceiver::new("127.0.0.1", 4, &AUDIO_KEY, clock, callbacks).unwrap();
        receiver.init_aes(STREAM_ID);
        let port = receiver.start(0, false).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();

        let sps = [0x67, 0x42, 0x00, 0x1e];
        let pps = [0x68, 0xce, 0x06, 0xe2];
        let mut codec = vec![0; 6];
        codec.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&sps);
        codec.push(1);
        codec.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        codec.extend_from_slice(&pps);
        send_fragmented(&mut stream, &wire_packet(1, 42, &codec));
        send_fragmented(
            &mut stream,
            &wire_packet(0, 42, &encrypt(&[0, 0, 0, 2, 0x65, 0xaa])),
        );

        assert!(matches!(
            next_event(&rx),
            MirrorEvent::CodecConfigured {
                codec: VideoCodec::H264,
                ..
            }
        ));
        assert!(
            matches!(next_event(&rx), MirrorEvent::VideoFrame(frame) if frame.data.ends_with(&[0x65, 0xaa]))
        );

        // Re-key without stopping the worker, exactly as a second SETUP does.
        receiver.init_aes(NEXT_STREAM_ID);
        assert!(receiver.is_running());
        send_fragmented(
            &mut stream,
            &wire_packet(
                0,
                42,
                &encrypt_for_stream(&[0, 0, 0, 2, 0x65, 0xbb], NEXT_STREAM_ID),
            ),
        );
        let rekeyed = next_event(&rx);
        assert!(
            matches!(&rekeyed, MirrorEvent::VideoFrame(frame) if frame.data == [0, 0, 0, 1, 0x65, 0xbb]),
            "running worker must decrypt with the new stream: {rekeyed:?}"
        );

        // The previous stream must no longer decrypt. The source signals a
        // failed payload with a leading 0x01 byte.
        send_fragmented(
            &mut stream,
            &wire_packet(0, 42, &encrypt(&[0, 0, 0, 2, 0x65, 0xcc])),
        );
        match next_event(&rx) {
            MirrorEvent::VideoFrame(frame) => assert_eq!(frame.data.first(), Some(&1)),
            MirrorEvent::InvalidVideoPayload => {}
            other => panic!("stale stream must not decode: {other:?}"),
        }

        receiver.stop().unwrap();
    }
}
