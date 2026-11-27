//! Shared reverse-HTTP channel state used by AirPlay FCUP requests.

use plist::Value;
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct AirPlayEventChannel {
    connections: AtomicUsize,
    writer: Mutex<Option<Arc<Mutex<TcpStream>>>>,
}

impl AirPlayEventChannel {
    /// Mark a connection as reverse HTTP and return the source's post-mark count.
    pub fn register(&self) -> usize {
        self.connections.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn unregister(&self) {
        let _ = self
            .connections
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
    }

    #[cfg(test)]
    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    pub fn install(&self, stream: TcpStream) {
        *lock(&self.writer) = Some(Arc::new(Mutex::new(stream)));
    }

    pub fn clear(&self) {
        if let Some(writer) = lock(&self.writer).take() {
            let _ = lock(&writer).shutdown(Shutdown::Both);
        }
    }

    #[allow(dead_code)] // Convenience API used by the protocol integration tests.
    pub fn send_fcup_request(
        &self,
        media_url: &str,
        client_session_id: &str,
        request_id: u64,
    ) -> io::Result<()> {
        self.send_fcup_request_with_logger(media_url, client_session_id, request_id, None)
    }

    pub fn send_fcup_request_with_logger(
        &self,
        media_url: &str,
        client_session_id: &str,
        request_id: u64,
        logger: Option<&crate::logger::Logger>,
    ) -> io::Result<()> {
        let request = fcup_request_bytes(media_url, client_session_id, request_id)?;
        let writer = lock(&self.writer).clone();
        let socket_id = writer
            .as_ref()
            .map(|writer| source_socket_id(&lock(writer)))
            .unwrap_or(-1);
        if let Some(logger) = logger {
            logger.log(
                crate::logger::DEBUG,
                format!("fcup_request send socket = {socket_id}"),
            );
        }
        let Some(writer) = writer else {
            return Err(invalid_socket_error());
        };
        // The C source performs one `send()` and treats any nonnegative byte
        // count as success. Preserve that partial-send behavior instead of
        // retrying the remainder as `write_all()` would.
        let sent = lock(&writer).write(&request)?;
        if let Some(logger) = logger {
            if logger.level() >= crate::logger::DEBUG {
                let mut readable = Vec::with_capacity(request.len() + 1);
                readable.push(b'\n');
                readable.extend(request.iter().map(
                    |byte| {
                        if *byte == b'\r' { b' ' } else { *byte }
                    },
                ));
                logger.log_bytes(crate::logger::DEBUG, &readable);
            }
            logger.log(
                crate::logger::DEBUG,
                format!(
                    "fcup_request: send sent Request of {sent} bytes from socket {socket_id}\n"
                ),
            );
        }
        Ok(())
    }
}

fn invalid_socket_error() -> io::Error {
    #[cfg(windows)]
    {
        // send(INVALID_SOCKET, ...) returns WSAENOTSOCK.
        io::Error::from_raw_os_error(10038)
    }
    #[cfg(unix)]
    {
        // send(-1, ...) returns EBADF.
        io::Error::from_raw_os_error(libc::EBADF)
    }
    #[cfg(not(any(windows, unix)))]
    {
        io::Error::new(io::ErrorKind::NotConnected, "invalid socket")
    }
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
    #[cfg(not(any(unix, windows)))]
    {
        let _ = stream;
        -1
    }
}

/// Build the source's XML-plist FCUP event wrapped in a reverse-HTTP request.
#[allow(dead_code)] // The wire encoder is also exercised directly by the oracle tests.
pub fn fcup_request_bytes(
    media_url: &str,
    client_session_id: &str,
    request_id: u64,
) -> io::Result<Vec<u8>> {
    let mut headers = plist::Dictionary::new();
    headers.insert(
        "X-Playback-Session-Id".to_owned(),
        Value::String(client_session_id.to_owned()),
    );
    headers.insert(
        "User-Agent".to_owned(),
        Value::String(
            "AppleCoreMedia/1.0.0.11B554a (Apple TV; U; CPU OS 7_0_4 like Mac OS X; en_us"
                .to_owned(),
        ),
    );

    let mut request = plist::Dictionary::new();
    request.insert(
        "FCUP_Response_ClientInfo".to_owned(),
        Value::Integer(1.into()),
    );
    request.insert(
        "FCUP_Response_ClientRef".to_owned(),
        Value::Integer(40_030_004.into()),
    );
    request.insert(
        "FCUP_Response_RequestID".to_owned(),
        Value::Integer(request_id.into()),
    );
    request.insert(
        "FCUP_Response_URL".to_owned(),
        Value::String(media_url.to_owned()),
    );
    request.insert("sessionID".to_owned(), Value::Integer(1.into()));
    request.insert(
        "FCUP_Response_Headers".to_owned(),
        Value::Dictionary(headers),
    );

    let mut root = plist::Dictionary::new();
    root.insert("sessionID".to_owned(), Value::Integer(1.into()));
    root.insert(
        "type".to_owned(),
        Value::String("unhandledURLRequest".to_owned()),
    );
    root.insert("request".to_owned(), Value::Dictionary(request));

    let mut body = Vec::new();
    Value::Dictionary(root)
        .to_writer_xml(&mut body)
        .map_err(|error| io::Error::other(format!("FCUP plist serialization failed: {error}")))?;
    // libplist's `plist_to_xml` includes a final newline in `datalen`.
    body.push(b'\n');

    let mut output = Vec::with_capacity(body.len() + 256);
    output.extend_from_slice(b"POST /event HTTP/1.1\r\n");
    output.extend_from_slice(b"X-Apple-Session-ID: ");
    output.extend_from_slice(client_session_id.as_bytes());
    output.extend_from_slice(b"\r\nContent-Type: text/x-apple-plist+xml\r\nContent-Length: ");
    output.extend_from_slice(body.len().to_string().as_bytes());
    output.extend_from_slice(b"\r\n\r\n");
    output.extend_from_slice(&body);
    Ok(output)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn reverse_connection_count_and_registration_follow_source() {
        let channel = AirPlayEventChannel::default();
        assert_eq!(channel.register(), 1);
        assert_eq!(channel.register(), 2);
        channel.unregister();
        assert_eq!(channel.connection_count(), 1);
        channel.unregister();
        channel.unregister();
        assert_eq!(channel.connection_count(), 0);
    }

    #[test]
    fn concurrent_reverse_disconnects_do_not_lose_connection_count_updates() {
        use std::sync::Barrier;
        use std::thread;

        const WORKERS: usize = 24;
        const ROUNDS: usize = 50;

        let channel = Arc::new(AirPlayEventChannel::default());
        let barrier = Arc::new(Barrier::new(WORKERS));
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                let channel = Arc::clone(&channel);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        channel.register();
                        barrier.wait();
                        channel.unregister();
                        barrier.wait();
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(channel.connection_count(), 0);
    }

    #[test]
    fn fcup_request_contains_xml_plist_and_source_http_headers() {
        let bytes =
            fcup_request_bytes("http://localhost:7000/master.m3u8", "session-1", 7).unwrap();
        assert_eq!(
            bytes,
            include_bytes!("../testdata/fcup_request.expected.bin"),
            "FCUP request wire bytes must match create_fcup_request() and http_response.c"
        );
        let head_end = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap();
        let headers = &bytes[..head_end];
        let body = &bytes[head_end + 4..];
        assert_eq!(
            body,
            include_bytes!("../testdata/fcup_request.expected.xml"),
            "FCUP XML must match the unchanged C create_fcup_request() output byte for byte"
        );
        assert!(headers.starts_with(b"POST /event HTTP/1.1\r\nX-Apple-Session-ID: "));
        assert!(
            headers
                .windows(b"X-Apple-Session-ID: session-1".len())
                .any(|part| part == b"X-Apple-Session-ID: session-1")
        );
        assert!(
            headers
                .windows(b"Content-Type: text/x-apple-plist+xml".len())
                .any(|part| part == b"Content-Type: text/x-apple-plist+xml")
        );
        let plist = Value::from_reader_xml(std::io::Cursor::new(body)).unwrap();
        let root = plist.as_dictionary().unwrap();
        assert_eq!(
            root.get("type").and_then(Value::as_string),
            Some("unhandledURLRequest")
        );
        let request = root.get("request").and_then(Value::as_dictionary).unwrap();
        assert_eq!(
            request.get("FCUP_Response_URL").and_then(Value::as_string),
            Some("http://localhost:7000/master.m3u8")
        );
        assert_eq!(
            request
                .get("FCUP_Response_RequestID")
                .and_then(Value::as_unsigned_integer),
            Some(7)
        );
    }

    #[test]
    fn sends_fcup_request_over_installed_reverse_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let channel = AirPlayEventChannel::default();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback_events = std::sync::Arc::clone(&events);
        let logger = crate::logger::Logger::new();
        logger.set_level(crate::logger::DEBUG);
        logger.set_callback(Some(std::sync::Arc::new(move |_, message| {
            callback_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message.to_vec());
        })));
        channel.install(server);
        channel
            .send_fcup_request_with_logger(
                "http://localhost/master.m3u8",
                "session-2",
                1,
                Some(&logger),
            )
            .unwrap();
        channel.clear();
        let mut client = client;
        let mut data = Vec::new();
        client.read_to_end(&mut data).unwrap();
        assert!(data.starts_with(b"POST /event HTTP/1.1\r\n"));
        assert!(
            data.windows(b"/master.m3u8".len())
                .any(|part| part == b"/master.m3u8")
        );
        let logs = events.lock().unwrap();
        assert!(logs[0].starts_with(b"fcup_request send socket = "));
        let mut readable = vec![b'\n'];
        readable.extend(
            data.iter()
                .map(|byte| if *byte == b'\r' { b' ' } else { *byte }),
        );
        assert_eq!(logs[1], readable);
        assert!(
            logs[2].starts_with(
                format!(
                    "fcup_request: send sent Request of {} bytes from socket ",
                    data.len()
                )
                .as_bytes()
            )
        );
    }
}
