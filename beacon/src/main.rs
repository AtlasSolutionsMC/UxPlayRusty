use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uxplay_beacon::backend::bleuio::BleuIoBackend;
#[cfg(target_os = "linux")]
use uxplay_beacon::backend::bluez::BluezBackend;
use uxplay_beacon::backend::hci::HciBackend;
#[cfg(target_os = "windows")]
use uxplay_beacon::backend::winrt::WinRtBackend;
use uxplay_beacon::{
    BeaconBackend, BeaconOptions, BleType, SystemProcessVerifier, entry_options,
    python_traceback_for_monitor_error, run_monitor,
};

struct MissingBackend {
    name: &'static str,
}

impl BeaconBackend for MissingBackend {
    fn start(&mut self, _: &BeaconOptions, _: u16) -> Result<u16, String> {
        Err(format!(
            "BLE backend {} is not available in this build",
            self.name
        ))
    }

    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}

fn main() -> ExitCode {
    let options = match entry_options() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(code) => return code,
    };
    let [will_use, import_message] = module_intro_lines(options.ble_type);
    uxplay_beacon::source_println!("{will_use}");
    if options.ble_type == BleType::Hci
        && let Err(error) = HciBackend::check_supported_host()
    {
        uxplay_beacon::source_println!("{error}");
        return ExitCode::from(1);
    }
    uxplay_beacon::source_println!("{import_message}");

    let mut backend: Box<dyn BeaconBackend> = match options.ble_type {
        BleType::BleuIo => match BleuIoBackend::find_device(options.device.as_deref()) {
            Ok(backend) => {
                if let Some([error, hint]) =
                    requested_device_mismatch(options.device.as_deref(), backend.serial_device())
                {
                    uxplay_beacon::source_println!("{error}");
                    uxplay_beacon::source_println!("{hint}");
                    return ExitCode::from(1);
                }
                uxplay_beacon::source_println!(
                    "using the required device found at {}",
                    backend.serial_device()
                );
                Box::new(backend)
            }
            Err(error) => {
                if !error.is_empty() {
                    uxplay_beacon::source_println!("{error}");
                }
                return ExitCode::from(1);
            }
        },
        BleType::Hci => match HciBackend::find_device(options.device.as_deref()) {
            Ok(backend) => {
                if let Some([error, hint]) =
                    requested_device_mismatch(options.device.as_deref(), backend.device())
                {
                    uxplay_beacon::source_println!("{error}");
                    uxplay_beacon::source_println!("{hint}");
                    return ExitCode::from(1);
                }
                uxplay_beacon::source_println!(
                    "using the required device found at {}",
                    backend.device()
                );
                Box::new(backend)
            }
            Err(error) => {
                if error.starts_with("Traceback (most recent call last):") {
                    uxplay_beacon::source_eprint!("{error}");
                } else {
                    uxplay_beacon::source_println!("{error}");
                }
                return ExitCode::from(1);
            }
        },
        BleType::BlueZ => {
            #[cfg(target_os = "linux")]
            {
                match BluezBackend::find_device() {
                    Ok(backend) => Box::new(backend),
                    Err(error) => {
                        uxplay_beacon::source_println!("{error}");
                        return ExitCode::from(1);
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                Box::new(MissingBackend { name: "BlueZ" })
            }
        }
        BleType::WinRt => {
            #[cfg(target_os = "windows")]
            {
                Box::new(WinRtBackend::new())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Box::new(MissingBackend { name: "winrt" })
            }
        }
    };

    if matches!(options.ble_type, BleType::BlueZ | BleType::WinRt) {
        match backend.preflight(&options) {
            Ok(true) if options.ble_type == BleType::WinRt => {
                uxplay_beacon::source_println!("test passed ({})", options.ble_type.source_name());
            }
            Ok(true) => {}
            Ok(false) => {}
            Err(error) => {
                uxplay_beacon::source_println!("{error}");
                return ExitCode::from(1);
            }
        }
    }

    uxplay_beacon::source_println!("{}", startup_summary(&options));
    uxplay_beacon::source_println!("Advertising IP address {}", options.ipv4);
    uxplay_beacon::source_println!("(Press Ctrl+C to exit)");

    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_state = Arc::clone(&interrupted);
    if let Err(error) = ctrlc::set_handler(move || signal_state.store(true, Ordering::SeqCst)) {
        uxplay_beacon::source_println!("cannot install Ctrl+C handler: {error}");
        return ExitCode::from(1);
    }

    let mut verifier = SystemProcessVerifier::default();
    match run_monitor(&options, &mut backend, &mut verifier, || {
        interrupted.load(Ordering::SeqCst)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.starts_with("Traceback (most recent call last):") {
                uxplay_beacon::source_eprint!("{error}");
            } else if let Some(traceback) = python_traceback_for_monitor_error(&error) {
                uxplay_beacon::source_eprint!("{traceback}");
            } else {
                uxplay_beacon::source_println!("{error}");
            }
            ExitCode::from(1)
        }
    }
}

fn requested_device_mismatch(requested: Option<&str>, selected: &str) -> Option<[String; 2]> {
    let requested = requested.filter(|requested| *requested != selected)?;
    Some([
        format!(
            "Error: A required device was NOT found at  {requested} given as an optional argument"
        ),
        "(Note: required devices WERE found and are listed above)".to_owned(),
    ])
}

fn module_intro_lines(ble_type: BleType) -> [String; 2] {
    let module_name = format!("uxplay_beacon_module_{}", ble_type.source_name());
    let import_message = match ble_type {
        BleType::BleuIo => format!("Imported {module_name}"),
        BleType::BlueZ => format!("loaded {module_name} "),
        BleType::Hci | BleType::WinRt => format!("loaded {module_name}"),
    };
    [
        format!("Will use BLE module {module_name}.py"),
        import_message,
    ]
}

fn startup_summary(options: &BeaconOptions) -> String {
    let advminmax = if options.ble_type == BleType::WinRt {
        String::new()
    } else {
        format!(
            "[advmin:advmax]={}:{}",
            options.advmin.unwrap_or(100),
            options.advmax.unwrap_or(100)
        )
    };
    let index = if options.ble_type == BleType::BlueZ {
        format!("index {}", options.index.unwrap_or(0))
    } else {
        String::new()
    };
    format!(
        "AirPlay Service-Discovery Bluetooth LE beacon: BLE file {} {} {}",
        options.path.display(),
        advminmax,
        index
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options(ble_type: BleType) -> BeaconOptions {
        BeaconOptions {
            ble_type,
            file: None,
            path: "beacon.ble".into(),
            ipv4: "192.0.2.8".parse().unwrap(),
            advmin: (ble_type != BleType::WinRt).then_some(100),
            advmax: (ble_type != BleType::WinRt).then_some(100),
            index: (ble_type == BleType::BlueZ).then_some(0),
            device: None,
            exit_when_stopped: false,
            notices: Vec::new(),
        }
    }

    #[test]
    fn bluez_summary_contains_intervals_and_index() {
        assert_eq!(
            startup_summary(&sample_options(BleType::BlueZ)),
            "AirPlay Service-Discovery Bluetooth LE beacon: BLE file beacon.ble [advmin:advmax]=100:100 index 0"
        );
    }

    #[test]
    fn winrt_summary_preserves_the_source_blank_fields() {
        assert_eq!(
            startup_summary(&sample_options(BleType::WinRt)),
            "AirPlay Service-Discovery Bluetooth LE beacon: BLE file beacon.ble  "
        );
    }

    #[test]
    fn module_intro_orders_selection_before_python_module_import() {
        assert_eq!(
            module_intro_lines(BleType::Hci),
            [
                "Will use BLE module uxplay_beacon_module_HCI.py",
                "loaded uxplay_beacon_module_HCI"
            ]
        );
        assert_eq!(
            module_intro_lines(BleType::BleuIo),
            [
                "Will use BLE module uxplay_beacon_module_BleuIO.py",
                "Imported uxplay_beacon_module_BleuIO"
            ]
        );
    }

    #[test]
    fn serial_summary_has_trailing_index_field_space() {
        assert_eq!(
            startup_summary(&sample_options(BleType::BleuIo)),
            "AirPlay Service-Discovery Bluetooth LE beacon: BLE file beacon.ble [advmin:advmax]=100:100 "
        );
    }

    #[test]
    fn requested_device_mismatch_matches_python_post_probe_diagnostics() {
        assert_eq!(
            requested_device_mismatch(Some("COM_MISSING"), "COM_TEST"),
            Some([
                "Error: A required device was NOT found at  COM_MISSING given as an optional argument"
                    .to_owned(),
                "(Note: required devices WERE found and are listed above)".to_owned(),
            ])
        );
        assert_eq!(
            requested_device_mismatch(Some("COM_TEST"), "COM_TEST"),
            None
        );
        assert_eq!(requested_device_mismatch(None, "COM_TEST"), None);
    }
}
