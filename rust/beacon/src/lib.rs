//! CLI configuration and process handoff lifecycle for the standalone BLE beacon.

pub mod backend;
pub mod output;

use clap::{Parser, ValueEnum, error::ErrorKind};
use std::ffi::OsStr;
use std::fs;
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};
use uxplay_core::beacon::{BeaconFileData, BeaconFileError, decode_beacon_file};

pub(crate) const WINRT_STOPPED_PUBLISHER_ERROR: &str = "WinRtStoppedPublisherCallback";
pub(crate) const WINRT_STOPPED_PUBLISHER_INTERRUPT_ERROR: &str =
    "WinRtStoppedPublisherCallbackDuringInterrupt";
pub(crate) const BLEUIO_ZERO_PORT_ASSERTION_ERROR: &str = "BleuIoZeroPortAssertion";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum BleType {
    #[value(name = "BleuIO")]
    BleuIo,
    #[value(name = "HCI")]
    Hci,
    #[value(skip)]
    BlueZ,
    #[value(skip)]
    WinRt,
}

impl BleType {
    pub fn source_name(self) -> &'static str {
        match self {
            Self::BleuIo => "BleuIO",
            Self::Hci => "HCI",
            Self::BlueZ => "BlueZ",
            Self::WinRt => "winrt",
        }
    }
}

pub fn no_devices_message(ble_type: BleType) -> String {
    format!(
        "No devices  needed for BLE type {} were found",
        ble_type.source_name()
    )
}

#[derive(Debug, Parser)]
#[command(
    name = "uxplay-beacon",
    about = "A program that runs an AirPlay service discovery BLE beacon.",
    long_about = None,
    disable_version_flag = true,
    after_help = "Example: uxplay-beacon --ipv4 192.168.1.100 --advmax 200 --path ~/my_ble\n\nOptional arguments in the beacon startup file (if present) are processed first, and are overridden by command-line entries.\nFormat: one entry (key, value) (or just ble_type) per line; lines starting with # are ignored.\n  BleuIO\n  --ipv4 192.168.1.100"
)]
struct Args {
    #[arg(
        value_enum,
        help = "Alternative BLE implementation: BleuIO serial or direct HCI access"
    )]
    ble_type: Option<BleType>,
    #[arg(long, help = "Beacon startup file (default: ~/.uxplay.beacon)")]
    file: Option<PathBuf>,
    #[arg(
        long,
        help = "Path to the AirPlay server BLE beacon file (default: ~/.uxplay.ble)"
    )]
    path: Option<PathBuf>,
    #[arg(
        long,
        help = "IPv4 address of the AirPlay server (default: automatic detection)"
    )]
    ipv4: Option<String>,
    #[arg(long, help = "Minimum advertising interval in milliseconds (>= 100)")]
    advmin: Option<i64>,
    #[arg(
        long,
        help = "Maximum advertising interval in milliseconds (>= advmin, <= 10240)"
    )]
    advmax: Option<i64>,
    #[arg(long, help = "Adapter index used by BlueZ (default: 0)")]
    index: Option<i64>,
    #[arg(long, help = "Required BLE adapter or serial device address")]
    device: Option<String>,
    #[arg(long, hide = true)]
    exit_when_stopped: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BeaconOptions {
    pub ble_type: BleType,
    pub file: Option<PathBuf>,
    pub path: PathBuf,
    pub ipv4: Ipv4Addr,
    pub advmin: Option<i64>,
    pub advmax: Option<i64>,
    pub index: Option<i64>,
    pub device: Option<String>,
    /// Exit after an app-managed handoff is removed and advertising has stopped.
    pub exit_when_stopped: bool,
    pub notices: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum OptionsError {
    InvalidArgument(String),
    InvalidConfigLine(String),
    ConfigIo(String),
    InvalidIpv4(String),
    AddressUnavailable,
    ConfigExit {
        file: PathBuf,
        diagnostic: Option<String>,
        message: String,
        repeats_message: bool,
    },
    EmptyConfig(PathBuf),
}

impl std::fmt::Display for OptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgument(message)
            | Self::InvalidConfigLine(message)
            | Self::ConfigIo(message)
            | Self::InvalidIpv4(message) => f.write_str(message),
            Self::AddressUnavailable => f.write_str(
                "Failed to obtain Server IPv4 address with gethostbyname: provide it with option --ipv4",
            ),
            Self::ConfigExit { message, .. } => f.write_str(message),
            Self::EmptyConfig(_) => f.write_str(
                "UnboundLocalError: cannot access local variable 'err' where it is not associated with a value",
            ),
        }
    }
}

impl std::error::Error for OptionsError {}

#[derive(Default)]
struct ConfigValues {
    ble_type: Option<BleType>,
    path: Option<PathBuf>,
    ipv4: Option<Ipv4Addr>,
    advmin: Option<i64>,
    advmax: Option<i64>,
    index: Option<i64>,
    device: Option<String>,
}

/// Resolves command-line options, the optional startup file, and platform defaults.
pub fn parse_options_from<I, T>(arguments: I) -> Result<BeaconOptions, OptionsError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = Args::try_parse_from(arguments)
        .map_err(|error| OptionsError::InvalidArgument(error.to_string()))?;
    resolve_options(args, home_directory())
}

fn resolve_options(args: Args, home: PathBuf) -> Result<BeaconOptions, OptionsError> {
    let default_config = home.join(".uxplay.beacon");
    let config_file = if let Some(path) = args.file.as_ref() {
        if !path.is_file() {
            return Err(OptionsError::ConfigExit {
                file: path.clone(),
                diagnostic: None,
                message: format!(
                    "optional argument --file \"{}\" does not point to a valid file",
                    path.display()
                ),
                repeats_message: false,
            });
        }
        Some(path.clone())
    } else if default_config.is_file() {
        Some(default_config)
    } else {
        None
    };

    let mut config = ConfigValues::default();
    if let Some(path) = config_file.as_ref() {
        let contents = fs::read(path)
            .and_then(decode_config_bytes)
            .map_err(|error| {
                let message = match error.kind() {
                    std::io::ErrorKind::NotFound => {
                        format!("the configuration file {} was not found", path.display())
                    }
                    std::io::ErrorKind::PermissionDenied => format!(
                        "PermissionError when trying to read configuration file {}",
                        path.display()
                    ),
                    _ => format!("IOError when reading configuration file {}", path.display()),
                };
                OptionsError::ConfigExit {
                    file: path.clone(),
                    diagnostic: None,
                    message,
                    repeats_message: false,
                }
            })?;
        let normalized = contents.replace("\r\n", "\n").replace('\r', "\n");
        let mut last_error = None;
        for line_with_ending in normalized.split_inclusive('\n') {
            let line = line_with_ending
                .strip_suffix('\n')
                .unwrap_or(line_with_ending);
            if line.starts_with('#') {
                continue;
            }
            let error_message =
                format!("Invalid line \"{line_with_ending}\" in configuration file");
            last_error = Some(error_message.clone());
            let invalid = || OptionsError::ConfigExit {
                file: path.clone(),
                diagnostic: None,
                message: error_message.clone(),
                repeats_message: true,
            };
            let stripped = line.trim();
            let (key, value) = stripped
                .split_once(' ')
                .map_or((stripped, ""), |(key, value)| (key.trim(), value.trim()));
            if value.is_empty() {
                config.ble_type = Some(match key {
                    "BleuIO" => BleType::BleuIo,
                    "HCI" => BleType::Hci,
                    _ => return Err(invalid()),
                });
                continue;
            }
            match key {
                "--path" => config.path = Some(PathBuf::from(value)),
                "--ipv4" => {
                    config.ipv4 =
                        Some(parse_ipv4(value).map_err(|error| OptionsError::ConfigExit {
                            file: path.clone(),
                            diagnostic: Some(error.to_string()),
                            message: error_message.clone(),
                            repeats_message: true,
                        })?)
                }
                "--advmin" => config.advmin = Some(parse_config_integer(value, &invalid)?),
                "--advmax" => config.advmax = Some(parse_config_integer(value, &invalid)?),
                "--index" => config.index = Some(parse_config_integer(value, &invalid)?),
                "--device" => config.device = Some(value.to_owned()),
                _ => return Err(invalid()),
            }
        }
        let Some(message) = last_error else {
            return Err(OptionsError::EmptyConfig(path.clone()));
        };
        // The Python source calls exit(err) in a finally block, so even a valid
        // non-comment line terminates option parsing with this error.
        return Err(OptionsError::ConfigExit {
            file: path.clone(),
            diagnostic: None,
            message,
            repeats_message: false,
        });
    }

    let mut ipv4 = match args.ipv4 {
        Some(value) => Some(parse_ipv4(&value)?),
        None => config.ipv4,
    };
    let detected_type = args
        .ble_type
        .or(config.ble_type)
        .unwrap_or_else(default_ble_type);
    let mut advmin = args.advmin.or(config.advmin);
    let mut advmax = args.advmax.or(config.advmax);
    let mut index = args.index.or(config.index);
    let mut device = args.device.or(config.device);
    let mut notices = Vec::new();

    // The source's argparse path default is always non-null and therefore overrides a config path.
    let path = args.path.unwrap_or_else(|| home.join(".uxplay.ble"));
    let _ = config.path; // The source's non-null argparse default overwrites this config value.
    if ipv4.is_none() {
        ipv4 = detect_ipv4();
    }
    let ipv4 = ipv4.ok_or(OptionsError::AddressUnavailable)?;

    if detected_type == BleType::WinRt {
        if advmin.is_some() {
            notices.push(format!(
                " --advmin option is not used when ble_type = {}",
                detected_type.source_name()
            ));
        }
        if advmax.is_some() {
            notices.push(format!(
                " --advmax option is not used when ble_type = {}",
                detected_type.source_name()
            ));
        }
        advmin = None;
        advmax = None;
    } else {
        if advmin.is_none() {
            advmin = Some(100);
        }
        if advmax.is_none() {
            advmax = Some(100);
        }
    }
    if detected_type != BleType::BlueZ {
        if index.is_some() {
            notices.push(format!(
                " --index option is not used when ble_type = {}",
                detected_type.source_name()
            ));
        }
        index = None;
    } else if index.is_none() {
        index = Some(0);
    }
    if matches!(detected_type, BleType::BlueZ | BleType::WinRt) {
        if device.is_some() {
            notices.push(format!(
                " --device option is not used when ble_type = {}",
                detected_type.source_name()
            ));
        }
        device = None;
    }

    Ok(BeaconOptions {
        ble_type: detected_type,
        file: config_file,
        path,
        ipv4,
        advmin,
        advmax,
        index,
        device,
        exit_when_stopped: args.exit_when_stopped,
        notices,
    })
}

fn parse_config_integer(
    value: &str,
    invalid: &impl Fn() -> OptionsError,
) -> Result<i64, OptionsError> {
    if value.is_empty() {
        return Err(invalid());
    }
    value.chars().try_fold(0_i64, |number, character| {
        let digit = decimal_digit(character).ok_or_else(invalid)?;
        Ok(number.saturating_mul(10).saturating_add(i64::from(digit)))
    })
}

#[cfg(windows)]
fn decode_config_bytes(bytes: Vec<u8>) -> std::io::Result<String> {
    let code_page = windows_ansi_code_page()?;
    decode_windows_config_bytes(&bytes, code_page)
}

#[cfg(windows)]
fn windows_ansi_code_page() -> std::io::Result<u16> {
    let key = windows_registry::LOCAL_MACHINE
        .open("SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage")
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let code_page = key
        .get_string("ACP")
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    code_page.parse().map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid Windows ANSI code page {code_page}: {error}"),
        )
    })
}

#[cfg(windows)]
fn decode_windows_config_bytes(bytes: &[u8], code_page: u16) -> std::io::Result<String> {
    // Python's cp1252 codec rejects these five undefined slots, even though
    // Windows' MultiByteToWideChar maps them to C1 control characters.
    if code_page == 1252
        && bytes
            .iter()
            .any(|byte| matches!(byte, 0x81 | 0x8d | 0x8f | 0x90 | 0x9d))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "undefined byte in Windows-1252 input",
        ));
    }

    let mut decoded = String::with_capacity(bytes.len());
    for (index, segment) in bytes.split(|byte| *byte == 0).enumerate() {
        if index > 0 {
            decoded.push('\0');
        }
        let mut terminated = segment.to_vec();
        terminated.push(0);
        let text = windy::DAStr::from_bytes_with_nul(u32::from(code_page), &terminated)
            .and_then(|text| text.try_to_string())
            .map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
            })?;
        decoded.push_str(&text);
    }
    Ok(decoded)
}

#[cfg(not(windows))]
fn decode_config_bytes(bytes: Vec<u8>) -> std::io::Result<String> {
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn decimal_digit(character: char) -> Option<u8> {
    // Python's `int()` accepts Unicode Decimal_Number digits, not just ASCII.
    const DECIMAL_ZEROES: [u32; 68] = [
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
        0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60,
        0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0,
        0x1e950, 0x1fbf0,
    ];
    let codepoint = character as u32;
    DECIMAL_ZEROES.iter().find_map(|zero| {
        codepoint
            .checked_sub(*zero)
            .filter(|digit| *digit < 10)
            .map(|digit| digit as u8)
    })
}

fn parse_ipv4(value: &str) -> Result<Ipv4Addr, OptionsError> {
    value
        .parse()
        .map_err(|_| OptionsError::InvalidIpv4(format!("{value} is not a valid IPv4 address")))
}

fn default_ble_type() -> BleType {
    if cfg!(target_os = "windows") {
        BleType::WinRt
    } else if cfg!(target_os = "linux") {
        BleType::BlueZ
    } else {
        BleType::BleuIo
    }
}

fn reference_beacon_script() -> PathBuf {
    if let Some(root) = std::env::var_os("UXPLAY_REFERENCE_ROOT") {
        return PathBuf::from(root)
            .join("Bluetooth_LE_beacon")
            .join("uxplay-beacon.py");
    }

    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")))
        .join("UxPlay")
        .join("Bluetooth_LE_beacon")
        .join("uxplay-beacon.py")
}

#[cfg(test)]
pub(crate) fn canonicalize_reference_fixture(fixture: &str) -> String {
    const RECORDED_REFERENCE_ROOT: &str = r"D:\Save\Uxplay\UxPlay";
    let script = reference_beacon_script();
    let root = script
        .parent()
        .and_then(Path::parent)
        .expect("reference beacon script has a project root");
    let fixture = fixture.replace(RECORDED_REFERENCE_ROOT, &root.display().to_string());
    if cfg!(windows) {
        fixture
    } else {
        fixture.replace("\\Bluetooth_LE_beacon\\", "/Bluetooth_LE_beacon/")
    }
}

fn empty_config_traceback(script: &Path) -> String {
    let script = script.display();
    format!(
        "Traceback (most recent call last):\n  File \"{script}\", line 474, in <module>\n    [ble_type, path, ipv4_str, advmin, advmax, index, device_address] = parse_params()\n                                                                        ~~~~~~~~~~~~^^\n  File \"{script}\", line 404, in parse_params\n    exit(err)\n         ^^^\nUnboundLocalError: cannot access local variable 'err' where it is not associated with a value\n"
    )
}

fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn detect_ipv4() -> Option<Ipv4Addr> {
    if cfg!(target_os = "windows") {
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()?;
        return (hostname.as_str(), 0)
            .to_socket_addrs()
            .ok()?
            .find_map(|address| match address.ip() {
                std::net::IpAddr::V4(ip) => Some(ip),
                _ => None,
            });
    }

    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    if socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).is_ok()
        && let Ok(address) = socket.local_addr()
        && let std::net::IpAddr::V4(ip) = address.ip()
        && !ip.is_loopback()
    {
        return Some(ip);
    }
    let hostname = std::env::var("HOSTNAME").ok()?;
    (hostname.as_str(), 0)
        .to_socket_addrs()
        .ok()?
        .find_map(|address| match address.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
            _ => None,
        })
}

pub trait BeaconBackend {
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String>;
    fn stop(&mut self) -> Result<(), String>;

    /// Lets a backend preserve exception chaining when cleanup runs inside Ctrl+C handling.
    fn stop_after_interrupt(&mut self) -> Result<(), String> {
        self.stop()
    }

    /// Matches the source script's top-level `check_port()` global.
    fn check_port(&self, advertised_port: Option<u16>, candidate_port: u16) -> bool {
        advertised_port.is_none_or(|port| port == candidate_port)
    }

    /// The source performs a fake port-1 start/stop for native backends before
    /// printing its startup summary. BlueZ uses port 1 internally as a preflight
    /// sentinel but suppresses the source's `test passed` line; WinRT reports the
    /// successful port-1 probe to that line.
    fn preflight(&mut self, options: &BeaconOptions) -> Result<bool, String> {
        let advertised_port = self.start(options, 1)?;
        self.stop()?;
        Ok(advertised_port == 1)
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn bluez_port_one_start_result(is_preflight: bool) -> Result<u16, String> {
    if is_preflight {
        Ok(1)
    } else {
        Err("Failed to start beacon:\ngiving up, check Bluetooth adapter".to_owned())
    }
}

impl<T: BeaconBackend + ?Sized> BeaconBackend for Box<T> {
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String> {
        (**self).start(options, port)
    }

    fn stop(&mut self) -> Result<(), String> {
        (**self).stop()
    }

    fn stop_after_interrupt(&mut self) -> Result<(), String> {
        (**self).stop_after_interrupt()
    }
}

pub trait ProcessVerifier {
    fn pid_exists(&mut self, pid: u32) -> bool;
    fn matches(&mut self, pid: u32, process_name: &str) -> bool;
}

#[derive(Default)]
pub struct SystemProcessVerifier {
    system: System,
}

impl ProcessVerifier for SystemProcessVerifier {
    fn pid_exists(&mut self, pid: u32) -> bool {
        let pid = Pid::from_u32(pid);
        self.system
            .refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        self.system.process(pid).is_some()
    }

    fn matches(&mut self, pid: u32, expected_name: &str) -> bool {
        let pid = Pid::from_u32(pid);
        self.system
            .refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        self.system
            .process(pid)
            .is_some_and(|process| process.name().to_string_lossy().starts_with(expected_name))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MonitorState {
    pub running: bool,
    pub pending_on: bool,
    pub pending_off: bool,
    /// Mirrors `uxplay-beacon.py`'s module-global `advertised_port`. The assignment in
    /// `start_beacon()` is local, so this remains `None` after a successful start.
    pub advertised_port: Option<u16>,
    pub pending_port: Option<u16>,
}

impl MonitorState {
    pub fn tick<B, P>(
        &mut self,
        options: &BeaconOptions,
        backend: &mut B,
        verifier: &mut P,
    ) -> Result<(), String>
    where
        B: BeaconBackend,
        P: ProcessVerifier,
    {
        self.inspect_file(options, backend, verifier)?;
        if self.running {
            if self.pending_off {
                backend.stop()?;
                self.advertised_port = None;
                self.running = false;
                self.pending_off = false;
            }
        } else if self.pending_on {
            let port = self
                .pending_port
                .take()
                .ok_or_else(|| "handoff port was not recorded".to_owned())?;
            backend.start(options, port)?;
            self.running = true;
            self.pending_on = false;
        }
        Ok(())
    }

    fn inspect_file<B, P>(
        &mut self,
        options: &BeaconOptions,
        backend: &B,
        verifier: &mut P,
    ) -> Result<(), String>
    where
        B: BeaconBackend,
        P: ProcessVerifier,
    {
        if !options.path.is_file() {
            if self.running {
                self.pending_off = true;
            }
            return Ok(());
        }

        let bytes = fs::read(&options.path).map_err(|_| {
            "UnboundLocalError: cannot access local variable 'pid' where it is not associated with a value"
                .to_owned()
        })?;
        if bytes.len() < 2 {
            return Err("struct.error: unpack requires a buffer of 2 bytes".to_owned());
        }
        if bytes.len() < 6 {
            return Err("struct.error: unpack requires a buffer of 4 bytes".to_owned());
        }
        let data = decode_beacon_file(&bytes).expect("six-byte handoff header checked above");

        if let Some(process_name) = source_orphan_name(&data, verifier)? {
            self.remove_orphan(&options.path, data.pid, &process_name);
        } else if self.running {
            if !backend.check_port(self.advertised_port, data.raop_port) {
                self.pending_off = true;
            }
        } else {
            self.pending_on = true;
            self.pending_port = Some(data.raop_port);
        }
        Ok(())
    }

    fn remove_orphan(&mut self, path: &Path, pid: u32, process_name: &str) {
        crate::source_println!(
            "Orphan beacon file exists, but process pid {pid} ({process_name}) is no longer active"
        );
        match fs::remove_file(path) {
            Ok(()) => crate::source_println!(
                "Orphan beacon file \"{}\" deleted successfully.",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::source_println!("File \"{}\" not found.", path.display());
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                crate::source_println!(
                    "Permission Errror {error}: cannot delete  \"{}\".",
                    path.display()
                );
            }
            Err(_) => {}
        }
        self.pending_off |= self.running;
    }
}

fn source_orphan_name<P: ProcessVerifier>(
    data: &BeaconFileData,
    verifier: &mut P,
) -> Result<Option<String>, String> {
    if !verifier.pid_exists(data.pid) {
        return Ok(Some("process name unread".to_owned()));
    }
    let name = std::str::from_utf8(&data.process_name)
        .map_err(|error| python_unicode_decode_error(&data.process_name, error))?;
    let basename = Path::new(name)
        .file_name()
        .unwrap_or_else(|| OsStr::new(name))
        .to_string_lossy();
    if verifier.matches(data.pid, &basename) {
        Ok(None)
    } else {
        Ok(Some(name.to_owned()))
    }
}

pub(crate) fn python_unicode_decode_error(bytes: &[u8], error: std::str::Utf8Error) -> String {
    let position = error.valid_up_to();
    let invalid = bytes[position];
    let reason = if !matches!(invalid, 0xc2..=0xf4) {
        "invalid start byte"
    } else if error.error_len().is_none() {
        "unexpected end of data"
    } else {
        "invalid continuation byte"
    };
    match error.error_len() {
        None if bytes.len() - position > 1 => format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position {position}-{}: {reason}",
            bytes.len() - 1
        ),
        Some(length) if length > 1 => format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position {position}-{}: {reason}",
            position + length - 1
        ),
        _ => format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{invalid:02x} in position {position}: {reason}"
        ),
    }
}

/// Recreates the CPython traceback for exceptions raised while decoding the handoff file.
pub fn python_traceback_for_monitor_error(error: &str) -> Option<String> {
    let script_path = reference_beacon_script();
    let script = script_path.display();
    if error == WINRT_STOPPED_PUBLISHER_INTERRUPT_ERROR {
        let module = script_path
            .with_file_name("uxplay_beacon_module_winrt.py")
            .display()
            .to_string();
        return Some(format!(
            "Traceback (most recent call last):\n  File \"{script}\", line 196, in main\n    time.sleep(1)\n    ~~~~~~~~~~^^^\nKeyboardInterrupt\n\nDuring handling of the above exception, another exception occurred:\n\nTraceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 203, in main\n    stop_beacon()\n    ~~~~~~~~~~~^^\n  File \"{script}\", line 87, in stop_beacon\n    beacon_off()\n    ~~~~~~~~~~^^\n  File \"{module}\", line 119, in beacon_off\n    publisher.stop()\n    ^^^^^^^^^^^^^^\nAttributeError: 'NoneType' object has no attribute 'stop'\n"
        ));
    }
    if error == BLEUIO_ZERO_PORT_ASSERTION_ERROR {
        let module = script_path
            .with_file_name("uxplay_beacon_module_BleuIO.py")
            .display()
            .to_string();
        return Some(format!(
            "Traceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 119, in check_pending\n    start_beacon()\n    ~~~~~~~~~~~~^^\n  File \"{script}\", line 78, in start_beacon\n    setup_beacon(ipv4_str, port, advmin, advmax, index)\n    ~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{module}\", line 64, in setup_beacon\n    assert port > 0\n           ^^^^^^^^\nAssertionError\n"
        ));
    }
    if error == WINRT_STOPPED_PUBLISHER_ERROR {
        let module = script_path
            .with_file_name("uxplay_beacon_module_winrt.py")
            .display()
            .to_string();
        return Some(format!(
            "Traceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 115, in check_pending\n    stop_beacon()\n    ~~~~~~~~~~~^^\n  File \"{script}\", line 87, in stop_beacon\n    beacon_off()\n    ~~~~~~~~~~^^\n  File \"{module}\", line 119, in beacon_off\n    publisher.stop()\n    ^^^^^^^^^^^^^^\nAttributeError: 'NoneType' object has no attribute 'stop'\n"
        ));
    }
    let common = format!(
        "Traceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^\n  File \"{script}\", line 173, in on_timeout\n    check_file_exists(file_path)\n    ~~~~~~~~~~~~~~~~~^^^^^^^^^^\n"
    );
    let frame = if error == "struct.error: unpack requires a buffer of 2 bytes" {
        format!(
            "  File \"{script}\", line 133, in check_file_exists\n    port = struct.unpack('<H', data)[0]\n           ~~~~~~~~~~~~~^^^^^^^^^^^^\n{error}\n"
        )
    } else if error == "struct.error: unpack requires a buffer of 4 bytes" {
        format!(
            "  File \"{script}\", line 135, in check_file_exists\n    pid = struct.unpack('<I', data)[0]\n          ~~~~~~~~~~~~~^^^^^^^^^^^^\n{error}\n"
        )
    } else if error.starts_with("UnicodeDecodeError:") {
        format!(
            "  File \"{script}\", line 142, in check_file_exists\n    pname = data.split(b'\\0',1)[0].decode('utf-8')\n{error}\n"
        )
    } else if error.starts_with("UnboundLocalError: cannot access local variable 'pid'") {
        format!(
            "  File \"{script}\", line 157, in check_file_exists\n    print(f'Orphan beacon file exists, but process pid {{pid}} ({{pname}}) is no longer active')\n                                                        ^^^\n{error}\n"
        )
    } else {
        return None;
    };
    Some(format!("{common}{frame}"))
}

pub fn read_handoff(path: &Path) -> Result<(u16, BeaconFileData), BeaconFileError> {
    let bytes = fs::read(path).map_err(|_| BeaconFileError::Truncated {
        expected_at_least: 6,
        actual: 0,
    })?;
    let data = decode_beacon_file(&bytes)?;
    Ok((data.raop_port, data))
}

pub fn run_monitor<B, P>(
    options: &BeaconOptions,
    backend: &mut B,
    verifier: &mut P,
    interrupted: impl Fn() -> bool,
) -> Result<(), String>
where
    B: BeaconBackend,
    P: ProcessVerifier,
{
    let mut state = MonitorState::default();
    let was_interrupted = loop {
        if interrupted() {
            break true;
        }
        state.tick(options, backend, verifier)?;
        if options.exit_when_stopped && !state.running && !options.path.is_file() {
            break false;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    finish_monitor(&state, backend, was_interrupted, |line| {
        crate::source_println!("{line}");
    })
}

fn finish_monitor<B: BeaconBackend>(
    state: &MonitorState,
    backend: &mut B,
    was_interrupted: bool,
    mut emit_line: impl FnMut(&str),
) -> Result<(), String> {
    if was_interrupted {
        emit_line("");
    }
    if state.running {
        (if was_interrupted {
            backend.stop_after_interrupt()
        } else {
            backend.stop()
        })
        .map_err(|error| {
            if was_interrupted && error == WINRT_STOPPED_PUBLISHER_ERROR {
                WINRT_STOPPED_PUBLISHER_INTERRUPT_ERROR.to_owned()
            } else {
                error
            }
        })?;
    }
    if was_interrupted {
        emit_line("Exiting ...");
    }
    Ok(())
}

pub fn entry_options() -> Result<Option<BeaconOptions>, std::process::ExitCode> {
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == OsStr::new("-h") || argument == OsStr::new("--help"))
    {
        use std::io::Write as _;
        let _ = std::io::stdout()
            .lock()
            .write_all(include_bytes!("../assets/help-python313.txt"));
        return Ok(None);
    }

    match Args::try_parse() {
        Ok(args) => resolve_options(args, home_directory())
            .map(|options| {
                for notice in &options.notices {
                    crate::source_println!("{notice}");
                }
                Some(options)
            })
            .map_err(|error| {
                match &error {
                    OptionsError::ConfigExit {
                        file,
                        diagnostic,
                        repeats_message,
                        ..
                    } => {
                        if error.to_string().starts_with("optional argument --file") {
                            crate::source_println!("{error}");
                        } else {
                            crate::source_println!(
                                "Read uxplay-beacon.py configuration file  {}",
                                file.display()
                            );
                            if let Some(diagnostic) = diagnostic {
                                crate::source_println!("{diagnostic}");
                            }
                            crate::source_println!("{error}");
                            if *repeats_message {
                                crate::source_println!("{error}");
                            }
                        }
                    }
                    OptionsError::EmptyConfig(file) => {
                        // The source raises UnboundLocalError from its finally block
                        // after announcing the configuration file.
                        crate::source_println!(
                            "Read uxplay-beacon.py configuration file  {}",
                            file.display()
                        );
                        crate::source_eprint!(
                            "{}",
                            empty_config_traceback(&reference_beacon_script())
                        );
                    }
                    OptionsError::InvalidIpv4(_) | OptionsError::AddressUnavailable => {
                        crate::source_println!("{error}");
                    }
                    _ => crate::source_eprintln!("{error}"),
                }
                std::process::ExitCode::from(1)
            }),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            crate::source_print!("{error}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            Ok(None)
        }
        Err(error) => {
            crate::source_eprint!("{error}");
            let _ = std::io::Write::flush(&mut std::io::stderr());
            Err(std::process::ExitCode::from(2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use uxplay_core::beacon::encode_beacon_file;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("uxplay-beacon-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    struct FakeVerifier {
        pid_exists: bool,
        name_matches: bool,
    }
    impl ProcessVerifier for FakeVerifier {
        fn pid_exists(&mut self, _: u32) -> bool {
            self.pid_exists
        }

        fn matches(&mut self, _: u32, _: &str) -> bool {
            self.name_matches
        }
    }

    #[test]
    fn missing_backend_devices_match_python_double_space_diagnostic() {
        assert_eq!(
            no_devices_message(BleType::BleuIo),
            "No devices  needed for BLE type BleuIO were found"
        );
        assert_eq!(
            no_devices_message(BleType::Hci),
            "No devices  needed for BLE type HCI were found"
        );
    }

    #[test]
    fn hidden_app_lifecycle_flag_is_parsed_without_changing_default() {
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "--ipv4",
            "192.0.2.8",
            "--path",
            "beacon.ble",
            "--exit-when-stopped",
        ])
        .unwrap();
        let options = resolve_options(args, PathBuf::from("nonexistent-home")).unwrap();
        assert!(options.exit_when_stopped);

        let args = Args::try_parse_from(["uxplay-beacon", "--ipv4", "192.0.2.8"]).unwrap();
        let options = resolve_options(args, PathBuf::from("nonexistent-home")).unwrap();
        assert!(!options.exit_when_stopped);
    }

    #[test]
    fn bluez_port_one_is_a_success_sentinel_only_for_preflight() {
        assert_eq!(bluez_port_one_start_result(true), Ok(1));
        assert_eq!(
            bluez_port_one_start_result(false),
            Err("Failed to start beacon:\ngiving up, check Bluetooth adapter".to_owned())
        );
    }

    #[test]
    fn app_managed_monitor_exits_when_handoff_is_absent() {
        let directory = temp_dir();
        let mut options = options(directory.join("uxplay.ble"));
        options.exit_when_stopped = true;
        let mut backend = FakeBackend::default();
        let mut verifier = FakeVerifier {
            pid_exists: true,
            name_matches: true,
        };

        assert_eq!(
            run_monitor(&options, &mut backend, &mut verifier, || false),
            Ok(())
        );
        assert!(backend.starts.is_empty());
        assert_eq!(backend.stops, 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[derive(Default)]
    struct FakeBackend {
        starts: Vec<u16>,
        stops: usize,
        module_port: Option<u16>,
    }
    impl BeaconBackend for FakeBackend {
        fn start(&mut self, _: &BeaconOptions, port: u16) -> Result<u16, String> {
            self.starts.push(port);
            self.module_port = Some(port);
            Ok(port)
        }
        fn stop(&mut self) -> Result<(), String> {
            self.stops += 1;
            self.module_port = None;
            Ok(())
        }
    }

    #[test]
    fn interrupt_cleanup_prints_blank_line_stops_then_exits() {
        let events = Rc::new(RefCell::new(Vec::new()));
        struct OrderedBackend(Rc<RefCell<Vec<&'static str>>>);
        impl BeaconBackend for OrderedBackend {
            fn start(&mut self, _: &BeaconOptions, port: u16) -> Result<u16, String> {
                Ok(port)
            }

            fn stop(&mut self) -> Result<(), String> {
                self.0.borrow_mut().push("stop");
                Ok(())
            }
        }
        let mut backend = OrderedBackend(Rc::clone(&events));
        let state = MonitorState {
            running: true,
            ..MonitorState::default()
        };

        finish_monitor(&state, &mut backend, true, |line| {
            events
                .borrow_mut()
                .push(if line.is_empty() { "blank" } else { "exit" });
        })
        .unwrap();

        assert_eq!(*events.borrow(), ["blank", "stop", "exit"]);
    }

    #[test]
    fn interrupted_winrt_cleanup_maps_callback_stop_failure() {
        struct FailedStop;
        impl BeaconBackend for FailedStop {
            fn start(&mut self, _: &BeaconOptions, port: u16) -> Result<u16, String> {
                Ok(port)
            }

            fn stop(&mut self) -> Result<(), String> {
                Err(WINRT_STOPPED_PUBLISHER_ERROR.to_owned())
            }
        }

        let state = MonitorState {
            running: true,
            ..MonitorState::default()
        };
        let mut lines = Vec::new();
        let result = finish_monitor(&state, &mut FailedStop, true, |line| {
            lines.push(line.to_owned());
        });

        assert_eq!(lines, [""]);
        assert_eq!(
            result,
            Err(WINRT_STOPPED_PUBLISHER_INTERRUPT_ERROR.to_owned())
        );
    }

    fn options(path: PathBuf) -> BeaconOptions {
        BeaconOptions {
            ble_type: BleType::BleuIo,
            file: None,
            path,
            ipv4: Ipv4Addr::new(192, 168, 1, 5),
            advmin: Some(100),
            advmax: Some(100),
            index: None,
            device: None,
            exit_when_stopped: false,
            notices: Vec::new(),
        }
    }

    #[test]
    fn ignored_backend_options_are_reported_in_source_order() {
        let args = Args {
            ble_type: Some(BleType::WinRt),
            file: None,
            path: Some(PathBuf::from("beacon.ble")),
            ipv4: Some("192.0.2.8".to_owned()),
            advmin: Some(150),
            advmax: Some(250),
            index: Some(2),
            device: Some("adapter-1".to_owned()),
            exit_when_stopped: false,
        };
        let options = resolve_options(args, PathBuf::from("nonexistent-home")).unwrap();
        assert_eq!(
            options.notices,
            [
                " --advmin option is not used when ble_type = winrt",
                " --advmax option is not used when ble_type = winrt",
                " --index option is not used when ble_type = winrt",
                " --device option is not used when ble_type = winrt",
            ]
        );
        assert_eq!(options.advmin, None);
        assert_eq!(options.advmax, None);
        assert_eq!(options.index, None);
        assert_eq!(options.device, None);
    }

    #[test]
    fn any_non_comment_startup_line_exits_from_the_source_finally_block() {
        let dir = temp_dir();
        let config_path = dir.join("settings");
        fs::write(
            &config_path,
            "BleuIO\n--ipv4 192.168.1.20\n--advmin 250\n--device COM4\n",
        )
        .unwrap();
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "HCI",
            "--file",
            config_path.to_str().unwrap(),
            "--ipv4",
            "192.168.1.30",
        ])
        .unwrap();
        let error = resolve_options(args, dir.clone()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid line \"--device COM4\n\" in configuration file"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_rejects_unknown_lines_and_invalid_ipv4() {
        let dir = temp_dir();
        let config_path = dir.join("bad");
        fs::write(&config_path, "--mystery x\n").unwrap();
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "--file",
            config_path.to_str().unwrap(),
            "--ipv4",
            "127.0.0.1",
        ])
        .unwrap();
        assert!(matches!(
            resolve_options(args, dir.clone()),
            Err(OptionsError::ConfigExit {
                repeats_message: true,
                ..
            })
        ));
        assert!(matches!(
            parse_ipv4("not-ip"),
            Err(OptionsError::InvalidIpv4(_))
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn unicode_decimal_config_values_follow_python_int_before_finally_exit() {
        let dir = temp_dir();
        let config_path = dir.join("unicode-decimal");
        fs::write(&config_path, "--advmin ١٢٣\n").unwrap();
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "--file",
            config_path.to_str().unwrap(),
            "--ipv4",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(
            resolve_options(args, dir.clone()).unwrap_err(),
            OptionsError::ConfigExit {
                file: config_path,
                diagnostic: None,
                message: "Invalid line \"--advmin ١٢٣\n\" in configuration file".into(),
                repeats_message: false,
            }
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_config_uses_python_locale_decoding_before_digit_validation() {
        if windows_ansi_code_page().unwrap() != 1252 {
            return;
        }
        let dir = temp_dir();
        let config_path = dir.join("unicode-decimal");
        fs::write(&config_path, "--advmin ١٢٣\n").unwrap();
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "--file",
            config_path.to_str().unwrap(),
            "--ipv4",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(
            resolve_options(args, dir.clone()).unwrap_err(),
            OptionsError::ConfigExit {
                file: config_path,
                diagnostic: None,
                message: "Invalid line \"--advmin Ù¡Ù¢Ù£\n\" in configuration file".into(),
                repeats_message: true,
            }
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_config_decoder_uses_the_active_ansi_codepage() {
        let active_code_page = windows_ansi_code_page().unwrap();
        assert_ne!(active_code_page, 0);

        let (bytes, expected): (&[u8], &str) = match active_code_page {
            1252 => (&[0x80][..], "€"),
            1251 => (&[0x80][..], "Ђ"),
            _ => (b"ASCII config\n", "ASCII config\n"),
        };
        assert_eq!(decode_config_bytes(bytes.to_vec()).unwrap(), expected);
        assert_eq!(
            decode_windows_config_bytes(bytes, active_code_page).unwrap(),
            expected
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_codepage_decoder_matches_cp1252_and_rejects_invalid_sequences() {
        assert_eq!(
            decode_windows_config_bytes("١٢٣".as_bytes(), 1252).unwrap(),
            "Ù¡Ù¢Ù£"
        );
        assert_eq!(decode_windows_config_bytes(&[0x80], 1251).unwrap(), "Ђ");
        assert!(decode_windows_config_bytes(&[0x81], 1252).is_err());
        assert!(decode_windows_config_bytes(&[0x82], 932).is_err());
    }

    #[test]
    fn comment_only_config_reproduces_unbound_error_condition() {
        let dir = temp_dir();
        let config_path = dir.join("comments");
        fs::write(&config_path, "# only a comment\n").unwrap();
        let args = Args::try_parse_from([
            "uxplay-beacon",
            "--file",
            config_path.to_str().unwrap(),
            "--ipv4",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(
            resolve_options(args, dir.clone()).unwrap_err(),
            OptionsError::EmptyConfig(config_path)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_config_traceback_matches_the_python_313_reference_shape() {
        let source = Path::new(r"D:\Save\Uxplay\UxPlay\Bluetooth_LE_beacon\uxplay-beacon.py");
        assert_eq!(
            empty_config_traceback(source),
            concat!(
                "Traceback (most recent call last):\n",
                "  File \"D:\\Save\\Uxplay\\UxPlay\\Bluetooth_LE_beacon\\uxplay-beacon.py\", line 474, in <module>\n",
                "    [ble_type, path, ipv4_str, advmin, advmax, index, device_address] = parse_params()\n",
                "                                                                        ~~~~~~~~~~~~^^\n",
                "  File \"D:\\Save\\Uxplay\\UxPlay\\Bluetooth_LE_beacon\\uxplay-beacon.py\", line 404, in parse_params\n",
                "    exit(err)\n",
                "         ^^^\n",
                "UnboundLocalError: cannot access local variable 'err' where it is not associated with a value\n"
            )
        );
    }

    #[test]
    fn local_advertised_port_assignment_matches_python_runpy_fixture() {
        let dir = temp_dir();
        let file = dir.join("handoff");
        fs::write(&file, encode_beacon_file(7100, 7, b"uxplay")).unwrap();
        let opts = options(file.clone());
        let mut state = MonitorState::default();
        let mut backend = FakeBackend::default();
        let mut verifier = FakeVerifier {
            pid_exists: true,
            name_matches: true,
        };
        state.tick(&opts, &mut backend, &mut verifier).unwrap();
        assert_eq!(backend.starts, [7100]);
        assert!(state.running);

        let rust_output = format!(
            "beacon_is_running={}\nmodule_global_advertised_port={:?}\nchanged_port_matches={}\n",
            if state.running { "True" } else { "False" },
            state.advertised_port,
            if backend.check_port(state.advertised_port, 7101) {
                "True"
            } else {
                "False"
            }
        );
        assert_eq!(
            rust_output,
            include_str!("../tests/fixtures/source-local-advertised-port.txt")
        );

        fs::write(&file, encode_beacon_file(7101, 7, b"uxplay")).unwrap();
        state.tick(&opts, &mut backend, &mut verifier).unwrap();
        assert_eq!(backend.stops, 0);
        assert!(state.running);
        assert_eq!(backend.starts, [7100]);
        assert_eq!(state.advertised_port, None);

        fs::remove_file(&file).unwrap();
        state.tick(&opts, &mut backend, &mut verifier).unwrap();
        assert_eq!(backend.stops, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn backend_module_port_callback_does_not_change_script_port_global() {
        let dir = temp_dir();
        let file = dir.join("handoff");
        fs::write(&file, encode_beacon_file(7100, 7, b"uxplay")).unwrap();
        let opts = options(file.clone());
        let mut state = MonitorState::default();
        let mut backend = FakeBackend::default();
        let mut verifier = FakeVerifier {
            pid_exists: true,
            name_matches: true,
        };
        state.tick(&opts, &mut backend, &mut verifier).unwrap();

        // BlueZ's async registration error callback clears backend-module state, but the
        // script's separate top-level advertised_port stays None after its local assignment.
        backend.module_port = None;
        fs::write(&file, encode_beacon_file(7101, 7, b"uxplay")).unwrap();
        state.tick(&opts, &mut backend, &mut verifier).unwrap();

        assert!(state.running);
        assert_eq!(backend.stops, 0);
        assert_eq!(backend.starts, [7100]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_process_removes_orphan_handoff_file() {
        let dir = temp_dir();
        let file = dir.join("handoff");
        fs::write(&file, encode_beacon_file(7100, 99, b"uxplay")).unwrap();
        let opts = options(file.clone());
        let mut state = MonitorState::default();
        let mut backend = FakeBackend::default();
        let mut verifier = FakeVerifier {
            pid_exists: false,
            name_matches: false,
        };
        state.tick(&opts, &mut backend, &mut verifier).unwrap();
        assert!(!file.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn truncated_handoff_preserves_source_struct_errors_without_deleting_file() {
        let dir = temp_dir();
        let file = dir.join("handoff");
        let opts = options(file.clone());
        let mut state = MonitorState::default();
        let mut backend = FakeBackend::default();
        let mut verifier = FakeVerifier {
            pid_exists: false,
            name_matches: false,
        };

        for (bytes, expected) in [
            (
                &[0x10][..],
                "struct.error: unpack requires a buffer of 2 bytes",
            ),
            (
                &[0x10, 0x1b, 1, 0][..],
                "struct.error: unpack requires a buffer of 4 bytes",
            ),
        ] {
            fs::write(&file, bytes).unwrap();
            assert_eq!(
                state.tick(&opts, &mut backend, &mut verifier),
                Err(expected.to_owned())
            );
            assert!(
                file.exists(),
                "source does not delete data after struct.error"
            );
        }
        assert!(backend.starts.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dead_pid_skips_invalid_utf8_but_live_name_mismatch_reports_source_path() {
        let data = BeaconFileData {
            raop_port: 7100,
            pid: 99,
            process_name: b"C:\\Apps\\other.exe".to_vec(),
        };
        let mut dead_process = FakeVerifier {
            pid_exists: false,
            name_matches: false,
        };
        assert_eq!(
            source_orphan_name(&data, &mut dead_process),
            Ok(Some("process name unread".to_owned()))
        );

        let mut live_mismatch = FakeVerifier {
            pid_exists: true,
            name_matches: false,
        };
        assert_eq!(
            source_orphan_name(&data, &mut live_mismatch),
            Ok(Some("C:\\Apps\\other.exe".to_owned()))
        );
    }

    #[test]
    fn invalid_utf8_process_name_fails_only_after_pid_is_confirmed() {
        let data = BeaconFileData {
            raop_port: 7100,
            pid: 99,
            process_name: vec![0xff],
        };
        let mut dead_process = FakeVerifier {
            pid_exists: false,
            name_matches: false,
        };
        assert_eq!(
            source_orphan_name(&data, &mut dead_process),
            Ok(Some("process name unread".to_owned()))
        );

        let mut live_process = FakeVerifier {
            pid_exists: true,
            name_matches: false,
        };
        assert!(
            source_orphan_name(&data, &mut live_process)
                .unwrap_err()
                .starts_with("UnicodeDecodeError:")
        );
    }

    #[test]
    fn unicode_decode_errors_match_python_utf8_codec_messages() {
        for (bytes, expected) in [
            (
                &[0xff][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
            ),
            (
                &[0xc2, b'A'][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc2 in position 0: invalid continuation byte",
            ),
            (
                &[0xc2][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc2 in position 0: unexpected end of data",
            ),
            (
                &[0xe2, 0x82][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: unexpected end of data",
            ),
            (
                &[0xf0, 0x90, 0x80][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-2: unexpected end of data",
            ),
            (
                &[0xf0, 0x90, b'A', 0x80][..],
                "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte",
            ),
        ] {
            assert_eq!(
                python_unicode_decode_error(bytes, std::str::from_utf8(bytes).unwrap_err()),
                expected
            );
        }
    }

    #[test]
    fn malformed_handoff_tracebacks_match_cpython_reference_frames() {
        let script = reference_beacon_script();
        let script = script.display();
        let common = format!(
            "Traceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^\n  File \"{script}\", line 173, in on_timeout\n    check_file_exists(file_path)\n    ~~~~~~~~~~~~~~~~~^^^^^^^^^^\n"
        );

        let short_port = "struct.error: unpack requires a buffer of 2 bytes";
        assert_eq!(
            python_traceback_for_monitor_error(short_port).unwrap(),
            format!(
                "{common}  File \"{script}\", line 133, in check_file_exists\n    port = struct.unpack('<H', data)[0]\n           ~~~~~~~~~~~~~^^^^^^^^^^^^\n{short_port}\n"
            )
        );

        let short_pid = "struct.error: unpack requires a buffer of 4 bytes";
        assert_eq!(
            python_traceback_for_monitor_error(short_pid).unwrap(),
            format!(
                "{common}  File \"{script}\", line 135, in check_file_exists\n    pid = struct.unpack('<I', data)[0]\n          ~~~~~~~~~~~~~^^^^^^^^^^^^\n{short_pid}\n"
            )
        );

        let invalid_utf8 = "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte";
        assert_eq!(
            python_traceback_for_monitor_error(invalid_utf8).unwrap(),
            format!(
                "{common}  File \"{script}\", line 142, in check_file_exists\n    pname = data.split(b'\\0',1)[0].decode('utf-8')\n{invalid_utf8}\n"
            )
        );

        let io_error = "UnboundLocalError: cannot access local variable 'pid' where it is not associated with a value";
        assert_eq!(
            python_traceback_for_monitor_error(io_error).unwrap(),
            format!(
                "{common}  File \"{script}\", line 157, in check_file_exists\n    print(f'Orphan beacon file exists, but process pid {{pid}} ({{pname}}) is no longer active')\n{}^^^\n{io_error}\n",
                " ".repeat(56)
            )
        );
        assert!(python_traceback_for_monitor_error("other error").is_none());
    }

    #[test]
    fn stopped_winrt_publisher_traceback_matches_cpython_source_fixture() {
        assert_eq!(
            python_traceback_for_monitor_error(WINRT_STOPPED_PUBLISHER_ERROR).unwrap(),
            canonicalize_reference_fixture(include_str!(
                "../tests/fixtures/source-winrt-stopped-stop-traceback.txt"
            ))
        );
    }

    #[test]
    fn bleuio_zero_port_traceback_matches_cpython_source_fixture() {
        let actual = python_traceback_for_monitor_error(BLEUIO_ZERO_PORT_ASSERTION_ERROR).unwrap();
        let expected = canonicalize_reference_fixture(include_str!(
            "../tests/fixtures/source-bleuio-zero-port-traceback.txt"
        ));
        let first_difference = actual
            .bytes()
            .zip(expected.bytes())
            .position(|(actual, expected)| actual != expected);
        assert!(
            actual == expected,
            "trace differs at byte {first_difference:?}; actual length {}, fixture length {}",
            actual.len(),
            expected.len()
        );
    }

    #[test]
    fn interrupted_stopped_winrt_traceback_matches_cpython_source_fixture() {
        assert_eq!(
            python_traceback_for_monitor_error(WINRT_STOPPED_PUBLISHER_INTERRUPT_ERROR).unwrap(),
            canonicalize_reference_fixture(include_str!(
                "../tests/fixtures/source-winrt-stopped-interrupt-traceback.txt"
            ))
        );
    }

    #[test]
    fn process_prefix_matches_source_name_check() {
        struct NameVerifier(Cell<bool>);
        impl ProcessVerifier for NameVerifier {
            fn pid_exists(&mut self, _: u32) -> bool {
                true
            }

            fn matches(&mut self, _: u32, name: &str) -> bool {
                self.0.set(name == "uxplay.exe");
                self.0.get()
            }
        }
        let mut verifier = NameVerifier(Cell::new(false));
        let data = BeaconFileData {
            raop_port: 7100,
            pid: 1,
            process_name: b"C:/Apps/uxplay.exe".to_vec(),
        };
        assert_eq!(source_orphan_name(&data, &mut verifier), Ok(None));
    }
}
