//! The sequence-numbered 256-packet RAOP audio queue from `lib/raop_buffer.c`.
//!
//! AES-CBC is supplied by the caller. The queue owns only decoded payload bytes
//! and packet metadata, so it can be checked independently from the crypto layer.

use crate::rtp_wire::{AudioPacket, PacketError};

pub const RAOP_BUFFER_LENGTH: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    sequence_number: u16,
    timestamp: u32,
    payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DequeuedPacket {
    pub sequence_number: u16,
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueResult {
    Added,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DequeueResult {
    Packet(DequeuedPacket),
    Empty,
    WaitingForResend,
    MissingPacketSkipped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResendRange {
    pub sequence_number: u16,
    pub count: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RaopBuffer {
    entries: Vec<Option<Entry>>,
    is_empty: bool,
    first_sequence_number: u16,
    last_sequence_number: u16,
}

impl Default for RaopBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RaopBuffer {
    pub fn new() -> Self {
        Self {
            entries: vec![None; RAOP_BUFFER_LENGTH],
            is_empty: true,
            first_sequence_number: 0,
            last_sequence_number: 0,
        }
    }

    /// Enqueue one RTP audio datagram.
    ///
    /// The caller's decrypt function receives the packet payload (the bytes after
    /// the 12-byte RTP header). It is skipped for the source's cleartext marker and
    /// is not called for packets the queue ignores.
    pub fn enqueue<F>(
        &mut self,
        datagram: &[u8],
        use_sequence_number: bool,
        decrypt: F,
    ) -> Result<EnqueueResult, PacketError>
    where
        F: FnOnce(&[u8]) -> Vec<u8>,
    {
        let Some(packet) = AudioPacket::parse(datagram)? else {
            return Ok(EnqueueResult::Ignored);
        };
        let sequence_number =
            packet.selected_sequence_number(use_sequence_number, self.first_sequence_number);

        // C's seqnum_cmp returns the wrapping unsigned subtraction narrowed to short.
        if !self.is_empty && sequence_cmp(sequence_number, self.first_sequence_number) < 0 {
            return Ok(EnqueueResult::Ignored);
        }

        let window_end = self
            .first_sequence_number
            .wrapping_add(RAOP_BUFFER_LENGTH as u16);
        if sequence_cmp(sequence_number, window_end) >= 0 {
            self.flush(Some(sequence_number));
        }

        let slot = usize::from(sequence_number) % RAOP_BUFFER_LENGTH;
        if self.entries[slot]
            .as_ref()
            .is_some_and(|entry| entry.sequence_number == sequence_number)
        {
            return Ok(EnqueueResult::Ignored);
        }

        let payload = if packet.is_empty_marker {
            packet.payload.to_vec()
        } else {
            decrypt(packet.payload)
        };
        self.entries[slot] = Some(Entry {
            sequence_number,
            timestamp: packet.timestamp,
            payload,
        });

        if self.is_empty {
            self.first_sequence_number = sequence_number;
            self.last_sequence_number = sequence_number;
            self.is_empty = false;
        }
        if sequence_cmp(sequence_number, self.last_sequence_number) > 0 {
            self.last_sequence_number = sequence_number;
        }
        Ok(EnqueueResult::Added)
    }

    /// Return the packet at the queue head, or mirror the source's missing-head behavior.
    pub fn dequeue(&mut self, no_resend: bool) -> DequeueResult {
        let entry_count =
            sequence_cmp(self.last_sequence_number, self.first_sequence_number).wrapping_add(1);
        if self.is_empty || entry_count <= 0 {
            return DequeueResult::Empty;
        }

        let slot = usize::from(self.first_sequence_number) % RAOP_BUFFER_LENGTH;
        if self.entries[slot].is_none() {
            if !no_resend && entry_count < RAOP_BUFFER_LENGTH as i16 {
                return DequeueResult::WaitingForResend;
            }
            self.first_sequence_number = self.first_sequence_number.wrapping_add(1);
            return DequeueResult::MissingPacketSkipped;
        }

        self.first_sequence_number = self.first_sequence_number.wrapping_add(1);
        let entry = self.entries[slot].take().expect("entry checked above");
        DequeueResult::Packet(DequeuedPacket {
            sequence_number: entry.sequence_number,
            timestamp: entry.timestamp,
            payload: entry.payload,
        })
    }

    /// Return the first missing range up to the next filled packet, if any.
    pub fn pending_resend(&self) -> Option<ResendRange> {
        if sequence_cmp(self.first_sequence_number, self.last_sequence_number) >= 0 {
            return None;
        }
        let mut sequence_number = self.first_sequence_number;
        let mut count = 0_u16;
        while sequence_cmp(sequence_number, self.last_sequence_number) < 0 {
            let slot = usize::from(sequence_number) % RAOP_BUFFER_LENGTH;
            let filled = self.entries[slot].is_some();
            if filled {
                break;
            }
            count = count.wrapping_add(1);
            sequence_number = sequence_number.wrapping_add(1);
        }
        (count > 0).then_some(ResendRange {
            sequence_number: self.first_sequence_number,
            count,
        })
    }

    /// Clear every queue slot. `None` matches the C out-of-range flush behavior.
    pub fn flush(&mut self, next_sequence_number: Option<u16>) {
        self.entries.fill(None);
        match next_sequence_number {
            Some(next) => {
                self.first_sequence_number = next;
                self.last_sequence_number = next.wrapping_sub(1);
            }
            None => self.is_empty = true,
        }
    }

    pub fn first_sequence_number(&self) -> Option<u16> {
        (!self.is_empty).then_some(self.first_sequence_number)
    }

    pub fn last_sequence_number(&self) -> Option<u16> {
        (!self.is_empty).then_some(self.last_sequence_number)
    }
}

fn sequence_cmp(left: u16, right: u16) -> i16 {
    left.wrapping_sub(right) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagram(sequence_number: u16, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x80, 0x60];
        packet.extend_from_slice(&sequence_number.to_be_bytes());
        packet.extend_from_slice(&timestamp.to_be_bytes());
        packet.extend_from_slice(&[0; 4]);
        packet.extend_from_slice(payload);
        packet
    }

    fn identity(payload: &[u8]) -> Vec<u8> {
        payload.to_vec()
    }

    #[test]
    fn enqueues_and_dequeues_payload_metadata() {
        let mut buffer = RaopBuffer::new();
        assert_eq!(
            buffer.enqueue(&datagram(7, 0x1234_5678, b"audio"), true, identity),
            Ok(EnqueueResult::Added)
        );
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 7,
                timestamp: 0x1234_5678,
                payload: b"audio".to_vec(),
            })
        );
        assert_eq!(buffer.dequeue(false), DequeueResult::Empty);
    }

    #[test]
    fn ignores_empty_header_and_duplicate_packets() {
        let mut buffer = RaopBuffer::new();
        assert_eq!(
            buffer.enqueue(&[0; 12], true, identity),
            Ok(EnqueueResult::Ignored)
        );
        let packet = datagram(4, 1, b"frame");
        assert_eq!(
            buffer.enqueue(&packet, true, identity),
            Ok(EnqueueResult::Added)
        );
        assert_eq!(
            buffer.enqueue(&packet, true, |_| panic!("duplicate must not decrypt")),
            Ok(EnqueueResult::Ignored)
        );
    }

    #[test]
    fn ignores_late_packets_before_decrypting() {
        let mut buffer = RaopBuffer::new();
        buffer
            .enqueue(&datagram(20, 0, b"new"), true, identity)
            .unwrap();
        assert_eq!(
            buffer.enqueue(&datagram(19, 0, b"late"), true, |_| panic!(
                "late must not decrypt"
            )),
            Ok(EnqueueResult::Ignored)
        );
    }

    #[test]
    fn waits_for_missing_head_then_no_resend_skips_it() {
        let mut buffer = RaopBuffer::new();
        buffer
            .enqueue(&datagram(10, 0, b"a"), true, identity)
            .unwrap();
        buffer
            .enqueue(&datagram(12, 0, b"c"), true, identity)
            .unwrap();
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 10,
                timestamp: 0,
                payload: b"a".to_vec(),
            })
        );
        assert_eq!(buffer.dequeue(false), DequeueResult::WaitingForResend);
        assert_eq!(buffer.dequeue(true), DequeueResult::MissingPacketSkipped);
        assert_eq!(buffer.first_sequence_number(), Some(12));
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 12,
                timestamp: 0,
                payload: b"c".to_vec(),
            })
        );
    }

    #[test]
    fn requests_only_the_missing_head_range() {
        let mut buffer = RaopBuffer::new();
        buffer
            .enqueue(&datagram(30, 0, b"a"), true, identity)
            .unwrap();
        buffer
            .enqueue(&datagram(33, 0, b"d"), true, identity)
            .unwrap();
        assert!(matches!(buffer.dequeue(false), DequeueResult::Packet(_)));
        assert_eq!(
            buffer.pending_resend(),
            Some(ResendRange {
                sequence_number: 31,
                count: 2,
            })
        );
    }

    #[test]
    fn handles_sequence_wraparound() {
        let mut buffer = RaopBuffer::new();
        buffer
            .enqueue(&datagram(u16::MAX, 1, b"last"), true, identity)
            .unwrap();
        buffer
            .enqueue(&datagram(0, 2, b"wrapped"), true, identity)
            .unwrap();
        assert_eq!(buffer.pending_resend(), None);
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: u16::MAX,
                timestamp: 1,
                payload: b"last".to_vec(),
            })
        );
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 0,
                timestamp: 2,
                payload: b"wrapped".to_vec(),
            })
        );
    }

    #[test]
    fn flushes_when_the_packet_is_outside_the_256_sequence_window() {
        let mut buffer = RaopBuffer::new();
        buffer
            .enqueue(&datagram(1, 1, b"old"), true, identity)
            .unwrap();
        buffer
            .enqueue(&datagram(257, 2, b"new"), true, identity)
            .unwrap();
        assert_eq!(buffer.first_sequence_number(), Some(257));
        assert_eq!(buffer.last_sequence_number(), Some(257));
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 257,
                timestamp: 2,
                payload: b"new".to_vec(),
            })
        );
    }

    #[test]
    fn cleartext_marker_bypasses_decryption() {
        let mut buffer = RaopBuffer::new();
        let packet = datagram(1, 0, &crate::rtp_wire::EMPTY_AUDIO_MARKER);
        assert_eq!(
            buffer.enqueue(&packet, true, |_| panic!(
                "cleartext marker must bypass decrypt"
            )),
            Ok(EnqueueResult::Added)
        );
        assert_eq!(
            buffer.dequeue(false),
            DequeueResult::Packet(DequeuedPacket {
                sequence_number: 1,
                timestamp: 0,
                payload: crate::rtp_wire::EMPTY_AUDIO_MARKER.to_vec(),
            })
        );
    }
}
