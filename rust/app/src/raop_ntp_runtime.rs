//! RAOP NTP UDP session lifecycle and timing thread from `lib/raop_ntp.c`.
//!
//! The socket remains bound to the negotiated local port for a session. The
//! worker sends one 32-byte timing request, waits up to 300 ms for a response,
//! and then waits 250 ms during its initial burst or 3 seconds after eight
//! successful requests. NTP math and the eight-sample filter live in the
//! adjacent `ntp_math` and `ntp_filter` modules.
//!
//! Unix builds enable `SO_TIMESTAMP`; Windows enables the source's optional
//! `SIO_TIMESTAMPING`/NetAdapter `RxAll` extension and both consume ancillary
//! receive timestamps when the local socket and adapter provide them.

use crate::logger::{DEBUG, ERR, INFO};
use crate::netutils::{AddressFamily, Transport, bind_socket, parse_numeric_address};
use crate::ntp_filter::{NTP_SAMPLE_COUNT, NtpSample, estimate};
use crate::ntp_math::{
    NtpClock, TimeProtocol, adjust_remote_timestamp_offset, fixed_offset_from_first_timestamp,
    timestamp_to_nanoseconds, unix_nanoseconds_to_ntp_q32_32,
};
use std::io;
#[cfg(windows)]
use std::mem::{size_of, size_of_val};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TIMING_RESPONSE_TIMEOUT: Duration = Duration::from_millis(300);
pub const INITIAL_REQUEST_INTERVAL: Duration = Duration::from_millis(250);
pub const STEADY_REQUEST_INTERVAL: Duration = Duration::from_secs(3);
pub const INITIAL_BURST_REQUESTS: usize = NTP_SAMPLE_COUNT;
const RESPONSE_BUFFER_SIZE: usize = 128;
const REQUEST_SIZE: usize = 32;
const REQUEST_HEADER: [u8; 4] = [0x80, 0xd2, 0x00, 0x07];
const NTP_KERNEL_TIMESTAMP_COOLDOWN_NS: u64 = 12_000_000_000;

/// Callback compatible with the app logger's `(level, message)` shape.
pub type NtpLogCallback = Arc<dyn Fn(i32, &str) + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NtpSyncSnapshot {
    pub clock: NtpClock,
    pub delay_seconds: f64,
    pub root_distance_nanoseconds: u64,
    pub is_synced: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ExchangeSlot {
    sent_timestamp: u64,
    received_client_timestamp: u64,
}

#[derive(Clone)]
struct WorkerMemory {
    exchanges: [ExchangeSlot; NTP_SAMPLE_COUNT],
    samples: [NtpSample; NTP_SAMPLE_COUNT],
}

impl Default for WorkerMemory {
    fn default() -> Self {
        Self {
            exchanges: [ExchangeSlot::default(); NTP_SAMPLE_COUNT],
            samples: [NtpSample::empty(); NTP_SAMPLE_COUNT],
        }
    }
}

#[derive(Default)]
struct SharedState {
    clock: NtpClock,
    delay_seconds: f64,
    root_distance_nanoseconds: u64,
    is_synced: bool,
    video_arrival_offset: u64,
    fixed_remote_offset: Option<i64>,
    kernel_timestamp_inconsistency_detected: bool,
    last_timestamp_inconsistency_ns: u64,
}

#[derive(Default)]
struct RunState {
    running: bool,
}

struct WorkerControl {
    run: Mutex<RunState>,
    wake: Condvar,
}

/// One negotiated RAOP timing exchange session.
///
/// `remote_address_length` must be 4 for IPv4 or 16 for IPv6, matching the
/// source API. The remote string must be a numeric literal; host lookup is not
/// performed. `start` binds the local UDP socket lazily, as the C API does.
pub struct RaopNtpRuntime {
    remote: SocketAddr,
    timing_rport: u16,
    timing_lport: u16,
    time_protocol: TimeProtocol,
    shared: Arc<Mutex<SharedState>>,
    memory: Arc<Mutex<WorkerMemory>>,
    control: Arc<WorkerControl>,
    worker: Option<JoinHandle<()>>,
    joined: bool,
    logger: Option<NtpLogCallback>,
}

impl RaopNtpRuntime {
    pub fn new(
        logger: Option<NtpLogCallback>,
        remote_address: &str,
        remote_address_length: usize,
        timing_rport: u16,
        time_protocol: TimeProtocol,
    ) -> io::Result<Self> {
        log(
            &logger,
            DEBUG,
            format!("raop_ntp parse remote ip = {remote_address}"),
        );
        let family = match remote_address_length {
            4 => AddressFamily::Ipv4,
            16 => AddressFamily::Ipv6,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "remote address length must be 4 or 16",
                ));
            }
        };
        let mut remote = parse_numeric_address(family, remote_address)?;
        remote.set_port(timing_rport);
        Ok(Self {
            remote,
            timing_rport,
            timing_lport: 0,
            time_protocol,
            shared: Arc::new(Mutex::new(SharedState::default())),
            memory: Arc::new(Mutex::new(WorkerMemory::default())),
            control: Arc::new(WorkerControl {
                run: Mutex::new(RunState::default()),
                wake: Condvar::new(),
            }),
            worker: None,
            joined: true,
            logger,
        })
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.remote
    }

    pub fn remote_port(&self) -> u16 {
        self.timing_rport
    }

    pub fn local_port(&self) -> u16 {
        self.timing_lport
    }

    pub fn is_running(&self) -> bool {
        lock_unpoisoned(&self.control.run).running
    }

    pub fn sync_snapshot(&self) -> NtpSyncSnapshot {
        let state = lock_unpoisoned(&self.shared);
        NtpSyncSnapshot {
            clock: state.clock,
            delay_seconds: state.delay_seconds,
            root_distance_nanoseconds: state.root_distance_nanoseconds,
            is_synced: state.is_synced,
        }
    }

    pub fn get_sync_params(&self) -> (i64, f64) {
        let state = lock_unpoisoned(&self.shared);
        (state.clock.sync_offset_nanoseconds, state.delay_seconds)
    }

    pub fn set_video_arrival_offset(&self, offset: u64) {
        lock_unpoisoned(&self.shared).video_arrival_offset = offset;
    }

    pub fn video_arrival_offset(&self) -> u64 {
        lock_unpoisoned(&self.shared).video_arrival_offset
    }

    pub fn local_time_ns() -> u64 {
        unix_time_ns()
    }

    pub fn remote_time_ns(&self) -> u64 {
        self.sync_snapshot().clock.remote_time(unix_time_ns())
    }

    pub fn convert_remote_time_ns(&self, remote_time_ns: u64) -> u64 {
        self.sync_snapshot()
            .clock
            .convert_remote_time(remote_time_ns)
    }

    pub fn convert_local_time_ns(&self, local_time_ns: u64) -> u64 {
        self.sync_snapshot().clock.convert_local_time(local_time_ns)
    }

    pub fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
        timestamp_to_nanoseconds(timestamp, self.time_protocol)
    }

    /// Normalize a client Q32.32 timestamp to the server's wall-clock epoch.
    /// The first call captures the source's fixed offset and later calls reuse it.
    pub fn adjust_remote_timestamp_offset(&self, timestamp: u64, add_epoch_seconds: bool) -> u64 {
        let mut state = lock_unpoisoned(&self.shared);
        let (fixed_offset, newly_set) = match state.fixed_remote_offset {
            Some(offset) => (offset, false),
            None => {
                let value =
                    fixed_offset_from_first_timestamp(unix_time_ns(), timestamp, add_epoch_seconds);
                state.fixed_remote_offset = Some(value);
                (value, true)
            }
        };
        drop(state);
        if newly_set {
            log(
                &self.logger,
                DEBUG,
                format!("set fixed client NTP offset: {fixed_offset:21}"),
            );
        }
        adjust_remote_timestamp_offset(timestamp, fixed_offset, add_epoch_seconds)
    }

    /// Start the timing worker. If `*timing_lport` is zero, the bound ephemeral
    /// port is written back. A socket setup failure leaves the port unchanged
    /// and leaves the session retryable.
    pub fn start(&mut self, timing_lport: &mut u16) -> io::Result<()> {
        self.start_with_peer_to_peer(timing_lport, false)
    }

    /// Start the timing worker with the source's Apple peer-to-peer receive option.
    pub fn start_with_peer_to_peer(
        &mut self,
        timing_lport: &mut u16,
        peer_to_peer: bool,
    ) -> io::Result<()> {
        self.start_with_socket(timing_lport, |local_port, family| {
            bind_socket(local_port, family, Transport::Udp, peer_to_peer).map(Into::into)
        })
    }

    fn start_with_socket(
        &mut self,
        timing_lport: &mut u16,
        mut bind: impl FnMut(&mut u16, AddressFamily) -> io::Result<UdpSocket>,
    ) -> io::Result<()> {
        log(&self.logger, DEBUG, "raop_ntp starting time".to_owned());
        self.timing_lport = *timing_lport;
        {
            let run = lock_unpoisoned(&self.control.run);
            if run.running || !self.joined {
                return Ok(());
            }
        }

        let family = if self.remote.is_ipv4() {
            AddressFamily::Ipv4
        } else {
            AddressFamily::Ipv6
        };
        let mut local_port = self.timing_lport;
        let timestamp_logger = self.logger.clone();
        let socket = match bind(&mut local_port, family).and_then(|socket| {
            socket.set_read_timeout(Some(TIMING_RESPONSE_TIMEOUT))?;
            let timestamp_session = enable_kernel_timestamps(&socket, &timestamp_logger);
            Ok((socket, timestamp_session))
        }) {
            Ok(socket) => socket,
            Err(error) => {
                log(
                    &self.logger,
                    ERR,
                    "raop_ntp initializing timing socket failed".to_owned(),
                );
                return Err(error);
            }
        };

        let (socket, timestamp_session) = socket;
        let log_socket = socket_descriptor(&socket);
        let logger = self.logger.clone();
        let remote = self.remote;
        let shared = Arc::clone(&self.shared);
        let memory = Arc::clone(&self.memory);
        let control = Arc::clone(&self.control);
        {
            lock_unpoisoned(&self.control.run).running = true;
        }

        let worker = thread::Builder::new()
            .name("uxplay-raop-ntp".to_owned())
            .spawn(move || {
                run_worker(
                    socket,
                    remote,
                    shared,
                    memory,
                    control,
                    logger,
                    timestamp_session,
                );
            });
        match worker {
            Ok(worker) => {
                self.worker = Some(worker);
                self.joined = false;
                self.timing_lport = local_port;
                *timing_lport = local_port;
                log(
                    &self.logger,
                    DEBUG,
                    format!(
                        "raop_ntp local timing port socket {log_socket} port UDP {}",
                        local_port
                    ),
                );
                Ok(())
            }
            Err(error) => {
                lock_unpoisoned(&self.control.run).running = false;
                self.control.wake.notify_all();
                log(
                    &self.logger,
                    ERR,
                    "raop_ntp initializing timing socket failed".to_owned(),
                );
                Err(error)
            }
        }
    }

    /// Signal the worker, join it, and release its UDP socket. Repeated stops
    /// are harmless, matching `raop_ntp_stop`.
    pub fn stop(&mut self) {
        let running = {
            let mut run = lock_unpoisoned(&self.control.run);
            if !run.running && self.joined {
                return;
            }
            let was_running = run.running;
            run.running = false;
            was_running
        };
        if running {
            log(
                &self.logger,
                DEBUG,
                "raop_ntp stopping time thread".to_owned(),
            );
            self.control.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
            log(
                &self.logger,
                DEBUG,
                "raop_ntp stopped time thread".to_owned(),
            );
        }
        self.joined = true;
    }
}

impl Drop for RaopNtpRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TimestampHealth {
    inconsistency_detected: bool,
    last_detection_time_ns: u64,
}

fn choose_receive_time(
    send_time_ns: u64,
    local_time_ns: u64,
    kernel_time_ns: u64,
    clock_time_ns: u64,
    health: &mut TimestampHealth,
) -> (u64, bool) {
    if kernel_time_ns == 0 && clock_time_ns == 0 {
        return (local_time_ns, false);
    }
    if kernel_time_ns == 0 {
        return (clock_time_ns, false);
    }
    let sane = signed_ns_difference(kernel_time_ns, send_time_ns) > 0
        && signed_ns_difference(clock_time_ns, kernel_time_ns) > 0;
    if !sane {
        health.inconsistency_detected = true;
        health.last_detection_time_ns = local_time_ns;
        return (clock_time_ns, false);
    }
    let inconsistency_expired = signed_ns_difference(local_time_ns, health.last_detection_time_ns)
        > NTP_KERNEL_TIMESTAMP_COOLDOWN_NS as i64;
    if !health.inconsistency_detected || inconsistency_expired {
        (kernel_time_ns, true)
    } else {
        (clock_time_ns, false)
    }
}

fn run_worker(
    socket: UdpSocket,
    remote: SocketAddr,
    shared: Arc<Mutex<SharedState>>,
    memory: Arc<Mutex<WorkerMemory>>,
    control: Arc<WorkerControl>,
    logger: Option<NtpLogCallback>,
    mut timestamp_session: TimestampSession,
) {
    let mut worker_memory = lock_unpoisoned(&memory).clone();
    let mut data_index = 0;
    let mut successful_request_count = 0usize;
    let mut have_offset = false;
    let mut request = initial_ntp_request();
    loop {
        if !is_running(&control) {
            break;
        }
        flush_socket(&socket);

        let send_time_ns = unix_time_ns();
        request[24..32]
            .copy_from_slice(&unix_nanoseconds_to_ntp_q32_32(send_time_ns).to_be_bytes());
        if let Err(error) = socket.send_to(&request, remote) {
            let code = error.raw_os_error().unwrap_or(0);
            log(
                &logger,
                ERR,
                format!(
                    "raop_ntp error sending request. Error {code}:{}",
                    crate::compat::socket_error_message(code)
                ),
            );
            wait_before_next_request(&control, successful_request_count);
            continue;
        }
        successful_request_count = successful_request_count.saturating_add(1);
        let position = data_index;
        worker_memory.exchanges[position].sent_timestamp = read_u64_be(&request, 24);
        log(
            &logger,
            DEBUG,
            format!(
                "\nraop_ntp request  type_t={} packetlen = {}, send_time = {:8.6}\n{}",
                request[1] & 0x7f,
                REQUEST_SIZE,
                send_time_ns as f64 / 1e9,
                data_to_string(&request, 16)
            ),
        );

        let mut response = [0u8; RESPONSE_BUFFER_SIZE];
        let receive_result =
            receive_timestamped(&socket, &mut timestamp_session, &mut response, &logger);
        let local_time_ns = unix_time_ns();
        let (response_length, kernel_time_ns, clock_time_ns, receive_failed) = match receive_result
        {
            Ok(received) => (
                received.length,
                received.kernel_time_ns,
                received.clock_time_ns,
                false,
            ),
            Err(_) => (0, 0, 0, true),
        };

        if response_length < 32 {
            let send_time = format_local_timestamp(send_time_ns);
            if receive_failed {
                log(
                    &logger,
                    DEBUG,
                    format!("raop_ntp received timeout (request sent {send_time})"),
                );
            } else {
                log(
                    &logger,
                    ERR,
                    format!(
                        "raop_ntp received truncated response (request sent {send_time}, response_len {response_length} < 32)"
                    ),
                );
            }
        } else {
            let t1 = read_u64_be(&response, 8);
            let t2_raw = read_u64_be(&response, 16);
            let t3_raw = read_u64_be(&response, 24);
            request[8..16].copy_from_slice(&t3_raw.to_be_bytes());
            let t2 = adjust_shared_remote_timestamp(&shared, &logger, t2_raw, false);
            let t3 = adjust_shared_remote_timestamp(&shared, &logger, t3_raw, false);
            let mut health = {
                let state = lock_unpoisoned(&shared);
                TimestampHealth {
                    inconsistency_detected: state.kernel_timestamp_inconsistency_detected,
                    last_detection_time_ns: state.last_timestamp_inconsistency_ns,
                }
            };
            let (receive_time_ns, used_kernel_time) = choose_receive_time(
                send_time_ns,
                local_time_ns,
                kernel_time_ns,
                clock_time_ns,
                &mut health,
            );
            {
                let mut state = lock_unpoisoned(&shared);
                state.kernel_timestamp_inconsistency_detected = health.inconsistency_detected;
                state.last_timestamp_inconsistency_ns = health.last_detection_time_ns;
            }
            request[16..24]
                .copy_from_slice(&unix_nanoseconds_to_ntp_q32_32(receive_time_ns).to_be_bytes());
            let t4 = read_u64_be(&request, 16);
            log(
                &logger,
                DEBUG,
                format!(
                    "raop_ntp response type_t={} packetlen = {}, recv_time = {:8.6} ({})\n{}",
                    response[1] & 0x7f,
                    response_length,
                    receive_time_ns as f64 / 1e9,
                    if used_kernel_time { "kernel" } else { "clock" },
                    data_to_string(&response[..response_length], 16)
                ),
            );
            log(
                &logger,
                DEBUG,
                format!("t1 = {t1}\nt2 = {t2} ({t2_raw})\nt3 = {t3} ({t3_raw})\nt4 = {t4}\n"),
            );

            let mut has_matching_send = false;
            for delta in 0..NTP_SAMPLE_COUNT {
                let candidate = (position + delta) % NTP_SAMPLE_COUNT;
                if worker_memory.exchanges[candidate].sent_timestamp != 0
                    && worker_memory.exchanges[candidate].sent_timestamp == t1
                {
                    worker_memory.exchanges[candidate].sent_timestamp = 0;
                    has_matching_send = true;
                    break;
                }
            }
            if !has_matching_send {
                log(
                    &logger,
                    INFO,
                    format!("raop_ntp received NTP packet with invalid server ref: {t1}"),
                );
            }

            let duplicate = worker_memory
                .exchanges
                .iter()
                .any(|exchange| exchange.received_client_timestamp == t3_raw);
            if duplicate {
                log(
                    &logger,
                    INFO,
                    format!("raop_ntp received NTP packet with duplicate t3: {t3_raw}"),
                );
            } else {
                worker_memory.exchanges[position].received_client_timestamp = t3_raw;
            }

            if has_matching_send && !duplicate {
                worker_memory.samples[position] = NtpSample::from_exchange(t1, t2, t3, t4);
                data_index = (position + 1) % NTP_SAMPLE_COUNT;
                log(
                    &logger,
                    DEBUG,
                    format!(
                        "      T4 (NTP Q32:32)      offset     delay   dispersion\n{}",
                        render_sample_table(&worker_memory.samples, position)
                    ),
                );
                let previous_offset =
                    have_offset.then(|| lock_unpoisoned(&shared).clock.sync_offset_nanoseconds);
                if let Some(estimate) = estimate(&mut worker_memory.samples, t4, previous_offset) {
                    {
                        let mut state = lock_unpoisoned(&shared);
                        state.clock.client_time_received = true;
                        state.clock.sync_offset_nanoseconds = estimate.sync_offset_nanoseconds;
                        state.is_synced = estimate.is_synced;
                        state.root_distance_nanoseconds = estimate.root_distance_nanoseconds;
                        state.delay_seconds = estimate.sync_delay_seconds;
                    }
                    have_offset = true;
                    log(
                        &logger,
                        DEBUG,
                        format!(
                            "best NTP data: offset {:10.6}, delay {:10.6}",
                            estimate.best_sample.offset_seconds, estimate.best_sample.delay_seconds
                        ),
                    );
                    log(
                        &logger,
                        DEBUG,
                        format!(
                            "current client NTP offset (secs) = {:9.6}, root_distance (secs) = {:8.6}, is_synced = {}\n",
                            estimate.sync_offset_nanoseconds as f64 / 1e9,
                            estimate.root_distance_nanoseconds as f64 / 1e9,
                            if estimate.is_synced { "TRUE" } else { "FALSE" }
                        ),
                    );
                }
            }
        }

        wait_before_next_request(&control, successful_request_count);
        successful_request_count = successful_request_count.min(INITIAL_BURST_REQUESTS);
    }
    *lock_unpoisoned(&memory) = worker_memory;
    lock_unpoisoned(&control.run).running = false;
    log(&logger, DEBUG, "raop_ntp exiting thread".to_owned());
}

#[cfg(test)]
fn build_ntp_request(send_time_ns: u64) -> [u8; REQUEST_SIZE] {
    let mut request = initial_ntp_request();
    request[24..32].copy_from_slice(&unix_nanoseconds_to_ntp_q32_32(send_time_ns).to_be_bytes());
    request
}

fn initial_ntp_request() -> [u8; REQUEST_SIZE] {
    let mut request = [0u8; REQUEST_SIZE];
    request[..REQUEST_HEADER.len()].copy_from_slice(&REQUEST_HEADER);
    request
}

fn read_u64_be(packet: &[u8], offset: usize) -> u64 {
    let mut value = 0u64;
    for byte in &packet[offset..offset + 8] {
        value = (value << 8) | u64::from(*byte);
    }
    value
}

fn data_to_string(data: &[u8], bytes_per_line: usize) -> String {
    let mut output = String::with_capacity(data.len().saturating_mul(3).saturating_add(1));
    for (index, byte) in data.iter().enumerate() {
        if index > 0 && index % bytes_per_line == 0 {
            output.push('\n');
        }
        output.push_str(&format!("{byte:02x} "));
    }
    output.push('\n');
    output
}

fn render_sample_table(samples: &[NtpSample; NTP_SAMPLE_COUNT], current: usize) -> String {
    let mut output = String::with_capacity(NTP_SAMPLE_COUNT * 90);
    for (index, sample) in samples.iter().enumerate() {
        output.push_str(&format!(
            "{index}{} {:20} {:9.6} {:9.6} {:12.9} \n",
            if index == current { '*' } else { ' ' },
            sample.time_q32_32,
            sample.offset_seconds,
            sample.delay_seconds,
            sample.dispersion_seconds
        ));
    }
    output
}

fn adjust_shared_remote_timestamp(
    shared: &Mutex<SharedState>,
    logger: &Option<NtpLogCallback>,
    timestamp: u64,
    add_epoch_seconds: bool,
) -> u64 {
    let mut state = lock_unpoisoned(shared);
    let (fixed_offset, newly_set) = match state.fixed_remote_offset {
        Some(offset) => (offset, false),
        None => {
            let value =
                fixed_offset_from_first_timestamp(unix_time_ns(), timestamp, add_epoch_seconds);
            state.fixed_remote_offset = Some(value);
            (value, true)
        }
    };
    drop(state);
    if newly_set {
        log(
            logger,
            DEBUG,
            format!("set fixed client NTP offset: {fixed_offset:21}"),
        );
    }
    adjust_remote_timestamp_offset(timestamp, fixed_offset, add_epoch_seconds)
}

fn wait_before_next_request(control: &WorkerControl, successful_request_count: usize) {
    let interval = if successful_request_count > INITIAL_BURST_REQUESTS {
        STEADY_REQUEST_INTERVAL
    } else {
        INITIAL_REQUEST_INTERVAL
    };
    let run = lock_unpoisoned(&control.run);
    if !run.running {
        return;
    }
    let (run, _) = control
        .wake
        .wait_timeout(run, interval)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drop(run);
}

fn is_running(control: &WorkerControl) -> bool {
    lock_unpoisoned(&control.run).running
}

fn flush_socket(socket: &UdpSocket) {
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    let mut discard = [0u8; 1];
    loop {
        match socket.recv_from(&mut discard) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = socket.set_nonblocking(false);
}

struct ReceivedDatagram {
    length: usize,
    kernel_time_ns: u64,
    clock_time_ns: u64,
}

#[cfg(windows)]
struct TimestampSession {
    receive_message: windows_sys::Win32::Networking::WinSock::LPFN_WSARECVMSG,
    qpc_frequency: i64,
    base_qpc_ticks: i64,
    base_system_time_us: u64,
}

#[cfg(not(windows))]
struct TimestampSession;

fn receive_timestamped(
    socket: &UdpSocket,
    session: &mut TimestampSession,
    buffer: &mut [u8],
    logger: &Option<NtpLogCallback>,
) -> io::Result<ReceivedDatagram> {
    #[cfg(unix)]
    {
        let _ = (session, logger);
        receive_timestamped_unix(socket, buffer)
    }
    #[cfg(windows)]
    {
        receive_timestamped_windows(socket, session, buffer, logger)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (session, logger);
        receive_timestamped_fallback(socket, buffer)
    }
}

#[cfg(unix)]
fn enable_kernel_timestamps(
    socket: &UdpSocket,
    _logger: &Option<NtpLogCallback>,
) -> TimestampSession {
    use std::os::fd::AsRawFd;
    let enabled: libc::c_int = 1;
    // SAFETY: the integer option value remains valid for the duration of this call.
    unsafe {
        let _ = libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TIMESTAMP,
            (&enabled as *const libc::c_int).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        );
    }
    TimestampSession
}

#[cfg(not(any(unix, windows)))]
fn enable_kernel_timestamps(
    _socket: &UdpSocket,
    _logger: &Option<NtpLogCallback>,
) -> TimestampSession {
    TimestampSession
}

#[cfg(not(any(unix, windows)))]
fn receive_timestamped_fallback(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> io::Result<ReceivedDatagram> {
    let (length, _) = socket.recv_from(buffer)?;
    Ok(ReceivedDatagram {
        length,
        kernel_time_ns: 0,
        clock_time_ns: unix_time_ns(),
    })
}

#[cfg(windows)]
fn enable_kernel_timestamps(
    socket: &UdpSocket,
    logger: &Option<NtpLogCallback>,
) -> TimestampSession {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Networking::WinSock as ws;
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

    let socket = socket.as_raw_socket() as ws::SOCKET;
    let mut frequency = 0_i64;
    let mut base_qpc_ticks = 0_i64;
    let frequency_ok = unsafe { QueryPerformanceFrequency(&mut frequency) } != 0;
    let counter_ok = unsafe { QueryPerformanceCounter(&mut base_qpc_ticks) } != 0;
    let mut file_time = FILETIME::default();
    unsafe { GetSystemTimeAsFileTime(&mut file_time) };
    let windows_ticks =
        (u64::from(file_time.dwHighDateTime) << 32) | u64::from(file_time.dwLowDateTime);
    let base_system_time_us = windows_ticks.saturating_sub(116_444_736_000_000_000) / 10;

    let mut session = TimestampSession {
        receive_message: None,
        qpc_frequency: frequency,
        base_qpc_ticks,
        base_system_time_us,
    };
    if !frequency_ok || !counter_ok || frequency <= 0 {
        return session;
    }

    let guid = ws::WSAID_WSARECVMSG;
    let mut bytes_returned = 0_u32;
    let mut receive_message: ws::LPFN_WSARECVMSG = None;
    let result = unsafe {
        ws::WSAIoctl(
            socket,
            ws::SIO_GET_EXTENSION_FUNCTION_POINTER,
            (&guid as *const windows_sys::core::GUID).cast(),
            size_of::<windows_sys::core::GUID>() as u32,
            (&mut receive_message as *mut ws::LPFN_WSARECVMSG).cast(),
            size_of::<ws::LPFN_WSARECVMSG>() as u32,
            &mut bytes_returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == -1 {
        let code = unsafe { ws::WSAGetLastError() };
        log(
            logger,
            ERR,
            format!(
                "Not enabling kernel timestamps due to socket error {code}:{}",
                crate::compat::socket_error_message(code)
            ),
        );
        return session;
    }
    session.receive_message = receive_message;

    let config = ws::TIMESTAMPING_CONFIG {
        Flags: ws::TIMESTAMPING_FLAG_RX,
        ..ws::TIMESTAMPING_CONFIG::default()
    };
    bytes_returned = 0;
    let result = unsafe {
        ws::WSAIoctl(
            socket,
            ws::SIO_TIMESTAMPING,
            (&config as *const ws::TIMESTAMPING_CONFIG).cast(),
            size_of::<ws::TIMESTAMPING_CONFIG>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == -1 {
        let code = unsafe { ws::WSAGetLastError() };
        if code != 0 {
            session.receive_message = None;
            if code == 10045 {
                log(
                    logger,
                    DEBUG,
                    format!(
                        "Socket error {code}: no Windows support for kernel timestamps on this socket's NetAdapater"
                    ),
                );
            } else {
                log(
                    logger,
                    ERR,
                    format!(
                        " kernel timestamps were not initialized due to socket error {code}:{}",
                        crate::compat::socket_error_message(code)
                    ),
                );
            }
        }
    } else {
        let enabled = 1_i32;
        unsafe {
            let _ = ws::setsockopt(
                socket,
                ws::IPPROTO_IP,
                ws::IP_PKTINFO,
                (&enabled as *const i32).cast(),
                size_of_val(&enabled) as i32,
            );
            let _ = ws::setsockopt(
                socket,
                ws::IPPROTO_IPV6,
                ws::IPV6_PKTINFO,
                (&enabled as *const i32).cast(),
                size_of_val(&enabled) as i32,
            );
        }
    }
    session
}

#[cfg(windows)]
fn receive_timestamped_windows(
    socket: &UdpSocket,
    session: &mut TimestampSession,
    buffer: &mut [u8],
    logger: &Option<NtpLogCallback>,
) -> io::Result<ReceivedDatagram> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock as ws;

    let Some(receive_message) = session.receive_message else {
        return receive_timestamped_fallback(socket, buffer);
    };
    let mut remote_addr = ws::SOCKADDR_STORAGE::default();
    let mut data = ws::WSABUF {
        len: u32::try_from(buffer.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "NTP receive buffer is too large",
            )
        })?,
        buf: buffer.as_mut_ptr(),
    };
    #[repr(align(16))]
    struct ControlBuffer([u8; 128]);
    let mut control = ControlBuffer([0; 128]);
    let mut message = ws::WSAMSG {
        name: (&mut remote_addr as *mut ws::SOCKADDR_STORAGE).cast(),
        namelen: size_of::<ws::SOCKADDR_STORAGE>() as i32,
        lpBuffers: &mut data,
        dwBufferCount: 1,
        Control: ws::WSABUF {
            len: control.0.len() as u32,
            buf: control.0.as_mut_ptr(),
        },
        dwFlags: 0,
    };
    let mut bytes_received = 0_u32;
    let result = unsafe {
        receive_message(
            socket.as_raw_socket() as ws::SOCKET,
            &mut message,
            &mut bytes_received,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == -1 {
        let code = unsafe { ws::WSAGetLastError() };
        let error = io::Error::from_raw_os_error(code);
        if is_transient_windows_receive_error(code) {
            return Err(error);
        }
        log(
            logger,
            ERR,
            format!(
                "Disabling kernel timestamps due to socket error {code}:{}",
                crate::compat::socket_error_message(code)
            ),
        );
        session.receive_message = None;
        return receive_timestamped_fallback(socket, buffer);
    }

    let mut qpc_now = 0_i64;
    let clock_time_ns = if unsafe {
        windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut qpc_now)
    } != 0
    {
        qpc_ticks_to_unix_nanoseconds(
            session.base_qpc_ticks,
            session.base_system_time_us,
            session.qpc_frequency,
            qpc_now,
        )
        .unwrap_or_else(unix_time_ns)
    } else {
        unix_time_ns()
    };

    let control_length = (message.Control.len as usize).min(control.0.len());
    let metadata = if message.dwFlags & ws::MSG_CTRUNC == 0 {
        parse_windows_control_messages(&control.0[..control_length])
    } else {
        WindowsControlMetadata::default()
    };
    let kernel_time_ns = metadata
        .timestamp_qpc
        .filter(|ticks| *ticks > session.base_qpc_ticks as u64)
        .and_then(|ticks| {
            i64::try_from(ticks).ok().and_then(|ticks| {
                qpc_ticks_to_unix_nanoseconds(
                    session.base_qpc_ticks,
                    session.base_system_time_us,
                    session.qpc_frequency,
                    ticks,
                )
            })
        })
        .unwrap_or(0);
    if kernel_time_ns == 0 {
        session.receive_message = None;
    }
    // Preserve the reference's observable receive-path diagnostic: C logs
    // this notice on every successful WSARecvMsg, including when a kernel
    // timestamp is present. Keep adapter lookup and its fallback text aligned.
    let adapter = metadata
        .interface_index
        .and_then(interface_alias)
        .filter(|name| !name.is_empty());
    let fallback = adapter.as_deref().unwrap_or("(adapter name not found)");
    let command_name = adapter.as_deref().unwrap_or("(adapter friendly name)");
    log(
        logger,
        INFO,
        format!(
            "*** Windows support for kernel timestamps on NetAdapter \"{fallback}\" is \"Disabled\":\nTo enable it, use the Windows PowerShell (Administrator) command:\n   Set-NetAdapterAdvancedProperty -Name \"{command_name}\" -DisplayName \"Software Timestamp\" -DisplayValue  \"RxAll\""
        ),
    );
    Ok(ReceivedDatagram {
        length: bytes_received as usize,
        kernel_time_ns,
        clock_time_ns,
    })
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WindowsControlMetadata {
    timestamp_qpc: Option<u64>,
    interface_index: Option<u32>,
}

#[cfg(windows)]
fn parse_windows_control_messages(control: &[u8]) -> WindowsControlMetadata {
    use windows_sys::Win32::Networking::WinSock as ws;

    let alignment = size_of::<usize>();
    let Some(data_offset) = align_control_length(size_of::<ws::CMSGHDR>(), alignment) else {
        return WindowsControlMetadata::default();
    };
    let mut offset = 0_usize;
    let mut metadata = WindowsControlMetadata::default();
    while offset
        .checked_add(size_of::<ws::CMSGHDR>())
        .is_some_and(|end| end <= control.len())
    {
        // SAFETY: the complete header was bounds-checked; CMSGHDR permits unaligned reads.
        let header =
            unsafe { std::ptr::read_unaligned(control.as_ptr().add(offset).cast::<ws::CMSGHDR>()) };
        let Some(message_end) = offset.checked_add(header.cmsg_len) else {
            break;
        };
        let Some(payload_start) = offset.checked_add(data_offset) else {
            break;
        };
        if header.cmsg_len < data_offset
            || message_end > control.len()
            || payload_start > message_end
        {
            break;
        }
        let payload = &control[payload_start..message_end];
        if header.cmsg_level == ws::SOL_SOCKET && header.cmsg_type == ws::SO_TIMESTAMP as i32 {
            if payload.len() >= size_of::<u64>() {
                // SAFETY: the payload contains a complete timestamp; unaligned access is allowed.
                metadata.timestamp_qpc =
                    Some(unsafe { std::ptr::read_unaligned(payload.as_ptr().cast::<u64>()) });
            }
        } else if header.cmsg_level == ws::IPPROTO_IP && header.cmsg_type == ws::IP_PKTINFO {
            if payload.len() >= size_of::<ws::IN_PKTINFO>() {
                // SAFETY: the payload contains a complete packet-info record.
                let info =
                    unsafe { std::ptr::read_unaligned(payload.as_ptr().cast::<ws::IN_PKTINFO>()) };
                metadata.interface_index = Some(info.ipi_ifindex);
            }
        } else if header.cmsg_level == ws::IPPROTO_IPV6
            && header.cmsg_type == ws::IPV6_PKTINFO
            && payload.len() >= size_of::<ws::IN6_PKTINFO>()
        {
            // SAFETY: the payload contains a complete packet-info record.
            let info =
                unsafe { std::ptr::read_unaligned(payload.as_ptr().cast::<ws::IN6_PKTINFO>()) };
            metadata.interface_index = Some(info.ipi6_ifindex);
        }
        let Some(next_length) = align_control_length(header.cmsg_len, alignment) else {
            break;
        };
        let Some(next_offset) = offset.checked_add(next_length) else {
            break;
        };
        if next_offset <= offset || next_offset > control.len() {
            break;
        }
        offset = next_offset;
    }
    metadata
}

#[cfg(windows)]
fn align_control_length(length: usize, alignment: usize) -> Option<usize> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }
    length
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

#[cfg(windows)]
fn qpc_ticks_to_unix_nanoseconds(
    base_qpc_ticks: i64,
    base_system_time_us: u64,
    frequency: i64,
    qpc_ticks: i64,
) -> Option<u64> {
    if frequency <= 0 || qpc_ticks < base_qpc_ticks {
        return None;
    }
    let elapsed_ticks = i128::from(qpc_ticks) - i128::from(base_qpc_ticks);
    let elapsed_us = elapsed_ticks.checked_mul(1_000_000)? / i128::from(frequency);
    let elapsed_us = u64::try_from(elapsed_us).ok()?;
    base_system_time_us
        .checked_add(elapsed_us)?
        .checked_mul(1_000)
}

#[cfg(windows)]
fn is_transient_windows_receive_error(code: i32) -> bool {
    use windows_sys::Win32::Networking::WinSock as ws;
    matches!(
        code,
        ws::WSAETIMEDOUT | ws::WSAECONNRESET | ws::WSAENETRESET | ws::WSAECONNABORTED
    )
}

#[cfg(windows)]
fn interface_alias(index: u32) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2};
    let mut row = MIB_IF_ROW2 {
        InterfaceIndex: index,
        ..MIB_IF_ROW2::default()
    };
    if unsafe { GetIfEntry2(&mut row) } != 0 {
        return None;
    }
    let end = row
        .Alias
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(row.Alias.len());
    Some(String::from_utf16_lossy(&row.Alias[..end]))
}

#[cfg(windows)]
fn receive_timestamped_fallback(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> io::Result<ReceivedDatagram> {
    let (length, _) = socket.recv_from(buffer)?;
    Ok(ReceivedDatagram {
        length,
        kernel_time_ns: 0,
        clock_time_ns: unix_time_ns(),
    })
}

#[cfg(unix)]
fn receive_timestamped_unix(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
    use std::os::fd::AsRawFd;

    #[repr(align(16))]
    struct ControlBuffer([u8; 128]);

    let mut control = ControlBuffer([0; 128]);
    let mut iovec = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: zero is a valid initial state for msghdr before setting the used fields.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iovec;
    message.msg_iovlen = 1;
    message.msg_control = control.0.as_mut_ptr().cast();
    message.msg_controllen = control.0.len() as _;

    // SAFETY: `message` points to a live one-element iovec and aligned ancillary buffer;
    // `buffer` is writable for exactly `iov_len` bytes and the socket remains owned.
    let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, 0) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }

    let clock_time_ns = unix_time_ns();
    let mut kernel_time_ns = 0;
    if message.msg_flags & libc::MSG_CTRUNC == 0 {
        // `cmsghdr::cmsg_len` varies between libc ABIs; compare in usize so
        // Linux and BSD do not rely on an inferred `as _` target type.
        let timestamp_cmsg_len =
            unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::timeval>() as u32) as usize };
        // SAFETY: recvmsg initialized the control metadata within `control`.
        let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
        while !header.is_null() {
            // SAFETY: each header is bounded by the control buffer as maintained by CMSG_NXTHDR.
            let current = unsafe { &*header };
            if current.cmsg_level == libc::SOL_SOCKET
                && current.cmsg_type == libc::SCM_TIMESTAMP
                && current.cmsg_len as usize >= timestamp_cmsg_len
            {
                // SAFETY: cmsg length was checked and CMSG_DATA is suitably aligned, but
                // use an unaligned read to remain correct on every libc ABI.
                let timeval = unsafe {
                    std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::timeval>())
                };
                kernel_time_ns = (timeval.tv_sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add((timeval.tv_usec as u64).saturating_mul(1_000));
                break;
            }
            // SAFETY: `header` belongs to this message's control buffer.
            header = unsafe { libc::CMSG_NXTHDR(&message, header) };
        }
    }

    Ok(ReceivedDatagram {
        length: length as usize,
        kernel_time_ns,
        clock_time_ns,
    })
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

fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            time.as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(time.subsec_nanos()))
        })
}

fn format_local_timestamp(timestamp_ns: u64) -> String {
    let seconds = timestamp_ns / 1_000_000_000;
    let nanoseconds = timestamp_ns % 1_000_000_000;
    #[cfg(unix)]
    {
        let timestamp = seconds as libc::time_t;
        let mut local = std::mem::MaybeUninit::<libc::tm>::zeroed();
        // SAFETY: both pointers refer to initialized storage of the libc types;
        // localtime_r writes the broken-down local time into `local`.
        let result = unsafe { libc::localtime_r(&timestamp, local.as_mut_ptr()) };
        if !result.is_null() {
            // SAFETY: localtime_r returned this same pointer, marking it initialized.
            let local = unsafe { local.assume_init() };
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{nanoseconds:09}",
                local.tm_year + 1900,
                local.tm_mon + 1,
                local.tm_mday,
                local.tm_hour,
                local.tm_min,
                local.tm_sec
            )
        } else {
            format!("{seconds}.{nanoseconds:09}")
        }
    }
    #[cfg(windows)]
    {
        format_windows_local_timestamp(seconds, nanoseconds)
    }
    #[cfg(not(any(unix, windows)))]
    format!("{seconds}.{nanoseconds:09}")
}

#[cfg(windows)]
#[repr(C)]
struct WindowsSystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SystemTimeToTzSpecificLocalTime(
        time_zone: *const std::ffi::c_void,
        universal_time: *const WindowsSystemTime,
        local_time: *mut WindowsSystemTime,
    ) -> i32;
}

#[cfg(windows)]
fn format_windows_local_timestamp(seconds: u64, nanoseconds: u64) -> String {
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_date_from_days(days);
    let utc = WindowsSystemTime {
        year,
        month,
        day_of_week: ((days + 4) % 7) as u16,
        day,
        hour: (day_seconds / 3600) as u16,
        minute: ((day_seconds % 3600) / 60) as u16,
        second: (day_seconds % 60) as u16,
        milliseconds: 0,
    };
    let mut local = WindowsSystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    // SAFETY: `utc` and `local` are valid SYSTEMTIME-compatible structures; a null
    // time-zone pointer asks Windows to use the machine's current local time zone.
    let converted =
        unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } != 0;
    if converted {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{nanoseconds:09}",
            local.year, local.month, local.day, local.hour, local.minute, local.second
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{nanoseconds:09}",
            utc.year, utc.month, utc.day, utc.hour, utc.minute, utc.second
        )
    }
}

#[cfg(windows)]
fn civil_date_from_days(days_since_epoch: u64) -> (u16, u16, u16) {
    let days = days_since_epoch as i64;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as u16, month as u16, day as u16)
}

fn signed_ns_difference(end: u64, start: u64) -> i64 {
    end.wrapping_sub(start) as i64
}

fn log(logger: &Option<NtpLogCallback>, level: i32, message: String) {
    if let Some(logger) = logger {
        logger(level, &message);
    }
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::Mutex as StdMutex;

    const C_ORACLE: &str =
        include_str!("../../../tools/record-shims/raop_ntp_runtime_oracle.expected.txt");
    type TestLogEvents = Arc<StdMutex<Vec<(i32, String)>>>;

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(char::from(DIGITS[usize::from(byte >> 4)]));
            output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn test_logger() -> (NtpLogCallback, TestLogEvents) {
        let messages = Arc::new(StdMutex::new(Vec::new()));
        let output = Arc::clone(&messages);
        (
            Arc::new(move |level, message| {
                output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((level, message.to_owned()));
            }),
            messages,
        )
    }

    #[cfg(windows)]
    fn append_windows_control_message(
        control: &mut Vec<u8>,
        level: i32,
        kind: i32,
        payload: &[u8],
    ) {
        use windows_sys::Win32::Networking::WinSock::CMSGHDR;

        let data_offset = align_control_length(size_of::<CMSGHDR>(), size_of::<usize>()).unwrap();
        let message_length = data_offset + payload.len();
        let padded_length = align_control_length(message_length, size_of::<usize>()).unwrap();
        let start = control.len();
        control.resize(start + padded_length, 0);
        let header = CMSGHDR {
            cmsg_len: message_length,
            cmsg_level: level,
            cmsg_type: kind,
        };
        // SAFETY: the resized vector has room for the complete unaligned header.
        unsafe {
            std::ptr::write_unaligned(control.as_mut_ptr().add(start).cast::<CMSGHDR>(), header);
        }
        control[start + data_offset..start + message_length].copy_from_slice(payload);
    }

    #[test]
    fn request_and_response_timestamp_wire_values_match_c_byteutils() {
        let request = build_ntp_request(1_700_000_000_123_456_789);
        let mut next_request = request;
        next_request[8..16].copy_from_slice(&0xfedc_ba98_7654_3210u64.to_be_bytes());
        next_request[16..24].copy_from_slice(
            &unix_nanoseconds_to_ntp_q32_32(1_700_000_000_987_654_321).to_be_bytes(),
        );
        next_request[24..32].copy_from_slice(
            &unix_nanoseconds_to_ntp_q32_32(1_700_000_001_123_456_789).to_be_bytes(),
        );
        let mut response = [0u8; 32];
        response[8..16].copy_from_slice(&0xe123_4567_89ab_cdefu64.to_be_bytes());
        response[16..24].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_be_bytes());
        response[24..32].copy_from_slice(&0xfedc_ba98_7654_3210u64.to_be_bytes());
        let actual = format!(
            "request|{}\nnext-request|{}\nresponse|{:016x}|{:016x}|{:016x}\n",
            hex(&request),
            hex(&next_request),
            read_u64_be(&response, 8),
            read_u64_be(&response, 16),
            read_u64_be(&response, 24)
        );
        assert_eq!(actual, C_ORACLE);
    }

    #[test]
    fn remote_parse_uses_address_length_and_numeric_family() {
        let (logger, _) = test_logger();
        let ipv4 = RaopNtpRuntime::new(
            Some(logger.clone()),
            "192.0.2.10",
            4,
            6000,
            TimeProtocol::Ntp,
        )
        .unwrap();
        assert_eq!(ipv4.remote_address(), "192.0.2.10:6000".parse().unwrap());
        let ipv6 = RaopNtpRuntime::new(
            Some(logger.clone()),
            "fe80::1%7",
            16,
            6001,
            TimeProtocol::None,
        )
        .unwrap();
        assert_eq!(ipv6.remote_port(), 6001);
        assert!(ipv6.remote_address().is_ipv6());
        assert!(RaopNtpRuntime::new(None, "192.0.2.1", 16, 9, TimeProtocol::Ntp).is_err());
        assert!(RaopNtpRuntime::new(None, "localhost", 4, 9, TimeProtocol::Ntp).is_err());
        assert!(RaopNtpRuntime::new(None, "192.0.2.1", 5, 9, TimeProtocol::Ntp).is_err());
    }

    #[test]
    fn start_binds_the_negotiated_family_reports_ephemeral_port_and_restarts() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let remote = receiver.local_addr().unwrap();
        let (logger, messages) = test_logger();
        let mut runtime = RaopNtpRuntime::new(
            Some(logger),
            &remote.ip().to_string(),
            4,
            remote.port(),
            TimeProtocol::Ntp,
        )
        .unwrap();
        let mut local_port = 0;
        runtime
            .start_with_peer_to_peer(
                &mut local_port,
                cfg!(all(target_os = "macos", feature = "apple-p2p")),
            )
            .unwrap();
        assert_ne!(local_port, 0);
        assert_eq!(runtime.local_port(), local_port);
        assert!(runtime.is_running());

        let mut packet = [0u8; 64];
        let (length, source) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        assert_eq!(packet[..4], REQUEST_HEADER);
        assert_eq!(source.port(), local_port);

        let t1 = read_u64_be(&packet, 24);
        let t2 = t1.wrapping_add(1 << 24);
        let t3 = t1.wrapping_add(2 << 24);
        let mut response = [0u8; REQUEST_SIZE];
        response[8..16].copy_from_slice(&t1.to_be_bytes());
        response[16..24].copy_from_slice(&t2.to_be_bytes());
        response[24..32].copy_from_slice(&t3.to_be_bytes());
        receiver.send_to(&response, source).unwrap();

        let (length, next_source) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        assert_eq!(next_source.port(), local_port);
        assert_eq!(read_u64_be(&packet, 8), t3);
        assert_ne!(read_u64_be(&packet, 16), 0);
        assert!(runtime.sync_snapshot().clock.client_time_received);

        runtime.start(&mut local_port).unwrap();
        assert_eq!(runtime.local_port(), local_port);
        runtime.stop();
        runtime.stop();
        assert!(!runtime.is_running());
        assert!(
            messages
                .lock()
                .unwrap()
                .iter()
                .any(|(_, message)| { message == "raop_ntp stopping time thread" })
        );

        runtime.start(&mut local_port).unwrap();
        assert_eq!(runtime.local_port(), local_port);
        let _ = receiver.recv_from(&mut packet).unwrap();
        runtime.stop();
    }

    #[test]
    fn invalid_server_ref_and_duplicate_client_timestamp_are_rejected() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let remote = receiver.local_addr().unwrap();
        let (logger, messages) = test_logger();
        let mut runtime = RaopNtpRuntime::new(
            Some(logger),
            "127.0.0.1",
            4,
            remote.port(),
            TimeProtocol::Ntp,
        )
        .unwrap();
        let mut local_port = 0;
        runtime.start(&mut local_port).unwrap();

        let mut packet = [0u8; 64];
        let (length, source) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        let client_timestamp = 0x0123_4567_89ab_cdefu64;

        let mut invalid_response = [0u8; REQUEST_SIZE];
        invalid_response[16..24]
            .copy_from_slice(&client_timestamp.wrapping_sub(1 << 24).to_be_bytes());
        invalid_response[24..32].copy_from_slice(&client_timestamp.to_be_bytes());
        receiver.send_to(&invalid_response, source).unwrap();
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        assert_eq!(read_u64_be(&packet, 8), client_timestamp);

        let (length, source) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        let matching_t1 = read_u64_be(&packet, 24);
        let mut duplicate_response = [0u8; REQUEST_SIZE];
        duplicate_response[8..16].copy_from_slice(&matching_t1.to_be_bytes());
        duplicate_response[16..24]
            .copy_from_slice(&client_timestamp.wrapping_sub(1 << 24).to_be_bytes());
        duplicate_response[24..32].copy_from_slice(&client_timestamp.to_be_bytes());
        receiver.send_to(&duplicate_response, source).unwrap();
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, REQUEST_SIZE);
        runtime.stop();

        let messages = messages.lock().unwrap();
        assert!(messages.iter().any(|(_, message)| {
            message == "raop_ntp received NTP packet with invalid server ref: 0"
        }));
        assert!(messages.iter().any(|(_, message)| {
            message
                == &format!("raop_ntp received NTP packet with duplicate t3: {client_timestamp}")
        }));
        assert!(!runtime.sync_snapshot().clock.client_time_received);
    }

    #[test]
    fn failed_socket_start_is_retryable_without_publishing_a_new_port() {
        let mut runtime =
            RaopNtpRuntime::new(None, "127.0.0.1", 4, 6000, TimeProtocol::Ntp).unwrap();
        let mut requested_port = 0;
        let mut fail_once = true;
        let mut bind = |port: &mut u16, family| {
            if fail_once {
                fail_once = false;
                Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "injected timing bind failure",
                ))
            } else {
                bind_socket(port, family, Transport::Udp, false).map(Into::into)
            }
        };
        assert!(
            runtime
                .start_with_socket(&mut requested_port, &mut bind)
                .is_err()
        );
        assert_eq!(requested_port, 0);
        assert!(!runtime.is_running());
        runtime
            .start_with_socket(&mut requested_port, &mut bind)
            .unwrap();
        assert_ne!(requested_port, 0);
        runtime.stop();
    }

    #[test]
    fn stop_signal_interrupts_the_initial_wait_and_releases_the_port() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let remote = receiver.local_addr().unwrap();
        let mut runtime =
            RaopNtpRuntime::new(None, "127.0.0.1", 4, remote.port(), TimeProtocol::Ntp).unwrap();
        let mut port = 0;
        runtime.start(&mut port).unwrap();
        let start = std::time::Instant::now();
        runtime.stop();
        assert!(start.elapsed() < TIMING_RESPONSE_TIMEOUT + Duration::from_secs(1));
        assert!(!runtime.is_running());
        let rebound = UdpSocket::bind(SocketAddr::new("0.0.0.0".parse().unwrap(), port));
        assert!(rebound.is_ok());
    }

    #[test]
    fn receive_timestamp_selection_matches_source_sanity_and_cooldown_rules() {
        let mut health = TimestampHealth::default();
        assert_eq!(choose_receive_time(10, 30, 20, 30, &mut health), (20, true));
        assert_eq!(
            choose_receive_time(25, 30, 20, 30, &mut health),
            (30, false)
        );
        assert!(health.inconsistency_detected);
        assert_eq!(
            choose_receive_time(10, 12, 11, 12, &mut health),
            (12, false)
        );
        assert_eq!(
            choose_receive_time(
                10,
                NTP_KERNEL_TIMESTAMP_COOLDOWN_NS + 31,
                15,
                NTP_KERNEL_TIMESTAMP_COOLDOWN_NS + 31,
                &mut health
            ),
            (15, true)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_qpc_conversion_matches_the_c_baseline_math() {
        assert_eq!(
            qpc_ticks_to_unix_nanoseconds(1_000, 5_000_000, 10_000_000, 10_001_000),
            Some(6_000_000_000)
        );
        assert_eq!(
            qpc_ticks_to_unix_nanoseconds(1_000, 5_000_000, 10_000_000, 999),
            None
        );
        assert_eq!(
            qpc_ticks_to_unix_nanoseconds(1_000, 5_000_000, 0, 10_001_000),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ancillary_parser_reads_timestamp_and_ipv4_ipv6_indices_safely() {
        use windows_sys::Win32::Networking::WinSock as ws;

        let mut control = Vec::new();
        append_windows_control_message(
            &mut control,
            ws::SOL_SOCKET,
            ws::SO_TIMESTAMP as i32,
            &42_123_456_u64.to_ne_bytes(),
        );
        let mut ipv4 = vec![0; size_of::<ws::IN_PKTINFO>()];
        ipv4[size_of::<ws::IN_ADDR>()..size_of::<ws::IN_ADDR>() + size_of::<u32>()]
            .copy_from_slice(&7_u32.to_ne_bytes());
        append_windows_control_message(&mut control, ws::IPPROTO_IP, ws::IP_PKTINFO, &ipv4);
        let metadata = parse_windows_control_messages(&control);
        assert_eq!(metadata.timestamp_qpc, Some(42_123_456));
        assert_eq!(metadata.interface_index, Some(7));

        let mut ipv6 = vec![0; size_of::<ws::IN6_PKTINFO>()];
        ipv6[size_of::<ws::IN6_ADDR>()..size_of::<ws::IN6_ADDR>() + size_of::<u32>()]
            .copy_from_slice(&19_u32.to_ne_bytes());
        let mut control = Vec::new();
        append_windows_control_message(&mut control, ws::IPPROTO_IPV6, ws::IPV6_PKTINFO, &ipv6);
        assert_eq!(
            parse_windows_control_messages(&control).interface_index,
            Some(19)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ancillary_parser_rejects_truncated_headers_and_payloads() {
        use windows_sys::Win32::Networking::WinSock::{CMSGHDR, SO_TIMESTAMP, SOL_SOCKET};

        let mut truncated = vec![0; size_of::<CMSGHDR>() - 1];
        assert_eq!(
            parse_windows_control_messages(&truncated),
            WindowsControlMetadata::default()
        );

        let header = CMSGHDR {
            cmsg_len: usize::MAX,
            cmsg_level: SOL_SOCKET,
            cmsg_type: SO_TIMESTAMP as i32,
        };
        truncated.resize(size_of::<CMSGHDR>(), 0);
        // SAFETY: the vector contains the full header and intentionally corrupt length.
        unsafe {
            std::ptr::write_unaligned(truncated.as_mut_ptr().cast::<CMSGHDR>(), header);
        }
        assert_eq!(
            parse_windows_control_messages(&truncated),
            WindowsControlMetadata::default()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_timestamp_fallback_only_treats_network_errors_as_routine() {
        use windows_sys::Win32::Networking::WinSock as ws;
        assert!(is_transient_windows_receive_error(ws::WSAETIMEDOUT));
        assert!(is_transient_windows_receive_error(ws::WSAECONNRESET));
        assert!(is_transient_windows_receive_error(ws::WSAENETRESET));
        assert!(is_transient_windows_receive_error(ws::WSAECONNABORTED));
        assert!(!is_transient_windows_receive_error(10045));
    }

    #[test]
    fn public_sync_and_video_timing_state_start_unsynced() {
        let runtime = RaopNtpRuntime::new(None, "192.0.2.1", 4, 6000, TimeProtocol::Other).unwrap();
        assert_eq!(runtime.remote_time_ns(), 0);
        assert_eq!(runtime.convert_remote_time_ns(7), 0);
        assert_eq!(runtime.convert_local_time_ns(7), 0);
        assert_eq!(runtime.timestamp_to_nanoseconds(5 << 32), 5_000_000_000);
        runtime.set_video_arrival_offset(123);
        assert_eq!(runtime.video_arrival_offset(), 123);
        assert_eq!(runtime.get_sync_params(), (0, 0.0));
    }
}
