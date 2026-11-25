//! Safe model of the pure state operations in `lib/dnssd.c`.
//!
//! Service registration, TXT construction and the embedded/native mDNS
//! backends are separate units. This module covers only initialization data,
//! name normalization, PIN mode storage and AirPlay feature-bit access.

const DEFAULT_FEATURES_1: u32 = 0x5a7f_fee6;
const DEFAULT_FEATURES_2: u32 = 0;
const DOT_LOCAL: &[u8] = b".local";

/// The initialization errors returned by `dnssd_init` for invalid input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitError {
    BadName,
    BadFeatures,
}

impl InitError {
    /// Numeric value of the corresponding `DNSSD_ERROR_*` source constant.
    pub const fn source_code(self) -> i32 {
        match self {
            Self::BadFeatures => 5,
            Self::BadName => 6,
        }
    }
}

/// The owned, backend-independent state initialized by `dnssd_init`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryState {
    name: Vec<u8>,
    hardware_address: Vec<u8>,
    pin_mode: u8,
    features_1: u32,
    features_2: u32,
}

impl DiscoveryState {
    /// Construct state from C-style name bytes and an explicit `name_len`.
    ///
    /// `name_bytes` must contain the terminating NUL, and the first NUL must
    /// be exactly at `name_len`, matching the source's combined terminator and
    /// `strlen(name) == name_len` validation. The source rejects the exact
    /// name `.local`, then truncates any other name at its first `.local`
    /// substring before storing it.
    pub fn new(
        name_bytes: &[u8],
        name_len: usize,
        hardware_address: &[u8],
        pin_mode: u8,
    ) -> Result<Self, InitError> {
        if name_bytes.get(name_len) != Some(&0)
            || name_bytes.iter().position(|byte| *byte == 0) != Some(name_len)
        {
            return Err(InitError::BadName);
        }

        let c_name = &name_bytes[..name_len];
        if c_name == DOT_LOCAL {
            return Err(InitError::BadName);
        }

        let stored_name_len = find_subslice(c_name, DOT_LOCAL).unwrap_or(c_name.len());
        Ok(Self {
            name: c_name[..stored_name_len].to_vec(),
            hardware_address: hardware_address.to_vec(),
            pin_mode,
            features_1: DEFAULT_FEATURES_1,
            features_2: DEFAULT_FEATURES_2,
        })
    }

    /// Return the stored service instance name as bytes (not UTF-8-decoded).
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Return the byte length reported by `dnssd_get_name`.
    pub fn name_len(&self) -> usize {
        self.name.len()
    }

    /// Return the owned hardware address bytes and their source-reported size.
    pub fn hardware_address(&self) -> &[u8] {
        &self.hardware_address
    }

    pub fn hardware_address_len(&self) -> usize {
        self.hardware_address.len()
    }

    /// Return the source's raw PIN/password mode byte.
    pub fn pin_mode(&self) -> u8 {
        self.pin_mode
    }

    /// Return the two source feature words as a single 64-bit value.
    pub fn airplay_features(&self) -> u64 {
        (u64::from(self.features_2) << 32) | u64::from(self.features_1)
    }

    /// Read one feature bit. Invalid bit positions return `None`.
    pub fn feature(&self, bit: i32) -> Option<bool> {
        if !(0..=63).contains(&bit) {
            return None;
        }
        let (word, offset) = if bit >= 32 {
            (self.features_2, bit - 32)
        } else {
            (self.features_1, bit)
        };
        Some(word & (1_u32 << offset) != 0)
    }

    /// Set or clear one bit, preserving the C function's invalid-input no-op.
    pub fn set_feature(&mut self, bit: i32, value: i32) {
        if !(0..=63).contains(&bit) || !(0..=1).contains(&value) {
            return;
        }

        let (word, offset) = if bit >= 32 {
            (&mut self.features_2, bit - 32)
        } else {
            (&mut self.features_1, bit)
        };
        let mask = 1_u32 << offset;
        if value == 1 {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &[u8], pin_mode: u8) -> Result<DiscoveryState, InitError> {
        let name_len = name.len();
        let mut name_bytes = name.to_vec();
        name_bytes.push(0);
        DiscoveryState::new(
            &name_bytes,
            name_len,
            &[0, 1, 2, 0xaa, 0xbb, 0xff],
            pin_mode,
        )
    }

    #[test]
    fn rejects_wrong_terminator_length_and_exact_local_name() {
        assert_eq!(
            DiscoveryState::new(b"receiver", 8, &[], 0),
            Err(InitError::BadName)
        );
        assert_eq!(
            DiscoveryState::new(b"a\0b\0", 3, &[], 0),
            Err(InitError::BadName)
        );
        assert_eq!(state(b".local", 0), Err(InitError::BadName));
    }

    #[test]
    fn truncates_first_local_substring_and_preserves_bytes() {
        let value = state(b"Living Room.local.extra", 1).unwrap();
        assert_eq!(value.name(), b"Living Room");
        assert_eq!(value.name_len(), 11);

        let non_utf8 = state(b"A\xff.local", 3).unwrap();
        assert_eq!(non_utf8.name(), b"A\xff");
    }

    #[test]
    fn owns_hardware_address_and_keeps_pin_mode_byte() {
        let address = [0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff];
        let value = DiscoveryState::new(b"receiver\0", 8, &address, 255).unwrap();
        assert_eq!(value.hardware_address(), &address);
        assert_eq!(value.hardware_address_len(), address.len());
        assert_eq!(value.pin_mode(), 255);
    }

    #[test]
    fn feature_get_set_matches_the_source_bit_ranges() {
        let mut value = state(b"receiver", 0).unwrap();
        assert_eq!(value.airplay_features(), 0x0000_0000_5a7f_fee6);
        assert_eq!(value.feature(1), Some(true));
        assert_eq!(value.feature(32), Some(false));
        assert_eq!(value.feature(-1), None);
        assert_eq!(value.feature(64), None);

        value.set_feature(1, 0);
        value.set_feature(32, 1);
        assert_eq!(value.airplay_features(), 0x0000_0001_5a7f_fee4);
        value.set_feature(63, 1);
        assert_eq!(value.airplay_features(), 0x8000_0001_5a7f_fee4);

        let before = value.airplay_features();
        value.set_feature(-1, 1);
        value.set_feature(64, 1);
        value.set_feature(5, -1);
        value.set_feature(5, 2);
        assert_eq!(value.airplay_features(), before);
    }

    #[test]
    fn empty_instance_name_is_retained_when_source_input_allows_it() {
        let value = state(b"", 0).unwrap();
        assert_eq!(value.name(), b"");
        assert_eq!(value.name_len(), 0);
    }
}
