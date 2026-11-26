//! DNS-SD service names and registration state from `dnssd_mdnsd.c`.
//!
//! This is the portable metadata and registration state layer only. TXT
//! payloads use `discovery_txt`, and actions are handed to a publisher layer;
//! this module does not own multicast sockets or responder callbacks.

use crate::discovery_txt::{TxtRecordError, TxtRecordInputs, build_airplay_txt, build_raop_txt};
use crate::mdns_wire::{PacketBuilder, WireError};

pub const MAX_DNSSD_NAME: usize = 256;
pub const MAX_TXT_RECORD: usize = 512;
pub const MDNS_SERVICE_TTL: u32 = 4500;

const RAOP_SERVICE_TYPE: &[u8] = b"_raop._tcp.local";
const AIRPLAY_SERVICE_TYPE: &[u8] = b"_airplay._tcp.local";
const RAOP_NAME_SUFFIX: &[u8] = b"@";
const RAOP_TYPE_SUFFIX: &[u8] = b"._raop._tcp.local";
const AIRPLAY_NAME_SUFFIX: &[u8] = b"._airplay._tcp.local";
const HOST_NAME_SUFFIX: &[u8] = b".local";
const FALLBACK_HOST: &[u8] = b"UxPlay";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceNames {
    pub host_name: Vec<u8>,
    pub raop_name: Vec<u8>,
    pub airplay_name: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceNameError {
    HardwareAddress { length: usize },
    RaopName { length: usize },
    AirPlayName { length: usize },
}

/// Construct the mDNS host and service instance names using the source's
/// byte-oriented `snprintf` and `utils_hwaddr_raop` rules.
///
/// `host_name` is the result of the platform's `gethostname` call. `Err(())`
/// or an empty C string uses the source fallback `UxPlay`. The source's
/// 128-byte local hostname buffer keeps at most 127 bytes before sanitizing.
pub fn prepare_service_names(
    instance_name: &[u8],
    hardware_address: &[u8],
    host_name: Result<&[u8], ()>,
) -> Result<ServiceNames, ServiceNameError> {
    if hardware_address.len() > 6 {
        return Err(ServiceNameError::HardwareAddress {
            length: hardware_address.len(),
        });
    }

    let instance_name = c_string_prefix(instance_name);
    // `dnssd_init` stores only the bytes before its first case-sensitive
    // ".local" substring before `dnssd_prepare_names` formats service names.
    let instance_name_end = instance_name
        .windows(b".local".len())
        .position(|window| window == b".local")
        .unwrap_or(instance_name.len());
    let instance_name = &instance_name[..instance_name_end];
    let raop_address = format_raop_address(hardware_address);

    let mut raop_name = Vec::with_capacity(
        raop_address
            .len()
            .saturating_add(RAOP_NAME_SUFFIX.len())
            .saturating_add(instance_name.len())
            .saturating_add(RAOP_TYPE_SUFFIX.len()),
    );
    raop_name.extend_from_slice(&raop_address);
    raop_name.extend_from_slice(RAOP_NAME_SUFFIX);
    raop_name.extend_from_slice(instance_name);
    raop_name.extend_from_slice(RAOP_TYPE_SUFFIX);
    if raop_name.len() >= MAX_DNSSD_NAME {
        return Err(ServiceNameError::RaopName {
            length: raop_name.len(),
        });
    }

    let mut airplay_name = Vec::with_capacity(
        instance_name
            .len()
            .saturating_add(AIRPLAY_NAME_SUFFIX.len()),
    );
    airplay_name.extend_from_slice(instance_name);
    airplay_name.extend_from_slice(AIRPLAY_NAME_SUFFIX);
    if airplay_name.len() >= MAX_DNSSD_NAME {
        return Err(ServiceNameError::AirPlayName {
            length: airplay_name.len(),
        });
    }

    Ok(ServiceNames {
        host_name: make_host_name(host_name),
        raop_name,
        airplay_name,
    })
}

/// One service value passed to `mdnsd_set_services` or `mdnsd_goodbye`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServiceRecord {
    pub service_type: Vec<u8>,
    pub name: Vec<u8>,
    pub port: u16,
    pub txt: Vec<u8>,
    pub registered: bool,
}

/// The source-ordered TXT payloads prepared for the two DNS-SD services.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceTxtRecords {
    pub raop: Vec<u8>,
    pub airplay: Vec<u8>,
}

/// Build both records through the source-compatible TXT encoder.
pub fn build_service_txt(input: TxtRecordInputs<'_>) -> Result<ServiceTxtRecords, TxtRecordError> {
    Ok(ServiceTxtRecords {
        raop: build_raop_txt(input)?,
        airplay: build_airplay_txt(input)?,
    })
}

impl ServiceRecord {
    /// Append this record's TXT answer to a DNS message.
    ///
    /// The publisher decides whether the service is currently registered and
    /// owns the packet lifecycle; this only uses the shared wire encoder.
    pub fn append_txt_answer(&self, packet: &mut PacketBuilder, ttl: u32) -> Result<(), WireError> {
        packet.add_txt(&self.name, &self.txt, ttl)
    }
}

/// Side effects requested from the embedded mDNS responder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublisherAction {
    UpdateServices {
        airplay: ServiceRecord,
        raop: ServiceRecord,
    },
    Start,
    Announce {
        ttl: u32,
    },
    Goodbye(ServiceRecord),
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    TxtRecordTooLong { length: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrationTransition {
    /// The return value of `mdnsd_start`: zero on success, otherwise the
    /// platform error passed back through the source API.
    pub return_code: i32,
    pub actions: Vec<PublisherAction>,
}

/// Owns the two DNS-SD service records while the responder executes actions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryServices {
    host_name: Vec<u8>,
    airplay: ServiceRecord,
    raop: ServiceRecord,
}

impl DiscoveryServices {
    pub fn new(names: ServiceNames) -> Self {
        Self {
            host_name: names.host_name.clone(),
            airplay: ServiceRecord {
                service_type: AIRPLAY_SERVICE_TYPE.to_vec(),
                name: names.airplay_name.clone(),
                ..ServiceRecord::default()
            },
            raop: ServiceRecord {
                service_type: RAOP_SERVICE_TYPE.to_vec(),
                name: names.raop_name.clone(),
                ..ServiceRecord::default()
            },
        }
    }

    /// Create service state with both TXT payloads already built.
    pub fn new_with_txt(names: ServiceNames, txt: ServiceTxtRecords) -> Self {
        let mut services = Self::new(names);
        services.airplay.txt = txt.airplay;
        services.raop.txt = txt.raop;
        services
    }

    pub fn airplay(&self) -> &ServiceRecord {
        &self.airplay
    }

    pub fn host_name(&self) -> &[u8] {
        &self.host_name
    }

    pub fn raop(&self) -> &ServiceRecord {
        &self.raop
    }

    /// Model `dnssd_register_raop`: build TXT first, mark registered, publish
    /// both records, start the daemon, and announce only if start returned zero.
    pub fn register_raop(
        &mut self,
        port: u16,
        txt: &[u8],
        start_result: i32,
    ) -> Result<RegistrationTransition, RegistrationError> {
        if txt.len() > MAX_TXT_RECORD {
            return Err(RegistrationError::TxtRecordTooLong { length: txt.len() });
        }
        self.raop.port = port;
        self.raop.txt.clear();
        self.raop.txt.extend_from_slice(txt);
        self.raop.registered = true;

        let mut actions = vec![self.update_action(), PublisherAction::Start];
        if start_result == 0 {
            actions.push(PublisherAction::Announce {
                ttl: MDNS_SERVICE_TTL,
            });
        }
        Ok(RegistrationTransition {
            return_code: start_result,
            actions,
        })
    }

    /// Model `dnssd_register_airplay`, preserving the source's independent
    /// registration bit and shared responder start/announce sequence.
    pub fn register_airplay(
        &mut self,
        port: u16,
        txt: &[u8],
        start_result: i32,
    ) -> Result<RegistrationTransition, RegistrationError> {
        if txt.len() > MAX_TXT_RECORD {
            return Err(RegistrationError::TxtRecordTooLong { length: txt.len() });
        }
        self.airplay.port = port;
        self.airplay.txt.clear();
        self.airplay.txt.extend_from_slice(txt);
        self.airplay.registered = true;

        let mut actions = vec![self.update_action(), PublisherAction::Start];
        if start_result == 0 {
            actions.push(PublisherAction::Announce {
                ttl: MDNS_SERVICE_TTL,
            });
        }
        Ok(RegistrationTransition {
            return_code: start_result,
            actions,
        })
    }

    /// Model `dnssd_unregister_raop`, including a stop when AirPlay is not
    /// registered after the optional goodbye/update sequence.
    pub fn unregister_raop(&mut self) -> Vec<PublisherAction> {
        let mut actions = Vec::new();
        if self.raop.registered {
            actions.push(PublisherAction::Goodbye(self.raop.clone()));
            self.raop.registered = false;
            actions.push(self.update_action());
        }
        if !self.airplay.registered {
            actions.push(PublisherAction::Stop);
        }
        actions
    }

    /// Model `dnssd_unregister_airplay` with the symmetric RAOP stop rule.
    pub fn unregister_airplay(&mut self) -> Vec<PublisherAction> {
        let mut actions = Vec::new();
        if self.airplay.registered {
            actions.push(PublisherAction::Goodbye(self.airplay.clone()));
            self.airplay.registered = false;
            actions.push(self.update_action());
        }
        if !self.raop.registered {
            actions.push(PublisherAction::Stop);
        }
        actions
    }

    fn update_action(&self) -> PublisherAction {
        PublisherAction::UpdateServices {
            airplay: self.airplay.clone(),
            raop: self.raop.clone(),
        }
    }
}

fn format_raop_address(address: &[u8]) -> Vec<u8> {
    let mut formatted = Vec::with_capacity(address.len().saturating_mul(2));
    for byte in address {
        formatted.push(upper_hex(byte >> 4));
        formatted.push(upper_hex(byte & 0x0f));
    }
    formatted
}

fn upper_hex(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + value - 10,
    }
}

fn make_host_name(result: Result<&[u8], ()>) -> Vec<u8> {
    let candidate = result.map_or(&[][..], c_string_prefix);
    let candidate = if candidate.is_empty() {
        FALLBACK_HOST
    } else {
        &candidate[..candidate.len().min(127)]
    };
    let label = candidate
        .split(|byte| *byte == b'.')
        .next()
        .unwrap_or_default();
    let mut host_name = Vec::with_capacity(label.len().saturating_add(HOST_NAME_SUFFIX.len()));
    for byte in label {
        host_name.push(if byte.is_ascii_alphanumeric() || *byte == b'-' {
            *byte
        } else {
            b'-'
        });
    }
    host_name.extend_from_slice(HOST_NAME_SUFFIX);
    host_name
}

fn c_string_prefix(value: &[u8]) -> &[u8] {
    &value[..value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_ORACLE: &str =
        include_str!("../../../tools/record-shims/discovery_services_oracle.expected.txt");

    fn hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len().saturating_mul(2));
        for byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn render_service(event: &str, service: &ServiceRecord) -> String {
        format!(
            "{event}|{}|{}|{}|{}|{}",
            text(&service.service_type),
            text(&service.name),
            service.port,
            if service.registered { 1 } else { 0 },
            hex(&service.txt)
        )
    }

    fn render_actions(actions: &[PublisherAction], start_result: i32, rows: &mut Vec<String>) {
        for action in actions {
            match action {
                PublisherAction::UpdateServices { airplay, raop } => {
                    rows.push(render_service("service-airplay", airplay));
                    rows.push(render_service("service-raop", raop));
                }
                PublisherAction::Start => rows.push(format!("start|{start_result}")),
                PublisherAction::Announce { ttl } => rows.push(format!("announce|{ttl}")),
                PublisherAction::Goodbye(service) => {
                    rows.push(render_service("goodbye", service));
                }
                PublisherAction::Stop => rows.push("stop".to_owned()),
            }
        }
    }

    fn names_row(label: &str, names: &ServiceNames) -> String {
        format!(
            "{label}|0|{}|{}|{}",
            text(&names.host_name),
            text(&names.raop_name),
            text(&names.airplay_name)
        )
    }

    fn services() -> DiscoveryServices {
        let names = prepare_service_names(
            b"Kitchen",
            &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            Ok(b"host name.example"),
        )
        .unwrap();
        DiscoveryServices::new(names)
    }

    #[test]
    fn prepares_the_source_raop_airplay_and_host_names() {
        let names = prepare_service_names(
            b"Kitchen",
            &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            Ok(b"host name.example"),
        )
        .unwrap();
        assert_eq!(names.host_name, b"host-name.local");
        assert_eq!(names.raop_name, b"001122AABBFF@Kitchen._raop._tcp.local");
        assert_eq!(names.airplay_name, b"Kitchen._airplay._tcp.local");
    }

    #[test]
    fn non_ascii_service_instance_name_preserves_source_utf8_bytes() {
        let instance_name = "Living Room Cafe\u{301} 🏠";
        let names = prepare_service_names(
            instance_name.as_bytes(),
            &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            Ok(b"host"),
        )
        .unwrap();

        assert_eq!(
            names.airplay_name,
            [instance_name.as_bytes(), b"._airplay._tcp.local"].concat()
        );
        assert_eq!(
            names.raop_name,
            [
                &b"001122AABBFF@"[..],
                instance_name.as_bytes(),
                b"._raop._tcp.local",
            ]
            .concat()
        );
        assert!(std::str::from_utf8(&names.airplay_name).is_ok());
    }

    #[test]
    fn service_names_drop_the_first_dot_local_suffix_before_formatting() {
        let names = prepare_service_names(
            b"Living Room.local@host",
            &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            Ok(b"host"),
        )
        .unwrap();
        assert_eq!(
            names.raop_name,
            b"001122AABBFF@Living Room._raop._tcp.local"
        );
        assert_eq!(names.airplay_name, b"Living Room._airplay._tcp.local");
    }

    #[test]
    fn hostname_uses_first_dot_c_string_and_source_fallbacks() {
        assert_eq!(
            prepare_service_names(b"x", &[], Ok(b"rack..local\0tail"))
                .unwrap()
                .host_name,
            b"rack.local"
        );
        assert_eq!(
            prepare_service_names(b"x", &[], Ok(b"")).unwrap().host_name,
            b"UxPlay.local"
        );
        assert_eq!(
            prepare_service_names(b"x", &[], Err(())).unwrap().host_name,
            b"UxPlay.local"
        );
    }

    #[test]
    fn hostname_buffer_truncates_at_127_bytes_before_sanitizing() {
        let hostname = vec![b'a'; 130];
        let names = prepare_service_names(b"x", &[], Ok(&hostname)).unwrap();
        assert_eq!(names.host_name.len(), 127 + HOST_NAME_SUFFIX.len());
        assert!(names.host_name[..127].iter().all(|byte| *byte == b'a'));
    }

    #[test]
    fn names_reject_oversized_hardware_and_snprintf_output() {
        assert_eq!(
            prepare_service_names(b"x", &[0; 7], Ok(b"host")),
            Err(ServiceNameError::HardwareAddress { length: 7 })
        );
        let too_long_raop_instance = vec![b'a'; MAX_DNSSD_NAME - AIRPLAY_NAME_SUFFIX.len()];
        assert_eq!(
            prepare_service_names(&too_long_raop_instance, &[0; 6], Ok(b"host")),
            Err(ServiceNameError::RaopName {
                length: 12 + 1 + too_long_raop_instance.len() + RAOP_TYPE_SUFFIX.len()
            })
        );
    }

    #[test]
    fn records_start_in_source_default_state_and_registration_announces_only_on_success() {
        let mut services = services();
        assert_eq!(services.airplay().service_type, AIRPLAY_SERVICE_TYPE);
        assert_eq!(services.raop().service_type, RAOP_SERVICE_TYPE);
        assert!(!services.airplay().registered);
        assert!(!services.raop().registered);

        let registered = services.register_raop(7000, b"txt", 0).unwrap();
        assert_eq!(registered.return_code, 0);
        assert_eq!(
            registered.actions,
            vec![
                PublisherAction::UpdateServices {
                    airplay: services.airplay().clone(),
                    raop: services.raop().clone(),
                },
                PublisherAction::Start,
                PublisherAction::Announce {
                    ttl: MDNS_SERVICE_TTL,
                },
            ]
        );
        assert!(services.raop().registered);

        let failed_start = services.register_airplay(7001, b"air", -97).unwrap();
        assert_eq!(failed_start.return_code, -97);
        assert_eq!(failed_start.actions.len(), 2);
        assert_eq!(failed_start.actions[1], PublisherAction::Start);
        assert!(services.airplay().registered);
    }

    #[test]
    fn unregister_sends_goodbye_updates_records_and_stops_after_last_service() {
        let mut services = services();
        services.register_raop(7000, b"r", 0).unwrap();
        services.register_airplay(7001, b"a", 0).unwrap();

        let raop = services.raop().clone();
        let actions = services.unregister_raop();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], PublisherAction::Goodbye(raop));
        assert!(!services.raop().registered);
        assert!(services.airplay().registered);

        let airplay = services.airplay().clone();
        let actions = services.unregister_airplay();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0], PublisherAction::Goodbye(airplay));
        assert_eq!(actions[2], PublisherAction::Stop);
        assert!(!services.airplay().registered);
    }

    #[test]
    fn failed_txt_creation_does_not_mutate_or_start_the_service() {
        let mut services = services();
        let before = services.clone();
        assert_eq!(
            services.register_raop(7000, &[0; MAX_TXT_RECORD + 1], 0),
            Err(RegistrationError::TxtRecordTooLong {
                length: MAX_TXT_RECORD + 1
            })
        );
        assert_eq!(services, before);
    }

    #[test]
    fn source_c_oracle_matches_names_txt_bytes_and_service_transitions() {
        let input = TxtRecordInputs {
            model: "AppleTV3,2",
            version: "220.68",
            public_key: "0123456789abcdef",
            hardware_address: &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            features1: 0x5a7f_fee6,
            features2: 0,
            pin_mode: 1,
        };
        let names =
            prepare_service_names(b"Kitchen", input.hardware_address, Ok(b"Host One.example"))
                .unwrap();
        let txt = build_service_txt(input).unwrap();
        let mut services = DiscoveryServices::new_with_txt(names.clone(), txt.clone());
        let mut rows = vec![
            names_row("names", &names),
            format!("raop-txt|{}|{}", txt.raop.len(), hex(&txt.raop)),
            format!("airplay-txt|{}|{}", txt.airplay.len(), hex(&txt.airplay)),
        ];

        let transition = services.register_raop(7000, &txt.raop, 0).unwrap();
        render_actions(&transition.actions, transition.return_code, &mut rows);
        rows.push(format!("return-raop|{}", transition.return_code));

        let transition = services.register_airplay(7001, &txt.airplay, 9).unwrap();
        render_actions(&transition.actions, transition.return_code, &mut rows);
        rows.push(format!("return-airplay|{}", transition.return_code));

        render_actions(&services.unregister_raop(), 0, &mut rows);
        render_actions(&services.unregister_airplay(), 0, &mut rows);
        render_actions(&services.unregister_raop(), 0, &mut rows);
        render_actions(&services.unregister_airplay(), 0, &mut rows);

        let empty_host =
            prepare_service_names(b"Kitchen", input.hardware_address, Ok(b"")).unwrap();
        rows.push(names_row("empty-host", &empty_host));
        let failed_host =
            prepare_service_names(b"Kitchen", input.hardware_address, Err(())).unwrap();
        rows.push(names_row("failed-host", &failed_host));

        assert_eq!(
            rows,
            C_ORACLE.lines().map(str::to_owned).collect::<Vec<_>>()
        );
    }

    #[test]
    fn service_txt_answer_uses_the_shared_dns_wire_encoder() {
        let txt = build_raop_txt(TxtRecordInputs {
            model: "AppleTV3,2",
            version: "220.68",
            public_key: "0123456789abcdef",
            hardware_address: &[0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff],
            features1: 0x5a7f_fee6,
            features2: 0,
            pin_mode: 1,
        })
        .unwrap();
        let mut service = services().raop().clone();
        service.txt = txt.clone();
        let mut packet = PacketBuilder::new();
        service
            .append_txt_answer(&mut packet, MDNS_SERVICE_TTL)
            .unwrap();

        let mut expected = PacketBuilder::new();
        expected
            .add_txt(&service.name, &txt, MDNS_SERVICE_TTL)
            .unwrap();
        assert_eq!(packet.bytes(), expected.bytes());
    }
}
