//! Pure DNS wire primitives from `lib/mdnsd/mdnsd.c`.
//!
//! This module ports packet encoding and question-name helpers only. Socket
//! ownership, multicast membership, announcements, and responder lifecycle
//! remain in the containing mDNS service unit.

pub const MAX_PACKET: usize = 1500;
pub const DNS_TYPE_A: u16 = 1;
pub const DNS_TYPE_PTR: u16 = 12;
pub const DNS_TYPE_TXT: u16 = 16;
pub const DNS_TYPE_AAAA: u16 = 28;
pub const DNS_TYPE_SRV: u16 = 33;
pub const DNS_TYPE_ANY: u16 = 255;
pub const DNS_CLASS_IN: u16 = 1;
pub const DNS_CACHE_FLUSH: u16 = 0x8000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    PacketFull,
    LabelTooLong,
    NameOutputTooSmall,
    NameOutOfBounds,
    InvalidCompressionPointer,
    TooManyCompressionPointers,
    InvalidRecordPosition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketBuilder {
    bytes: Vec<u8>,
    limit: usize,
}

impl Default for PacketBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketBuilder {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAX_PACKET),
            limit: MAX_PACKET,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn put_u8(&mut self, value: u32) -> Result<(), WireError> {
        self.ensure_space(1)?;
        self.bytes.push(value as u8);
        Ok(())
    }

    pub fn put_u16(&mut self, value: u32) -> Result<(), WireError> {
        self.ensure_space(2)?;
        self.bytes.extend_from_slice(&(value as u16).to_be_bytes());
        Ok(())
    }

    pub fn put_u32(&mut self, value: u32) -> Result<(), WireError> {
        self.ensure_space(4)?;
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn put_bytes(&mut self, value: &[u8]) -> Result<(), WireError> {
        self.ensure_space(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    /// Encode a C-string domain name as DNS labels followed by a root label.
    ///
    /// Matching `mdns_put_name`, a trailing dot is accepted, empty interior
    /// labels are emitted as zero length labels, and the input stops at NUL.
    pub fn put_name(&mut self, name: &[u8]) -> Result<(), WireError> {
        let name = c_string_prefix(name);
        let mut label_start = 0;

        while label_start < name.len() {
            let label_end = name[label_start..]
                .iter()
                .position(|byte| *byte == b'.')
                .map_or(name.len(), |relative| label_start + relative);
            let label = &name[label_start..label_end];
            if label.len() > 63 {
                return Err(WireError::LabelTooLong);
            }
            self.put_u8(label.len() as u32)?;
            self.put_bytes(label)?;

            if label_end == name.len() {
                break;
            }
            label_start = label_end + 1;
        }

        self.put_u8(0)
    }

    /// Write an answer's owner, type, class, TTL and empty RDLENGTH field.
    pub fn begin_record(
        &mut self,
        name: &[u8],
        record_type: u32,
        class: u32,
        ttl: u32,
    ) -> Result<usize, WireError> {
        self.put_name(name)?;
        self.put_u16(record_type)?;
        self.put_u16(class)?;
        self.put_u32(ttl)?;
        let rdlength_position = self.bytes.len();
        self.put_u16(0)?;
        Ok(rdlength_position)
    }

    /// Fill the RDLENGTH field created by [`Self::begin_record`].
    pub fn end_record(&mut self, rdlength_position: usize) -> Result<(), WireError> {
        let Some(data_start) = rdlength_position.checked_add(2) else {
            return Err(WireError::InvalidRecordPosition);
        };
        if data_start > self.bytes.len() {
            return Err(WireError::InvalidRecordPosition);
        }
        let rdlength = self.bytes.len() - data_start;
        let length = u16::try_from(rdlength).map_err(|_| WireError::InvalidRecordPosition)?;
        self.bytes[rdlength_position..data_start].copy_from_slice(&length.to_be_bytes());
        Ok(())
    }

    pub fn add_ptr(&mut self, name: &[u8], target: &[u8], ttl: u32) -> Result<(), WireError> {
        let rdlength =
            self.begin_record(name, u32::from(DNS_TYPE_PTR), u32::from(DNS_CLASS_IN), ttl)?;
        self.put_name(target)?;
        self.end_record(rdlength)
    }

    pub fn add_srv(
        &mut self,
        name: &[u8],
        port: u16,
        target: &[u8],
        ttl: u32,
    ) -> Result<(), WireError> {
        let class = DNS_CLASS_IN | DNS_CACHE_FLUSH;
        let rdlength = self.begin_record(name, u32::from(DNS_TYPE_SRV), u32::from(class), ttl)?;
        self.put_u16(0)?;
        self.put_u16(0)?;
        self.put_u16(u32::from(port))?;
        self.put_name(target)?;
        self.end_record(rdlength)
    }

    pub fn add_txt(&mut self, name: &[u8], text: &[u8], ttl: u32) -> Result<(), WireError> {
        let class = DNS_CLASS_IN | DNS_CACHE_FLUSH;
        let rdlength = self.begin_record(name, u32::from(DNS_TYPE_TXT), u32::from(class), ttl)?;
        self.put_bytes(text)?;
        self.end_record(rdlength)
    }

    /// Encode the source's native-memory representation of an IPv4 address.
    /// The original copies `uint32_t` bytes directly, where its value already
    /// contains the network-order `sin_addr.s_addr` representation.
    pub fn add_a(
        &mut self,
        name: &[u8],
        network_order_address: u32,
        ttl: u32,
    ) -> Result<(), WireError> {
        if network_order_address == 0 {
            return Ok(());
        }
        let class = DNS_CLASS_IN | DNS_CACHE_FLUSH;
        let rdlength = self.begin_record(name, u32::from(DNS_TYPE_A), u32::from(class), ttl)?;
        self.put_bytes(&network_order_address.to_ne_bytes())?;
        self.end_record(rdlength)
    }

    pub fn add_aaaa(&mut self, name: &[u8], address: &[u8; 16], ttl: u32) -> Result<(), WireError> {
        if address.iter().all(|byte| *byte == 0) {
            return Ok(());
        }
        let class = DNS_CLASS_IN | DNS_CACHE_FLUSH;
        let rdlength = self.begin_record(name, u32::from(DNS_TYPE_AAAA), u32::from(class), ttl)?;
        self.put_bytes(address)?;
        self.end_record(rdlength)
    }

    fn ensure_space(&self, additional: usize) -> Result<(), WireError> {
        if self
            .bytes
            .len()
            .checked_add(additional)
            .is_none_or(|length| length > self.limit)
        {
            return Err(WireError::PacketFull);
        }
        Ok(())
    }

    #[cfg(test)]
    fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit: limit.min(MAX_PACKET),
        }
    }
}

/// Decode a possibly compressed DNS name and advance the caller's packet offset.
///
/// `output_capacity` includes room for the C terminator. The returned byte
/// vector omits that terminator but retains any NUL bytes embedded in labels.
pub fn read_name(
    packet: &[u8],
    offset: &mut usize,
    output_capacity: usize,
) -> Result<Vec<u8>, WireError> {
    let mut position = *offset;
    let mut output = Vec::new();
    let mut jumped = false;
    let mut jumps = 0;

    while position < packet.len() {
        let length = usize::from(packet[position]);
        position += 1;

        if length == 0 {
            if !jumped {
                *offset = position;
            }
            if output.is_empty() {
                if output_capacity < 2 {
                    return Err(WireError::NameOutputTooSmall);
                }
                output.push(b'.');
            }
            return Ok(output);
        }

        if length & 0xc0 == 0xc0 {
            if position >= packet.len() || jumps == 16 {
                return Err(if jumps == 16 {
                    WireError::TooManyCompressionPointers
                } else {
                    WireError::InvalidCompressionPointer
                });
            }
            jumps += 1;
            let pointer = ((length & 0x3f) << 8) | usize::from(packet[position]);
            position += 1;
            if pointer >= packet.len() {
                return Err(WireError::InvalidCompressionPointer);
            }
            if !jumped {
                *offset = position;
            }
            position = pointer;
            jumped = true;
            continue;
        }

        if length & 0xc0 != 0
            || position
                .checked_add(length)
                .is_none_or(|end| end > packet.len())
        {
            return Err(WireError::NameOutOfBounds);
        }
        if !output.is_empty() {
            if output.len().saturating_add(1) >= output_capacity {
                return Err(WireError::NameOutputTooSmall);
            }
            output.push(b'.');
        }
        if output.len().saturating_add(length) >= output_capacity {
            return Err(WireError::NameOutputTooSmall);
        }
        output.extend_from_slice(&packet[position..position + length]);
        position += length;
    }

    Err(WireError::NameOutOfBounds)
}

pub fn read_u16(bytes: &[u8]) -> Result<u16, WireError> {
    let pair = bytes.get(..2).ok_or(WireError::NameOutOfBounds)?;
    Ok(u16::from_be_bytes([pair[0], pair[1]]))
}

pub fn name_equals(left: &[u8], right: &[u8]) -> bool {
    c_string_prefix(left).eq_ignore_ascii_case(c_string_prefix(right))
}

pub fn query_type_matches(query_type: u16, record_type: u16) -> bool {
    query_type == record_type || query_type == DNS_TYPE_ANY
}

fn c_string_prefix(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE: &str = include_str!("../../record-shims/mdns_wire_oracle.expected.txt");

    fn oracle(label: &str) -> Vec<&str> {
        ORACLE
            .lines()
            .find_map(|line| {
                let (entry, value) = line.split_once('|')?;
                (entry == label).then(|| value.split('|').collect())
            })
            .expect("oracle fixture")
    }

    fn expected_hex(label: &str) -> Vec<u8> {
        let values = oracle(label);
        let hex = values.first().expect("oracle packet fixture");
        if hex.is_empty() {
            return Vec::new();
        }
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn primitive_writes_match_c_network_byte_order_and_truncation() {
        let mut packet = PacketBuilder::new();
        packet.put_u8(0x123).unwrap();
        packet.put_u16(0xabcd).unwrap();
        packet.put_u32(0x12345678).unwrap();
        packet.put_bytes(&[0, 0x7f, 0xff]).unwrap();
        assert_eq!(packet.bytes(), expected_hex("primitive"));
    }

    #[test]
    fn labels_and_pointer_service_records_match_the_c_oracle() {
        let mut packet = PacketBuilder::new();
        packet.put_name(b"AirPlay._tcp.local.").unwrap();
        assert_eq!(packet.bytes(), expected_hex("name"));

        let mut empty_label = PacketBuilder::new();
        empty_label.put_name(b"foo..bar").unwrap();
        assert_eq!(empty_label.bytes(), expected_hex("empty-label"));

        let mut pointer = PacketBuilder::new();
        pointer
            .add_ptr(b"_airplay._tcp.local", b"UxPlay._airplay._tcp.local", 4500)
            .unwrap();
        assert_eq!(pointer.bytes(), expected_hex("ptr"));
    }

    #[test]
    fn srv_txt_a_and_aaaa_records_match_c_wire_bytes() {
        let mut srv = PacketBuilder::new();
        srv.add_srv(b"UxPlay._airplay._tcp.local", 7000, b"uxplay.local", 4500)
            .unwrap();
        assert_eq!(srv.bytes(), expected_hex("srv"));

        let mut txt = PacketBuilder::new();
        txt.add_txt(
            b"UxPlay._airplay._tcp.local",
            &[4, b'a', b'=', b'1', 4, b'b', b'=', b'2'],
            120,
        )
        .unwrap();
        assert_eq!(txt.bytes(), expected_hex("txt"));

        let mut a = PacketBuilder::new();
        a.add_a(b"uxplay.local", u32::from_ne_bytes([1, 2, 3, 4]), 120)
            .unwrap();
        assert_eq!(a.bytes(), expected_hex("a"));

        let mut aaaa = PacketBuilder::new();
        aaaa.add_aaaa(
            b"uxplay.local",
            &[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            120,
        )
        .unwrap();
        assert_eq!(aaaa.bytes(), expected_hex("aaaa"));

        let mut zero = PacketBuilder::new();
        zero.add_a(b"uxplay.local", 0, 120).unwrap();
        zero.add_aaaa(b"uxplay.local", &[0; 16], 120).unwrap();
        assert_eq!(zero.bytes(), expected_hex("zero-addresses"));
    }

    #[test]
    fn name_reader_handles_root_compression_and_advances_original_offset() {
        let packet = [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, b'w', b'w', b'w', 7, b'e', b'x', b'a', b'm',
            b'p', b'l', b'e', 3, b'c', b'o', b'm', 0, 5, b'_', b'a', b'i', b'r', b'p', 0xc0, 16,
        ];
        let mut offset = 12;
        assert_eq!(
            read_name(&packet, &mut offset, 256).unwrap(),
            b"www.example.com"
        );
        assert_eq!(offset, 29);

        offset = 29;
        assert_eq!(
            read_name(&packet, &mut offset, 256).unwrap(),
            b"_airp.example.com"
        );
        assert_eq!(offset, 37);

        let mut root_offset = 0;
        assert_eq!(read_name(&[0], &mut root_offset, 2).unwrap(), b".");
        assert_eq!(root_offset, 1);
    }

    #[test]
    fn parser_bounds_and_compression_limits_match_recorded_c_cases() {
        let mut offset = 0;
        assert_eq!(
            read_name(&[3, b'a'], &mut offset, 256),
            Err(WireError::NameOutOfBounds)
        );
        assert_eq!(offset, 0);

        offset = 0;
        assert_eq!(
            read_name(&[0xc0, 0xff], &mut offset, 256),
            Err(WireError::InvalidCompressionPointer)
        );
        assert_eq!(offset, 0);

        offset = 0;
        assert_eq!(
            read_name(&[0xc0, 0], &mut offset, 256),
            Err(WireError::TooManyCompressionPointers)
        );
        assert_eq!(offset, 2);

        offset = 0;
        assert_eq!(
            read_name(&[0], &mut offset, 1),
            Err(WireError::NameOutputTooSmall)
        );

        let short_output_packet = [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, b'w', b'w', b'w', 7, b'e', b'x', b'a', b'm',
            b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ];
        let cases: [(&str, &[u8], usize, usize); 4] = [
            ("read-truncated", &[3, b'a'], 0, 256),
            ("read-bad-pointer", &[0xc0, 0xff], 0, 256),
            ("read-pointer-loop", &[0xc0, 0], 0, 256),
            ("read-short-output", &short_output_packet, 12, 4),
        ];
        for (label, packet, start_offset, output_capacity) in cases {
            let expected = oracle(label);
            assert_eq!(expected[0], "-1", "{label}");
            let mut offset = start_offset;
            assert!(
                read_name(packet, &mut offset, output_capacity).is_err(),
                "{label}"
            );
            assert_eq!(offset, expected[1].parse::<usize>().unwrap(), "{label}");
        }
    }

    #[test]
    fn network_u16_reader_matches_c_fixture_and_checks_short_inputs() {
        let expected = oracle("read-u16");
        assert_eq!(
            read_u16(&[0x12, 0x34]).unwrap(),
            expected[0].parse().unwrap()
        );
        assert_eq!(
            read_u16(&[0xff, 0xff]).unwrap(),
            expected[1].parse().unwrap()
        );
        assert_eq!(read_u16(&[0x12]), Err(WireError::NameOutOfBounds));
    }

    #[test]
    fn c_string_comparison_and_query_type_wildcard_match_oracle() {
        let expected_equals = oracle("name-equals");
        let expected_query = oracle("query-type");
        let values = expected_equals
            .iter()
            .map(|value| value.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            name_equals(b"_AirPlay._tcp.local", b"_airplay._TCP.LOCAL"),
            values[0] != 0
        );
        assert_eq!(name_equals(b"left.local", b"right.local"), values[1] != 0);
        assert_eq!(name_equals(b"prefix\0suffix", b"PREFIX"), values[2] != 0);

        let values = expected_query
            .iter()
            .map(|value| value.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(query_type_matches(1, 1), values[0] != 0);
        assert_eq!(query_type_matches(255, 28), values[1] != 0);
        assert_eq!(query_type_matches(12, 16), values[2] != 0);
    }

    #[test]
    fn packet_full_and_long_label_fail_without_partial_field_write() {
        let mut packet = PacketBuilder::with_limit(2);
        packet.put_u8(1).unwrap();
        assert_eq!(packet.put_u16(2), Err(WireError::PacketFull));
        assert_eq!(packet.bytes(), &[1]);

        let mut packet = PacketBuilder::new();
        assert_eq!(packet.put_name(&[b'a'; 64]), Err(WireError::LabelTooLong));
        assert!(packet.is_empty());

        let expected_label = oracle("long-label");
        assert_eq!(expected_label[0], "-1");
        assert_eq!(expected_label[1], "0");

        let expected_limit = oracle("packet-limit");
        let mut full_packet = PacketBuilder::new();
        full_packet.put_bytes(&[0; MAX_PACKET]).unwrap();
        assert_eq!(
            full_packet.len(),
            expected_limit[2].parse::<usize>().unwrap()
        );
        assert_eq!(full_packet.put_u8(1), Err(WireError::PacketFull));
        assert_eq!(expected_limit[0], "0");
        assert_eq!(expected_limit[1], "-1");
        assert_eq!(full_packet.len(), MAX_PACKET);
    }
}
