#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged HTTP control effects; consumed by RAOP renderer callback integration"
    )
)]
mod airplay_controls;
mod airplay_event_channel;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged plist serializers; consumed by DNS-SD and renderer-backed HTTP routes"
    )
)]
mod airplay_info;
mod airplay_video;
#[cfg(any(test, all(any(unix, windows), feature = "external-dns-sd")))]
mod bonjour_backend;
mod cli;
mod compat;
mod compat_cli;
#[expect(
    dead_code,
    reason = "staged DNS-SD action executor; consumed by server startup wiring"
)]
mod discovery_publisher;
#[expect(
    dead_code,
    reason = "staged DNS-SD service state and publisher actions; consumed by the mDNS responder"
)]
mod discovery_services;
#[expect(
    dead_code,
    reason = "staged DNS-SD state subunit; consumed when Rust mDNS publication is integrated"
)]
mod discovery_state;
#[allow(dead_code)] // staged mDNS metadata unit; consumed by Rust service discovery
mod discovery_txt;
mod dmap_metadata;
#[expect(
    dead_code,
    reason = "staged host MAC selection; consumed by Rust server startup"
)]
mod host_identity;
#[allow(dead_code)] // staged HTTP request parsing unit; wired with the Rust server
mod http_event;
#[allow(dead_code)] // staged parser unit; wired into the server once listener orchestration lands
mod http_request;
#[expect(
    dead_code,
    reason = "staged HTTP transport subunit; consumed when AirPlay handlers are connected"
)]
mod http_transport;
#[allow(dead_code)] // staged server unit; installed when Rust protocol orchestration lands
mod logger;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged DNS-SD response construction and query routing; consumed by multicast sockets"
    )
)]
#[cfg_attr(test, allow(dead_code))]
mod mdns_responder;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged mDNS packet encoding and parsing; consumed by the DNS-SD responder"
    )
)]
mod mdns_wire;
#[allow(dead_code)] // staged server unit; consumed when the Rust listener orchestration lands
mod netutils;
mod ntp_filter;
#[allow(dead_code)] // staged NTP math unit; consumed by the Rust RAOP timing session
mod ntp_math;
pub mod options;
mod original;
mod pairing_registry;
mod password_auth;
mod peer_to_peer;
mod pin_display;
#[expect(
    dead_code,
    reason = "staged transport-to-protocol adapter; consumed by server startup and session integration"
)]
mod protocol_connection;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged RTSP dispatcher; wired into the server when session state is ready"
    )
)]
mod protocol_dispatch;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged RTSP/HTTP response assembly; consumed by protocol dispatch"
    )
)]
mod protocol_response;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged RAOP route classifier; consumed by protocol dispatch"
    )
)]
mod protocol_routes;
#[allow(dead_code)] // staged RAOP queue unit; consumed by the Rust RTP receiver
mod raop_buffer;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged binary-plist GET /info handler; consumed by DNS-SD and RAOP session state"
    )
)]
mod raop_info;
mod raop_media_runtime;
#[expect(
    dead_code,
    reason = "staged RAOP timing socket/session runtime; consumed by RTP session integration"
)]
mod raop_ntp_runtime;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "staged RTSP parameter parsing and effects; consumed by the RAOP session dispatcher"
    )
)]
mod raop_parameters;
#[allow(dead_code)] // staged RTP wire unit; consumed by the Rust RAOP packet buffer
mod rtp_wire;
mod server_runtime;
mod startup_validation;
mod units;

use options::{BuildCapabilities, ParseOutcome};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    original::report_fallbacks();
    let native_argv: Vec<OsString> = std::env::args_os().collect();
    if std::env::var_os("UXPLAY_REFERENCE").as_deref() == Some(OsStr::new("1")) {
        return original::run(&native_argv[1..]);
    }
    // The C entry point receives argv in the active ANSI code page on Windows.
    let argv = compat::c_runtime_arguments(&native_argv);
    if let Some(code) = compat_cli::run_explicit_rc_compat(&argv) {
        return code;
    }
    if let Some(error) = compat::c_runtime_argv_utf8_error(&native_argv) {
        write_stderr_bytes(&error);
        return ExitCode::SUCCESS;
    }
    if let Some(code) = compat_cli::run_compat(&argv) {
        return code;
    }
    if let Some((outcome, config_error)) = parse_startup_options(&argv) {
        if !config_error.is_empty() {
            write_stderr(&config_error);
        }
        match outcome {
            ParseOutcome::Exit(exit) => {
                write_stdout(&exit.stdout);
                write_stderr(&exit.stderr);
                return ExitCode::from(exit.code);
            }
            ParseOutcome::Ready(success) => {
                write_stdout(&success.stdout);
                write_stderr(&success.stderr);
                if success.options.peer_to_peer {
                    let startup = peer_to_peer::check_airplay_receiver_startup();
                    for line in startup.stdout_lines {
                        write_stdout(&format!("{line}\n"));
                    }
                    if startup.exit_code != 0 {
                        return ExitCode::from(startup.exit_code);
                    }
                }
                return server_runtime::run(success.options);
            }
        }
    }
    if let Ok(matches) = cli::command().try_get_matches_from(&argv) {
        let (command, sub) = matches.subcommand().unwrap_or((cli::ROOT, &matches));
        if let Some(code) = units::dispatch(command, sub) {
            return code;
        }
    }
    ExitCode::from(1)
}

/// Parses startup options and validates output paths before the Rust server starts.
fn parse_startup_options(argv: &[OsString]) -> Option<(ParseOutcome, String)> {
    let binary = argv
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or("uxplay");
    let cli_args = argv
        .iter()
        .skip(1)
        .map(|value| value.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let RcFileLoad {
        file: rc_file,
        error: config_error,
        invalid_argument: invalid_rc_argument,
    } = load_rc_file(argv);
    if let Some((path, error)) = invalid_rc_argument {
        write_stdout(&format!(
            "UxPlay: reading configuration from  {}\n",
            path.display()
        ));
        write_stderr_bytes(&error);
        return Some((
            ParseOutcome::Exit(options::CliExit {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }),
            String::new(),
        ));
    }
    let rc_ref = rc_file
        .as_ref()
        .map(|(path, contents)| (path.to_string_lossy().into_owned(), contents.clone()));
    let rc_ref = rc_ref
        .as_ref()
        .map(|(path, contents)| (path.as_str(), contents.as_str()));
    let outcome = options::parse_with_rc_named_and_path_check(
        rc_ref,
        &cli_args,
        BuildCapabilities::default(),
        binary,
        |check| startup_validation::path_has_write_access(&check.path),
    );
    Some((outcome, config_error))
}

struct RcFileLoad {
    file: Option<(PathBuf, String)>,
    error: String,
    invalid_argument: Option<(PathBuf, Vec<u8>)>,
}

fn load_rc_file(argv: &[OsString]) -> RcFileLoad {
    let args = &argv[1..];
    let explicit = args.iter().enumerate().find_map(|(index, value)| {
        (value == OsStr::new("-rc"))
            .then(|| args.get(index + 1).map(PathBuf::from))
            .flatten()
    });
    let Some(path) = explicit.or_else(default_rc_path) else {
        return RcFileLoad {
            file: None,
            error: String::new(),
            invalid_argument: None,
        };
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            if let Some(error) = invalid_rc_argument(&bytes) {
                return RcFileLoad {
                    file: None,
                    error: String::new(),
                    invalid_argument: Some((path, error)),
                };
            }
            let contents = rc_contents(&bytes);
            RcFileLoad {
                file: Some((path, contents)),
                error: String::new(),
                invalid_argument: None,
            }
        }
        Err(_) => RcFileLoad {
            file: None,
            error: format!(
                "UxPlay: failed to open configuration file at {}\n",
                path.display()
            ),
            invalid_argument: None,
        },
    }
}

fn invalid_rc_argument(contents: &[u8]) -> Option<Vec<u8>> {
    for line in contents.split(|byte| *byte == b'\n') {
        if line.first() == Some(&b'#') {
            continue;
        }
        for token in rc_line_arguments(line) {
            let token = token.split(|byte| *byte == 0).next().unwrap_or_default();
            if std::str::from_utf8(token).is_err() {
                let mut error = b"Error: detected a non-ascii or non-UTF-8 string \"".to_vec();
                error.extend_from_slice(token);
                error.extend_from_slice(b"\"while parsing input arguments");
                return Some(error);
            }
        }
    }
    None
}

fn rc_line_arguments(line: &[u8]) -> Vec<Vec<u8>> {
    let mut line = line.to_vec();
    let mut is_part_of_item = false;
    let mut in_quotes = false;
    let mut end_char = b' ';
    for index in 0..line.len() {
        if !is_part_of_item {
            if line[index] == b' ' {
                line[index] = 0;
            } else {
                is_part_of_item = true;
                match line[index] {
                    b'\'' | b'"' => {
                        end_char = line[index];
                        line[index] = 0;
                        in_quotes = true;
                    }
                    _ => {
                        end_char = b' ';
                        in_quotes = false;
                    }
                }
            }
        } else if line[index] == end_char {
            if in_quotes
                && index > 0
                && (line[index - 1] == b'\\' || (index + 1 < line.len() && line[index + 1] != b' '))
            {
                continue;
            }
            line[index] = 0;
            is_part_of_item = false;
        }
    }
    line.split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn rc_contents(bytes: &[u8]) -> String {
    if let Ok(contents) = std::str::from_utf8(bytes) {
        return contents.to_owned();
    }

    let mut valid = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.first() == Some(&b'#') {
            if line.last() == Some(&b'\n') {
                valid.push(b'\n');
            }
        } else {
            valid.extend_from_slice(line);
        }
    }
    String::from_utf8(valid).expect("non-UTF-8 configuration bytes must be in comments")
}

fn default_rc_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("UXPLAYRC").map(PathBuf::from)
        && path.exists()
    {
        return Some(path);
    }
    let home = std::env::var_os("XDG_CONFIG_HOMEDIR")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let legacy = home.join(".uxplayrc");
    if legacy.exists() {
        return Some(legacy);
    }
    let modern = home.join(".config").join("uxplayrc");
    modern.exists().then_some(modern)
}

fn write_stdout(text: &str) {
    write_stream(std::io::stdout(), text);
}

fn write_stderr(text: &str) {
    write_stream(std::io::stderr(), text);
}

fn write_stderr_bytes(bytes: &[u8]) {
    let _ = std::io::stderr().write_all(bytes);
}

fn write_stream(mut stream: impl Write, text: &str) {
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");
    #[cfg(not(windows))]
    let text = text.to_owned();
    let _ = stream.write_all(&compat::c_runtime_output(&text));
}
