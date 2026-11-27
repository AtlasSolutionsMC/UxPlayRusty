//! Wire data shared by the UxPlay BLE file handoff and advertisement backends.

use std::net::Ipv4Addr;

const FILE_HEADER_LEN: usize = 6;
const APPLE_COMPANY_ID: [u8; 2] = [0x4c, 0x00];
const AIRPLAY_DATA_UNIT: [u8; 4] = [0x09, 0x08, 0x13, 0x30];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BeaconFileData {
    /// The RAOP TCP port written by UxPlay, encoded little-endian in the file.
    pub raop_port: u16,
    pub pid: u32,
    /// The original writes `argv[0]` bytes without a trailing NUL.
    pub process_name: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BeaconFileError {
    Truncated {
        expected_at_least: usize,
        actual: usize,
    },
}

impl std::fmt::Display for BeaconFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated {
                expected_at_least,
                actual,
            } => write!(
                f,
                "beacon file is truncated: expected at least {expected_at_least} bytes, got {actual}"
            ),
        }
    }
}

impl std::error::Error for BeaconFileError {}

/// Encodes the file UxPlay writes for the independent beacon process.
pub fn encode_beacon_file(raop_port: u16, pid: u32, process_name: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FILE_HEADER_LEN + process_name.len());
    bytes.extend_from_slice(&raop_port.to_le_bytes());
    bytes.extend_from_slice(&pid.to_le_bytes());
    bytes.extend_from_slice(process_name);
    bytes
}

/// Decodes the fixed-width header and process-name bytes from a beacon handoff file.
pub fn decode_beacon_file(bytes: &[u8]) -> Result<BeaconFileData, BeaconFileError> {
    if bytes.len() < FILE_HEADER_LEN {
        return Err(BeaconFileError::Truncated {
            expected_at_least: FILE_HEADER_LEN,
            actual: bytes.len(),
        });
    }
    Ok(BeaconFileData {
        raop_port: u16::from_le_bytes([bytes[0], bytes[1]]),
        pid: u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
        process_name: bytes[FILE_HEADER_LEN..]
            .split(|byte| *byte == 0)
            .next()
            .unwrap_or_default()
            .to_vec(),
    })
}

/// The Apple manufacturer data (company ID is carried separately by BlueZ/WinRT).
///
/// The bytes are an Apple AirPlay data unit followed by IPv4 octets and a big-endian port.
pub fn manufacturer_data(ipv4: Ipv4Addr, port: u16) -> [u8; 10] {
    let mut data = [0; 10];
    data[..4].copy_from_slice(&AIRPLAY_DATA_UNIT);
    data[4..8].copy_from_slice(&ipv4.octets());
    data[8..10].copy_from_slice(&port.to_be_bytes());
    data
}

/// Builds the 31-byte payload supplied to the reference's raw HCI backend.
///
/// This preserves its legacy advertising-data layout: total length, AD-structure length,
/// manufacturer type, Apple company ID, AirPlay data, then zero padding.
pub fn hci_advertising_data(ipv4: Ipv4Addr, port: u16) -> [u8; 31] {
    let mut data = [0; 31];
    data[0] = 14;
    data[1] = 13;
    data[2] = 0xff;
    data[3..5].copy_from_slice(&APPLE_COMPANY_ID);
    data[5..15].copy_from_slice(&manufacturer_data(ipv4, port));
    data
}

#[cfg(test)]
mod tests {
    use super::{
        BeaconFileData, BeaconFileError, decode_beacon_file, encode_beacon_file,
        hci_advertising_data, manufacturer_data,
    };
    use std::net::Ipv4Addr;

    #[test]
    fn beacon_file_uses_little_endian_port_and_pid_without_a_nul_terminator() {
        let bytes = encode_beacon_file(7100, 0x1234_5678, b"C:\\UxPlay\\uxplay.exe");
        assert_eq!(&bytes[..6], &[0xbc, 0x1b, 0x78, 0x56, 0x34, 0x12]);
        assert_eq!(bytes.last(), Some(&b'e'));
        assert_eq!(
            decode_beacon_file(&bytes),
            Ok(BeaconFileData {
                raop_port: 7100,
                pid: 0x1234_5678,
                process_name: b"C:\\UxPlay\\uxplay.exe".to_vec(),
            })
        );
    }

    #[test]
    fn process_name_stops_at_the_same_optional_nul_as_the_python_reader() {
        let mut bytes = encode_beacon_file(5000, 7, b"uxplay.exe");
        bytes.extend_from_slice(&[0, b'j', b'u', b'n', b'k']);
        assert_eq!(
            decode_beacon_file(&bytes).unwrap().process_name,
            b"uxplay.exe"
        );
    }

    #[test]
    fn short_beacon_files_report_the_missing_header() {
        assert_eq!(
            decode_beacon_file(&[0, 1, 2]),
            Err(BeaconFileError::Truncated {
                expected_at_least: 6,
                actual: 3,
            })
        );
    }

    #[test]
    fn apple_manufacturer_data_matches_the_reference_layout() {
        assert_eq!(
            manufacturer_data(Ipv4Addr::new(192, 168, 1, 10), 7100),
            [0x09, 0x08, 0x13, 0x30, 192, 168, 1, 10, 0x1b, 0xbc]
        );
    }

    #[test]
    fn hci_legacy_payload_matches_the_reference_length_and_padding() {
        let data = hci_advertising_data(Ipv4Addr::new(192, 168, 1, 10), 7100);
        assert_eq!(data.len(), 31);
        assert_eq!(
            &data[..15],
            &[
                0x0e, 0x0d, 0xff, 0x4c, 0x00, 0x09, 0x08, 0x13, 0x30, 192, 168, 1, 10, 0x1b, 0xbc
            ]
        );
        assert!(data[15..].iter().all(|byte| *byte == 0));
    }
}
