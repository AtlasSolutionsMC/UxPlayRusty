//! Deterministic Q32.32/NTP time math from `lib/raop_ntp.c`.
//!
//! Socket timestamping, packet filtering, synchronization and thread lifecycle
//! remain in the server port. This module keeps only the clock arithmetic so it
//! can be exercised without a network clock or mutable C-style object.

pub const NTP_UNIX_EPOCH_SECONDS: u64 = 2_208_988_800;
pub const NTP_FRACTION_SCALE: u64 = 1_u64 << 32;
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// The time protocol values used by `timing_protocol_t` in the C source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeProtocol {
    Ntp,
    #[default]
    None,
    Other,
    Unspecified,
}

impl TimeProtocol {
    fn uses_ntp_epoch(self) -> bool {
        matches!(self, Self::Ntp)
    }
}

/// Convert the signed Q32.32 difference between two NTP timestamps to seconds.
///
/// C subtracts as `uint64_t`, reinterprets those bits as `int64_t`, then divides
/// by 2^32. The wrapping subtraction preserves that timestamp-wrap behavior.
pub fn diff_to_seconds(end: u64, start: u64) -> f64 {
    let signed_difference = end.wrapping_sub(start) as i64;
    signed_difference as f64 / NTP_FRACTION_SCALE as f64
}

/// Convert the source's little-endian-in-memory Q32.32 value to nanoseconds.
///
/// `NTP` timestamps count from 1900 and are shifted to the Unix epoch. Other
/// protocol variants keep the raw seconds field. Arithmetic wraps like the C
/// `uint64_t` implementation.
pub fn timestamp_to_nanoseconds(timestamp: u64, protocol: TimeProtocol) -> u64 {
    let mut seconds = timestamp >> 32;
    if protocol.uses_ntp_epoch() {
        seconds = seconds.wrapping_sub(NTP_UNIX_EPOCH_SECONDS);
    }
    let fraction = timestamp & 0xffff_ffff;
    seconds
        .wrapping_mul(NANOS_PER_SECOND)
        .wrapping_add(fraction.wrapping_mul(NANOS_PER_SECOND) >> 32)
}

/// Convert Unix nanoseconds to the NTP Q32.32 timestamp produced by
/// `byteutils_put_ntp_timestamp` and read back by `byteutils_get_long_be`.
pub fn unix_nanoseconds_to_ntp_q32_32(unix_nanoseconds: u64) -> u64 {
    let mut seconds = unix_nanoseconds / NANOS_PER_SECOND;
    let nanoseconds = unix_nanoseconds % NANOS_PER_SECOND;
    seconds = seconds.wrapping_add(NTP_UNIX_EPOCH_SECONDS);
    let fraction = ((nanoseconds << 32) / NANOS_PER_SECOND) as u32;
    ((seconds as u32 as u64) << 32) | u64::from(fraction)
}

/// Apply the optional 1900-to-1970 epoch adjustment to a raw remote timestamp.
pub fn add_ntp_epoch(raw_timestamp: u64, add_epoch_seconds: bool) -> u64 {
    if add_epoch_seconds {
        raw_timestamp.wrapping_add(NTP_UNIX_EPOCH_SECONDS << 32)
    } else {
        raw_timestamp
    }
}

/// Compute the fixed signed offset captured from the first remote timestamp.
///
/// The source first serializes local Unix nanoseconds as an NTP timestamp, then
/// subtracts the adjusted remote timestamp as signed 64-bit values.
pub fn fixed_offset_from_first_timestamp(
    local_unix_nanoseconds: u64,
    raw_remote_timestamp: u64,
    add_epoch_seconds: bool,
) -> i64 {
    let local_reference = unix_nanoseconds_to_ntp_q32_32(local_unix_nanoseconds) as i64;
    let remote = add_ntp_epoch(raw_remote_timestamp, add_epoch_seconds) as i64;
    local_reference.wrapping_sub(remote)
}

/// Adjust a remote raw timestamp with an already selected fixed signed offset.
pub fn adjust_remote_timestamp_offset(
    raw_remote_timestamp: u64,
    fixed_offset: i64,
    add_epoch_seconds: bool,
) -> u64 {
    let timestamp = add_ntp_epoch(raw_remote_timestamp, add_epoch_seconds) as i64;
    timestamp.wrapping_add(fixed_offset) as u64
}

/// Immutable view of the synchronization state needed by the public time helpers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NtpClock {
    pub client_time_received: bool,
    /// Remote minus local wall-clock offset, in nanoseconds.
    pub sync_offset_nanoseconds: i64,
}

impl NtpClock {
    /// Return the current remote wall-clock time; zero until the first sync.
    pub fn remote_time(self, local_unix_nanoseconds: u64) -> u64 {
        if !self.client_time_received {
            return 0;
        }
        (local_unix_nanoseconds as i64).wrapping_add(self.sync_offset_nanoseconds) as u64
    }

    /// Convert a remote-clock timestamp to local wall-clock time; zero until sync.
    pub fn convert_remote_time(self, remote_nanoseconds: u64) -> u64 {
        if !self.client_time_received {
            return 0;
        }
        (remote_nanoseconds as i64).wrapping_sub(self.sync_offset_nanoseconds) as u64
    }

    /// Convert a local-clock timestamp to remote wall-clock time; zero until sync.
    pub fn convert_local_time(self, local_nanoseconds: u64) -> u64 {
        if !self.client_time_received {
            return 0;
        }
        (local_nanoseconds as i64).wrapping_add(self.sync_offset_nanoseconds) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_conversion_handles_epoch_and_fraction() {
        let ntp_epoch = NTP_UNIX_EPOCH_SECONDS << 32;
        assert_eq!(timestamp_to_nanoseconds(ntp_epoch, TimeProtocol::Ntp), 0);
        assert_eq!(
            timestamp_to_nanoseconds(ntp_epoch | (1 << 31), TimeProtocol::Ntp),
            500_000_000
        );
        assert_eq!(
            timestamp_to_nanoseconds((2 << 32) | (1 << 31), TimeProtocol::None),
            2_500_000_000
        );
    }

    #[test]
    fn difference_uses_wrapping_subtraction_then_signed_interpretation() {
        assert_eq!(diff_to_seconds(3 << 32, 1 << 32), 2.0);
        assert_eq!(diff_to_seconds(0, 1 << 32), -1.0);
    }

    #[test]
    fn unix_nanoseconds_round_trip_as_ntp_timestamp() {
        let timestamp = unix_nanoseconds_to_ntp_q32_32(1_500_000_000);
        assert_eq!(timestamp >> 32, NTP_UNIX_EPOCH_SECONDS + 1);
        assert_eq!(timestamp as u32, 1 << 31);
        assert_eq!(
            timestamp_to_nanoseconds(timestamp, TimeProtocol::Ntp),
            1_500_000_000
        );
    }

    #[test]
    fn first_remote_sample_establishes_the_fixed_offset() {
        let local_reference = unix_nanoseconds_to_ntp_q32_32(12_500_000_000);
        let remote_raw = 7_u64 << 32;
        let offset = fixed_offset_from_first_timestamp(12_500_000_000, remote_raw, false);
        assert_eq!(
            adjust_remote_timestamp_offset(remote_raw, offset, false),
            local_reference
        );
    }

    #[test]
    fn clock_conversions_match_sync_offset_direction_and_unsynced_zero() {
        let unsynced = NtpClock::default();
        assert_eq!(unsynced.remote_time(42), 0);
        assert_eq!(unsynced.convert_remote_time(42), 0);
        assert_eq!(unsynced.convert_local_time(42), 0);

        let clock = NtpClock {
            client_time_received: true,
            sync_offset_nanoseconds: -250,
        };
        assert_eq!(clock.remote_time(1_000), 750);
        assert_eq!(clock.convert_local_time(1_000), 750);
        assert_eq!(clock.convert_remote_time(750), 1_000);
    }
}
