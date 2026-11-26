//! Thread-safe replacement for the callback logger in `lib/logger.c`.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

pub const EMERG: i32 = 0;
pub const ALERT: i32 = 1;
pub const CRIT: i32 = 2;
pub const ERR: i32 = 3;
pub const WARNING: i32 = 4;
pub const NOTICE: i32 = 5;
pub const INFO: i32 = 6;
pub const DEBUG: i32 = 7;
pub const DEBUG_DATA: i32 = 8;

pub type LogCallback = Arc<dyn Fn(i32, &[u8]) + Send + Sync + 'static>;

#[derive(Default)]
struct CallbackState {
    callback: Option<LogCallback>,
}

/// The source keeps separate locks for its log threshold and callback. Keeping the
/// same lock split allows callback replacement while threshold reads remain short.
pub struct Logger {
    level: Mutex<i32>,
    callback: Mutex<CallbackState>,
}

impl Logger {
    pub fn new() -> Self {
        Self {
            level: Mutex::new(WARNING),
            callback: Mutex::new(CallbackState::default()),
        }
    }

    pub fn set_level(&self, level: i32) {
        *self
            .level
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = level;
    }

    pub fn level(&self) -> i32 {
        *self
            .level
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_callback(&self, callback: Option<LogCallback>) {
        self.callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .callback = callback;
    }

    /// Emit a preformatted message at the given syslog-style level.
    ///
    /// The byte-oriented callback preserves the source's C-string truncation model,
    /// including truncating a message at its first embedded NUL byte.
    pub fn log(&self, level: i32, message: impl fmt::Display) {
        self.log_bytes(level, message.to_string().as_bytes());
    }

    /// Emit an already formatted byte message without changing non-UTF-8 payloads.
    /// This matches C call sites that log protocol buffers through `%s`.
    pub fn log_bytes(&self, level: i32, message: &[u8]) {
        if level > self.level() {
            return;
        }

        let visible_len = message
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(message.len());
        let truncated = visible_len >= 4096;
        let message_len = visible_len;
        let message_end = visible_len.min(4095);
        let message_bytes = &message[..message_end];
        let callback_state = self
            .callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let callback = callback_state
            .callback
            .as_ref()
            .expect("logger callback must be installed before logger_log");
        callback(level, message_bytes);
        if truncated {
            let marker =
                format!("---logger message is truncated from {message_len} to 4095 chars---\n");
            callback(level, marker.as_bytes());
        }
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the callback that `uxplay.cpp::log_callback` uses for the live process logger.
pub fn stdout_logger(level: i32) -> Arc<Logger> {
    let logger = Arc::new(Logger::new());
    logger.set_level(level);
    logger.set_callback(Some(Arc::new(|level, message| {
        if let Some(line) = source_stdout_line(level, message) {
            write_source_stdout(&line);
        }
    })));
    logger
}

fn write_source_stdout(line: &[u8]) {
    #[cfg(windows)]
    {
        let mut translated = Vec::with_capacity(line.len() + 1);
        for &byte in line {
            if byte == b'\n' {
                translated.push(b'\r');
            }
            translated.push(byte);
        }
        let _ = io::stdout().lock().write_all(&translated);
    }
    #[cfg(not(windows))]
    {
        let _ = io::stdout().lock().write_all(line);
    }
}

#[cfg(windows)]
pub fn set_console_output_utf8() {
    use windows_sys::Win32::System::Console::SetConsoleOutputCP;

    // The C++ entry point switches the console after argument processing and before logging.
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

#[cfg(not(windows))]
pub fn set_console_output_utf8() {}

fn source_stdout_line(level: i32, message: &[u8]) -> Option<Vec<u8>> {
    let prefix: &[u8] = match level {
        EMERG..=ERR => b"*** ERROR: ",
        WARNING => b"*** WARNING: ",
        INFO | DEBUG => b"",
        _ => return None,
    };
    let mut line = Vec::with_capacity(prefix.len() + message.len() + 1);
    line.extend_from_slice(prefix);
    line.extend_from_slice(message);
    line.push(b'\n');
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn collecting_logger() -> (Logger, Arc<Mutex<Vec<Vec<u8>>>>) {
        let logger = Logger::new();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&messages);
        logger.set_callback(Some(Arc::new(move |_, message| {
            output.lock().unwrap().push(message.to_vec());
        })));
        (logger, messages)
    }

    #[test]
    fn console_callback_matches_source_prefixes_and_ignores_unhandled_levels() {
        assert_eq!(
            source_stdout_line(ERR, b"failed"),
            Some(b"*** ERROR: failed\n".to_vec())
        );
        assert_eq!(
            source_stdout_line(WARNING, b"careful"),
            Some(b"*** WARNING: careful\n".to_vec())
        );
        assert_eq!(
            source_stdout_line(INFO, b"ready"),
            Some(b"ready\n".to_vec())
        );
        assert_eq!(
            source_stdout_line(DEBUG, b"detail"),
            Some(b"detail\n".to_vec())
        );
        assert_eq!(source_stdout_line(DEBUG_DATA, b"packet"), None);
        assert_eq!(source_stdout_line(NOTICE, b"notice"), None);
    }

    #[test]
    fn defaults_to_warning_and_filters_higher_levels() {
        let (logger, messages) = collecting_logger();
        assert_eq!(logger.level(), WARNING);
        logger.log(INFO, "filtered");
        logger.log(ERR, "visible");
        assert_eq!(*messages.lock().unwrap(), [b"visible".to_vec()]);
    }

    #[test]
    fn changing_level_controls_future_messages() {
        let (logger, messages) = collecting_logger();
        logger.set_level(DEBUG_DATA);
        logger.log(DEBUG_DATA, "packet");
        assert_eq!(logger.level(), DEBUG_DATA);
        assert_eq!(*messages.lock().unwrap(), [b"packet".to_vec()]);
    }

    #[test]
    fn truncates_to_4095_bytes_then_emits_source_marker() {
        let (logger, messages) = collecting_logger();
        logger.log(ERR, "x".repeat(5000));
        let messages = messages.lock().unwrap();
        assert_eq!(messages[0].len(), 4095);
        assert_eq!(
            messages[1],
            b"---logger message is truncated from 5000 to 4095 chars---\n"
        );
    }

    #[test]
    fn callback_replacement_is_applied() {
        let logger = Logger::new();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&messages);
        logger.set_callback(Some(Arc::new(move |_, message| {
            output.lock().unwrap().push(message.to_vec());
        })));
        logger.log(WARNING, "first");
        logger.set_callback(None);
        logger.set_callback(Some(Arc::new(|_, _| {})));
        logger.log(WARNING, "second");
        assert_eq!(*messages.lock().unwrap(), [b"first".to_vec()]);
    }

    #[test]
    fn embedded_nul_uses_c_string_boundary() {
        let (logger, messages) = collecting_logger();
        logger.log(ERR, "before\0after");
        assert_eq!(*messages.lock().unwrap(), [b"before".to_vec()]);
    }

    #[test]
    fn byte_messages_preserve_non_utf8_bytes_and_c_string_boundary() {
        let (logger, messages) = collecting_logger();
        logger.log_bytes(ERR, b"before\xff\0after");
        assert_eq!(*messages.lock().unwrap(), [b"before\xff".to_vec()]);
    }

    #[test]
    fn callback_events_match_the_recorded_c_logger_oracle() {
        type Event = (String, i32, Vec<u8>);

        fn callback(tag: &'static str, events: Arc<Mutex<Vec<Event>>>) -> LogCallback {
            Arc::new(move |level, message| {
                events
                    .lock()
                    .unwrap()
                    .push((tag.to_owned(), level, message.to_vec()));
            })
        }

        fn fnv1a(bytes: &[u8]) -> u64 {
            bytes
                .iter()
                .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
                    (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
                })
        }

        let events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let logger = Logger::new();
        logger.set_callback(Some(callback("first", Arc::clone(&events))));
        logger.log(INFO, "filtered");
        logger.log(ERR, "visible:17");
        logger.log(DEBUG_DATA, "filtered-debug-data");
        logger.set_level(DEBUG_DATA);
        logger.log(DEBUG_DATA, "packet");
        logger.log(ERR, "before\0after");
        logger.log(ERR, format!("prefix\0{}", "z".repeat(5000)));
        logger.log(ERR, "x".repeat(5000));
        logger.log(
            ERR,
            format!("{}A{}", "\u{0e01}".repeat(1364), "\u{0e01}".repeat(3)),
        );
        logger.set_callback(Some(callback("replacement", Arc::clone(&events))));
        logger.log(ERR, "callback-replaced");

        let actual = events.lock().unwrap();
        let expected = include_str!("../../../tools/record-shims/logger_oracle.expected.txt");
        let expected_rows = expected
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'));
        let actual_rows = actual.iter();
        let mut expected_rows = expected_rows;
        let mut actual_rows = actual_rows;

        loop {
            match (expected_rows.next(), actual_rows.next()) {
                (None, None) => break,
                (Some(expected), Some((tag, level, message))) => {
                    let mut fields = expected.split_ascii_whitespace();
                    assert_eq!(fields.next(), Some(tag.as_str()));
                    assert_eq!(fields.next().unwrap().parse::<i32>().unwrap(), *level);
                    assert_eq!(
                        fields.next().unwrap().parse::<usize>().unwrap(),
                        message.len()
                    );
                    assert_eq!(
                        u64::from_str_radix(fields.next().unwrap(), 16).unwrap(),
                        fnv1a(message)
                    );
                    assert!(fields.next().is_none());
                }
                (Some(row), None) => {
                    panic!("Rust logger emitted too few events; next C row: {row}")
                }
                (None, Some(event)) => panic!("Rust logger emitted extra event: {event:?}"),
            }
        }
    }
}
