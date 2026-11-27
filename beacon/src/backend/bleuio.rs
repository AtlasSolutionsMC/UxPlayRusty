//! BleuIO USB serial advertisement backend.

use crate::{BeaconBackend, BeaconOptions};
use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

const BLEUIO_VID: u16 = 0x2DCF;
const BAUD_RATE: u32 = 115_200;
const PORT_TIMEOUT: Duration = Duration::from_secs(1);
const COMMAND_SETTLE: Duration = Duration::from_millis(100);
const START_FAILURE: &str = "Failed to start beacon:\ngiving up, check Bluetooth adapter";

#[derive(Debug, Eq, PartialEq)]
enum AtCommandError {
    Serial(String),
    Unexpected(String),
}

impl std::fmt::Display for AtCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serial(message) | Self::Unexpected(message) => formatter.write_str(message),
        }
    }
}

#[derive(Debug)]
pub struct BleuIoBackend {
    serial_device: String,
    advertised_port: Option<u16>,
    advertised_address: Option<Ipv4Addr>,
    advertisement_data: Option<String>,
    advertisement_parameters: Option<String>,
}

impl BleuIoBackend {
    /// Finds a device with the BleuIO VID and verifies serial access with `AT`.
    pub fn find_device(requested: Option<&str>) -> Result<Self, String> {
        let ports = serialport::available_ports()
            .map_err(|error| format!("failed to enumerate serial devices: {error}"))?;
        let devices: Vec<_> = ports.iter().filter(|port| is_bleuio(port)).collect();

        let (selected, requested_found) = select_bleuio_device(&devices, requested);
        let Some(selected) = selected else {
            return Err(crate::no_devices_message(crate::BleType::BleuIo));
        };

        if !requested_found {
            if let Some(device) = devices.first() {
                crate::source_println!(
                    "=== detected BlueuIO 1. port: {} desc: {} hwid: {}",
                    device.port_name,
                    description(device),
                    hardware_id(device)
                );
            }
            for count in 2..=devices.len() {
                let [warning, override_hint] = additional_device_warning_lines(count);
                crate::source_println!("{warning}");
                crate::source_println!("{override_hint}");
            }
        }

        let mut port = open_serial(&selected.port_name).map_err(|error| {
            crate::source_println!("beacon_on: Serial port error: {error}");
            serial_permission_guidance(&selected.port_name)
        })?;
        send_at_command(&mut *port, "AT").map_err(|error| {
            crate::source_println!("beacon_on: Serial port error: {error}");
            serial_permission_guidance(&selected.port_name)
        })?;

        Ok(Self {
            serial_device: selected.port_name.clone(),
            advertised_port: None,
            advertised_address: None,
            advertisement_data: None,
            advertisement_parameters: None,
        })
    }

    pub fn serial_device(&self) -> &str {
        &self.serial_device
    }

    fn setup(&mut self, options: &BeaconOptions, port: u16) -> Result<(), String> {
        if options.index.is_some() {
            return Err(
                "uxplay_beacon_module_BleuIO called with value of index: not None".to_owned(),
            );
        }
        let min = options
            .advmin
            .ok_or_else(|| "advmin was not set".to_owned())?;
        let max = options
            .advmax
            .ok_or_else(|| "advmax was not set".to_owned())?;
        validate_intervals(min, max)?;
        if port == 0 {
            return Err(crate::BLEUIO_ZERO_PORT_ASSERTION_ERROR.to_owned());
        }

        self.advertisement_data = Some(advertisement_data(options.ipv4, port));
        self.advertisement_parameters = Some(advertisement_parameters(min, max));
        self.advertised_address = Some(options.ipv4);
        self.advertised_port = Some(port);
        Ok(())
    }
}

impl BeaconBackend for BleuIoBackend {
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String> {
        self.setup(options, port)?;
        crate::source_println!("Connecting to BleuIO dongle on {} ....", self.serial_device);
        let serial = open_serial(&self.serial_device).map_err(|error| error.to_string());
        let mut serial = match source_start_open_result(serial, port, |error| {
            crate::source_println!("beacon_on: Serial port error: {error}");
        }) {
            Ok(serial) => serial,
            Err(advertised_port) => {
                // Python's `return advertised_port` in `finally` overrides its
                // `SystemExit(1)` on SerialException, so start reports the port.
                return Ok(advertised_port);
            }
        };
        crate::source_println!("Connection established");
        let data_command =
            format_advertising_data_command(self.advertisement_data.as_deref().unwrap_or_default());
        let interval_command = format_advertising_start_command(
            self.advertisement_parameters.as_deref().unwrap_or_default(),
        );
        let command_result = (|| {
            send_at_command(&mut *serial, &data_command).map(|_| ())?;
            send_at_command(&mut *serial, &interval_command).map(|_| ())?;
            Ok(())
        })();
        source_start_command_result(command_result, port)?;
        let advertised_port = self
            .advertised_port
            .ok_or_else(|| "beacon port was not configured".to_owned())?;
        let advertised_address = self
            .advertised_address
            .ok_or_else(|| "beacon address was not configured".to_owned())?;
        crate::source_println!(
            "AirPlay Service Discovery advertising started, port = {advertised_port} ip address = {advertised_address}"
        );
        Ok(advertised_port)
    }

    fn stop(&mut self) -> Result<(), String> {
        let serial_device = self.serial_device.clone();
        self.stop_with_command(|command| {
            let mut serial = open_serial(&serial_device)
                .map_err(|error| AtCommandError::Serial(error.to_string()))?;
            send_at_command(&mut *serial, command)
        });
        Ok(())
    }
}

impl BleuIoBackend {
    fn stop_with_command(
        &mut self,
        send_command: impl FnOnce(&str) -> Result<String, AtCommandError>,
    ) -> bool {
        match send_command("AT+ADVSTOP") {
            Ok(_) => {
                crate::source_println!("AirPlay Service-Discovery beacon advertisement stopped");
                // The source assigns `advertised_Port` with a capital P, so
                // its `advertised_port` value remains set after a successful stop.
                self.advertised_address = None;
                self.advertisement_data = None;
                self.advertisement_parameters = None;
                true
            }
            Err(AtCommandError::Serial(error)) => {
                crate::source_println!("beacon_off: Serial port error: {error}");
                false
            }
            Err(AtCommandError::Unexpected(error)) => {
                crate::source_println!("beacon_off: An unexpected error occurred: {error}");
                false
            }
        }
    }
}

fn source_start_open_result<T>(
    open_result: Result<T, String>,
    advertised_port: u16,
    report_error: impl FnOnce(&str),
) -> Result<T, u16> {
    match open_result {
        Ok(serial) => Ok(serial),
        Err(error) => {
            report_error(&error);
            Err(advertised_port)
        }
    }
}

fn additional_device_warning_lines(count: usize) -> [String; 2] {
    [
        format!("warning: {count} BleueIO devices were found, the first found will be used"),
        "(to override this choice, specify \"--device =...\" in optional arguments)".to_owned(),
    ]
}

fn serial_permission_guidance(device: &str) -> String {
    format!(
        "\n  The user does not have sufficient privileges to access this serial port:\n  On Linux, the user should be added to the \"dialout\" or \"uucp\" group\n  On BSD systems, the necesary group is usually the \"dialer\" group.\n  The correct group can be found using  \"ls -l {device}\""
    )
}

fn is_bleuio(port: &SerialPortInfo) -> bool {
    match &port.port_type {
        SerialPortType::UsbPort(info) => info.vid == BLEUIO_VID,
        _ => false,
    }
}

fn select_bleuio_device<'a>(
    devices: &[&'a SerialPortInfo],
    requested: Option<&str>,
) -> (Option<&'a SerialPortInfo>, bool) {
    let requested_device =
        requested.and_then(|name| devices.iter().find(|port| port.port_name == name).copied());
    let requested_found = requested_device.is_some();
    (
        requested_device.or_else(|| devices.first().copied()),
        requested_found,
    )
}

fn description(port: &SerialPortInfo) -> String {
    match &port.port_type {
        SerialPortType::UsbPort(info) => info.product.clone().unwrap_or_else(|| "n/a".to_owned()),
        _ => "n/a".to_owned(),
    }
}

fn hardware_id(port: &SerialPortInfo) -> String {
    match &port.port_type {
        SerialPortType::UsbPort(info) => {
            let mut hardware_id = format!("USB VID:PID={:04X}:{:04X}", info.vid, info.pid);
            if let Some(serial_number) = &info.serial_number {
                hardware_id.push_str(&format!(" SER={serial_number}"));
            }
            hardware_id
        }
        _ => "n/a".to_owned(),
    }
}

fn open_serial(device: &str) -> Result<Box<dyn SerialPort>, serialport::Error> {
    serialport::new(device, BAUD_RATE)
        .timeout(PORT_TIMEOUT)
        .open()
}

trait SerialIo {
    fn write_command(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn bytes_to_read(&self) -> serialport::Result<u32>;
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

impl<T: SerialPort + ?Sized> SerialIo for T {
    fn write_command(&mut self, bytes: &[u8]) -> io::Result<()> {
        Write::write_all(self, bytes)
    }

    fn bytes_to_read(&self) -> serialport::Result<u32> {
        SerialPort::bytes_to_read(self)
    }

    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        Read::read(self, buffer)
    }
}

fn send_at_command(serial: &mut dyn SerialPort, command: &str) -> Result<String, AtCommandError> {
    send_at_command_with_timing(serial, command, COMMAND_SETTLE)
}

fn send_at_command_with_timing<S: SerialIo + ?Sized>(
    serial: &mut S,
    command: &str,
    settle: Duration,
) -> Result<String, AtCommandError> {
    serial
        .write_command(format!("{command}\r\n").as_bytes())
        .map_err(|error| AtCommandError::Serial(error.to_string()))?;
    thread::sleep(settle);
    let mut response = String::new();
    while serial
        .bytes_to_read()
        .map_err(|error| AtCommandError::Serial(error.to_string()))?
        > 0
    {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            // Each one-byte read gets the port's configured timeout anew,
            // matching the repeated read(1) calls in pyserial's IOBase.readline().
            match serial.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    line.push(byte[0]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                Err(error) => return Err(AtCommandError::Serial(error.to_string())),
            }
        }
        response.push_str(&decode_serial_response_line(line)?);
    }
    Ok(response)
}

fn decode_serial_response_line(line: Vec<u8>) -> Result<String, AtCommandError> {
    String::from_utf8(line).map_err(|error| {
        let message = crate::python_unicode_decode_error(error.as_bytes(), error.utf8_error());
        let message = message
            .strip_prefix("UnicodeDecodeError: ")
            .expect("the Python decoder helper always prefixes its exception class");
        AtCommandError::Unexpected(message.to_owned())
    })
}

fn source_start_command_result(
    command_result: Result<(), AtCommandError>,
    advertised_port: u16,
) -> Result<u16, String> {
    match command_result {
        Ok(()) => Ok(advertised_port),
        Err(AtCommandError::Serial(error)) => {
            crate::source_println!("beacon_on: Serial port error: {error}");
            // The Python `finally` return suppresses its SystemExit on serial errors.
            Ok(advertised_port)
        }
        Err(AtCommandError::Unexpected(error)) => {
            crate::source_println!("beacon_on: An unexpected error occurred: {error}");
            Err(START_FAILURE.to_owned())
        }
    }
}

/// Exact manufacturer-specific AD structure sent to `AT+ADVDATA=`.
pub fn manufacturer_ad_data(ipv4: Ipv4Addr, port: u16) -> [u8; 14] {
    let mut data = [0; 14];
    data[0] = 13;
    data[1] = 0xff;
    data[2] = 0x4c;
    data[3] = 0x00;
    data[4..8].copy_from_slice(&[0x09, 0x08, 0x13, 0x30]);
    data[8..12].copy_from_slice(&ipv4.octets());
    data[12..14].copy_from_slice(&port.to_be_bytes());
    data
}

pub fn advertisement_data(ipv4: Ipv4Addr, port: u16) -> String {
    manufacturer_ad_data(ipv4, port)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn advertisement_parameters(advmin: i64, advmax: i64) -> String {
    format!("0;{advmin};{advmax};0;")
}

pub fn format_advertising_data_command(data: &str) -> String {
    format!("AT+ADVDATA={data}")
}

pub fn format_advertising_start_command(parameters: &str) -> String {
    format!("AT+ADVSTART={parameters}")
}

pub fn validate_intervals(advmin: i64, advmax: i64) -> Result<(), String> {
    if advmin < 100 {
        return Err("advmin was smaller than 100 msecs".to_owned());
    }
    if advmax < advmin {
        return Err("advmax  was smaller than advmin".to_owned());
    }
    if advmax > 10_240 {
        return Err("advmax was larger than 10240 msecs".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AtCommandError, additional_device_warning_lines, advertisement_data,
        advertisement_parameters, decode_serial_response_line, description,
        format_advertising_data_command, format_advertising_start_command, hardware_id,
        manufacturer_ad_data, select_bleuio_device, send_at_command_with_timing,
        serial_permission_guidance, source_start_open_result, validate_intervals,
    };
    use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};
    use std::collections::VecDeque;
    use std::io;
    use std::net::Ipv4Addr;
    use std::thread;
    use std::time::Duration;

    struct DelayedSerial {
        response: VecDeque<u8>,
        write_buffer: Vec<u8>,
        byte_delay: Duration,
        timeout: Duration,
    }

    impl super::SerialIo for DelayedSerial {
        fn write_command(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.write_buffer.extend_from_slice(bytes);
            Ok(())
        }

        fn bytes_to_read(&self) -> serialport::Result<u32> {
            Ok(self.response.len() as u32)
        }

        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            assert!(self.byte_delay < self.timeout);
            thread::sleep(self.byte_delay);
            let Some(byte) = self.response.pop_front() else {
                return Ok(0);
            };
            buffer[0] = byte;
            Ok(1)
        }
    }

    #[test]
    fn zero_handoff_port_preserves_source_assertion_before_serial_io() {
        let mut backend = super::BleuIoBackend {
            serial_device: "mock-port".to_owned(),
            advertised_port: None,
            advertised_address: None,
            advertisement_data: None,
            advertisement_parameters: None,
        };
        let options = crate::BeaconOptions {
            ble_type: crate::BleType::BleuIo,
            file: None,
            path: "handoff".into(),
            ipv4: Ipv4Addr::LOCALHOST,
            advmin: Some(100),
            advmax: Some(100),
            index: None,
            device: None,
            exit_when_stopped: false,
            notices: Vec::new(),
        };

        assert_eq!(
            backend.setup(&options, 0).unwrap_err(),
            crate::BLEUIO_ZERO_PORT_ASSERTION_ERROR
        );
        assert_eq!(backend.advertised_port, None);
        assert_eq!(backend.advertisement_data, None);
    }

    #[test]
    fn usb_hardware_id_uses_pyserial_format_and_serial_suffix() {
        let port = SerialPortInfo {
            port_name: "COM5".to_owned(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x2DCF,
                pid: 0x6002,
                serial_number: Some("ABC123".to_owned()),
                manufacturer: None,
                product: None,
            }),
        };

        assert_eq!(description(&port), "n/a");
        assert_eq!(hardware_id(&port), "USB VID:PID=2DCF:6002 SER=ABC123");
    }

    #[test]
    fn missing_port_description_and_hardware_id_use_pyserial_defaults() {
        let port = SerialPortInfo {
            port_name: "COM5".to_owned(),
            port_type: SerialPortType::Unknown,
        };

        assert_eq!(description(&port), "n/a");
        assert_eq!(hardware_id(&port), "n/a");
    }

    #[test]
    fn unmatched_requested_device_falls_back_to_first_bleuio_like_python() {
        let devices = [SerialPortInfo {
            port_name: "COM5".to_owned(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x2DCF,
                pid: 0x6002,
                serial_number: None,
                manufacturer: None,
                product: Some("Fake BleuIO".to_owned()),
            }),
        }];
        let matching_bleuio: Vec<_> = devices.iter().collect();

        let (selected, requested_found) =
            select_bleuio_device(&matching_bleuio, Some("COM_MISSING"));

        assert_eq!(selected.map(|port| port.port_name.as_str()), Some("COM5"));
        assert!(!requested_found);
        let (exact, requested_found) = select_bleuio_device(&matching_bleuio, Some("COM5"));
        assert_eq!(exact.map(|port| port.port_name.as_str()), Some("COM5"));
        assert!(requested_found);
    }

    #[test]
    fn manufacturer_ad_has_length_apple_id_airplay_ip_and_big_endian_port() {
        let data = manufacturer_ad_data(Ipv4Addr::new(192, 168, 1, 10), 7100);
        assert_eq!(
            data,
            [
                13, 0xff, 0x4c, 0, 9, 8, 0x13, 0x30, 192, 168, 1, 10, 0x1b, 0xbc
            ]
        );
        assert_eq!(
            advertisement_data(Ipv4Addr::new(192, 168, 1, 10), 7100),
            "0d:ff:4c:00:09:08:13:30:c0:a8:01:0a:1b:bc"
        );
        assert_eq!(
            format_advertising_data_command(&advertisement_data(
                Ipv4Addr::new(192, 168, 1, 10),
                7100
            )),
            "AT+ADVDATA=0d:ff:4c:00:09:08:13:30:c0:a8:01:0a:1b:bc"
        );
    }

    #[test]
    fn interval_command_matches_source_syntax_and_bounds() {
        assert_eq!(advertisement_parameters(100, 250), "0;100;250;0;");
        assert_eq!(
            format_advertising_start_command(&advertisement_parameters(100, 250)),
            "AT+ADVSTART=0;100;250;0;"
        );
        assert!(validate_intervals(100, 100).is_ok());
        assert_eq!(
            validate_intervals(99, 100).unwrap_err(),
            "advmin was smaller than 100 msecs"
        );
        assert_eq!(
            validate_intervals(200, 100).unwrap_err(),
            "advmax  was smaller than advmin"
        );
        assert_eq!(
            validate_intervals(100, 10_241).unwrap_err(),
            "advmax was larger than 10240 msecs"
        );
    }

    #[test]
    fn stop_failure_keeps_source_beacon_state_and_does_not_report_success() {
        let mut backend = super::BleuIoBackend {
            serial_device: "mock-port".to_owned(),
            advertised_port: Some(7100),
            advertised_address: Some(Ipv4Addr::new(192, 168, 1, 10)),
            advertisement_data: Some("ad-data".to_owned()),
            advertisement_parameters: Some("ad-parameters".to_owned()),
        };

        let stopped = backend.stop_with_command(|command| {
            assert_eq!(command, "AT+ADVSTOP");
            Err(super::AtCommandError::Serial("mock timeout".to_owned()))
        });

        assert!(!stopped);
        assert_eq!(backend.advertised_port, Some(7100));
        assert_eq!(
            backend.advertised_address,
            Some(Ipv4Addr::new(192, 168, 1, 10))
        );
        assert_eq!(backend.advertisement_data.as_deref(), Some("ad-data"));
        assert_eq!(
            backend.advertisement_parameters.as_deref(),
            Some("ad-parameters")
        );
    }

    #[test]
    fn successful_stop_clears_source_fields_but_retains_misspelled_port_global() {
        let mut backend = super::BleuIoBackend {
            serial_device: "mock-port".to_owned(),
            advertised_port: Some(7100),
            advertised_address: Some(Ipv4Addr::new(192, 168, 1, 10)),
            advertisement_data: Some("ad-data".to_owned()),
            advertisement_parameters: Some("ad-parameters".to_owned()),
        };

        let stopped = backend.stop_with_command(|command| {
            assert_eq!(command, "AT+ADVSTOP");
            Ok(String::new())
        });

        assert!(stopped);
        assert_eq!(backend.advertised_port, Some(7100));
        assert_eq!(backend.advertised_address, None);
        assert_eq!(backend.advertisement_data, None);
        assert_eq!(backend.advertisement_parameters, None);
    }

    #[test]
    fn serial_open_exception_returns_the_configured_port_like_python_finally() {
        let mut errors = Vec::new();
        let result =
            source_start_open_result::<()>(Err("access denied".to_owned()), 7100, |error| {
                errors.push(error.to_owned());
            });

        assert_eq!(result, Err(7100));
        assert_eq!(errors, ["access denied"]);
    }

    #[test]
    fn start_serial_exception_returns_port_but_unexpected_error_fails_start() {
        assert_eq!(
            super::source_start_command_result(
                Err(super::AtCommandError::Serial("write failed".to_owned())),
                7100
            ),
            Ok(7100)
        );
        assert_eq!(
            super::source_start_command_result(
                Err(super::AtCommandError::Unexpected(
                    "invalid UTF-8".to_owned()
                )),
                7100
            ),
            Err(super::START_FAILURE.to_owned())
        );
    }

    #[test]
    fn invalid_serial_utf8_uses_python_unicode_decode_messages() {
        for (bytes, expected) in [
            (
                vec![0xff],
                "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
            ),
            (
                vec![0xe2, b'('],
                "'utf-8' codec can't decode byte 0xe2 in position 0: invalid continuation byte",
            ),
            (
                vec![0xe2, 0x82],
                "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data",
            ),
        ] {
            assert_eq!(
                decode_serial_response_line(bytes).unwrap_err(),
                AtCommandError::Unexpected(expected.to_owned())
            );
        }
    }

    #[test]
    fn serial_line_gives_each_byte_a_fresh_pyserial_timeout() {
        let timeout = Duration::from_millis(30);
        let byte_delay = Duration::from_millis(20);
        let mut serial = DelayedSerial {
            response: b"abc\n".iter().copied().collect(),
            write_buffer: Vec::new(),
            byte_delay,
            timeout,
        };
        let started = std::time::Instant::now();

        let response = send_at_command_with_timing(&mut serial, "AT", Duration::ZERO).unwrap();

        assert_eq!(response, "abc");
        assert_eq!(serial.write_buffer, b"AT\r\n");
        assert!(started.elapsed() > timeout);
    }

    #[test]
    fn multiple_device_warnings_match_python_count_and_spacing() {
        assert_eq!(
            additional_device_warning_lines(2),
            [
                "warning: 2 BleueIO devices were found, the first found will be used",
                "(to override this choice, specify \"--device =...\" in optional arguments)"
            ]
        );
        assert_eq!(
            additional_device_warning_lines(3)[0],
            "warning: 3 BleueIO devices were found, the first found will be used"
        );
    }

    #[test]
    fn serial_probe_failure_guidance_matches_python_multiline_output() {
        assert_eq!(
            serial_permission_guidance("/dev/ttyUSB0"),
            "\n  The user does not have sufficient privileges to access this serial port:\n  On Linux, the user should be added to the \"dialout\" or \"uucp\" group\n  On BSD systems, the necesary group is usually the \"dialer\" group.\n  The correct group can be found using  \"ls -l /dev/ttyUSB0\""
        );
    }
}
