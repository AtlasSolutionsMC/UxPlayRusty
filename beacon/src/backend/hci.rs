//! Direct Low Energy advertising through the host's HCI utilities.
//!
//! Linux uses hcitool/hciconfig and FreeBSD uses hccontrol, matching the reference backend. Both
//! paths require passwordless sudo for reset and advertising commands.

use crate::{BeaconBackend, BeaconOptions};
use std::net::Ipv4Addr;
use uxplay_core::beacon::{hci_advertising_data, manufacturer_data};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::ffi::OsStr;
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
use std::io;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::io::Read;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::fd::OwnedFd;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::unix::net::UnixStream;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::process::{Command, ExitStatus, Output, Stdio};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::thread;

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
const LMP_VERSIONS: [&str; 16] = [
    "1.0b", "1.1", "1.2", "2.0+EDR", "2.1+EDR", "3.0+HS", "4.0", "4.1", "4.2", "5.0", "5.1", "5.2",
    "5.3", "5.4", "6.0", "6.1",
];

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
const START_FAILURE: &str = "Failed to start beacon:\ngiving up, check Bluetooth adapter";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Host {
    Linux,
    FreeBsd,
}

#[derive(Debug)]
pub struct HciBackend {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    host: Host,
    device: String,
}

impl HciBackend {
    /// Finds an eligible Bluetooth 4.0+ adapter. An explicitly requested adapter is returned
    /// without reset, as in the source; automatic selection resets the first eligible device.
    pub fn find_device(requested: Option<&str>) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            Self::find_device_for_host(Host::Linux, requested)
        }
        #[cfg(target_os = "freebsd")]
        {
            Self::find_device_for_host(Host::FreeBsd, requested)
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            let _ = requested;
            Err(unsupported_host_error())
        }
    }

    /// Runs the source module's import-time platform notice before its loaded line.
    pub fn check_supported_host() -> Result<(), String> {
        #[cfg(target_os = "freebsd")]
        warn_freebsd_release();
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        return Err(unsupported_host_error());
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn find_device_for_host(host: Host, requested: Option<&str>) -> Result<Self, String> {
        let eligible = list_devices(host)?;
        if eligible.is_empty() {
            return Err(crate::no_devices_message(crate::BleType::Hci));
        }

        if let Some(requested) = requested
            && let Some(device) = eligible.iter().find(|device| device.name == requested)
        {
            return Ok(Self::new(host, device.name.clone()));
        }

        for (index, device) in eligible.iter().enumerate() {
            crate::source_println!(
                "=== detected HCI device {}. {}: Bluetooth v{}",
                index + 1,
                device.name,
                version_name(device.version).expect("adapter version was checked during listing")
            );
        }
        if eligible.len() > 1 {
            crate::source_println!(
                "warning: {} HCI devices were found, the first found will be used",
                eligible.len()
            );
            crate::source_println!(
                "(to override this choice, specify \"--device=...\" in optional arguments)"
            );
        }

        let device = eligible[0].name.clone();
        let reset = reset_command(host, &device);
        let output = run(&reset).map_err(|error| {
            hci_spawn_error_traceback(
                host,
                &device,
                HciSpawnContext::Reset,
                HciOperation::Enable(false),
                &error,
            )
        })?;
        if !output.status.success() {
            return Err(reset_failure_message(host, &output.stderr, &output.stdout));
        }
        Ok(Self::new(host, device))
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn new(host: Host, device: String) -> Self {
        Self { host, device }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn send_detailed(
        &self,
        operation: HciOperation,
        context: HciSpawnContext,
    ) -> Result<(), HciCommandFailure> {
        let output = run(&operation.command(self.host, &self.device)).map_err(|error| {
            HciCommandFailure::Spawn(hci_spawn_error_traceback(
                self.host,
                &self.device,
                context,
                operation,
                &error,
            ))
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(HciCommandFailure::NonZeroExit(python_command_failure(
                operation.error_prefix(),
                &output.stderr,
                &output.stdout,
            )))
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn stop_with_context(&self, context: HciSpawnContext) -> Result<(), String> {
        let operation = HciOperation::Enable(false);
        let output = run(&operation.command(self.host, &self.device)).map_err(|error| {
            hci_spawn_error_traceback(self.host, &self.device, context, operation, &error)
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(hci_called_process_error_traceback(
                self.host,
                &self.device,
                context,
                operation,
                output.status.code(),
            ))
        }
    }
}

impl BeaconBackend for HciBackend {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String> {
        let advmin = options
            .advmin
            .ok_or_else(|| "advertising minimum interval was not set".to_owned())?;
        let advmax = options
            .advmax
            .ok_or_else(|| "advertising maximum interval was not set".to_owned())?;
        start_sequence(options.ipv4, port, advmin, advmax, |operation| {
            self.send_detailed(operation, HciSpawnContext::Start)
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn start(&mut self, _: &BeaconOptions, _: u16) -> Result<u16, String> {
        Err(unsupported_host_error())
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn stop(&mut self) -> Result<(), String> {
        self.stop_with_context(HciSpawnContext::Stop)?;
        crate::source_println!("AirPlay Service-Discovery beacon transmission ended");
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn stop_after_interrupt(&mut self) -> Result<(), String> {
        self.stop_with_context(HciSpawnContext::InterruptStop)?;
        crate::source_println!("AirPlay Service-Discovery beacon transmission ended");
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn stop(&mut self) -> Result<(), String> {
        Err(unsupported_host_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
#[derive(Debug, Eq, PartialEq)]
enum HciCommandFailure {
    Spawn(String),
    NonZeroExit(String),
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HciSpawnContext {
    Reset,
    Start,
    Stop,
    InterruptStop,
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn hci_spawn_error_traceback(
    host: Host,
    device: &str,
    context: HciSpawnContext,
    operation: HciOperation,
    error: &io::Error,
) -> String {
    let command = if context == HciSpawnContext::Reset {
        reset_command(host, device)
    } else {
        operation.command(host, device)
    };
    let program = command.first().map(String::as_str).unwrap_or("sudo");
    let exception = python_os_exception(host, program, error);
    hci_traceback(context, operation, &exception)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn hci_called_process_error_traceback(
    host: Host,
    device: &str,
    context: HciSpawnContext,
    operation: HciOperation,
    exit_code: Option<i32>,
) -> String {
    let command = operation.command(host, device);
    let command = command
        .iter()
        .map(|argument| format!("'{argument}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let exception = format!(
        "CalledProcessError: Command '[{command}]' returned non-zero exit status {}.",
        exit_code.map_or_else(|| "None".to_owned(), |code| code.to_string())
    );
    hci_traceback(context, operation, &exception)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn hci_traceback(context: HciSpawnContext, operation: HciOperation, exception: &str) -> String {
    let script = crate::reference_beacon_script();
    let module = script.with_file_name("uxplay_beacon_module_HCI.py");
    let script = script.display();
    let module = module.display();
    let mut traceback = String::from("Traceback (most recent call last):\n");

    match context {
        HciSpawnContext::Reset => {
            traceback.push_str(&format!(
                "  File \"{script}\", line 506, in <module>\n    use_device  = find_device(device_address)\n  File \"{module}\", line 255, in find_device\n    result = subprocess.run(cmd, capture_output=True, text=True, check=True)\n"
            ));
        }
        HciSpawnContext::Start => match operation {
            HciOperation::Parameters { .. } => {
                traceback.push_str(&format!(
                    "  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 119, in check_pending\n    start_beacon()\n    ~~~~~~~~~~~~^^\n  File \"{script}\", line 78, in start_beacon\n    setup_beacon(ipv4_str, port, advmin, advmax, index)\n    ~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{module}\", line 105, in setup_beacon\n    result = le_cmd(hcicmd, args)\n"
                ));
            }
            HciOperation::Data { .. } => {
                traceback.push_str(&format!(
                    "  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 119, in check_pending\n    start_beacon()\n    ~~~~~~~~~~~~^^\n  File \"{script}\", line 78, in start_beacon\n    setup_beacon(ipv4_str, port, advmin, advmax, index)\n    ~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{module}\", line 128, in setup_beacon\n    le_cmd(hcicmd, args)\n    ~~~~~~^^^^^^^^^^^^^^\n"
                ));
            }
            HciOperation::Enable(true) => {
                traceback.push_str(&format!(
                    "  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 119, in check_pending\n    start_beacon()\n    ~~~~~~~~~~~~^^\n  File \"{script}\", line 79, in start_beacon\n    advertised_port = beacon_on()\n  File \"{module}\", line 146, in beacon_on\n    le_cmd(hcicmd, args)\n    ~~~~~~^^^^^^^^^^^^^^\n"
                ));
            }
            HciOperation::Enable(false) => unreachable!("start cannot disable advertising"),
        },
        HciSpawnContext::Stop => {
            traceback.push_str(&format!(
                "  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 195, in main\n    on_timeout(file_path)\n    ~~~~~~~~~~^^^^^^^^^^^\n  File \"{script}\", line 174, in on_timeout\n    check_pending()\n    ~~~~~~~~~~~~~^^\n  File \"{script}\", line 115, in check_pending\n    stop_beacon()\n    ~~~~~~~~~~~^^\n  File \"{script}\", line 87, in stop_beacon\n    beacon_off()\n    ~~~~~~~~~~^^\n  File \"{module}\", line 164, in beacon_off\n    le_cmd(hcicmd, args)\n    ~~~~~~^^^^^^^^^^^^^^\n"
            ));
        }
        HciSpawnContext::InterruptStop => {
            traceback = format!(
                "Traceback (most recent call last):\n  File \"{script}\", line 196, in main\n    time.sleep(1)\n    ~~~~~~~~~~^^^\nKeyboardInterrupt\n\nDuring handling of the above exception, another exception occurred:\n\nTraceback (most recent call last):\n  File \"{script}\", line 533, in <module>\n    main(path, ipv4_str, advmin, advmax, index)\n    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"{script}\", line 203, in main\n    stop_beacon()\n    ~~~~~~~~~~~^^\n  File \"{script}\", line 87, in stop_beacon\n    beacon_off()\n    ~~~~~~~~~~^^\n  File \"{module}\", line 164, in beacon_off\n    le_cmd(hcicmd, args)\n    ~~~~~~^^^^^^^^^^^^^^\n"
            );
        }
    }

    if context != HciSpawnContext::Reset {
        traceback.push_str(&format!(
            "  File \"{module}\", line 76, in le_cmd\n    subprocess.run(cmd, capture_output=True, text=True, check=True)\n    ~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n"
        ));
    }

    traceback.push_str(exception);
    traceback.push('\n');
    traceback
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn python_os_exception(_host: Host, program: &str, error: &io::Error) -> String {
    let Some(errno) = error.raw_os_error() else {
        return format!("OSError: {}", error);
    };
    let exception = match errno {
        1 | 13 => "PermissionError",
        2 => "FileNotFoundError",
        3 => "ProcessLookupError",
        4 => "InterruptedError",
        10 => "ChildProcessError",
        17 => "FileExistsError",
        20 => "NotADirectoryError",
        21 => "IsADirectoryError",
        _ => "OSError",
    };
    let description = match errno {
        1 => "Operation not permitted",
        2 => "No such file or directory",
        5 => "Input/output error",
        8 => "Exec format error",
        12 => "Cannot allocate memory",
        13 => "Permission denied",
        20 => "Not a directory",
        21 => "Is a directory",
        24 => "Too many open files",
        _ => return format!("{exception}: {}", error),
    };
    format!("{exception}: [Errno {errno}] {description}: '{program}'")
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn start_sequence(
    address: Ipv4Addr,
    port: u16,
    advmin: i64,
    advmax: i64,
    mut send: impl FnMut(HciOperation) -> Result<(), HciCommandFailure>,
) -> Result<u16, String> {
    let setup_operations = [
        HciOperation::Parameters { advmin, advmax },
        HciOperation::Data { address, port },
    ];
    let mut setup_failed = false;
    for operation in setup_operations {
        match send(operation) {
            Ok(()) => {}
            Err(HciCommandFailure::Spawn(error)) => return Err(error),
            Err(HciCommandFailure::NonZeroExit(error)) => {
                crate::source_println!("{error}");
                setup_failed = true;
                break;
            }
        }
    }

    match send(HciOperation::Enable(true)) {
        Ok(()) if setup_failed => {
            // The Python caller ignores setup_beacon's False result and still
            // runs beacon_on(), with the globals left unset by setup_beacon.
            crate::source_println!(
                "AirPlay Service-Discovery beacon transmission started None:None"
            );
            Err(START_FAILURE.to_owned())
        }
        Ok(()) => {
            crate::source_println!(
                "AirPlay Service-Discovery beacon transmission started {address}:{port}"
            );
            Ok(port)
        }
        Err(HciCommandFailure::Spawn(error)) => Err(error),
        Err(HciCommandFailure::NonZeroExit(error)) => {
            crate::source_println!("{error}");
            Err(START_FAILURE.to_owned())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciOperation {
    Parameters { advmin: i64, advmax: i64 },
    Data { address: Ipv4Addr, port: u16 },
    Enable(bool),
}

impl HciOperation {
    pub fn command(self, host: Host, device: &str) -> Vec<String> {
        let mut command = match host {
            Host::Linux => vec![
                "sudo".into(),
                "-n".into(),
                "hcitool".into(),
                "-i".into(),
                device.into(),
                "cmd".into(),
                "0x08".into(),
            ],
            Host::FreeBsd => vec![
                "sudo".into(),
                "-n".into(),
                "hccontrol".into(),
                "-n".into(),
                device.into(),
            ],
        };
        match (host, self) {
            (Host::Linux, Self::Parameters { advmin, advmax }) => {
                command.push("0x0006".into());
                command.extend(linux_parameter_args(advmin, advmax));
            }
            (Host::FreeBsd, Self::Parameters { advmin, advmax }) => {
                command.extend([
                    "le_set_advertising_param".into(),
                    format!("-m {advmin}"),
                    format!("-M {advmax}"),
                    "t = 3".into(),
                ]);
            }
            (Host::Linux, Self::Data { address, port }) => {
                command.push("0x0008".into());
                command.extend(linux_advertising_data_args(address, port));
            }
            (Host::FreeBsd, Self::Data { address, port }) => {
                command.extend([
                    "le_set_advertising_data".into(),
                    "-b".into(),
                    freebsd_advertising_data(address, port),
                ]);
            }
            (Host::Linux, Self::Enable(enabled)) => {
                command.extend([
                    "0x000a".into(),
                    if enabled { "0x01" } else { "0x00" }.into(),
                ]);
            }
            (Host::FreeBsd, Self::Enable(enabled)) => {
                command.extend([
                    "le_set_advertising_enable".into(),
                    if enabled { "enable" } else { "disable" }.into(),
                ]);
            }
        }
        command
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn error_prefix(self) -> &'static str {
        match self {
            Self::Parameters { .. } => "beacon_on error (set_advertising_parameters):",
            Self::Data { .. } => "beacon_on error (set_advertisng_parameters):",
            Self::Enable(true) => "beacon_on error:",
            Self::Enable(false) => "beacon_off error:",
        }
    }
}

pub fn linux_parameter_args(advmin_ms: i64, advmax_ms: i64) -> Vec<String> {
    // Python integers do not overflow when `advmin * 8` is scaled to HCI's 0.625 ms units.
    // Widen before multiplying so the full i64 CLI range keeps the same floor/modulo behavior.
    let min_units = (i128::from(advmin_ms) * 8).div_euclid(5);
    let max_units = (i128::from(advmax_ms) * 8).div_euclid(5);
    let values = [
        min_units.rem_euclid(256),
        min_units.div_euclid(256),
        max_units.rem_euclid(256),
        max_units.div_euclid(256),
    ];
    values
        .into_iter()
        .map(python_hex)
        .chain(
            [
                "0x03", "0x00", "0x00", "0x00", "0x00", "0x00", "0x00", "0x00", "0x00", "0x07",
                "0x00",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .collect()
}

fn python_hex(value: i128) -> String {
    if value < 0 {
        format!("-{:#04x}", value.unsigned_abs())
    } else {
        format!("{value:#04x}")
    }
}

pub fn linux_advertising_data_args(address: Ipv4Addr, port: u16) -> Vec<String> {
    hci_advertising_data(address, port)
        .into_iter()
        .map(|byte| format!("{byte:#04x}"))
        .collect()
}

pub fn freebsd_advertising_data(address: Ipv4Addr, port: u16) -> String {
    let mut bytes = vec![0xff, 0x4c, 0x00];
    bytes.extend_from_slice(&manufacturer_data(address, port));
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn reset_command(host: Host, device: &str) -> Vec<String> {
    match host {
        Host::Linux => vec![
            "sudo".into(),
            "-n".into(),
            "hciconfig".into(),
            device.into(),
            "reset".into(),
        ],
        Host::FreeBsd => vec![
            "sudo".into(),
            "-n".into(),
            "hccontrol".into(),
            "-n".into(),
            device.into(),
            "Reset".into(),
        ],
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn unsupported_host_error() -> String {
    let host = match std::env::consts::OS {
        "windows" => "Windows",
        "linux" => "Linux",
        "freebsd" => "FreeBSD",
        "macos" => "Darwin",
        other => other,
    };
    format!("{host} is not supported by the HCI module")
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn run(command: &[String]) -> io::Result<Output> {
    let Some((program, args)) = command.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty HCI command",
        ));
    };
    Command::new(program).args(args).output()
}

/// Runs a Python `check_output(stderr=STDOUT)` command with both child streams connected to one
/// socket. Reading them after process exit would reorder stderr after stdout, unlike the source.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn run_check_output<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let (mut reader, stdout) = UnixStream::pair()?;
    let stderr = stdout.try_clone()?;
    let output_reader = thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).map(|_| output)
    });

    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::from(OwnedFd::from(stdout)))
        .stderr(Stdio::from(OwnedFd::from(stderr)));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            drop(command);
            let _ = output_reader.join();
            return Err(error);
        }
    };
    // `Command` retains the parent's stdout/stderr handles after spawning.
    // Close those copies so the reader thread can observe EOF after the child
    // exits instead of waiting forever in `read_to_end`.
    drop(command);
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output_reader.join();
            return Err(error);
        }
    };
    let output = output_reader
        .join()
        .map_err(|_| io::Error::other("merged command-output reader panicked"))??;
    Ok((status, output))
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn python_command_failure(prefix: &str, stderr: &[u8], stdout: &[u8]) -> String {
    format!(
        "{prefix} {} {}",
        String::from_utf8_lossy(stderr),
        String::from_utf8_lossy(stdout)
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn reset_failure_message(host: Host, stderr: &[u8], stdout: &[u8]) -> String {
    format!(
        "{}\n{}\ncannot continue: SystemExit(1)",
        python_command_failure("hci reset error:", stderr, stdout),
        help_text(host)
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn python_check_output_text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct HciDevice {
    name: String,
    version: usize,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn list_devices(host: Host) -> Result<Vec<HciDevice>, String> {
    struct ListingCommand {
        program: &'static str,
        args: &'static [&'static str],
        parse_names: fn(&str) -> Vec<String>,
    }
    let command = match host {
        Host::Linux => ListingCommand {
            program: "hcitool",
            args: &["dev"],
            parse_names: linux_device_names,
        },
        Host::FreeBsd => ListingCommand {
            program: "hccontrol",
            args: &["Read_Node_List"],
            parse_names: freebsd_device_names,
        },
    };
    let (status, output) = run_check_output(command.program, command.args)
        .map_err(|_| listing_spawn_failure_message(command.program))?;
    if !status.success() {
        return Err(listing_exit_failure_message(command.program, &output));
    }
    let listing = python_check_output_text(&output);
    crate::source_print!("{}", python_printed_listing(&listing));
    let mut devices = Vec::new();
    for line in listing.lines() {
        for name in (command.parse_names)(line) {
            if let Some(version) = get_bluetooth_version(host, &name)
                && version >= 6
            {
                if version_name(version).is_none() {
                    return Err(lmp_index_error_traceback());
                }
                devices.push(HciDevice { name, version });
            }
        }
    }
    Ok(devices)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn python_printed_listing(listing: &str) -> String {
    format!("{listing}\n")
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn listing_spawn_failure_message(program: &str) -> String {
    format!(
        "Error: {program} command not found\n{}",
        crate::no_devices_message(crate::BleType::Hci)
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn listing_exit_failure_message(program: &str, output: &[u8]) -> String {
    format!(
        "Error running {program}: {}\n{}",
        python_check_output_text(output),
        crate::no_devices_message(crate::BleType::Hci)
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn linux_device_names(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut remaining = line;
    while let Some(index) = remaining.find("hci") {
        let after_name = &remaining[index + "hci".len()..];
        let digits = after_name.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            remaining = after_name;
            continue;
        }
        names.push(format!("hci{}", &after_name[..digits]));
        remaining = &after_name[digits..];
    }
    names
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn freebsd_device_names(line: &str) -> Vec<String> {
    freebsd_device_name(line).into_iter().collect()
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn freebsd_device_name(line: &str) -> Option<String> {
    let candidate = line.strip_prefix("ubt")?;
    let digits = candidate.bytes().take_while(u8::is_ascii_digit).count();
    let suffix = &candidate[digits..];
    (digits > 0 && suffix.starts_with("hci")).then(|| format!("ubt{}hci", &candidate[..digits]))
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn get_bluetooth_version(host: Host, device: &str) -> Option<usize> {
    let (program, args): (&str, Vec<String>) = match host {
        Host::Linux => ("hciconfig", vec![device.into(), "-a".into()]),
        Host::FreeBsd => (
            "hccontrol",
            vec![
                "-n".into(),
                device.into(),
                "Read_Local_Version_Information".into(),
            ],
        ),
    };
    let output = match run_check_output(program, &args) {
        Ok((status, output)) if status.success() => python_check_output_text(&output),
        Ok((_, output)) => {
            crate::source_println!(
                "Error running {program} for {device}: {}",
                python_check_output_text(&output)
            );
            return None;
        }
        Err(_) => {
            crate::source_println!("Error: {program} command not found");
            return None;
        }
    };
    parse_lmp_version(host, &output)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn parse_lmp_version(host: Host, output: &str) -> Option<usize> {
    let marker = match host {
        Host::Linux => "LMP Version:",
        Host::FreeBsd => "LMP version:",
    };
    output
        .lines()
        .filter(|line| line.contains(marker))
        .find_map(|line| {
            let version = match host {
                Host::Linux => parse_linux_lmp_version(line)?,
                Host::FreeBsd => parse_freebsd_lmp_version(line)?,
            };
            Some(version)
        })
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn parse_linux_lmp_version(line: &str) -> Option<usize> {
    let mut remaining = line.split_once("LMP Version:")?.1;
    while let Some(index) = remaining.find("(0x") {
        remaining = &remaining[index + "(0x".len()..];
        let mut chars = remaining.chars();
        let digit = chars.next()?;
        if digit.is_ascii_hexdigit() && chars.next() == Some(')') {
            return digit.to_digit(16).map(|version| version as usize);
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn parse_freebsd_lmp_version(line: &str) -> Option<usize> {
    let mut remaining = line.split_once("LMP version:")?.1;
    while let Some(open_index) = remaining.find('[') {
        remaining = &remaining[open_index + 1..];
        let Some(hex) = remaining.strip_prefix("0x") else {
            continue;
        };
        let digits = hex.chars().take_while(char::is_ascii_hexdigit).count();
        if digits > 0 && hex[digits..].starts_with(']') {
            return Some(hex[..digits].bytes().fold(0_usize, |value, digit| {
                let digit = match digit {
                    b'0'..=b'9' => usize::from(digit - b'0'),
                    b'a'..=b'f' => usize::from(digit - b'a' + 10),
                    b'A'..=b'F' => usize::from(digit - b'A' + 10),
                    _ => unreachable!("digits were checked as ASCII hex"),
                };
                value.saturating_mul(16).saturating_add(digit)
            }));
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn version_name(version: usize) -> Option<&'static str> {
    LMP_VERSIONS.get(version).copied()
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn lmp_index_error_traceback() -> String {
    let script = crate::reference_beacon_script();
    let module = script.with_file_name("uxplay_beacon_module_HCI.py");
    let script = script.display();
    let module = module.display();
    format!(
        "Traceback (most recent call last):\n  File \"{script}\", line 506, in <module>\n    use_device  = find_device(device_address)\n  File \"{module}\", line 232, in find_device\n    list = list_devices_by_version(min_version=6)\n  File \"{module}\", line 224, in list_devices_by_version\n    bt_version = LMP[version_decimal]\n                 ~~~^^^^^^^^^^^^^^^^^\nIndexError: list index out of range\n"
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn help_text(host: Host) -> &'static str {
    match host {
        Host::Linux => {
            "\n   This HCI module requires users of the module to have elevated privileges that\n   allow execution of a low-level Bluetooth HCI command using passwordless \"sudo\":\n     (1) As System Administrator, create a group such as \"hciusers\")\n\n     (2) use visudo to create a file /etc/sudoers.d/hciusers containing the line:\n          %hciusers ALL=(ALL) NOPASSWD: /usr/bin/hciconfig, /usr/bin/hcitool\n\n     (3) add users of uxplay_beacon_module_HCI.py to the group \"hciusers\"\n"
        }
        Host::FreeBsd => {
            "\n   This HCI module requires users of the module to have elevated privileges that\n   allow execution of a low-level Bluetooth HCI command using passwordless \"sudo\":\n     (1) As System Administrator, create a group such as \"hciusers\")\n\n     (2) use visudo to create a file /usr/local/etc/sudoers.d/hciusers containing the line:\n          %hciusers ALL=(ALL) NOPASSWD: /usr/sbin/hccontrol\n\n     (3) add users of uxplay_beacon_module_HCI.py to the group \"hciusers\"\n"
        }
    }
}

#[cfg(target_os = "freebsd")]
fn freebsd_release_warning(release: &str) -> Option<String> {
    let (major, minor) = release.split('-').next()?.split_once('.')?;
    let major = major.parse::<u32>().ok()?;
    let minor = minor.parse::<u32>().ok()?;
    if major < 15 || (major == 15 && minor < 1) {
        Some(format!(
            "FreeBSD release: {release}\n\n    ***********************************************************************\n    * FreeBSD < 15.1: this module requires a patch to FreeBSD's hccontrol *\n    * utility, to add an option first introduced in FreeBSD 15.1. A patch *\n    * for older FreeBSD releases is available at the UxPlay Wiki site:    *\n    * https://github.com/FDH2/UxPlay/wiki/hccontrol-patch-for-FreeBSD-15.0*\n    ***********************************************************************\n"
        ))
    } else {
        None
    }
}

#[cfg(target_os = "freebsd")]
fn warn_freebsd_release() {
    if let Ok(output) = Command::new("uname").arg("-r").output()
        && output.status.success()
    {
        let release = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if let Some(warning) = freebsd_release_warning(&release) {
            crate::source_println!("{warning}");
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    use super::run_check_output;
    use super::{HciCommandFailure, START_FAILURE, start_sequence};
    use super::{
        HciOperation, HciSpawnContext, Host, freebsd_advertising_data, freebsd_device_name,
        freebsd_device_names, hci_called_process_error_traceback, hci_spawn_error_traceback,
        help_text, linux_advertising_data_args, linux_device_names, linux_parameter_args,
        listing_exit_failure_message, listing_spawn_failure_message, lmp_index_error_traceback,
        parse_lmp_version, python_check_output_text, python_command_failure, python_os_exception,
        python_printed_listing, reset_command, reset_failure_message, version_name,
    };
    use std::io;
    use std::net::Ipv4Addr;

    #[test]
    fn listing_spawn_failure_includes_source_followup_no_device_error() {
        assert_eq!(
            listing_spawn_failure_message("hcitool"),
            "Error: hcitool command not found\nNo devices  needed for BLE type HCI were found"
        );
    }

    #[test]
    fn listing_command_failure_preserves_printed_output_separator_and_followup() {
        assert_eq!(
            listing_exit_failure_message("hcitool", b"adapter unavailable\n"),
            "Error running hcitool: adapter unavailable\n\nNo devices  needed for BLE type HCI were found"
        );
    }

    #[test]
    fn listing_command_failure_without_output_keeps_python_empty_output_shape() {
        assert_eq!(
            listing_exit_failure_message("hccontrol", b""),
            "Error running hccontrol: \nNo devices  needed for BLE type HCI were found"
        );
    }

    #[test]
    fn version_probe_failure_output_keeps_the_captured_stream_whitespace() {
        assert_eq!(
            python_check_output_text(b"controller offline \r\n\n"),
            "controller offline \r\n\n"
        );
        assert_eq!(
            python_check_output_text(b"permission denied \n"),
            "permission denied \n"
        );
    }

    #[test]
    fn successful_check_output_parses_text_written_only_to_stderr() {
        let listing = python_check_output_text(b"Devices:\n  hci0 00:11:22:33:44:55\n");
        assert_eq!(linux_device_names(&listing), ["hci0"]);
        assert_eq!(
            parse_lmp_version(
                Host::Linux,
                &python_check_output_text(b"LMP Version: 5.3 (0xc)\n")
            ),
            Some(12)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn check_output_merges_stdout_and_stderr_in_write_order() {
        let (status, output) = run_check_output(
            "sh",
            &["-c", "printf 'first'; printf 'second' >&2; printf 'third'"],
        )
        .expect("spawn merged output fixture");
        assert!(status.success());
        assert_eq!(output, b"firstsecondthird");
    }

    #[test]
    fn out_of_range_freebsd_lmp_index_reproduces_python_index_error_traceback() {
        assert_eq!(version_name(15), Some("6.1"));
        assert_eq!(version_name(16), None);

        let script = crate::reference_beacon_script();
        let module = script.with_file_name("uxplay_beacon_module_HCI.py");
        let script = script.display();
        let module = module.display();
        assert_eq!(
            lmp_index_error_traceback(),
            format!(
                "Traceback (most recent call last):\n  File \"{script}\", line 506, in <module>\n    use_device  = find_device(device_address)\n  File \"{module}\", line 232, in find_device\n    list = list_devices_by_version(min_version=6)\n  File \"{module}\", line 224, in list_devices_by_version\n    bt_version = LMP[version_decimal]\n                 ~~~^^^^^^^^^^^^^^^^^\nIndexError: list index out of range\n"
            )
        );
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    use super::HciBackend;

    #[test]
    fn linux_interval_units_and_hci_parameter_command_match_reference() {
        assert_eq!(
            linux_parameter_args(100, 250),
            [
                "0xa0", "0x00", "0x90", "0x01", "0x03", "0x00", "0x00", "0x00", "0x00", "0x00",
                "0x00", "0x00", "0x00", "0x07", "0x00"
            ]
        );
        assert_eq!(
            &HciOperation::Parameters {
                advmin: 100,
                advmax: 250
            }
            .command(Host::Linux, "hci0")[..8],
            [
                "sudo", "-n", "hcitool", "-i", "hci0", "cmd", "0x08", "0x0006"
            ]
        );
    }

    #[test]
    fn linux_interval_scaling_matches_python_at_i64_boundaries() {
        let actual = [i64::MAX, i64::MIN]
            .into_iter()
            .map(|interval| {
                let args = linux_parameter_args(interval, interval);
                format!("{interval}: {}", args[..4].join(" "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            format!("{actual}\n"),
            include_str!("../../tests/fixtures/hci-interval-i64.txt")
        );
    }

    #[test]
    fn linux_advertising_payload_is_31_bytes_with_source_padding() {
        let args = linux_advertising_data_args(Ipv4Addr::new(192, 168, 1, 10), 7100);
        assert_eq!(args.len(), 31);
        assert_eq!(
            &args[..15],
            [
                "0x0e", "0x0d", "0xff", "0x4c", "0x00", "0x09", "0x08", "0x13", "0x30", "0xc0",
                "0xa8", "0x01", "0x0a", "0x1b", "0xbc"
            ]
        );
        assert!(args[15..].iter().all(|byte| byte == "0x00"));
    }

    #[test]
    fn freebsd_payload_and_command_use_source_format() {
        let address = Ipv4Addr::new(192, 168, 1, 10);
        assert_eq!(
            freebsd_advertising_data(address, 7100),
            "ff,4c,00,09,08,13,30,c0,a8,01,0a,1b,bc"
        );
        assert_eq!(
            HciOperation::Data {
                address,
                port: 7100
            }
            .command(Host::FreeBsd, "ubt0hci"),
            [
                "sudo",
                "-n",
                "hccontrol",
                "-n",
                "ubt0hci",
                "le_set_advertising_data",
                "-b",
                "ff,4c,00,09,08,13,30,c0,a8,01,0a,1b,bc"
            ]
        );
    }

    #[test]
    fn enable_disable_and_reset_commands_keep_platform_syntax() {
        assert_eq!(
            HciOperation::Enable(true).command(Host::Linux, "hci2"),
            [
                "sudo", "-n", "hcitool", "-i", "hci2", "cmd", "0x08", "0x000a", "0x01"
            ]
        );
        assert_eq!(
            HciOperation::Enable(false).command(Host::FreeBsd, "ubt1hci"),
            [
                "sudo",
                "-n",
                "hccontrol",
                "-n",
                "ubt1hci",
                "le_set_advertising_enable",
                "disable"
            ]
        );
        assert_eq!(
            reset_command(Host::Linux, "hci0"),
            ["sudo", "-n", "hciconfig", "hci0", "reset"]
        );
        assert_eq!(
            reset_command(Host::FreeBsd, "ubt0hci"),
            ["sudo", "-n", "hccontrol", "-n", "ubt0hci", "Reset"]
        );
    }

    #[test]
    fn adapter_listing_and_lmp_version_parsing_cover_both_os_outputs() {
        assert_eq!(linux_device_names("  hci0    00:11:22:33:44:55"), ["hci0"]);
        assert_eq!(linux_device_names("  hci12   00:11:22:33:44:55"), ["hci12"]);
        assert_eq!(
            linux_device_names("controller=hci12: 00:11:22:33:44:55"),
            ["hci12"],
            "Python's Linux regex finds hci digits anywhere in the listing"
        );
        assert_eq!(
            freebsd_device_name("ubt0hci: <Bluetooth Controller>"),
            Some("ubt0hci".into())
        );
        assert_eq!(
            freebsd_device_name(" ubt0hci: <Bluetooth Controller>"),
            None,
            "Python's FreeBSD regex is anchored at the start of each line"
        );
        assert_eq!(
            freebsd_device_name("ubt12hci: <Bluetooth Controller>"),
            Some("ubt12hci".into())
        );
        assert_eq!(
            parse_lmp_version(Host::Linux, "LMP Version: 5.3 (0xc)"),
            Some(12)
        );
        assert_eq!(
            parse_lmp_version(Host::Linux, "LMP Version: 5.3 (0x0c)"),
            None,
            "Python's Linux regex rejects version values with two hex digits"
        );
        assert_eq!(
            parse_lmp_version(Host::Linux, "LMP Version: malformed (0xZ) then (0xc)"),
            Some(12),
            "Python's non-greedy regex continues searching on the same line after a malformed candidate"
        );
        assert_eq!(
            parse_lmp_version(Host::FreeBsd, "LMP version: 6.0 [0x10]"),
            Some(16)
        );
        assert_eq!(
            parse_lmp_version(Host::FreeBsd, "LMP version: 6.0 0x10"),
            None,
            "Python's FreeBSD regex requires square brackets"
        );
        assert_eq!(
            parse_lmp_version(Host::FreeBsd, "LMP version: 6.0 [0x10"),
            None,
            "Python's FreeBSD regex requires a closing bracket"
        );
        assert_eq!(
            parse_lmp_version(Host::FreeBsd, "LMP version: 6.0 [0x10g]"),
            None,
            "Python's FreeBSD regex requires the hex value to end at the bracket"
        );
        assert_eq!(
            parse_lmp_version(Host::FreeBsd, "LMP version: malformed [0x10g] then [0x10]"),
            Some(16),
            "Python's regex continues searching after a malformed candidate"
        );
        assert_eq!(
            parse_lmp_version(
                Host::Linux,
                "LMP Version: malformed (0xZ)\nLMP Version: 5.3 (0xc)"
            ),
            Some(12),
            "the Python re.search scans past a malformed matching line"
        );
        assert_eq!(
            parse_lmp_version(
                Host::FreeBsd,
                "LMP version: malformed [0x]\nLMP version: 6.0 [0x10]"
            ),
            Some(16),
            "the Python re.search scans past a malformed matching line"
        );
    }

    #[test]
    fn linux_findall_discovers_each_adapter_name_on_a_line() {
        assert_eq!(
            linux_device_names("hci0 00:00 hci12 11:11 hciX hci3x"),
            ["hci0", "hci12", "hci3"]
        );
        assert_eq!(
            freebsd_device_names("ubt0hci: first ubt1hci: second"),
            ["ubt0hci"]
        );
    }

    #[test]
    fn adapter_listing_print_preserves_source_newline_and_trailing_spaces() {
        assert_eq!(
            python_printed_listing("Devices:\n  hci0 00:11:22\n"),
            "Devices:\n  hci0 00:11:22\n\n"
        );
        assert_eq!(
            python_printed_listing("Read_Node_List  "),
            "Read_Node_List  \n"
        );
    }

    #[test]
    fn command_failure_output_preserves_python_print_argument_whitespace() {
        assert_eq!(
            python_command_failure("beacon_on error:", b"adapter busy\r\n", b"status\n"),
            "beacon_on error: adapter busy\r\n status\n"
        );
        assert_eq!(
            python_command_failure("beacon_on error:", b"", b""),
            "beacon_on error:  "
        );
    }

    #[test]
    fn reset_failure_matches_python_print_and_preserves_separate_help_output() {
        let error = reset_failure_message(Host::Linux, b"permission denied\r\n", b"reset failed\n");
        assert_eq!(
            error,
            format!(
                "hci reset error: permission denied\r\n reset failed\n\n{}\ncannot continue: SystemExit(1)",
                help_text(Host::Linux)
            )
        );
    }

    #[test]
    fn setup_command_failure_still_attempts_advertising_enable_like_source() {
        let address = Ipv4Addr::new(192, 168, 1, 10);
        let mut operations = Vec::new();
        let result = start_sequence(address, 7100, 100, 250, |operation| {
            operations.push(operation);
            if matches!(operation, HciOperation::Parameters { .. }) {
                Err(HciCommandFailure::NonZeroExit(
                    "parameters rejected".to_owned(),
                ))
            } else {
                Ok(())
            }
        });

        assert_eq!(
            operations,
            [
                HciOperation::Parameters {
                    advmin: 100,
                    advmax: 250
                },
                HciOperation::Enable(true)
            ]
        );
        assert_eq!(result, Err(START_FAILURE.to_owned()));
    }

    #[test]
    fn setup_command_spawn_failure_propagates_without_enabling() {
        let mut operations = Vec::new();
        let result = start_sequence(Ipv4Addr::LOCALHOST, 7100, 100, 250, |operation| {
            operations.push(operation);
            Err(HciCommandFailure::Spawn("hcitool not found".to_owned()))
        });

        assert_eq!(
            operations,
            [HciOperation::Parameters {
                advmin: 100,
                advmax: 250
            }]
        );
        assert_eq!(result, Err("hcitool not found".to_owned()));
    }

    #[test]
    fn subprocess_spawn_tracebacks_match_source_for_reset_start_and_stop_paths() {
        let error = io::Error::from_raw_os_error(2);
        let address = Ipv4Addr::new(192, 0, 2, 8);
        let cases = [
            (
                HciSpawnContext::Reset,
                HciOperation::Enable(false),
                include_str!("../../tests/fixtures/source-hci-spawn-reset.txt"),
            ),
            (
                HciSpawnContext::Start,
                HciOperation::Parameters {
                    advmin: 100,
                    advmax: 100,
                },
                include_str!("../../tests/fixtures/source-hci-spawn-parameters.txt"),
            ),
            (
                HciSpawnContext::Start,
                HciOperation::Data {
                    address,
                    port: 7100,
                },
                include_str!("../../tests/fixtures/source-hci-spawn-data.txt"),
            ),
            (
                HciSpawnContext::Start,
                HciOperation::Enable(true),
                include_str!("../../tests/fixtures/source-hci-spawn-enable.txt"),
            ),
            (
                HciSpawnContext::Stop,
                HciOperation::Enable(false),
                include_str!("../../tests/fixtures/source-hci-spawn-stop.txt"),
            ),
            (
                HciSpawnContext::InterruptStop,
                HciOperation::Enable(false),
                include_str!("../../tests/fixtures/source-hci-spawn-interrupt-stop.txt"),
            ),
        ];

        for (context, operation, expected) in cases {
            let expected = crate::canonicalize_reference_fixture(expected);
            assert_eq!(
                hci_spawn_error_traceback(Host::Linux, "hci0", context, operation, &error),
                expected,
                "traceback for {context:?} / {operation:?}"
            );
        }
    }

    #[test]
    fn uncaught_stop_command_failures_match_source_tracebacks() {
        let operation = HciOperation::Enable(false);
        for (context, expected) in [
            (
                HciSpawnContext::Stop,
                include_str!("../../tests/fixtures/source-hci-stop-nonzero.txt"),
            ),
            (
                HciSpawnContext::InterruptStop,
                include_str!("../../tests/fixtures/source-hci-interrupt-stop-nonzero.txt"),
            ),
        ] {
            let expected = crate::canonicalize_reference_fixture(expected);
            assert_eq!(
                hci_called_process_error_traceback(
                    Host::Linux,
                    "hci0",
                    context,
                    operation,
                    Some(7)
                ),
                expected,
                "traceback for {context:?}"
            );
        }
    }

    #[test]
    fn spawn_os_errors_use_python_exception_class_and_errno_text() {
        assert_eq!(
            python_os_exception(Host::Linux, "sudo", &io::Error::from_raw_os_error(2)),
            "FileNotFoundError: [Errno 2] No such file or directory: 'sudo'"
        );
        assert_eq!(
            python_os_exception(Host::Linux, "sudo", &io::Error::from_raw_os_error(13)),
            "PermissionError: [Errno 13] Permission denied: 'sudo'"
        );
        assert_eq!(
            python_os_exception(Host::Linux, "hcitool", &io::Error::from_raw_os_error(20)),
            "NotADirectoryError: [Errno 20] Not a directory: 'hcitool'"
        );
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    #[test]
    fn unsupported_host_fails_before_attempting_hci_commands() {
        let host = match std::env::consts::OS {
            "windows" => "Windows",
            "macos" => "Darwin",
            other => other,
        };
        let expected = format!("{host} is not supported by the HCI module");
        assert_eq!(HciBackend::check_supported_host().unwrap_err(), expected);
        assert_eq!(HciBackend::find_device(None).unwrap_err(), expected);
    }
}
