//! Typed command-line and rc-file option parsing for the UxPlay 1.74 interface.
//!
//! This module deliberately owns parsing and configuration only. Runtime effects such as
//! starting a D-Bus inhibitor, checking GStreamer plugins, probing macOS AirPlay settings, or
//! opening dump files belong to the server layer.

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;

const VERSION: &str = "1.74";
const LOWEST_ALLOWED_PORT: u32 = 1024;
const HIGHEST_PORT: u32 = 65535;
const MAX_PROFILE_LEN: usize = 64;
const CUSTOM_HELP: &str = include_str!("../assets/custom-profile-help.txt");
const RPI_HELP: &str = include_str!("../assets/rpi-model-help.txt");
const HELP: &str = include_str!("../assets/help-short.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    Linux,
    MacOs,
    BsdOrUnix,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildCapabilities {
    pub host: HostPlatform,
    pub dbus: bool,
    pub apple_p2p: bool,
    pub default_srgb: bool,
}

impl Default for BuildCapabilities {
    fn default() -> Self {
        #[cfg(target_os = "windows")]
        let host = HostPlatform::Windows;
        #[cfg(target_os = "macos")]
        let host = HostPlatform::MacOs;
        #[cfg(target_os = "linux")]
        let host = HostPlatform::Linux;
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
        let host = HostPlatform::BsdOrUnix;
        #[cfg(not(any(target_os = "windows", unix)))]
        let host = HostPlatform::BsdOrUnix;
        let default_srgb = cfg!(all(unix, not(target_os = "macos")));
        Self {
            host,
            dbus: cfg!(all(
                unix,
                not(target_os = "macos"),
                feature = "dbus-screensaver"
            )),
            apple_p2p: cfg!(all(target_os = "macos", feature = "apple-p2p")),
            default_srgb,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoFlip {
    None,
    Horizontal,
    Vertical,
    Invert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoRotation {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaspberryPiModel {
    Pi3,
    Pi3ActiveCooling,
    Pi4,
    Pi4ActiveCooling,
    Pi5,
    Pi5ActiveCooling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritablePathKind {
    VideoDump,
    AudioDump,
    Mp4Recording,
    CoverArt,
    Metadata,
    BleBeacon,
    BleBeaconDefault,
    PairingRegistration,
    PairingKey,
    Dacp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritablePathCheck {
    pub path: String,
    pub kind: WritablePathKind,
}

impl WritablePathCheck {
    /// Original stderr text if the startup layer's write-access probe fails.
    pub fn failure_message(&self) -> String {
        let path = &self.path;
        match self.kind {
            WritablePathKind::VideoDump => format!(
                "{path} cannot be written to:\noption \"-vdmp <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::AudioDump => format!(
                "{path} cannot be written to:\noption \"-admp <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::Mp4Recording => format!(
                "{path} cannot be written to:\noption \"-mp4 <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::CoverArt => format!(
                "{path} cannot be written to:\noption \"-ca <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::Metadata => format!(
                "{path} cannot be written to:\noption \"-md <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::BleBeacon => format!(
                "{path} cannot be written to:\noption \"-ble<fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::BleBeaconDefault => format!("{path} cannot be written to\n"),
            WritablePathKind::PairingRegistration => format!(
                "{path} cannot be written to:\noption \"-reg <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::PairingKey => format!(
                "{path} cannot be written to:\noption \"-key <fn>\" must be to a file with write access\n"
            ),
            WritablePathKind::Dacp => format!(
                "{path} cannot be written to:\noption \"-dacp <fn>\" must be to a file with write access\n"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub server_name: String,
    pub server_name_is_utf8: bool,
    pub append_hostname: bool,
    pub allowed_clients: Vec<String>,
    pub blocked_clients: Vec<String>,
    pub restrict_clients: bool,
    pub raspberry_pi: Option<RaspberryPiModel>,
    pub raspberry_pi_hls_support: bool,
    pub custom_profile: Option<String>,

    pub audio_sync: bool,
    pub video_sync: bool,
    /// Nanosecond offset written to the ALAC sync field by `-async`.
    pub audio_delay_alac_ns: i64,
    /// Nanosecond offset written to the AAC sync field by `-vsync` (source behavior).
    pub audio_delay_aac_ns: i64,
    pub screensaver_inhibit: u32,

    pub display_width: Option<u16>,
    pub display_height: Option<u16>,
    pub display_rate: Option<u16>,
    pub max_fps: Option<u16>,
    pub overscanned: bool,
    pub video_flip: VideoFlip,
    pub video_rotation: VideoRotation,
    pub tcp_ports: [u16; 3],
    pub udp_ports: [u16; 3],

    pub mac_address: Option<String>,
    pub random_mac_address: bool,
    pub use_audio: bool,
    pub debug_log: bool,
    pub suppress_packet_debug_data: bool,
    pub video_parser: String,
    pub video_decoder: String,
    pub video_converter: String,
    pub video_sink: String,
    pub video_sink_options: String,
    pub audio_sink: String,
    pub close_window_on_client_stop: bool,
    pub fullscreen: bool,
    pub show_client_fps: bool,
    pub missed_feedback_limit_seconds: u32,
    pub video_rtp_pipeline: Option<String>,
    pub audio_rtp_pipeline: Option<String>,
    /// Paths the source probes for writability during CLI parsing. The caller performs the
    /// filesystem check so parsing itself has no create/delete side effects.
    pub writable_paths: Vec<WritablePathCheck>,

    pub dump_video: bool,
    pub video_dump_limit: Option<u32>,
    pub video_dump_filename: String,
    pub mux_to_file: bool,
    pub mux_filename: String,
    pub dump_audio: bool,
    pub audio_dump_limit: Option<u32>,
    pub audio_dump_filename: String,
    pub render_cover_art: bool,
    pub cover_art_filename: Option<String>,
    pub metadata_filename: Option<String>,
    pub ble_filename: Option<String>,

    pub bt709_fix: bool,
    pub srgb_fix: bool,
    pub nohold: bool,
    pub nofreeze: bool,
    /// Microseconds; `None` retains the library default.
    pub audio_latency_us: Option<i32>,

    pub legacy_pairing: bool,
    pub peer_to_peer: bool,
    /// 0=no control, 1=displayed PIN, 2=fixed password, 3=random password.
    pub pin_password_mode: u8,
    pub pin: Option<u16>,
    pub password: Option<String>,
    pub key_file: Option<String>,
    pub registration_list: bool,
    pub registration_file: Option<String>,
    pub dacp_file: Option<String>,

    pub taper_volume: bool,
    pub db_range: (f64, f64),
    pub initial_volume_attenuation_db: f64,
    pub hls_support: bool,
    pub playbin_version: u32,
    pub requested_language: String,
    pub subtitle_language: String,
    pub h265_support: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self::with_capabilities(BuildCapabilities::default())
    }
}

impl Options {
    pub fn with_capabilities(capabilities: BuildCapabilities) -> Self {
        Self {
            server_name: "UxPlay".into(),
            server_name_is_utf8: false,
            append_hostname: true,
            allowed_clients: Vec::new(),
            blocked_clients: Vec::new(),
            restrict_clients: false,
            raspberry_pi: None,
            raspberry_pi_hls_support: false,
            custom_profile: None,
            audio_sync: false,
            video_sync: true,
            audio_delay_alac_ns: 0,
            audio_delay_aac_ns: 0,
            screensaver_inhibit: 0,
            display_width: None,
            display_height: None,
            display_rate: None,
            max_fps: None,
            overscanned: false,
            video_flip: VideoFlip::None,
            video_rotation: VideoRotation::None,
            tcp_ports: [0; 3],
            udp_ports: [0; 3],
            mac_address: None,
            random_mac_address: false,
            use_audio: true,
            debug_log: false,
            suppress_packet_debug_data: false,
            video_parser: "h264parse".into(),
            video_decoder: "decodebin".into(),
            video_converter: "videoconvert".into(),
            video_sink: "autovideosink".into(),
            video_sink_options: String::new(),
            audio_sink: "autoaudiosink".into(),
            close_window_on_client_stop: capabilities.host != HostPlatform::MacOs,
            fullscreen: false,
            show_client_fps: false,
            missed_feedback_limit_seconds: 15,
            video_rtp_pipeline: None,
            audio_rtp_pipeline: None,
            writable_paths: Vec::new(),
            dump_video: false,
            video_dump_limit: None,
            video_dump_filename: "videodump".into(),
            mux_to_file: false,
            mux_filename: "recording".into(),
            dump_audio: false,
            audio_dump_limit: None,
            audio_dump_filename: "audiodump".into(),
            render_cover_art: false,
            cover_art_filename: None,
            metadata_filename: None,
            ble_filename: None,
            bt709_fix: false,
            srgb_fix: capabilities.default_srgb,
            nohold: false,
            nofreeze: false,
            audio_latency_us: None,
            legacy_pairing: false,
            peer_to_peer: false,
            pin_password_mode: 0,
            pin: None,
            password: None,
            key_file: None,
            registration_list: false,
            registration_file: None,
            dacp_file: None,
            taper_volume: false,
            db_range: (-30.0, 0.0),
            initial_volume_attenuation_db: 0.0,
            hls_support: false,
            playbin_version: 3,
            requested_language: String::new(),
            subtitle_language: String::new(),
            h265_support: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliExit {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParseSuccess {
    pub options: Options,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "ParseOutcome is a process-startup value; boxing Options would add heap allocation per invocation"
)]
pub enum ParseOutcome {
    Ready(ParseSuccess),
    Exit(CliExit),
}

/// Parse CLI arguments (without argv[0]), applying them over the option defaults.
pub fn parse_arguments(args: &[String], capabilities: BuildCapabilities) -> ParseOutcome {
    parse_with_prefix(args, capabilities, String::new(), "uxplay")
}

/// Parse CLI arguments with the executable name used in help output.
pub fn parse_arguments_named(
    args: &[String],
    capabilities: BuildCapabilities,
    binary_name: &str,
) -> ParseOutcome {
    parse_with_prefix(args, capabilities, String::new(), binary_name)
}

/// Parse an optional rc file before CLI arguments, matching UxPlay's precedence.
/// `rc_file` is `(displayed path, contents)`; an unreadable file is represented by `None` and
/// should be handled by the startup layer, which owns file lookup and open errors.
pub fn parse_with_rc(
    rc_file: Option<(&str, &str)>,
    cli_args: &[String],
    capabilities: BuildCapabilities,
) -> ParseOutcome {
    parse_with_rc_named(rc_file, cli_args, capabilities, "uxplay")
}

/// As `parse_with_rc`, with the executable name used by the help text.
pub fn parse_with_rc_named(
    rc_file: Option<(&str, &str)>,
    cli_args: &[String],
    capabilities: BuildCapabilities,
    binary_name: &str,
) -> ParseOutcome {
    parse_with_rc_named_and_path_check(rc_file, cli_args, capabilities, binary_name, |_| true)
}

/// Parse rc and CLI options in source order, asking the caller to validate each output path
/// immediately when the corresponding option is encountered.
pub fn parse_with_rc_named_and_path_check<F>(
    rc_file: Option<(&str, &str)>,
    cli_args: &[String],
    capabilities: BuildCapabilities,
    binary_name: &str,
    mut path_check: F,
) -> ParseOutcome
where
    F: FnMut(&WritablePathCheck) -> bool,
{
    let mut prefix = String::new();
    let mut args = Vec::new();
    if let Some((path, text)) = rc_file {
        prefix.push_str(&format!("UxPlay: reading configuration from  {path}\n"));
        args.extend(parse_rc_options(text));
    }
    args.extend_from_slice(cli_args);
    parse_with_prefix_and_path_check(&args, capabilities, prefix, binary_name, &mut path_check)
}

/// Convert rc-file lines into arguments. Quotes are stripped and a leading dash is added to the
/// first item on every non-comment line, following `read_config_file` in `uxplay.cpp`.
pub fn parse_rc_options(contents: &str) -> Vec<String> {
    let mut args = Vec::new();
    for line in contents.lines() {
        if line.starts_with('#') {
            continue;
        }
        let items = tokenize_rc_line(line);
        for (index, item) in items
            .into_iter()
            .filter(|item| !item.is_empty())
            .enumerate()
        {
            if index == 0 {
                args.push(format!("-{item}"));
            } else {
                args.push(item);
            }
        }
    }
    args
}

/// Parse native argv while preserving the original invalid-encoding early exit (status 0).
pub fn parse_os_arguments(args: &[OsString], capabilities: BuildCapabilities) -> ParseOutcome {
    parse_os_arguments_named(args, capabilities, "uxplay")
}

/// Parse native argv with the executable name used in help output.
pub fn parse_os_arguments_named(
    args: &[OsString],
    capabilities: BuildCapabilities,
    binary_name: &str,
) -> ParseOutcome {
    let mut converted = Vec::with_capacity(args.len());
    for arg in args {
        match arg.to_str() {
            Some(arg) => converted.push(arg.to_owned()),
            None => {
                return ParseOutcome::Exit(CliExit {
                    code: 0,
                    stdout: String::new(),
                    stderr: format!(
                        "Error: detected a non-ascii or non-UTF-8 string \"{}\"while parsing input arguments",
                        arg.to_string_lossy()
                    ),
                });
            }
        }
    }
    parse_arguments_named(&converted, capabilities, binary_name)
}

fn tokenize_rc_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut items = Vec::new();
    let mut current = String::new();
    let mut in_item = false;
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if !in_item {
            if ch == ' ' {
                index += 1;
                continue;
            }
            in_item = true;
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                index += 1;
                continue;
            }
            quote = Some(' ');
            current.push(ch);
            index += 1;
            continue;
        }

        if Some(ch) == quote {
            if quote != Some(' ') {
                if index > 0 && chars[index - 1] == '\\' {
                    current.push(ch);
                    index += 1;
                    continue;
                }
                if index + 1 < chars.len() && chars[index + 1] != ' ' {
                    current.push(ch);
                    index += 1;
                    continue;
                }
            }
            items.push(std::mem::take(&mut current));
            in_item = false;
            quote = None;
            index += 1;
            continue;
        }
        current.push(ch);
        index += 1;
    }
    if in_item && !current.is_empty() {
        items.push(current);
    }
    items
}

fn parse_with_prefix(
    args: &[String],
    capabilities: BuildCapabilities,
    prefix: String,
    binary_name: &str,
) -> ParseOutcome {
    let mut accept_path = |_: &WritablePathCheck| true;
    parse_with_prefix_and_path_check(args, capabilities, prefix, binary_name, &mut accept_path)
}

fn parse_with_prefix_and_path_check<F>(
    args: &[String],
    capabilities: BuildCapabilities,
    prefix: String,
    binary_name: &str,
    path_check: &mut F,
) -> ParseOutcome
where
    F: FnMut(&WritablePathCheck) -> bool,
{
    let mut options = Options::with_capabilities(capabilities);
    let mut notices = prefix;
    let stderr_notices = RefCell::new(String::new());
    let fail = |code: u8, stdout: String, stderr: String| {
        let mut combined = stderr_notices.borrow().clone();
        combined.push_str(&stderr);
        error(code, stdout, combined)
    };
    let fail_missing = |option: &str, stdout: String| {
        let mut stdout = stdout;
        stdout.push_str(&format!(
            "*** ERROR: invalid: \"{option}\" had no argument\n"
        ));
        error(1, stdout, stderr_notices.borrow().clone())
    };
    macro_rules! record_writable_path {
        ($path:expr, $kind:expr) => {{
            let check = WritablePathCheck {
                path: $path,
                kind: $kind,
            };
            if !path_check(&check) {
                return fail(1, notices, check.failure_message());
            }
            options.writable_paths.push(check);
        }};
    }
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let next = args.get(i + 1).map(String::as_str);
        match arg {
            "-rc" => i += usize::from(next.is_some()),
            "-allow" | "-block" => {
                let Some(value) = required_arg(arg, next, &notices) else {
                    return fail_missing(arg, notices);
                };
                if arg == "-allow" {
                    options.allowed_clients.push(value.into());
                } else {
                    options.blocked_clients.push(value.into());
                }
                i += 1;
            }
            "-restrict" => {
                if next == Some("no") {
                    options.restrict_clients = false;
                    i += 2;
                    continue;
                }
                options.restrict_clients = true;
            }
            "-n" => {
                let Some(value) = required_arg(arg, next, &notices) else {
                    return fail_missing(arg, notices);
                };
                options.server_name_is_utf8 = !printable_ascii(value);
                options.server_name = value.into();
                if options.server_name_is_utf8 {
                    notices.push_str(&format!("WARNING: a non-ascii (UTF-8) server-name \"{value}\" was specified: ensure correct locale settings to display it\n"));
                }
                i += 1;
            }
            "-nh" => options.append_hostname = false,
            "-rpi" => {
                let Some(value) = optional_non_option(next) else {
                    return fail(0, notices, format!("invalid: \"-rpi\" must be followed by a model code:\n{RPI_HELP}\n"));
                };
                let Some(code) = parse_uint(value, 0) else {
                    return fail(1, notices, format!("invalid \"-rpi {value}\"; -rpi must be followed by an integer model code:\n{RPI_HELP}\n"));
                };
                let model = match code {
                    3 => RaspberryPiModel::Pi3,
                    30 => RaspberryPiModel::Pi3ActiveCooling,
                    4 => RaspberryPiModel::Pi4,
                    40 => RaspberryPiModel::Pi4ActiveCooling,
                    5 => RaspberryPiModel::Pi5,
                    50 => RaspberryPiModel::Pi5ActiveCooling,
                    _ => return fail(1, notices, format!("invalid Raspberry Pi model code {value}:\n{RPI_HELP}\n")),
                };
                options.raspberry_pi = Some(model);
                options.raspberry_pi_hls_support = true;
                options.custom_profile = None;
                i += 1;
            }
            "-custom" => {
                let Some(value) = optional_non_option(next) else {
                    return fail(0, notices, format!("invalid \"-custom\" must be followed by s string encoding maximum HLS resolution choices:\n{CUSTOM_HELP}\n"));
                };
                let max_bytes = value.len().min(MAX_PROFILE_LEN);
                let Ok(truncated) = String::from_utf8(value.as_bytes()[..max_bytes].to_vec()) else {
                    return fail(1, notices, format!("*** invalid custom HLS profile string \"{value}\"\n\n{CUSTOM_HELP}\n"));
                };
                let mut profile: String = truncated.chars().filter(|c| !c.is_whitespace()).collect();
                let (valid, diagnostic) = validate_custom_profile(&profile);
                if !valid {
                    if let Some(diagnostic) = diagnostic {
                        notices.push_str(&format!("*** ERROR: {diagnostic}\n"));
                    }
                    return fail(1, notices, format!("*** invalid custom HLS profile string \"{value}\"\n\n{CUSTOM_HELP}\n"));
                }
                notices.push_str(&format!("custom HLS profile string is valid, stored as: \"{profile}\"\n"));
                options.custom_profile = Some(std::mem::take(&mut profile));
                i += 1;
            }
            "-async" | "-vsync" => {
                let audio = arg == "-async";
                if audio { options.audio_sync = true; } else { options.video_sync = true; }
                if next == Some("no") {
                    if audio { options.audio_sync = false; } else { options.video_sync = false; }
                    i += 2;
                    continue;
                }
                if let Some(value) = next.and_then(parse_c_f32) {
                    let millis = (value * 1000.0) as i32;
                    i += 1;
                    if !(-1_000_000..1_000_000).contains(&millis) {
                        let option = if audio { "-async" } else { "-vsync" };
                        return fail(1, notices, format!("invalid {option} {}: requested delays must be smaller than +/- 1000 millisecs\n", next.unwrap_or("")));
                    }
                    if audio { options.audio_delay_alac_ns = i64::from(millis) * 1000; }
                    else { options.audio_delay_aac_ns = i64::from(millis) * 1000; }
                }
            }
            "-scrsv" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                let Some(n) = parse_uint(value, 0).filter(|n| *n <= 2) else {
                    return fail(1, notices, format!("invalid \"-scrsv {value}\"; values 0, 1, 2 allowed\n"));
                };
                if !capabilities.dbus {
                    return fail(1, notices, "invalid: option \"-scrsv\" is currently only implemented for Linux/*BSD systems with D-Bus service\n".into());
                }
                options.screensaver_inhibit = n;
                i += 1;
            }
            "-s" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                if !parse_display(value, &mut options) {
                    return fail(1, notices, format!("invalid \"-s {value}\"; -s wxh : max w,h=9999; -s wxh@r : max r=255\n"));
                }
                i += 1;
            }
            "-fps" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                let Some(n) = parse_uint(value, 255).filter(|n| *n != 0) else {
                    return fail(1, notices, format!("invalid \"-fps {value}\"; -fps n : max n=255, default n=30\n"));
                };
                options.max_fps = Some(n as u16);
                i += 1;
            }
            "-o" => options.overscanned = true,
            "-f" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                options.video_flip = match value { "H" => VideoFlip::Horizontal, "V" => VideoFlip::Vertical, "I" => VideoFlip::Invert, _ => return fail(1, notices, format!("invalid \"-f {value}\" , unknown flip type, choices are H, V, I\n")) };
                i += 1;
            }
            "-r" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                options.video_rotation = match value { "R" => VideoRotation::Right, "L" => VideoRotation::Left, _ => return fail(1, notices, format!("invalid \"-r {value}\" , unknown rotation  type, choices are R, L\n")) };
                i += 1;
            }
            "-p" => {
                let Some(value) = optional_non_option(next) else {
                    options.tcp_ports = [7100, 7000, 7001];
                    options.udp_ports = [7011, 6001, 6000];
                    i += 1;
                    continue;
                };
                i += 1;
                if value == "tcp" || value == "udp" {
                    let Some(ports_value) = args.get(i + 1).filter(|v| !v.starts_with('-')).map(String::as_str) else {
                        return fail(1, notices, format!("invalid \"-p {value}\": missing second argument (ports must be specified after \"{value}\") \n"));
                    };
                    let option = format!("-p {value}");
                    let Some(ports) = parse_ports(ports_value) else {
                        return fail(1, format!("{notices}*** ERROR: invalid \"{option} {ports_value}\", all 3 distinct ports must be in range [1024,65535]\n"), String::new());
                    };
                    if value == "tcp" { options.tcp_ports = ports; } else { options.udp_ports = ports; }
                    i += 1;
                } else {
                    let Some(ports) = parse_ports(value) else {
                        return fail(1, format!("{notices}*** ERROR: invalid \"-p {value}\", all 3 distinct ports must be in range [1024,65535]\n"), String::new());
                    };
                    options.tcp_ports = ports;
                    options.udp_ports = ports;
                }
            }
            "-m" => {
                if let Some(value) = optional_non_option(next) {
                    if !valid_mac(value) {
                        return fail(1, notices, format!("invalid mac address \"{value}\": address must have form \"xx:xx:xx:xx:xx:xx\", x = 0-9, A-F or a-f\n"));
                    }
                    options.mac_address = Some(value.into());
                    options.random_mac_address = false;
                    i += 1;
                } else {
                    options.random_mac_address = true;
                }
            }
            "-a" => options.use_audio = false,
            "-d" => {
                if let Some(value) = optional_non_option(next) {
                    if parse_uint(value, 1) != Some(1) {
                        return fail(1, notices, format!("invalid \"-d {value}\"; -d n : max n=1 (suppress packet data in debug output)\n"));
                    }
                    options.debug_log = true;
                    options.suppress_packet_debug_data = true;
                    i += 1;
                } else {
                    options.debug_log = !options.debug_log;
                    options.suppress_packet_debug_data = false;
                }
            }
            "-h" | "--help" | "-?" | "-help" => {
                return ParseOutcome::Exit(CliExit { code: 0, stdout: format!("{}{}", notices, format_help(binary_name)), stderr: stderr_notices.borrow().clone() });
            }
            "-v" => return ParseOutcome::Exit(CliExit { code: 0, stdout: format!("{}UxPlay version {VERSION}; for help, use option \"-h\"\n", notices), stderr: stderr_notices.borrow().clone() }),
            "-vp" | "-vd" | "-vc" | "-vs" | "-as" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                match arg {
                    "-vp" => options.video_parser = value.into(),
                    "-vd" => options.video_decoder = value.into(),
                    "-vc" => options.video_converter = value.into(),
                    "-vs" => {
                        options.video_sink = value.into();
                        if let Some(pos) = options.video_sink.find(' ') {
                            options.video_sink_options = options.video_sink[pos..].to_owned();
                            options.video_sink.truncate(pos);
                        }
                    }
                    _ => options.audio_sink = value.into(),
                }
                i += 1;
            }
            "-t" => return fail(1, notices, "The uxplay option \"-t\" has been removed: it was a workaround for an  Avahi issue.\nThe correct solution is to open network port UDP 5353 in the firewall for mDNS queries\n".into()),
            "-nc" => {
                options.close_window_on_client_stop = false;
                if next == Some("no") {
                    options.close_window_on_client_stop = true;
                    i += 2;
                    continue;
                }
            }
            "-avdec" => { options.video_parser = "h264parse".into(); options.video_decoder = "avdec_h264".into(); options.video_converter = "videoconvert".into(); }
            "-v4l2" => { options.video_decoder = "v4l2h264dec".into(); options.video_converter = "v4l2convert".into(); }
            "-fs" => options.fullscreen = true,
            "-FPSdata" => options.show_client_fps = true,
            "-reset" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                stderr_notices.borrow_mut().push_str("*** NOTE CHANGE: -reset n now means reset n seconds (not 3n seconds) after client goes offline\n");
                let Some(n) = parse_uint(value, 0) else { return fail(1, notices, format!("invalid \"-reset {value}\"; -reset n must have n >= 0,  default n = 15 seconds\n")); };
                options.missed_feedback_limit_seconds = n;
                i += 1;
            }
            "-vrtp" | "-artp" => {
                let Some(value) = required_arg(arg, next, &notices) else {
                    let (what, example) = if arg == "-vrtp" { ("video", "<rtph26[4,5]pay options> ! udpsink host=127.0.0.1 port -= 5000") } else { ("audio", "<rtpL16pay options> ! udpsink host=127.0.0.1 port=5002") };
                    notices.push_str(&format!("*** ERROR: invalid: \"{arg}\" had no argument\n"));
                    return fail(1, notices, format!("option \"{arg}\" must be followed by a pipeline for sending the {what} stream:\ne.g., \"{example}\"\n"));
                };
                if arg == "-vrtp" { options.video_rtp_pipeline = Some(value.into()); } else { options.audio_rtp_pipeline = Some(value.into()); }
                i += 1;
            }
            "-vdmp" | "-admp" => {
                let video = arg == "-vdmp";
                if video { options.dump_video = true; } else { options.dump_audio = true; }
                if let Some(value) = optional_non_option(next) {
                    if let Some(n) = parse_uint(value, 0) {
                        if n == 0 { return fail(1, notices, format!("invalid \"{arg} 0 {value}\"; {arg} n  needs a non-zero value of n\n")); }
                        if video { options.video_dump_limit = Some(n); } else { options.audio_dump_limit = Some(n); }
                        i += 1;
                        if let Some(filename) = args.get(i + 1).filter(|s| !s.starts_with('-')) {
                            if video { options.video_dump_filename = filename.clone(); } else { options.audio_dump_filename = filename.clone(); }
                            i += 1;
                        }
                    } else {
                        if video { options.video_dump_filename = value.into(); } else { options.audio_dump_filename = value.into(); }
                        i += 1;
                    }
                    let (path, kind) = if video { (&options.video_dump_filename, WritablePathKind::VideoDump) } else { (&options.audio_dump_filename, WritablePathKind::AudioDump) };
                    record_writable_path!(path.clone(), kind);
                }
            }
            "-mp4" => {
                options.mux_to_file = true;
                if let Some(value) = optional_non_option(next) {
                    options.mux_filename = value.into();
                    record_writable_path!(options.mux_filename.clone(), WritablePathKind::Mp4Recording);
                    i += 1;
                }
            }
            "-ca" => {
                if let Some(value) = optional_non_option(next) {
                    options.cover_art_filename = Some(value.into());
                    options.render_cover_art = false;
                    record_writable_path!(value.into(), WritablePathKind::CoverArt);
                    i += 1;
                } else { options.render_cover_art = true; }
            }
            "-md" => {
                let Some(value) = required_arg(arg, next, &notices) else {
                    notices.push_str("*** ERROR: invalid: \"-md\" had no argument\n");
                    return fail(1, notices, "option -md must be followed by a filename for metadata text output\n".into());
                };
                options.metadata_filename = Some(value.into());
                record_writable_path!(value.into(), WritablePathKind::Metadata);
                i += 1;
            }
            "-ble" => {
                if let Some(value) = optional_non_option(next) {
                    options.ble_filename = (value != "off").then(|| value.to_owned());
                    if value != "off" { record_writable_path!(value.into(), WritablePathKind::BleBeacon); }
                    i += 1;
                } else {
                    let Some(home) = std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                    else { return fail(1, notices, "failed to obtain home directory\n".into()); };
                    options.ble_filename = Some(PathBuf::from(home).join(".uxplay.ble").to_string_lossy().into_owned());
                    if let Some(path) = options.ble_filename.as_ref() { record_writable_path!(path.clone(), WritablePathKind::BleBeaconDefault); }
                }
            }
            "-bt709" => options.bt709_fix = true,
            "-srgb" => {
                options.srgb_fix = true;
                if next == Some("no") { options.srgb_fix = false; i += 2; continue; }
            }
            "-nohold" => options.nohold = true,
            "-al" => {
                if let Some(value) = optional_non_option(next).and_then(parse_c_f32) {
                    let micros = (value * 1_000_000.0) as i32;
                    if (0..=10_000_000).contains(&micros) {
                        options.audio_latency_us = Some(micros);
                        i += 2;
                        continue;
                    }
                }
                let current = if optional_non_option(next).is_some() { next.unwrap_or(arg) } else { arg };
                return fail(1, notices, format!("invalid -al {current}: value must be a decimal time offset in seconds, range [0,10]\n(like 5 or 4.8, which will be converted to a whole number of microseconds)\n"));
            }
            "-pin" => {
                options.legacy_pairing = true;
                options.pin_password_mode = 1;
                if let Some(value) = optional_non_option(next) {
                    let Some(n) = parse_uint(value, 9999).filter(|n| *n != 0) else { return fail(1, notices, format!("invalid \"-pin {value}\"; -pin nnnn : nnnn range [0001:9999], (4 digits)\n")); };
                    options.pin = Some((n + 10000) as u16);
                    i += 1;
                }
            }
            "-p2p" => {
                if !capabilities.apple_p2p {
                    return fail(1, notices, "invalid: the \"-p2p\" option is only available on macOS hosts with UxPlay compiled to use Bonjour services\n".into());
                }
                options.peer_to_peer = true;
                options.legacy_pairing = true;
                options.pin_password_mode = 1;
            }
            "-reg" => {
                options.registration_list = true;
                options.registration_file = optional_non_option(next).map(str::to_owned);
                if let Some(path) = options.registration_file.as_ref() {
                    record_writable_path!(path.clone(), WritablePathKind::PairingRegistration);
                    i += 1;
                }
            }
            "-key" => {
                options.key_file = Some(optional_non_option(next).map_or_else(|| "0".into(), str::to_owned));
                if let Some(path) = optional_non_option(next) {
                    record_writable_path!(path.into(), WritablePathKind::PairingKey);
                    i += 1;
                }
            }
            "-pw" => {
                options.legacy_pairing = false;
                if let Some(value) = optional_non_option(next) {
                    if value.len() < 4 { return fail(1, notices, format!("invalid client-access password \"{value}\": length must be at least 4 characters\n")); }
                    options.password = Some(value.into());
                    options.pin_password_mode = 2;
                    i += 1;
                } else { options.pin_password_mode = 3; }
            }
            "-dacp" => {
                options.dacp_file = Some(optional_non_option(next).map_or_else(|| home_file(".uxplay.dacp"), str::to_owned));
                if let Some(path) = optional_non_option(next) {
                    record_writable_path!(path.into(), WritablePathKind::Dacp);
                    i += 1;
                }
            }
            "-taper" => options.taper_volume = true,
            "-db" => {
                let Some(value) = next else { return fail(1, notices, "invalid \"-db\": this option requires a value: db value must be \"low\" or \"low:high\", low < 0 and high > low are decibel gains\n".into()); };
                let initial_value = value.split_once(':').map_or(value, |(initial, _)| initial);
                if parse_c_double(initial_value).is_none() && value.starts_with('-') {
                    return fail(1, notices, "invalid \"-db\": this option requires a value: db value must be \"low\" or \"low:high\", low < 0 and high > low are decibel gains\n".into());
                }
                let Some((low, high)) = parse_db_range(value) else {
                    return fail(1, notices, format!("invalid \"-db {value}\": db value must be \"low\" or \"low:high\", low < 0 and high > low are decibel gains\n"));
                };
                notices.push_str(&format!("db range {low:.6}:{high:.6}\n"));
                options.db_range = (low, high);
                i += 1;
            }
            "-vol" => {
                let Some(value) = required_arg(arg, next, &notices) else { return fail_missing(arg, notices); };
                let Some(frac) = value.trim_start().parse::<f64>().ok().filter(|_| !value.trim_start().is_empty()) else { return fail(1, notices, format!("invalid \"-vol {value}\", value must be between 0.0 (mute) and 1.0 (full volume)\n")); };
                if !frac.is_finite() || !(0.0..=1.0).contains(&frac) || value.trim_start().chars().last().is_some_and(char::is_whitespace) {
                    return fail(1, notices, format!("invalid \"-vol {value}\", value must be between 0.0 (mute) and 1.0 (full volume)\n"));
                }
                options.initial_volume_attenuation_db = if frac == 0.0 { -144.0 } else if frac == 1.0 { 0.0 } else { -30.0 + 30.0 * frac };
                notices.push_str(&format!("initial_volume attenuation {:.6} db\n", options.initial_volume_attenuation_db));
                i += 1;
            }
            "-hls" => {
                options.hls_support = true;
                if let Some(value) = optional_non_option(next) {
                    let Some(n) = parse_uint(value, 3).filter(|n| *n >= 2) else { return fail(1, notices, format!("invalid \"-hls {value}\"; -hls n only allows \"playbin\" video player versions 2 or 3\n")); };
                    options.playbin_version = n;
                    i += 1;
                }
            }
            "-lang" | "-slang" => {
                let value = optional_non_option(next).unwrap_or("").replace(' ', "");
                if arg == "-lang" { options.requested_language = value; } else { options.subtitle_language = value; }
                if optional_non_option(next).is_some() { i += 1; }
            }
            "-h265" => options.h265_support = true,
            "-nofreeze" => options.nofreeze = true,
            _ => return fail(1, notices, format!("unknown option {arg}, stopping (for help use option \"-h\")\n")),
        }
        i += 1;
    }
    ParseOutcome::Ready(ParseSuccess {
        options,
        stdout: notices,
        stderr: stderr_notices.into_inner(),
    })
}

fn required_arg<'a>(_option: &str, next: Option<&'a str>, _stdout: &str) -> Option<&'a str> {
    next.filter(|value| !value.starts_with('-'))
}

fn optional_non_option(next: Option<&str>) -> Option<&str> {
    next.filter(|value| !value.starts_with('-'))
}

fn error(code: u8, stdout: String, stderr: String) -> ParseOutcome {
    ParseOutcome::Exit(CliExit {
        code,
        stdout,
        stderr,
    })
}

fn parse_uint(text: &str, maximum: u32) -> Option<u32> {
    if text.is_empty() || text.len() > 10 || text.starts_with('-') {
        return None;
    }
    let digits = text.trim_start_matches([' ', '\t', '\n', '\r', '\u{000b}', '\u{000c}']);
    let digits = digits.strip_prefix('+').unwrap_or(digits);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = digits.parse::<u64>().ok()?;
    if maximum != 0 && (number == 0 || number > maximum as u64) {
        return None;
    }
    Some(number as u32)
}

fn parse_c_f32(text: &str) -> Option<f32> {
    if text.is_empty() {
        return Some(0.0);
    }
    let text = text.trim_start_matches(char::is_whitespace);
    if text.is_empty() || text.trim_end_matches(char::is_whitespace).len() != text.len() {
        return None;
    }
    text.parse::<f32>().ok()
}

fn parse_display(text: &str, options: &mut Options) -> bool {
    let Some((width, rest)) = text.split_once('x') else {
        return false;
    };
    let (height, rate) = match rest.split_once('@') {
        Some((h, r)) => (h, Some(r)),
        None => (rest, None),
    };
    let Some(width) = parse_display_dimension(width) else {
        return false;
    };
    let Some(height) = parse_display_dimension(height) else {
        return false;
    };
    let parsed_rate = if let Some(rate) = rate {
        if rate.is_empty() || rate.len() > 3 || rate.starts_with('-') {
            return false;
        }
        let Some(rate) = parse_display_number(rate) else {
            return false;
        };
        if rate == 0 || rate > 255 {
            return false;
        }
        Some(rate as u16)
    } else {
        None
    };
    options.display_width = Some(width as u16);
    options.display_height = Some(height as u16);
    if parsed_rate.is_some() {
        options.display_rate = parsed_rate;
    }
    true
}

fn parse_display_dimension(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 4 || text.starts_with('-') {
        return None;
    }
    parse_display_number(text).filter(|n| *n != 0)
}

fn parse_display_number(text: &str) -> Option<u32> {
    let text = text.trim_start_matches([' ', '\t', '\n', '\r', '\u{000b}', '\u{000c}']);
    let text = text.strip_prefix('+').unwrap_or(text);
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_ports(text: &str) -> Option<[u16; 3]> {
    let parts: Vec<_> = text.split(',').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut ports = [0u16; 3];
    for (index, value) in parts.iter().enumerate() {
        if value.is_empty() || value.len() > 5 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let n = value.parse::<u32>().ok()?;
        if !(LOWEST_ALLOWED_PORT..=HIGHEST_PORT).contains(&n) {
            return None;
        }
        ports[index] = n as u16;
    }
    for index in parts.len()..3 {
        let n = u32::from(ports[index - 1]) + 1;
        if n > HIGHEST_PORT {
            return None;
        }
        ports[index] = n as u16;
    }
    for index in 0..ports.len() {
        if ports[index + 1..].contains(&ports[index]) {
            return None;
        }
    }
    Some(ports)
}

fn valid_mac(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 17
        && bytes.iter().enumerate().all(|(i, b)| {
            if [2, 5, 8, 11, 14].contains(&i) {
                *b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn validate_custom_profile(profile: &str) -> (bool, Option<String>) {
    for entry in profile.split(';') {
        let mut fields = entry.split(':');
        let Some(codec) = fields.next() else {
            return (false, None);
        };
        let Some(values) = fields.next() else {
            return (false, None);
        };
        if fields.next().is_some() {
            return (false, None);
        }
        if !["AVC", "HEVC", "VP9", "AV1"].contains(&codec) {
            return (false, Some(format!("{codec} is not a valid codec")));
        }
        let heights: Vec<_> = values.split(',').collect();
        if !(heights.len() == 1 || heights.len() == 2) {
            return (false, None);
        }
        let Some(h30) = parse_signed_decimal(heights[0]) else {
            return (false, None);
        };
        if h30 < 0 {
            return (false, None);
        }
        if heights.len() == 2 {
            let Some(h60) = parse_signed_decimal(heights[1]) else {
                return (false, None);
            };
            if h60 < 0 {
                return (false, None);
            }
            if h60 > h30 {
                return (
                    false,
                    Some(format!("{entry} : invalid heights, {h30} > {h60}")),
                );
            }
        }
    }
    (true, None)
}

fn parse_signed_decimal(text: &str) -> Option<i32> {
    if text.is_empty() || text.trim() != text {
        return None;
    }
    text.parse().ok()
}

fn parse_db_range(text: &str) -> Option<(f64, f64)> {
    let (low, high) = if let Some((low, high)) = text.split_once(':') {
        let low = parse_c_double(low)?;
        let high = parse_c_double(high)?;
        if low < 0.0 && high > low {
            (low, high)
        } else {
            return None;
        }
    } else {
        let low = parse_c_double(text)?;
        if low < 0.0 {
            (low, 0.0)
        } else {
            return None;
        }
    };
    Some((low, high))
}

fn parse_c_double(text: &str) -> Option<f64> {
    let trimmed = text.trim_start_matches(char::is_whitespace);
    if trimmed.is_empty() || trimmed.trim_end_matches(char::is_whitespace).len() != trimmed.len() {
        return None;
    }
    trimmed.parse().ok()
}

fn printable_ascii(text: &str) -> bool {
    text.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

fn home_file(filename: &str) -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(filename).to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.into())
}

fn format_help(binary: &str) -> String {
    // `help-short.txt` is the byte-for-byte printable text maintained for the front door.
    HELP.replace("@@UXPLAY_BIN@@", binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> ParseOutcome {
        let args: Vec<String> = words.iter().map(|word| (*word).into()).collect();
        parse_arguments(
            &args,
            BuildCapabilities {
                host: HostPlatform::Linux,
                dbus: true,
                apple_p2p: false,
                default_srgb: false,
            },
        )
    }

    fn ready(outcome: ParseOutcome) -> Options {
        match outcome {
            ParseOutcome::Ready(result) => result.options,
            ParseOutcome::Exit(exit) => panic!("unexpected exit: {exit:?}"),
        }
    }

    fn exited(outcome: ParseOutcome) -> CliExit {
        match outcome {
            ParseOutcome::Exit(exit) => exit,
            ParseOutcome::Ready(_) => panic!("expected exit"),
        }
    }

    #[test]
    fn output_path_failure_precedes_later_cli_error_in_rc_order() {
        let mut checked = Vec::new();
        let exit = exited(parse_with_rc_named_and_path_check(
            Some(("settings.rc", "mp4 missing.mp4\n")),
            &["-unknown".into()],
            BuildCapabilities::default(),
            "uxplay",
            |check| {
                checked.push((check.path.clone(), check.kind));
                false
            },
        ));

        assert_eq!(
            checked,
            [("missing.mp4".into(), WritablePathKind::Mp4Recording)]
        );
        assert_eq!(exit.code, 1);
        assert_eq!(
            exit.stdout,
            "UxPlay: reading configuration from  settings.rc\n"
        );
        assert_eq!(
            exit.stderr,
            "missing.mp4 cannot be written to:\noption \"-mp4 <fn>\" must be to a file with write access\n"
        );
    }

    #[test]
    fn defaults_and_cli_overrides_are_typed() {
        let options = ready(parse(&[
            "-n",
            "Living Room",
            "-nh",
            "-allow",
            "phone",
            "-restrict",
            "-s",
            "1920x1080@60",
            "-p",
            "7000,7001,7002",
            "-d",
            "1",
        ]));
        assert_eq!(options.server_name, "Living Room");
        assert!(!options.append_hostname);
        assert_eq!(options.allowed_clients, ["phone"]);
        assert!(options.restrict_clients);
        assert_eq!(
            (
                options.display_width,
                options.display_height,
                options.display_rate
            ),
            (Some(1920), Some(1080), Some(60))
        );
        assert_eq!(options.tcp_ports, [7000, 7001, 7002]);
        assert_eq!(options.udp_ports, options.tcp_ports);
        assert!(options.debug_log && options.suppress_packet_debug_data);
        assert_eq!(options.video_sink, "autovideosink");
    }

    #[test]
    fn default_srgb_fix_matches_source_unix_platform_condition() {
        let capabilities = BuildCapabilities::default();
        let expected = cfg!(all(unix, not(target_os = "macos")));
        assert_eq!(capabilities.default_srgb, expected);
        assert_eq!(Options::default().srgb_fix, expected);
    }

    #[test]
    fn nc_switches_and_cancels_source_window_close_behavior() {
        assert!(ready(parse(&[])).close_window_on_client_stop);
        assert!(!ready(parse(&["-nc"])).close_window_on_client_stop);
        assert!(ready(parse(&["-nc", "no"])).close_window_on_client_stop);
    }

    #[test]
    fn rc_quotes_and_cli_precedence_match_startup_behavior() {
        let rc = parse_rc_options(
            "# ignored\nname \"Living Room\"\nvs \"ximagesink sync=false\"\nlang en, fr\n",
        );
        assert_eq!(
            rc,
            [
                "-name",
                "Living Room",
                "-vs",
                "ximagesink sync=false",
                "-lang",
                "en,",
                "fr"
            ]
        );
        let outcome = parse_with_rc(
            Some(("sample.rc", "n \"Old Name\"\nnh\n")),
            &["-n".into(), "New Name".into()],
            BuildCapabilities::default(),
        );
        let options = ready(outcome);
        assert_eq!(options.server_name, "New Name");
        assert!(!options.append_hostname);
    }

    #[test]
    fn optional_argument_flags_do_not_consume_next_option() {
        let options = ready(parse(&[
            "-p", "-a", "-mp4", "-fs", "-pin", "-lang", "-h265",
        ]));
        assert_eq!(options.tcp_ports, [7100, 7000, 7001]);
        assert_eq!(options.udp_ports, [7011, 6001, 6000]);
        assert!(!options.use_audio);
        assert!(options.mux_to_file && options.fullscreen);
        assert_eq!(options.pin_password_mode, 1);
        assert!(options.requested_language.is_empty() && options.h265_support);
    }

    #[test]
    fn port_lists_reject_duplicates_in_explicit_and_generated_entries() {
        for (arguments, expected) in [
            (&["-p", "6000,6000"][..], "-p 6000,6000"),
            (&["-p", "tcp", "6000,6000"][..], "-p tcp 6000,6000"),
            (&["-p", "6001,6000"][..], "-p 6001,6000"),
            (&["-p", "+6000"][..], "-p +6000"),
            (&["-p", " 6000"][..], "-p  6000"),
        ] {
            let exit = exited(parse(arguments));
            assert_eq!(exit.code, 1);
            assert!(
                exit.stdout.contains(&format!(
                    "invalid \"{expected}\", all 3 distinct ports must be in range [1024,65535]"
                )),
                "stdout: {}\nstderr: {}",
                exit.stdout,
                exit.stderr,
            );
        }

        let options = ready(parse(&["-p", "6002,6000,6001"]));
        assert_eq!(options.tcp_ports, [6002, 6000, 6001]);
    }

    #[test]
    fn source_validation_messages_and_exit_codes_are_preserved() {
        let missing = exited(parse(&["-fps", "-a"]));
        assert_eq!(missing.code, 1);
        assert!(
            missing
                .stdout
                .contains("*** ERROR: invalid: \"-fps\" had no argument")
        );
        let invalid = exited(parse(&["-fps", "0"]));
        assert_eq!(invalid.code, 1);
        assert!(invalid.stderr.contains("default n=30"));
        let unsupported = exited(parse(&["-t"]));
        assert!(
            unsupported
                .stderr
                .contains("option \"-t\" has been removed")
        );
        assert_eq!(exited(parse(&["-rpi"])).code, 0);
    }

    #[test]
    fn rpi_and_custom_profile_options_follow_command_line_order() {
        let generated = ready(parse(&["-custom", "AVC:720", "-rpi", "3"]));
        assert!(generated.custom_profile.is_none());
        assert_eq!(generated.raspberry_pi, Some(RaspberryPiModel::Pi3));

        let custom = ready(parse(&["-rpi", "3", "-custom", "AVC:720"]));
        assert_eq!(custom.custom_profile.as_deref(), Some("AVC:720"));
        assert_eq!(custom.raspberry_pi, Some(RaspberryPiModel::Pi3));
    }

    #[test]
    fn delayed_sync_and_ranges_keep_source_semantics() {
        let outcome = parse(&[
            "-async", "-2.5", "-vsync", "no", "-reset", "0", "-db", "-24:-3", "-vol", "0.5",
            "-hls", "2", "-al", "4.8",
        ]);
        let result = match outcome {
            ParseOutcome::Ready(result) => result,
            ParseOutcome::Exit(exit) => panic!("unexpected exit: {exit:?}"),
        };
        let options = result.options;
        assert!(options.audio_sync);
        assert_eq!(options.audio_delay_alac_ns, -2_500_000);
        assert!(!options.video_sync);
        assert_eq!(options.missed_feedback_limit_seconds, 0);
        assert_eq!(options.db_range, (-24.0, -3.0));
        assert_eq!(options.initial_volume_attenuation_db, -15.0);
        assert_eq!(options.playbin_version, 2);
        assert_eq!(options.audio_latency_us, Some(4_800_000));
        assert!(
            result
                .stderr
                .contains("*** NOTE CHANGE: -reset n now means reset n seconds")
        );
    }

    #[test]
    fn platform_capabilities_gate_dbus_and_p2p() {
        let no_dbus = BuildCapabilities {
            host: HostPlatform::Linux,
            dbus: false,
            apple_p2p: false,
            default_srgb: false,
        };
        let args = vec!["-scrsv".into(), "1".into()];
        let exit = match parse_arguments(&args, no_dbus) {
            ParseOutcome::Exit(exit) => exit,
            _ => panic!("expected gated option error"),
        };
        assert!(exit.stderr.contains("only implemented for Linux/*BSD"));
        assert_eq!(exited(parse(&["-p2p"])).code, 1);
    }

    #[test]
    fn default_dbus_capability_matches_the_supported_build_feature() {
        assert_eq!(
            BuildCapabilities::default().dbus,
            cfg!(all(
                unix,
                not(target_os = "macos"),
                feature = "dbus-screensaver"
            ))
        );

        if BuildCapabilities::default().dbus {
            let options = ready(parse(&["-scrsv", "2"]));
            assert_eq!(options.screensaver_inhibit, 2);
        }
    }

    #[test]
    fn help_and_version_exit_without_mutating_runtime() {
        assert!(
            exited(parse(&["-v"]))
                .stdout
                .contains("UxPlay version 1.74")
        );
        assert!(exited(parse(&["-h"])).stdout.contains("Usage:"));
    }

    #[test]
    fn short_port_lists_expand_and_output_paths_remain_typed() {
        let outcome = parse(&[
            "-p",
            "6000",
            "-md",
            "metadata.txt",
            "-ca",
            "cover.png",
            "-mp4",
            "record.mp4",
        ]);
        let options = ready(outcome);
        assert_eq!(options.tcp_ports, [6000, 6001, 6002]);
        assert_eq!(options.udp_ports, options.tcp_ports);
        assert_eq!(options.writable_paths.len(), 3);
        assert!(
            options.writable_paths[0]
                .failure_message()
                .contains("option \"-md <fn>\"")
        );
        assert!(
            options.writable_paths[1]
                .failure_message()
                .contains("option \"-ca <fn>\"")
        );
        assert!(
            options.writable_paths[2]
                .failure_message()
                .contains("option \"-mp4 <fn>\"")
        );
    }

    #[test]
    fn rc_help_keeps_reading_message_and_requested_binary_name() {
        let result = parse_with_rc_named(
            Some(("sample.rc", "h\n")),
            &[],
            BuildCapabilities::default(),
            "custom-uxplay",
        );
        let exit = exited(result);
        assert!(
            exit.stdout
                .starts_with("UxPlay: reading configuration from  sample.rc\n")
        );
        assert!(exit.stdout.contains("Usage: custom-uxplay"));
    }
}
