//! Host MAC selection from `uxplay.cpp::find_mac` and `random_mac`.

use crate::options::Options;
use std::io;
use std::ptr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub fn parse(value: &str) -> io::Result<Self> {
        let mut octets = [0; 6];
        let mut parts = value.split(':');
        for octet in &mut octets {
            let part = parts.next().ok_or_else(invalid_mac)?;
            if part.len() != 2 {
                return Err(invalid_mac());
            }
            *octet = u8::from_str_radix(part, 16).map_err(|_| invalid_mac())?;
        }
        if parts.next().is_some() {
            return Err(invalid_mac());
        }
        Ok(Self(octets))
    }

    pub fn from_bytes(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    pub fn octets(self) -> [u8; 6] {
        self.0
    }

    pub fn as_lowercase(self) -> String {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// Apply the source's precedence: explicit `-m`, the first usable system
/// interface unless random mode is requested, then a locally administered MAC.
pub fn select_mac_address(options: &Options) -> io::Result<MacAddress> {
    if options.random_mac_address {
        return random_mac_address();
    }
    if let Some(configured) = options.mac_address.as_deref() {
        return MacAddress::parse(configured);
    }
    if let Some(address) = find_system_mac_address() {
        return Ok(address);
    }
    random_mac_address()
}

pub fn random_mac_address() -> io::Result<MacAddress> {
    let mut octets = [0; 6];
    getrandom::fill(&mut octets).map_err(io::Error::other)?;
    octets[0] = (octets[0] & !1) | 2;
    Ok(MacAddress(octets))
}

/// Return the first nonzero six-byte MAC on an active Ethernet/Wi-Fi adapter.
/// Interface ordering follows the host API, like the original implementation.
pub fn find_system_mac_address() -> Option<MacAddress> {
    #[cfg(windows)]
    {
        find_windows_mac_address()
    }
    #[cfg(unix)]
    {
        find_unix_mac_address()
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

fn invalid_mac() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid MAC address")
}

#[cfg(target_os = "linux")]
fn find_unix_mac_address() -> Option<MacAddress> {
    use std::ffi::c_int;

    struct IfAddrsGuard(*mut libc::ifaddrs);

    impl Drop for IfAddrsGuard {
        fn drop(&mut self) {
            // SAFETY: `getifaddrs` initialized this list and this guard owns its one free.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }

    let mut first = ptr::null_mut();
    // SAFETY: `first` is a valid out-pointer; the returned linked list is owned by the guard.
    if unsafe { libc::getifaddrs(&mut first) } as c_int != 0 || first.is_null() {
        return None;
    }
    let _guard = IfAddrsGuard(first);
    let mut current = first;
    while !current.is_null() {
        // SAFETY: each node is part of the live `getifaddrs` list until the guard drops.
        let entry = unsafe { &*current };
        if !entry.ifa_addr.is_null()
            // SAFETY: `ifa_addr` is non-null and starts with `sockaddr` for every family.
            && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_PACKET
        {
            // SAFETY: AF_PACKET addresses use Linux `sockaddr_ll` layout.
            let link = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_ll>() };
            let mut octets = [0; 6];
            for (target, source) in octets.iter_mut().zip(link.sll_addr.iter()) {
                *target = *source;
            }
            if octets.iter().any(|octet| *octet != 0) {
                return Some(MacAddress(octets));
            }
        }
        current = entry.ifa_next;
    }
    None
}

#[cfg(all(unix, not(target_os = "linux")))]
fn find_unix_mac_address() -> Option<MacAddress> {
    struct IfAddrsGuard(*mut libc::ifaddrs);

    impl Drop for IfAddrsGuard {
        fn drop(&mut self) {
            // SAFETY: `getifaddrs` initialized this list and this guard owns its one free.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }

    let mut first = ptr::null_mut();
    // SAFETY: `first` is a valid out-pointer; the returned linked list is owned by the guard.
    if unsafe { libc::getifaddrs(&mut first) } != 0 || first.is_null() {
        return None;
    }
    let _guard = IfAddrsGuard(first);
    let mut current = first;
    while !current.is_null() {
        // SAFETY: each node is part of the live `getifaddrs` list until the guard drops.
        let entry = unsafe { &*current };
        if !entry.ifa_addr.is_null()
            // SAFETY: `ifa_addr` is non-null and starts with `sockaddr` for every family.
            && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_LINK
        {
            // SAFETY: AF_LINK addresses use the target's `sockaddr_dl` layout.
            let link = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_dl>() };
            let name_len = usize::from(link.sdl_nlen);
            let address_len = usize::from(link.sdl_alen);
            let data_len = link.sdl_len as usize;
            let data = ptr::addr_of!(link.sdl_data).cast::<u8>();
            if address_len >= 6
                && name_len.checked_add(6).is_some_and(|end| end <= data_len)
                && name_len + 6 <= link.sdl_data.len()
            {
                let mut octets = [0; 6];
                // SAFETY: the length checks keep this range within `sdl_data`.
                unsafe { ptr::copy_nonoverlapping(data.add(name_len), octets.as_mut_ptr(), 6) };
                if octets.iter().any(|octet| *octet != 0) {
                    return Some(MacAddress(octets));
                }
            }
        }
        current = entry.ifa_next;
    }
    None
}

#[cfg(windows)]
fn find_windows_mac_address() -> Option<MacAddress> {
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    let mut byte_len = std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>() as u32;
    let mut storage = vec![0usize; (byte_len as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: the aligned storage is at least `byte_len` bytes and the API writes within it.
    let mut result = unsafe {
        GetAdaptersAddresses(
            u32::from(AF_UNSPEC),
            0,
            ptr::null(),
            storage.as_mut_ptr().cast(),
            &mut byte_len,
        )
    };
    if result == ERROR_BUFFER_OVERFLOW {
        storage = vec![0usize; byte_len.div_ceil(std::mem::size_of::<usize>() as u32) as usize];
        // SAFETY: the resized, aligned storage uses the required byte count returned by the API.
        result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                0,
                ptr::null(),
                storage.as_mut_ptr().cast(),
                &mut byte_len,
            )
        };
    }
    if result != NO_ERROR {
        return None;
    }

    let mut current = storage.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !current.is_null() {
        // SAFETY: each `Next` pointer refers to an entry within the API-filled storage buffer.
        let adapter = unsafe { &*current };
        if adapter.PhysicalAddressLength == 6
            && (adapter.IfType == IF_TYPE_ETHERNET_CSMACD || adapter.IfType == IF_TYPE_IEEE80211)
            && adapter.OperStatus == IfOperStatusUp
        {
            return Some(MacAddress([
                adapter.PhysicalAddress[0],
                adapter.PhysicalAddress[1],
                adapter.PhysicalAddress[2],
                adapter.PhysicalAddress[3],
                adapter.PhysicalAddress[4],
                adapter.PhysicalAddress[5],
            ]));
        }
        current = adapter.Next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_six_octet_addresses() {
        let address = MacAddress::parse("02:00:a5:c4:b8:ff").unwrap();
        assert_eq!(address.octets(), [0x02, 0x00, 0xa5, 0xc4, 0xb8, 0xff]);
        assert_eq!(address.as_lowercase(), "02:00:a5:c4:b8:ff");
        assert_eq!(
            MacAddress::parse("02:00:A5:C4:B8:FF")
                .unwrap()
                .as_lowercase(),
            "02:00:a5:c4:b8:ff"
        );
    }

    #[test]
    fn rejects_malformed_address_lengths_and_digits() {
        for value in [
            "",
            "00:11:22:33:44",
            "00:11:22:33:44:55:66",
            "00:11:22:33:44:gg",
        ] {
            assert_eq!(
                MacAddress::parse(value).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn random_mac_is_locally_administered_and_unicast() {
        for _ in 0..64 {
            let address = random_mac_address().unwrap().octets();
            assert_eq!(address[0] & 3, 2);
        }
    }

    #[test]
    fn explicit_and_random_options_precede_system_interface_selection() {
        let options = Options {
            mac_address: Some("12:34:56:78:9a:bc".to_owned()),
            ..Options::default()
        };
        assert_eq!(
            select_mac_address(&options).unwrap().octets(),
            [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]
        );

        let random_options = Options {
            random_mac_address: true,
            ..Options::default()
        };
        let address = select_mac_address(&random_options).unwrap().octets();
        assert_eq!(address[0] & 3, 2);
    }
}
