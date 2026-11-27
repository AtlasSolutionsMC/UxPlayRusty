//! Persistent HTTP/1 transport for the AirPlay protocol dispatcher.
//!
//! This is a transport layer only. It accepts connections, feeds request bytes to
//! `http_request`, forwards complete requests to a caller-provided dispatcher, and
//! writes the response bytes supplied by that dispatcher. It does not implement
//! AirPlay, RAOP, HLS, pairing, or route behavior.

use crate::http_request::{EventMessage, HttpRequest, ParserFailure};
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SOURCE_MAX_CONNECTIONS: usize = 12;
pub const SOURCE_READ_BUFFER_SIZE: usize = 1024;
pub const SOURCE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const SOCKET_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MIN_READ_POLL_INTERVAL: Duration = Duration::from_millis(1);
const UPGRADE_PREFIX: &[u8] = b"HTTP/1.1 101";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionInfo {
    pub local_addr: SocketAddr,
    pub peer_addr: SocketAddr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpReply {
    /// Complete status line, headers, delimiter, and optional body.
    pub bytes: Vec<u8>,
    /// Mirrors `http_response_set_disconnect`.
    pub disconnect: bool,
    /// Connection classes the reference removes after sending this response.
    pub remove_classes_after_write: Vec<ConnectionClass>,
}

impl HttpReply {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            disconnect: false,
            remove_classes_after_write: Vec::new(),
        }
    }

    pub fn disconnect_after_write(mut self) -> Self {
        self.disconnect = true;
        self
    }

    pub fn remove_classes_after_write(
        mut self,
        classes: impl IntoIterator<Item = ConnectionClass>,
    ) -> Self {
        self.remove_classes_after_write.extend(classes);
        self
    }

    fn upgrades_to_event(&self) -> bool {
        self.bytes.starts_with(UPGRADE_PREFIX)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConnectionClass {
    Unknown,
    Raop,
    AirPlay,
    Event,
    Hls,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveErrorStage {
    Request,
    ReverseEvent,
}

// The standalone transport conformance harness compiles this module without the
// protocol dispatcher that consumes the media-control hook.
#[allow(dead_code)]
pub(crate) trait RaopMediaControl: Send + Sync {
    fn stop_services_for_airplay(&self) -> io::Result<()>;
}

/// Tracks accepted sockets so source reset callbacks can remove selected
/// protocol classes while leaving RAOP and unknown connections alive.
#[derive(Default)]
#[allow(dead_code)]
pub struct ConnectionRegistry {
    next_id: AtomicU64,
    connections: Mutex<BTreeMap<u64, (ConnectionClass, TcpStream)>>,
    next_raop_media_id: AtomicU64,
    raop_media: Mutex<BTreeMap<u64, Weak<dyn RaopMediaControl>>>,
}

impl ConnectionRegistry {
    #[cfg(windows)]
    pub(crate) fn has_connections(&self) -> bool {
        !self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        stream: &TcpStream,
    ) -> io::Result<ConnectionRegistration> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, (ConnectionClass::Unknown, stream.try_clone()?));
        Ok(ConnectionRegistration {
            id,
            registry: Arc::clone(self),
        })
    }

    pub(crate) fn has_class(&self, class: ConnectionClass) -> bool {
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .any(|(connection_class, _)| *connection_class == class)
    }

    pub(crate) fn count_class(&self, class: ConnectionClass) -> usize {
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|(connection_class, _)| *connection_class == class)
            .count()
    }

    #[allow(dead_code)]
    pub(crate) fn register_raop_media<M: RaopMediaControl + 'static>(
        self: &Arc<Self>,
        media: &Arc<M>,
    ) -> RaopMediaRegistration {
        let id = self.next_raop_media_id.fetch_add(1, Ordering::Relaxed);
        let media: Arc<dyn RaopMediaControl> = Arc::clone(media) as Arc<dyn RaopMediaControl>;
        self.raop_media
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, Arc::downgrade(&media));
        RaopMediaRegistration {
            id,
            registry: Arc::clone(self),
        }
    }

    /// Stop the first live RAOP media session when the source identifies a new
    /// AirPlay connection. Keep the registry lock out of the worker shutdown path.
    #[allow(dead_code)]
    pub(crate) fn stop_raop_media_for_airplay(&self) -> io::Result<bool> {
        let media = {
            let mut sessions = self
                .raop_media
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sessions.retain(|_, media| media.strong_count() != 0);
            sessions.values().find_map(Weak::upgrade)
        };
        let Some(media) = media else {
            return Ok(false);
        };
        media.stop_services_for_airplay()?;
        Ok(true)
    }

    /// Close source-classified sockets, leaving newly accepted unknown
    /// connections alive until their first request identifies their class.
    pub(crate) fn remove_known_connections(&self) {
        let connections = self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (class, stream) in connections.values() {
            if *class != ConnectionClass::Unknown {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }

    fn remove_classes(&self, classes: &[ConnectionClass]) {
        let connections = self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (class, stream) in connections.values() {
            if classes.contains(class) {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) struct RaopMediaRegistration {
    id: u64,
    registry: Arc<ConnectionRegistry>,
}

impl Drop for RaopMediaRegistration {
    fn drop(&mut self) {
        self.registry
            .raop_media
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

pub(crate) struct ConnectionRegistration {
    id: u64,
    registry: Arc<ConnectionRegistry>,
}

impl ConnectionRegistration {
    pub(crate) fn set_class(&self, class: ConnectionClass) {
        if let Some((registered_class, _)) = self
            .registry
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&self.id)
        {
            *registered_class = class;
        }
    }

    fn remove_classes(&self, classes: &[ConnectionClass]) {
        self.registry.remove_classes(classes);
    }
}

impl Drop for ConnectionRegistration {
    fn drop(&mut self) {
        self.registry
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// Per-connection AirPlay dispatcher created by [`serve`]'s factory.
///
/// Each connection gets its own handler, matching the C `conn_init` callback's
/// per-connection user data. A missing response leaves the HTTP connection open.
pub trait ConnectionHandler: Send + 'static {
    fn request(&mut self, request: &HttpRequest) -> io::Result<Option<HttpReply>>;

    fn idle_timeout(&mut self, _socket_id: i32, _idle_timeout_seconds: u64) {}

    fn receive_error(
        &mut self,
        _error: &ParserFailure,
        _recv_data: &[u8],
        _stage: ReceiveErrorStage,
    ) {
    }

    fn connection_class(&self) -> ConnectionClass {
        ConnectionClass::Unknown
    }

    fn upgraded(&mut self, _stream: TcpStream) -> io::Result<()> {
        Ok(())
    }

    fn event(&mut self, _event: EventMessage) -> io::Result<()> {
        Ok(())
    }

    fn disconnected(&mut self) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerConfig {
    pub max_connections: usize,
    pub idle_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: SOURCE_MAX_CONNECTIONS,
            idle_timeout: SOURCE_IDLE_TIMEOUT,
        }
    }
}

/// Accept connections until `shutdown` is set, spawning one worker per client.
///
/// Callers bind IPv4 and IPv6 listeners using the app's socket policy, then pass
/// both here. When the source's 12-connection limit is reached, this function
/// stops accepting so the operating system's listen backlog can queue new peers.
pub fn serve<F, H>(
    listeners: Vec<TcpListener>,
    shutdown: Arc<AtomicBool>,
    config: ServerConfig,
    make_handler: F,
) -> io::Result<()>
where
    F: Fn(ConnectionInfo) -> io::Result<H> + Send + Sync + 'static,
    H: ConnectionHandler,
{
    serve_inner(listeners, shutdown, config, make_handler, None)
}

/// Serve while registering each socket for class-specific source resets.
pub fn serve_with_registry<F, H>(
    listeners: Vec<TcpListener>,
    shutdown: Arc<AtomicBool>,
    config: ServerConfig,
    registry: Arc<ConnectionRegistry>,
    make_handler: F,
) -> io::Result<()>
where
    F: Fn(ConnectionInfo) -> io::Result<H> + Send + Sync + 'static,
    H: ConnectionHandler,
{
    serve_inner(listeners, shutdown, config, make_handler, Some(registry))
}

fn serve_inner<F, H>(
    mut listeners: Vec<TcpListener>,
    shutdown: Arc<AtomicBool>,
    config: ServerConfig,
    make_handler: F,
    registry: Option<Arc<ConnectionRegistry>>,
) -> io::Result<()>
where
    F: Fn(ConnectionInfo) -> io::Result<H> + Send + Sync + 'static,
    H: ConnectionHandler,
{
    if listeners.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP transport needs at least one listener",
        ));
    }
    if config.max_connections == 0 || config.idle_timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP transport limits must be nonzero",
        ));
    }
    for listener in &mut listeners {
        listener.set_nonblocking(true)?;
    }

    let make_handler = Arc::new(make_handler);
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers: Vec<JoinHandle<io::Result<()>>> = Vec::new();
    let mut accept_error = None;

    while !shutdown.load(Ordering::Acquire) {
        reap_finished_workers(&mut workers);
        if active.load(Ordering::Acquire) < config.max_connections {
            for listener in &listeners {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let handler_factory = Arc::clone(&make_handler);
                        let active_count = Arc::clone(&active);
                        let stop = Arc::clone(&shutdown);
                        let connection_registry = registry.clone();
                        active.fetch_add(1, Ordering::AcqRel);
                        match thread::Builder::new().spawn(move || {
                            let _active = ActiveConnection(active_count);
                            let registration = connection_registry
                                .as_ref()
                                .map(|registry| registry.register(&stream))
                                .transpose()?;
                            let info = match connection_info(&stream) {
                                Ok(info) => info,
                                Err(error) => return Err(error),
                            };
                            let mut handler = match handler_factory(info) {
                                Ok(handler) => handler,
                                Err(error) => return Err(error),
                            };
                            run_connection_registered(
                                stream,
                                &mut handler,
                                stop,
                                config.idle_timeout,
                                registration,
                            )
                        }) {
                            Ok(worker) => workers.push(worker),
                            Err(error) => {
                                active.fetch_sub(1, Ordering::AcqRel);
                                accept_error = Some(error);
                                break;
                            }
                        }
                        // Match the reference loop's one accept per ready listener per poll.
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        accept_error = Some(error);
                        break;
                    }
                }
                if active.load(Ordering::Acquire) >= config.max_connections {
                    break;
                }
            }
        }
        if accept_error.is_some() {
            break;
        }
        thread::sleep(ACCEPT_POLL_INTERVAL);
    }

    if accept_error.is_some() {
        shutdown.store(true, Ordering::Release);
    }
    for worker in workers {
        let _ = worker.join();
    }
    if let Some(error) = accept_error {
        Err(error)
    } else {
        Ok(())
    }
}

/// Serve one accepted stream, preserving keep-alive and reverse-HTTP EVENT mode.
pub fn run_connection<H: ConnectionHandler>(
    stream: TcpStream,
    handler: &mut H,
    shutdown: Arc<AtomicBool>,
    idle_timeout: Duration,
) -> io::Result<()> {
    run_connection_registered(stream, handler, shutdown, idle_timeout, None)
}

fn run_connection_registered<H: ConnectionHandler>(
    mut stream: TcpStream,
    handler: &mut H,
    shutdown: Arc<AtomicBool>,
    idle_timeout: Duration,
    mut registration: Option<ConnectionRegistration>,
) -> io::Result<()> {
    if idle_timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP idle timeout must be nonzero",
        ));
    }
    configure_source_write_timeout(&stream)?;

    let result = serve_connection_loop(
        &mut stream,
        handler,
        shutdown,
        idle_timeout,
        registration.as_mut(),
    );
    handler.disconnected();
    result
}

fn serve_connection_loop<H: ConnectionHandler>(
    stream: &mut TcpStream,
    handler: &mut H,
    shutdown: Arc<AtomicBool>,
    idle_timeout: Duration,
    registration: Option<&mut ConnectionRegistration>,
) -> io::Result<()> {
    let mut state = ConnectionState::default();
    let mut read_buffer = [0u8; SOURCE_READ_BUFFER_SIZE];
    let idle_timeout_seconds = idle_timeout.as_secs();
    let mut last_active_seconds = unix_time_seconds();
    let socket_id = source_socket_id(stream);

    loop {
        let now_seconds = unix_time_seconds();
        if shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        if notify_idle_timeout(
            handler,
            socket_id,
            now_seconds,
            last_active_seconds,
            idle_timeout_seconds,
        ) {
            return Ok(());
        }
        let timeout = source_idle_timeout_poll_interval(
            now_seconds,
            last_active_seconds,
            idle_timeout_seconds,
        );
        stream.set_read_timeout(Some(timeout))?;

        let read_len = match stream.read(&mut read_buffer) {
            Ok(0) => return Ok(()),
            Ok(read_len) => read_len,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        last_active_seconds = unix_time_seconds();

        let incoming = match receive_with_handler(&mut state, handler, &read_buffer[..read_len]) {
            Ok(incoming) => incoming,
            // The reference logs parser errors and closes only that connection.
            Err(_parser_error) => return Ok(()),
        };
        for message in incoming {
            match message {
                Incoming::RequestReady => {
                    let Some(request) = state.request() else {
                        if state.is_event_mode() {
                            continue;
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "request-ready event without an HTTP request",
                        ));
                    };
                    let reply = handler.request(request)?;
                    if let Some(registration) = registration.as_deref() {
                        registration.set_class(handler.connection_class());
                    }
                    if let Some(reply) = &reply {
                        stream.write_all(&reply.bytes)?;
                        if !reply.remove_classes_after_write.is_empty()
                            && let Some(registration) = registration.as_deref()
                        {
                            registration.remove_classes(&reply.remove_classes_after_write);
                        }
                    }
                    let upgrades_to_event =
                        reply.as_ref().is_some_and(HttpReply::upgrades_to_event);
                    match state.response_written(reply.as_ref()) {
                        ConnectionAction::Upgraded => {
                            handler.upgraded(stream.try_clone()?)?;
                            if let Some(registration) = registration.as_deref() {
                                registration.set_class(handler.connection_class());
                            }
                        }
                        ConnectionAction::Close => {
                            // C switches the request and connection to EVENT after
                            // writing every 101 response, before honoring its
                            // disconnect flag. Preserve that callback ordering even
                            // when the upgraded connection closes immediately.
                            if upgrades_to_event {
                                handler.upgraded(stream.try_clone()?)?;
                                if let Some(registration) = registration.as_deref() {
                                    registration.set_class(handler.connection_class());
                                }
                            }
                            return Ok(());
                        }
                        ConnectionAction::KeepAlive => {}
                    }
                }
                Incoming::Event(event) => handler.event(event)?,
            }
        }
    }
}

fn receive_with_handler<H: ConnectionHandler>(
    state: &mut ConnectionState,
    handler: &mut H,
    recv_data: &[u8],
) -> Result<Vec<Incoming>, ParserFailure> {
    match state.receive(recv_data) {
        Ok(incoming) => Ok(incoming),
        Err(error) => {
            let stage = if state.is_event_mode() {
                ReceiveErrorStage::ReverseEvent
            } else {
                ReceiveErrorStage::Request
            };
            handler.receive_error(&error, recv_data, stage);
            Err(error)
        }
    }
}

fn unix_time_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn source_socket_id(stream: &TcpStream) -> i32 {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        stream.as_raw_fd()
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        stream.as_raw_socket() as i32
    }
}

fn notify_idle_timeout<H: ConnectionHandler>(
    handler: &mut H,
    socket_id: i32,
    now_seconds: u64,
    last_active_seconds: u64,
    idle_timeout_seconds: u64,
) -> bool {
    if !source_idle_timeout_expired(now_seconds, last_active_seconds, idle_timeout_seconds) {
        return false;
    }
    handler.idle_timeout(socket_id, idle_timeout_seconds);
    true
}

fn configure_source_write_timeout(stream: &TcpStream) -> io::Result<()> {
    // The C server uses blocking `send()` calls without SO_SNDTIMEO. The idle
    // timeout applies to reads only, so replies must not inherit it as a cap.
    stream.set_write_timeout(None)
}

fn source_idle_timeout_expired(
    now_seconds: u64,
    last_active_seconds: u64,
    limit_seconds: u64,
) -> bool {
    now_seconds.saturating_sub(last_active_seconds) > limit_seconds
}

fn source_idle_timeout_poll_interval(
    now_seconds: u64,
    last_active_seconds: u64,
    limit_seconds: u64,
) -> Duration {
    let elapsed_seconds = now_seconds.saturating_sub(last_active_seconds);
    let remaining_seconds = limit_seconds
        .saturating_add(1)
        .saturating_sub(elapsed_seconds);
    Duration::from_secs(remaining_seconds)
        .min(SOCKET_POLL_INTERVAL)
        .max(MIN_READ_POLL_INTERVAL)
}

fn connection_info(stream: &TcpStream) -> io::Result<ConnectionInfo> {
    Ok(ConnectionInfo {
        local_addr: stream.local_addr()?,
        peer_addr: stream.peer_addr()?,
    })
}

fn reap_finished_workers(workers: &mut Vec<JoinHandle<io::Result<()>>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            let _ = worker.join();
        } else {
            index += 1;
        }
    }
}

struct ActiveConnection(Arc<AtomicUsize>);

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Default)]
pub struct ConnectionState {
    request: HttpRequest,
    ready_requests: VecDeque<HttpRequest>,
    event_mode: bool,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionAction {
    KeepAlive,
    Upgraded,
    Close,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Incoming {
    RequestReady,
    Event(EventMessage),
}

impl ConnectionState {
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Vec<Incoming>, ParserFailure> {
        if self.closed {
            return Ok(Vec::new());
        }
        if !self.event_mode {
            self.request.add_data(bytes)?;
            if !self.request.is_complete() {
                return Ok(Vec::new());
            }
            let read_ahead = self.request.take_pending_data();
            if let Err(error) = self.request.apply_source_read_ahead(&read_ahead) {
                self.ready_requests.clear();
                self.closed = true;
                return Err(error);
            }
            self.ready_requests
                .push_back(std::mem::replace(&mut self.request, HttpRequest::new()));
            return Ok(vec![Incoming::RequestReady]);
        }

        self.request.add_event_data(bytes)?;
        let mut events = Vec::new();
        while let Some(event) = self.request.parse_event_data() {
            events.push(Incoming::Event(event));
        }
        Ok(events)
    }

    pub fn request(&self) -> Option<&HttpRequest> {
        (!self.event_mode)
            .then(|| self.ready_requests.front())
            .flatten()
    }

    /// Apply the C server's post-response lifecycle to the current request.
    pub fn response_written(&mut self, reply: Option<&HttpReply>) -> ConnectionAction {
        if let Some(reply) = reply {
            if reply.upgrades_to_event() {
                let mut request = self.ready_requests.pop_front().unwrap_or_default();
                request.upgrade_data();
                request.set_reverse();
                self.request = request;
                self.ready_requests.clear();
                self.event_mode = true;
                if reply.disconnect {
                    self.closed = true;
                    return ConnectionAction::Close;
                }
                return ConnectionAction::Upgraded;
            }
            if reply.disconnect {
                self.ready_requests.clear();
                self.closed = true;
                return ConnectionAction::Close;
            }
        }
        if !self.event_mode {
            self.ready_requests.pop_front();
        }
        ConnectionAction::KeepAlive
    }

    pub fn is_event_mode(&self) -> bool {
        self.event_mode
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct CapturedReceiveError {
        name: &'static str,
        description: String,
        recv_data: Vec<u8>,
        stage: ReceiveErrorStage,
    }

    struct CaptureReceiveErrorHandler {
        errors: Arc<Mutex<Vec<CapturedReceiveError>>>,
    }

    impl ConnectionHandler for CaptureReceiveErrorHandler {
        fn request(&mut self, _request: &HttpRequest) -> io::Result<Option<HttpReply>> {
            Ok(None)
        }

        fn receive_error(
            &mut self,
            error: &ParserFailure,
            recv_data: &[u8],
            stage: ReceiveErrorStage,
        ) {
            self.errors.lock().unwrap().push(CapturedReceiveError {
                name: error.name,
                description: error.description.clone(),
                recv_data: recv_data.to_vec(),
                stage,
            });
        }
    }

    struct CaptureIdleTimeoutHandler {
        expirations: Arc<Mutex<Vec<(i32, u64)>>>,
    }

    impl ConnectionHandler for CaptureIdleTimeoutHandler {
        fn request(&mut self, _request: &HttpRequest) -> io::Result<Option<HttpReply>> {
            Ok(None)
        }

        fn idle_timeout(&mut self, socket_id: i32, idle_timeout_seconds: u64) {
            self.expirations
                .lock()
                .unwrap()
                .push((socket_id, idle_timeout_seconds));
        }
    }

    struct RemoveAfterReplyHandler;

    struct CaptureUpgradeDisconnectHandler {
        events: Arc<Mutex<Vec<(&'static str, ConnectionClass)>>>,
        class: ConnectionClass,
    }

    impl ConnectionHandler for CaptureUpgradeDisconnectHandler {
        fn request(&mut self, _request: &HttpRequest) -> io::Result<Option<HttpReply>> {
            Ok(Some(
                HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec())
                    .disconnect_after_write(),
            ))
        }

        fn connection_class(&self) -> ConnectionClass {
            self.class
        }

        fn upgraded(&mut self, _stream: TcpStream) -> io::Result<()> {
            self.class = ConnectionClass::Event;
            self.events.lock().unwrap().push(("upgraded", self.class));
            Ok(())
        }

        fn disconnected(&mut self) {
            self.events
                .lock()
                .unwrap()
                .push(("disconnected", self.class));
        }
    }

    #[test]
    fn source_transport_does_not_apply_the_read_idle_limit_to_writes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        server
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        configure_source_write_timeout(&server).unwrap();

        assert_eq!(server.write_timeout().unwrap(), None);
        drop(client);
    }

    impl ConnectionHandler for RemoveAfterReplyHandler {
        fn request(&mut self, _request: &HttpRequest) -> io::Result<Option<HttpReply>> {
            Ok(Some(
                HttpReply::new(b"HTTP/1.1 200 OK\r\n\r\nreset".to_vec())
                    .remove_classes_after_write([ConnectionClass::Hls]),
            ))
        }

        fn connection_class(&self) -> ConnectionClass {
            ConnectionClass::Hls
        }
    }

    #[test]
    fn registry_shutdown_runs_after_the_current_reply_is_written() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let registry = Arc::new(ConnectionRegistry::default());
        let server = thread::spawn(move || {
            serve_with_registry(
                vec![listener],
                server_shutdown,
                ServerConfig::default(),
                registry,
                |_| Ok(RemoveAfterReplyHandler),
            )
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET /reset HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();

        assert_eq!(response, b"HTTP/1.1 200 OK\r\n\r\nreset");
        shutdown.store(true, Ordering::Release);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn registry_removes_only_the_requested_connection_classes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let airplay_peer = TcpStream::connect(address).unwrap();
        let (airplay_stream, _) = listener.accept().unwrap();
        let raop_peer = TcpStream::connect(address).unwrap();
        let (raop_stream, _) = listener.accept().unwrap();
        let registry = Arc::new(ConnectionRegistry::default());
        let airplay_registration = registry.register(&airplay_stream).unwrap();
        airplay_registration.set_class(ConnectionClass::AirPlay);
        let raop_registration = registry.register(&raop_stream).unwrap();
        raop_registration.set_class(ConnectionClass::Raop);
        assert_eq!(registry.count_class(ConnectionClass::AirPlay), 1);
        assert_eq!(registry.count_class(ConnectionClass::Raop), 1);
        assert_eq!(registry.count_class(ConnectionClass::Hls), 0);

        registry.remove_classes(&[
            ConnectionClass::Hls,
            ConnectionClass::Event,
            ConnectionClass::AirPlay,
        ]);

        let mut airplay_peer = airplay_peer;
        let mut closed = Vec::new();
        airplay_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        drop(airplay_registration);
        assert_eq!(registry.count_class(ConnectionClass::AirPlay), 0);
        assert_eq!(registry.count_class(ConnectionClass::Raop), 1);

        let mut raop_peer = raop_peer;
        raop_peer.write_all(b"still-open").unwrap();
        let mut received = [0; 10];
        (&raop_stream).read_exact(&mut received).unwrap();
        assert_eq!(&received, b"still-open");
    }

    #[test]
    fn nohold_removes_classified_connections_but_keeps_new_unknown_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let old_peer = TcpStream::connect(address).unwrap();
        let (old_stream, _) = listener.accept().unwrap();
        let mut new_peer = TcpStream::connect(address).unwrap();
        let (new_stream, _) = listener.accept().unwrap();
        let registry = Arc::new(ConnectionRegistry::default());
        let old_registration = registry.register(&old_stream).unwrap();
        old_registration.set_class(ConnectionClass::Raop);
        let new_registration = registry.register(&new_stream).unwrap();

        assert!(registry.has_class(ConnectionClass::Raop));
        registry.remove_known_connections();

        let mut old_peer = old_peer;
        let mut closed = Vec::new();
        old_peer.read_to_end(&mut closed).unwrap();
        assert!(closed.is_empty());
        drop(old_registration);
        assert!(!registry.has_class(ConnectionClass::Raop));
        new_peer
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        new_peer.write_all(b"still-open").unwrap();
        let mut received = [0; 10];
        (&new_stream).read_exact(&mut received).unwrap();
        assert_eq!(&received, b"still-open");
        assert_eq!(
            registry
                .connections
                .lock()
                .unwrap()
                .get(&new_registration.id)
                .map(|(class, _)| *class),
            Some(ConnectionClass::Unknown)
        );
    }

    #[test]
    fn source_defaults_keep_twelve_connections_and_sixty_second_idle_timeout() {
        let config = ServerConfig::default();
        assert_eq!(config.max_connections, 12);
        assert_eq!(config.idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn source_idle_timeout_expires_only_after_more_than_sixty_integer_seconds() {
        assert!(!source_idle_timeout_expired(160, 100, 60));
        assert!(source_idle_timeout_expired(161, 100, 60));
        assert!(!source_idle_timeout_expired(99, 100, 60));
        assert_eq!(
            source_idle_timeout_poll_interval(160, 100, 60),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn idle_timeout_hook_uses_the_source_integer_second_boundary() {
        let expirations = Arc::new(Mutex::new(Vec::new()));
        let mut handler = CaptureIdleTimeoutHandler {
            expirations: Arc::clone(&expirations),
        };

        assert!(!notify_idle_timeout(&mut handler, 37, 160, 100, 60));
        assert!(notify_idle_timeout(&mut handler, 37, 161, 100, 60));
        assert_eq!(*expirations.lock().unwrap(), [(37, 60)]);
    }

    #[test]
    fn request_parser_errors_reach_the_transport_hook_with_the_raw_recv_buffer() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mut handler = CaptureReceiveErrorHandler {
            errors: Arc::clone(&errors),
        };
        let mut state = ConnectionState::default();
        let recv_data = b"GET / HTTP/1.1\r\nBad Header: x\r\n\r\n";

        assert!(receive_with_handler(&mut state, &mut handler, recv_data).is_err());
        assert_eq!(
            errors.lock().unwrap().as_slice(),
            &[CapturedReceiveError {
                name: "HPE_INVALID_HEADER_TOKEN",
                description: "Invalid header token".into(),
                recv_data: recv_data.to_vec(),
                stage: ReceiveErrorStage::Request,
            }]
        );
    }

    #[test]
    fn event_buffer_overflow_reaches_the_transport_hook_before_close() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mut handler = CaptureReceiveErrorHandler {
            errors: Arc::clone(&errors),
        };
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /events HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        let upgrade = HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec());
        assert_eq!(
            state.response_written(Some(&upgrade)),
            ConnectionAction::Upgraded
        );

        let recv_data = [b'x'; SOURCE_READ_BUFFER_SIZE];
        for _ in 0..3 {
            assert_eq!(
                receive_with_handler(&mut state, &mut handler, &recv_data),
                Ok(Vec::new())
            );
        }
        assert_eq!(
            receive_with_handler(&mut state, &mut handler, &recv_data),
            Err(ParserFailure {
                name: "HPE_USER",
                description: "reverse HTTP event buffer limit exceeded".into(),
            })
        );
        assert_eq!(
            errors.lock().unwrap().as_slice(),
            &[CapturedReceiveError {
                name: "HPE_USER",
                description: "reverse HTTP event buffer limit exceeded".into(),
                recv_data: recv_data.to_vec(),
                stage: ReceiveErrorStage::ReverseEvent,
            }]
        );
        assert!(state.is_event_mode());
    }

    #[test]
    fn fragmented_request_becomes_ready_only_after_its_body_arrives() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"POST /play HTTP/1.1\r\nContent-Length: 4\r\n\r\nbo"),
            Ok(Vec::new())
        );
        assert_eq!(state.request().and_then(HttpRequest::method), None);
        assert_eq!(state.receive(b"dy"), Ok(vec![Incoming::RequestReady]));
        assert_eq!(state.request().and_then(HttpRequest::method), Some("POST"));
        assert_eq!(
            state.request().and_then(HttpRequest::body),
            Some(&b"body"[..])
        );
    }

    #[test]
    fn pri_http2_connection_preface_does_not_dispatch_as_an_http_request() {
        let mut state = ConnectionState::default();
        assert_eq!(state.receive(b"PRI * HTTP/2.0\r\n\r\n"), Ok(Vec::new()));
        assert_eq!(state.receive(b"SM\r\n"), Ok(Vec::new()));
        assert_eq!(state.receive(b"\r\n"), Ok(Vec::new()));
        assert!(state.request().is_none());
    }

    #[test]
    fn expect_continue_waits_for_the_declared_body_without_an_interim_reply() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"POST /upload HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\nbo"
            ),
            Ok(Vec::new())
        );
        assert!(state.request().is_none());
        assert_eq!(state.receive(b"dy"), Ok(vec![Incoming::RequestReady]));
        assert_eq!(
            state.request().and_then(HttpRequest::body),
            Some(&b"body"[..])
        );
    }

    #[test]
    fn coalesced_http_second_protocol_hits_user_error_before_dispatch() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"GET /one HTTP/1.1\r\nCSeq: 1\r\n\r\nGET /two HTTP/1.1\r\nCSeq: 2\r\n\r\n"
            ),
            Err(ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            })
        );
        assert!(state.request().is_none());
        assert!(state.is_closed());
    }

    #[test]
    fn coalesced_http_partial_method_dispatches_first_request_and_drops_tail() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /one HTTP/1.1\r\n\r\nG"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(state.request().and_then(HttpRequest::method), Some("GET"));
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/one"[..])
        );
        assert_eq!(state.response_written(None), ConnectionAction::KeepAlive);
        assert!(state.request().is_none());
    }

    #[test]
    fn coalesced_http_partial_target_appends_to_the_dispatched_url() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /one HTTP/1.1\r\n\r\nGET /two"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/one/two"[..])
        );
        assert_eq!(state.response_written(None), ConnectionAction::KeepAlive);
        assert!(state.request().is_none());
    }

    #[test]
    fn coalesced_complete_http_09_message_runs_callbacks_on_the_dispatched_request() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"GET /one HTTP/1.1\r\n\r\nPOST /two\r\nHost: x\r\nContent-Length: 3\r\n\r\nabc"
            ),
            Ok(vec![Incoming::RequestReady])
        );
        let request = state.request().unwrap();
        assert_eq!(request.method(), Some("POST"));
        assert_eq!(request.url(), Some(&b"/one/two"[..]));
        assert_eq!(request.protocol(), Some("HTTP/1.1"));
        assert_eq!(request.header(b"Host"), Some(&b"x"[..]));
        assert_eq!(request.body(), Some(&b"abc"[..]));
        assert!(!state.is_closed());
    }

    #[test]
    fn coalesced_protocol_prefix_fails_before_dispatch() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /one HTTP/1.1\r\n\r\nGET /two H"),
            Err(ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            })
        );
        assert!(state.request().is_none());
    }

    #[test]
    fn coalesced_rtsp_and_ice_method_errors_are_preempted_by_protocol_callback() {
        // The C on_protocol callback fails before llhttp's generated RTSP/ICE
        // method checks when this recv already completed an HTTP/1.1 request.
        for request in [
            &b"GET /one HTTP/1.1\r\n\r\nPATCH * RTSP/1.0\r\n\r\n"[..],
            &b"GET /one HTTP/1.1\r\n\r\nGET /two ICE/1.0\r\n\r\n"[..],
        ] {
            let mut state = ConnectionState::default();
            assert_eq!(
                state.receive(request),
                Err(ParserFailure {
                    name: "HPE_USER",
                    description: String::new(),
                })
            );
            assert!(state.request().is_none());
        }
    }

    #[test]
    fn non_keepalive_source_protocols_reject_same_read_tail_before_dispatch() {
        for request in [
            &b"GET /one HTTP/1.0\r\n\r\nGET /two HTTP/1.1\r\n\r\n"[..],
            &b"GET /one HTTP/1.1\r\nConnection: close\r\n\r\nGET /two HTTP/1.1\r\n\r\n"[..],
            &b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\nOPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\n"[..],
        ] {
            let mut state = ConnectionState::default();
            assert_eq!(
                state.receive(request),
                Err(ParserFailure {
                    name: "HPE_CLOSED_CONNECTION",
                    description: "Data after `Connection: close`".into(),
                })
            );
            assert!(state.request().is_none());
        }
    }

    #[test]
    fn http10_keepalive_still_reaches_the_second_protocol_callback() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"GET /one HTTP/1.0\r\nConnection: keep-alive\r\n\r\nGET /two HTTP/1.1\r\n\r\n"
            ),
            Err(ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            })
        );
        assert!(state.request().is_none());
    }

    #[test]
    fn connect_request_discards_declared_tunnel_bytes_from_the_current_read() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"CONNECT host:443 HTTP/1.1\r\nContent-Length: 4\r\n\r\nDATAGET /next HTTP/1.1\r\n\r\n"
            ),
            Ok(vec![Incoming::RequestReady])
        );
        let request = state.request().unwrap();
        assert_eq!(request.method(), Some("CONNECT"));
        assert_eq!(request.body(), None);
    }

    #[test]
    fn fragmented_rtsp_request_with_same_read_tail_fails_before_dispatch() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"ANNOUNCE /one RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 3\r\n\r\nx"),
            Ok(Vec::new())
        );
        assert_eq!(
            state.receive(
                b"yZOPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\nPOST /three HTTP/1.1\r\nCSeq: 3\r\nContent-Length: 2\r\n\r\nok"
            ),
            Err(ParserFailure {
                name: "HPE_CLOSED_CONNECTION",
                description: "Data after `Connection: close`".into(),
            })
        );
        assert!(state.request().is_none());
    }

    #[test]
    fn fragmented_http_body_completed_with_same_read_request_hits_user_error() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"POST /one HTTP/1.1\r\nContent-Length: 3\r\n\r\nx"),
            Ok(Vec::new())
        );
        assert_eq!(
            state.receive(b"yzGET /two HTTP/1.1\r\n\r\n"),
            Err(ParserFailure {
                name: "HPE_USER",
                description: String::new(),
            })
        );
        assert!(state.request().is_none());
    }

    #[test]
    fn separate_rtsp_receives_use_fresh_source_parser_instances() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(|request| request.header(b"CSeq")),
            Some(&b"1"[..])
        );
        assert_eq!(state.response_written(None), ConnectionAction::KeepAlive);
        assert_eq!(
            state.receive(b"OPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(HttpRequest::protocol),
            Some("RTSP/1.0")
        );
        assert_eq!(state.response_written(None), ConnectionAction::KeepAlive);
        assert!(state.request().is_none());
    }

    #[test]
    fn source_none_response_after_fragmented_request_keeps_connection_for_next_request() {
        let mut state = ConnectionState::default();
        assert_eq!(state.receive(b"GET /one HTTP/1.1\r\n"), Ok(Vec::new()));
        assert!(state.request().is_none());
        assert_eq!(state.receive(b"\r\n"), Ok(vec![Incoming::RequestReady]));
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/one"[..])
        );
        // `httpd_thread` destroys each completed non-EVENT request even when
        // `conn_request` returns no response, then continues on the same socket.
        assert_eq!(state.response_written(None), ConnectionAction::KeepAlive);
        assert_eq!(
            state.receive(b"GET /two HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/two"[..])
        );
    }

    #[test]
    fn connection_close_header_does_not_close_without_source_disconnect_flag() {
        let mut state = ConnectionState::default();
        state
            .receive(b"GET / HTTP/1.1\r\nConnection: keep-alive, Close\r\n\r\n")
            .unwrap();
        assert_eq!(
            state.response_written(Some(&HttpReply::new(
                b"HTTP/1.1 404 Not Found\r\n\r\n".to_vec()
            ))),
            ConnectionAction::KeepAlive
        );
        assert!(!state.is_closed());
        assert_eq!(
            state.receive(b"GET /next HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/next"[..])
        );
    }

    #[test]
    fn only_source_101_prefix_upgrades_and_keeps_event_request_state() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /events HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        let upgrade = HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec());
        assert_eq!(
            state.response_written(Some(&upgrade)),
            ConnectionAction::Upgraded
        );
        assert!(state.is_event_mode());
        assert_eq!(
            state.receive(b"EVENT /ctrl HTTP/1.1\r\nContent-Length: 4\r\n\r\nbo"),
            Ok(Vec::new())
        );
        assert_eq!(
            state.receive(b"dy"),
            Ok(vec![Incoming::Event(EventMessage {
                header: b"EVENT /ctrl HTTP/1.1\r\nContent-Length: 4\r\n\r\n".to_vec(),
                body: b"body".to_vec(),
            })])
        );
    }

    #[test]
    fn upgrade_request_discards_same_read_protocol_bytes_before_101() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(
                b"POST /reverse HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: PTTH/1.0\r\nContent-Length: 0\r\n\r\nEVENT /early HTTP/1.1\r\n\r\n"
            ),
            Ok(vec![Incoming::RequestReady])
        );
        assert_eq!(
            state.request().and_then(HttpRequest::url),
            Some(&b"/reverse"[..])
        );
        let reply = HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec());
        assert_eq!(
            state.response_written(Some(&reply)),
            ConnectionAction::Upgraded
        );
        assert_eq!(
            state.receive(b"EVENT /after HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::Event(EventMessage {
                header: b"EVENT /after HTTP/1.1\r\n\r\n".to_vec(),
                body: Vec::new(),
            })])
        );
    }

    #[test]
    fn disconnect_flag_closes_after_reply_and_101_disconnect_does_not_stay_upgraded() {
        let mut state = ConnectionState::default();
        assert_eq!(
            state.receive(b"GET /bye HTTP/1.1\r\n\r\n"),
            Ok(vec![Incoming::RequestReady])
        );
        let response = HttpReply::new(b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).disconnect_after_write();
        assert_eq!(
            state.response_written(Some(&response)),
            ConnectionAction::Close
        );
        assert!(state.is_closed());

        let mut upgrade_state = ConnectionState::default();
        let _ = upgrade_state.receive(b"GET /events HTTP/1.1\r\n\r\n");
        let upgrade = HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec())
            .disconnect_after_write();
        assert_eq!(
            upgrade_state.response_written(Some(&upgrade)),
            ConnectionAction::Close
        );
        assert!(upgrade_state.is_closed());
    }

    #[test]
    fn upgrade_101_callback_runs_before_disconnect_cleanup() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let events = Arc::new(Mutex::new(Vec::new()));
        let server_events = Arc::clone(&events);
        let server = thread::spawn(move || {
            serve(
                vec![listener],
                server_shutdown,
                ServerConfig::default(),
                move |_| {
                    Ok(CaptureUpgradeDisconnectHandler {
                        events: Arc::clone(&server_events),
                        class: ConnectionClass::Unknown,
                    })
                },
            )
        });

        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.write_all(b"POST /reverse HTTP/1.1\r\n\r\n").unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();

        assert_eq!(response, b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        assert_eq!(
            *events.lock().unwrap(),
            [
                ("upgraded", ConnectionClass::Event),
                ("disconnected", ConnectionClass::Event),
            ]
        );
        shutdown.store(true, Ordering::Release);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn upgrade_detection_uses_the_reference_status_prefix() {
        assert!(HttpReply::new(b"HTTP/1.1 101".to_vec()).upgrades_to_event());
        assert!(!HttpReply::new(b"HTTP/1.0 101 Switching Protocols".to_vec()).upgrades_to_event());
        assert!(!HttpReply::new(b"HTTP/1.1 200 OK".to_vec()).upgrades_to_event());
    }

    #[test]
    fn event_mode_delivers_multiple_frames_from_one_read() {
        let mut state = ConnectionState::default();
        let _ = state.receive(b"GET /events HTTP/1.1\r\n\r\n");
        let upgrade = HttpReply::new(b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec());
        assert_eq!(
            state.response_written(Some(&upgrade)),
            ConnectionAction::Upgraded
        );

        let events = state
            .receive(b"EVENT /one HTTP/1.1\r\n\r\nEVENT /two HTTP/1.1\r\n\r\n")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], Incoming::Event(event) if event.header.starts_with(b"EVENT /one"))
        );
        assert!(
            matches!(&events[1], Incoming::Event(event) if event.header.starts_with(b"EVENT /two"))
        );
    }
}
