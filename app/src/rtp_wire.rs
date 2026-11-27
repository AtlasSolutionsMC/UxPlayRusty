//! Safe view of the fixed RTP audio fields read by `raop_buffer_enqueue`.
//!
//! This is only the wire-format leaf helper. It does not implement buffering,
//! resend handling, packet decryption, or a running RTP session.

pub const RTP_FIXED_HEADER_LEN: usize = 12;
pub const RTP_PACKET_MAX_LEN: usize = 32_768;
pub const EMPTY_AUDIO_MARKER: [u8; 4] = [0x00, 0x68, 0x34, 0x00];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketError {
    TooShort,
    TooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPacket<'a> {
    pub sequence_number: u16,
    pub timestamp: u32,
    pub payload: &'a [u8],
    /// Source bypasses AES decryption for exactly this four-byte marker.
    pub is_empty_marker: bool,
}

impl<'a> AudioPacket<'a> {
    /// Parse the fixed RTP header fields used by the C audio buffer.
    ///
    /// A valid 12-byte datagram is returned as `Ok(None)`, matching the C
    /// early return for pre-synchronization empty packets.
    pub fn parse(datagram: &'a [u8]) -> Result<Option<Self>, PacketError> {
        if datagram.len() < RTP_FIXED_HEADER_LEN {
            return Err(PacketError::TooShort);
        }
        if datagram.len() > RTP_PACKET_MAX_LEN {
            return Err(PacketError::TooLarge);
        }
        if datagram.len() == RTP_FIXED_HEADER_LEN {
            return Ok(None);
        }

        let sequence_number = u16::from_be_bytes([datagram[2], datagram[3]]);
        let timestamp = u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]);
        let payload = &datagram[RTP_FIXED_HEADER_LEN..];
        let is_empty_marker = payload == EMPTY_AUDIO_MARKER;

        Ok(Some(Self {
            sequence_number,
            timestamp,
            payload,
            is_empty_marker,
        }))
    }

    /// The C caller can select the sequence number from the packet or reuse
    /// the current queue head when `use_seqnum` is false.
    pub fn selected_sequence_number(&self, use_seqnum: bool, first_seqnum: u16) -> u16 {
        if use_seqnum {
            self.sequence_number
        } else {
            first_seqnum
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sequence_and_timestamp_as_big_endian() {
        let packet = [
            0x80, 0x60, 0x12, 0x34, 0x89, 0xab, 0xcd, 0xef, 0, 0, 0, 0, 9,
        ];
        let parsed = AudioPacket::parse(&packet).unwrap().unwrap();
        assert_eq!(parsed.sequence_number, 0x1234);
        assert_eq!(parsed.timestamp, 0x89ab_cdef);
    }

    #[test]
    fn payload_starts_after_the_fixed_twelve_byte_header() {
        let packet = [0; 12].into_iter().chain([1, 2, 3]).collect::<Vec<_>>();
        let parsed = AudioPacket::parse(&packet).unwrap().unwrap();
        assert_eq!(parsed.payload, &[1, 2, 3]);
    }

    #[test]
    fn rejects_datagrams_shorter_than_the_rtp_header() {
        assert_eq!(AudioPacket::parse(&[0; 11]), Err(PacketError::TooShort));
    }

    #[test]
    fn rejects_datagrams_larger_than_the_source_packet_limit() {
        assert_eq!(
            AudioPacket::parse(&vec![0; RTP_PACKET_MAX_LEN + 1]),
            Err(PacketError::TooLarge)
        );
    }

    #[test]
    fn ignores_a_header_only_pre_synchronization_packet() {
        assert_eq!(AudioPacket::parse(&[0; RTP_FIXED_HEADER_LEN]), Ok(None));
    }

    #[test]
    fn recognizes_only_the_exact_four_byte_cleartext_marker() {
        let marker_packet = [0; 12]
            .into_iter()
            .chain(EMPTY_AUDIO_MARKER)
            .collect::<Vec<_>>();
        let parsed = AudioPacket::parse(&marker_packet).unwrap().unwrap();
        assert!(parsed.is_empty_marker);
        assert_eq!(parsed.payload, &EMPTY_AUDIO_MARKER);

        let near_match = [0; 12]
            .into_iter()
            .chain([0x00, 0x68, 0x34, 0x01])
            .collect::<Vec<_>>();
        assert!(
            !AudioPacket::parse(&near_match)
                .unwrap()
                .unwrap()
                .is_empty_marker
        );
    }

    #[test]
    fn supports_the_source_sequence_number_fallback() {
        let packet = [0x80, 0x60, 0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 5];
        let parsed = AudioPacket::parse(&packet).unwrap().unwrap();
        assert_eq!(parsed.selected_sequence_number(true, 0x4567), 0x1234);
        assert_eq!(parsed.selected_sequence_number(false, 0x4567), 0x4567);
    }
}
