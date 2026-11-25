//! macOS peer-to-peer startup checks from `uxplay.cpp`.

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AirPlayReceiverSetting {
    Enabled,
    Disabled,
    MissingOrUnrecognized,
}

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_SETTINGS_HINT: &str = "macOS  point-to-point Airplay settings are in System Settings->General->AirDrop & Continuity->AirPlay->AirPlayReceiver";
#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_ENABLED_MESSAGE: &str = " macOS host reported AirplayReceiverEnabled is true";
#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_DISABLED_MESSAGE: &str =
    "*** ERROR:  macOS host reported AirplayReceiverEnabled was not true";
#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_MISSING_MESSAGE: &str = "*** ERROR:  macOS host did not report a value for AirplayReceiverEnabled; try switching AirPlay Receiver off then on";

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_ENABLED_LINES: &[&str] = &[RECEIVER_SETTINGS_HINT, RECEIVER_ENABLED_MESSAGE];
#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_DISABLED_LINES: &[&str] = &[RECEIVER_SETTINGS_HINT, RECEIVER_DISABLED_MESSAGE];
#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_MISSING_LINES: &[&str] = &[RECEIVER_SETTINGS_HINT, RECEIVER_MISSING_MESSAGE];

/// Output emitted by the C startup check through its `LOGI`/`LOGE` logger.
/// Every line goes to stdout, including the error line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReceiverStartupOutput {
    pub(crate) stdout_lines: &'static [&'static str],
    pub(crate) exit_code: u8,
}

#[cfg(not(all(target_os = "macos", feature = "apple-p2p")))]
const RECEIVER_CHECK_NOT_REQUIRED: ReceiverStartupOutput = ReceiverStartupOutput {
    stdout_lines: &[],
    exit_code: 0,
};

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
fn airplay_receiver_setting(output: &[u8]) -> AirPlayReceiverSetting {
    // The C implementation removes trailing spaces, tabs, CR, and LF but does
    // not trim leading whitespace or otherwise normalize the value.
    let end = output
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(0, |index| index + 1);
    match &output[..end] {
        b"1" | b"true" => AirPlayReceiverSetting::Enabled,
        b"0" | b"false" => AirPlayReceiverSetting::Disabled,
        _ => AirPlayReceiverSetting::MissingOrUnrecognized,
    }
}

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
fn receiver_startup_output(setting: AirPlayReceiverSetting) -> ReceiverStartupOutput {
    match setting {
        AirPlayReceiverSetting::Enabled => ReceiverStartupOutput {
            stdout_lines: RECEIVER_ENABLED_LINES,
            exit_code: 0,
        },
        AirPlayReceiverSetting::Disabled => ReceiverStartupOutput {
            stdout_lines: RECEIVER_DISABLED_LINES,
            exit_code: 1,
        },
        AirPlayReceiverSetting::MissingOrUnrecognized => ReceiverStartupOutput {
            stdout_lines: RECEIVER_MISSING_LINES,
            exit_code: 1,
        },
    }
}

#[cfg(any(test, all(target_os = "macos", feature = "apple-p2p")))]
fn receiver_startup_output_from_bytes(output: &[u8]) -> ReceiverStartupOutput {
    receiver_startup_output(airplay_receiver_setting(output))
}

/// Check the macOS current-host AirPlay Receiver setting and return the exact
/// ordered stdout lines and exit code used by the source startup path.
pub(crate) fn check_airplay_receiver_startup() -> ReceiverStartupOutput {
    #[cfg(all(target_os = "macos", feature = "apple-p2p"))]
    {
        use std::process::{Command, Stdio};

        let output = Command::new("defaults")
            .args([
                "-currentHost",
                "read",
                "com.apple.controlcenter",
                "AirplayReceiverEnabled",
            ])
            .stderr(Stdio::null())
            .output()
            .map_or_else(|_| Vec::new(), |output| output.stdout);

        return receiver_startup_output_from_bytes(&output);
    }

    #[cfg(not(all(target_os = "macos", feature = "apple-p2p")))]
    RECEIVER_CHECK_NOT_REQUIRED
}

#[cfg(test)]
mod tests {
    use super::{
        AirPlayReceiverSetting, RECEIVER_DISABLED_MESSAGE, RECEIVER_ENABLED_MESSAGE,
        RECEIVER_MISSING_MESSAGE, RECEIVER_SETTINGS_HINT, airplay_receiver_setting,
        receiver_startup_output_from_bytes,
    };

    #[test]
    fn receiver_setting_accepts_only_source_truth_values_after_trailing_whitespace() {
        for output in [b"1".as_slice(), b"true", b"true \t\r\n"] {
            assert_eq!(
                airplay_receiver_setting(output),
                AirPlayReceiverSetting::Enabled
            );
        }
        for output in [b"0".as_slice(), b"false", b"false\n"] {
            assert_eq!(
                airplay_receiver_setting(output),
                AirPlayReceiverSetting::Disabled
            );
        }
        for output in [b"".as_slice(), b"TRUE", b" true", b"true extra"] {
            assert_eq!(
                airplay_receiver_setting(output),
                AirPlayReceiverSetting::MissingOrUnrecognized
            );
        }
    }

    #[test]
    fn startup_output_matches_source_order_stream_shape_and_exit_status() {
        let enabled = receiver_startup_output_from_bytes(b"true\n");
        assert_eq!(
            enabled.stdout_lines,
            &[RECEIVER_SETTINGS_HINT, RECEIVER_ENABLED_MESSAGE]
        );
        assert_eq!(enabled.exit_code, 0);

        let disabled = receiver_startup_output_from_bytes(b"false\n");
        assert_eq!(
            disabled.stdout_lines,
            &[RECEIVER_SETTINGS_HINT, RECEIVER_DISABLED_MESSAGE]
        );
        assert_eq!(disabled.exit_code, 1);

        let missing = receiver_startup_output_from_bytes(b"");
        assert_eq!(
            missing.stdout_lines,
            &[RECEIVER_SETTINGS_HINT, RECEIVER_MISSING_MESSAGE]
        );
        assert_eq!(missing.exit_code, 1);
        assert!(RECEIVER_DISABLED_MESSAGE.starts_with("*** ERROR:  "));
        assert!(RECEIVER_MISSING_MESSAGE.starts_with("*** ERROR:  "));
    }
}
