//! Safe networking primitives corresponding to UxPlay's `lib/netutils.c`.
//!
//! Rust's standard networking runtime owns Winsock initialization on Windows; dropping
//! the returned sockets performs the cleanup that the C caller paired with
//! `netutils_cleanup()`.

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddressBytes {
    pub address: Vec<u8>,
    pub zone_id: u32,
    pub port: Option<u16>,
}

/// Extract the same address bytes and scope ID returned by `netutils_get_address`.
/// IPv4-mapped IPv6 addresses are exposed as four IPv4 bytes, as in the C helper.
pub fn address_bytes(address: SocketAddr, include_port: bool) -> AddressBytes {
    match address {
        SocketAddr::V4(value) => AddressBytes {
            address: value.ip().octets().to_vec(),
            zone_id: 0,
            port: include_port.then_some(value.port()),
        },
        SocketAddr::V6(value) => {
            if let Some(mapped) = value.ip().to_ipv4_mapped() {
                AddressBytes {
                    address: mapped.octets().to_vec(),
                    zone_id: 0,
                    port: include_port.then_some(value.port()),
                }
            } else {
                AddressBytes {
                    address: value.ip().octets().to_vec(),
                    zone_id: value.scope_id(),
                    port: include_port.then_some(value.port()),
                }
            }
        }
    }
}

/// Parse a numeric address only. Hostname lookup is intentionally disabled, matching
/// `AI_NUMERICHOST` in the source implementation.
pub fn parse_numeric_address(family: AddressFamily, source: &str) -> io::Result<SocketAddr> {
    match family {
        AddressFamily::Ipv4 => Ok(SocketAddr::V4(SocketAddrV4::new(
            source.parse::<Ipv4Addr>().map_err(invalid_address)?,
            0,
        ))),
        AddressFamily::Ipv6 => Ok(SocketAddr::V6(scoped_ipv6_socket_addr(source, 0)?)),
    }
}

fn invalid_address(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
}

/// Parse a numeric IPv6 address including an optional numeric scope ID.
pub fn scoped_ipv6_socket_addr(source: &str, port: u16) -> io::Result<SocketAddrV6> {
    let (literal, scope_id) = source
        .rsplit_once('%')
        .map_or((source, 0), |(address, zone)| {
            (address, zone.parse::<u32>().unwrap_or(u32::MAX))
        });
    if scope_id == u32::MAX {
        return Err(invalid_address("IPv6 scope ID must be numeric"));
    }
    let address = literal.parse::<Ipv6Addr>().map_err(invalid_address)?;
    Ok(SocketAddrV6::new(address, port, 0, scope_id))
}

/// Bind an unlistened TCP or UDP socket and update `port` when zero requests an
/// ephemeral port. The caller decides when to listen, matching the C call boundary.
pub fn bind_socket(
    port: &mut u16,
    family: AddressFamily,
    transport: Transport,
    apple_peer_to_peer: bool,
) -> io::Result<Socket> {
    let (domain, address) = match family {
        AddressFamily::Ipv4 => (
            Domain::IPV4,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, *port)),
        ),
        AddressFamily::Ipv6 => (
            Domain::IPV6,
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, *port, 0, 0)),
        ),
    };
    let (kind, protocol) = match transport {
        Transport::Tcp => (Type::STREAM, Protocol::TCP),
        Transport::Udp => (Type::DGRAM, Protocol::UDP),
    };
    let socket = Socket::new(domain, kind, Some(protocol))?;
    socket.set_reuse_address(true)?;
    #[cfg(all(target_os = "macos", feature = "apple-p2p"))]
    if apple_peer_to_peer {
        use std::os::fd::AsRawFd;
        const SO_RECV_ANYIF: libc::c_int = 0x1104;
        let enabled: libc::c_int = 1;
        // SO_RECV_ANYIF is a private XNU option used by UxPlay's Bonjour P2P mode.
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                SO_RECV_ANYIF,
                &enabled as *const _ as *const libc::c_void,
                std::mem::size_of_val(&enabled) as libc::socklen_t,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            eprintln!("{}", peer_to_peer_socket_error_message(&error));
            return Err(error);
        }
    }
    #[cfg(not(all(target_os = "macos", feature = "apple-p2p")))]
    let _ = apple_peer_to_peer;

    if family == AddressFamily::Ipv6 {
        #[cfg(not(windows))]
        set_only_v6_best_effort(|| socket.set_only_v6(true));
    }

    socket.bind(&SockAddr::from(address))?;
    *port = socket
        .local_addr()?
        .as_socket()
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "bound socket has no IP address")
        })?
        .port();
    Ok(socket)
}

// `netutils.c` attempts IPV6_V6ONLY but ignores the setsockopt result.
#[cfg(any(test, not(windows)))]
fn set_only_v6_best_effort(setter: impl FnOnce() -> io::Result<()>) {
    let _ = setter();
}

fn peer_to_peer_socket_error_message(error: &io::Error) -> String {
    format!("setsockopt(SO_RECV_ANYIF) failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_address_extraction_matches_network_order_bytes() {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 9), 7000));
        assert_eq!(
            address_bytes(address, true),
            AddressBytes {
                address: vec![192, 0, 2, 9],
                zone_id: 0,
                port: Some(7000),
            }
        );
        assert_eq!(address_bytes(address, false).port, None);
    }

    #[test]
    fn peer_to_peer_socket_option_error_keeps_the_source_diagnostic() {
        let error = io::Error::from_raw_os_error(13);
        assert_eq!(
            peer_to_peer_socket_error_message(&error),
            format!("setsockopt(SO_RECV_ANYIF) failed: {error}")
        );
    }

    #[test]
    fn ipv6_only_option_failure_is_ignored_like_the_source() {
        let mut attempted = false;
        set_only_v6_best_effort(|| {
            attempted = true;
            Err(io::Error::from_raw_os_error(22))
        });
        assert!(attempted);
    }

    #[test]
    fn ipv4_mapped_ipv6_is_returned_as_ipv4() {
        let address = SocketAddr::V6(SocketAddrV6::new(
            Ipv4Addr::new(198, 51, 100, 4).to_ipv6_mapped(),
            5000,
            0,
            21,
        ));
        let bytes = address_bytes(address, true);
        assert_eq!(bytes.address, [198, 51, 100, 4]);
        assert_eq!(bytes.zone_id, 0);
        assert_eq!(bytes.port, Some(5000));
    }

    #[test]
    fn ipv6_scope_is_preserved() {
        let address = SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(), 9, 0, 7));
        let bytes = address_bytes(address, true);
        let SocketAddr::V6(address) = address else {
            unreachable!();
        };
        assert_eq!(bytes.address, address.ip().octets());
        assert_eq!(bytes.zone_id, 7);
    }

    #[test]
    fn numeric_address_parser_rejects_hostnames_and_wrong_family() {
        assert_eq!(
            parse_numeric_address(AddressFamily::Ipv4, "192.0.2.1").unwrap(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 0))
        );
        assert!(parse_numeric_address(AddressFamily::Ipv4, "localhost").is_err());
        assert!(parse_numeric_address(AddressFamily::Ipv6, "192.0.2.1").is_err());
    }

    #[test]
    fn ipv6_scope_parser_accepts_numeric_scope() {
        assert_eq!(
            scoped_ipv6_socket_addr("fe80::1%5", 9).unwrap().scope_id(),
            5
        );
        assert!(scoped_ipv6_socket_addr("fe80::1%en0", 9).is_err());
    }

    #[test]
    fn tcp_and_udp_bind_update_ephemeral_port() {
        for transport in [Transport::Tcp, Transport::Udp] {
            let mut port = 0;
            let socket = bind_socket(&mut port, AddressFamily::Ipv4, transport, false).unwrap();
            assert_ne!(port, 0);
            assert_eq!(
                socket.local_addr().unwrap().as_socket().unwrap().port(),
                port
            );
        }
    }

    #[test]
    fn ipv6_bind_updates_ephemeral_port_when_available() {
        let mut port = 0;
        if let Ok(socket) = bind_socket(&mut port, AddressFamily::Ipv6, Transport::Udp, false) {
            assert_ne!(port, 0);
            assert!(socket.local_addr().unwrap().as_socket().unwrap().is_ipv6());
        }
    }

    #[test]
    fn address_and_numeric_parse_fixtures_match_c_netutils_oracle() {
        fn trace(label: &str, address: AddressBytes) -> String {
            let hex = address
                .address
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!(
                "{label} {} {hex} {} {}",
                address.address.len(),
                address.zone_id,
                address.port.unwrap_or_default()
            )
        }

        let ipv4 = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 9), 7000));
        let mapped = SocketAddr::V6(SocketAddrV6::new(
            Ipv4Addr::new(198, 51, 100, 4).to_ipv6_mapped(),
            5000,
            0,
            21,
        ));
        let scoped = SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(), 9, 0, 7));
        let parsed_v4 = parse_numeric_address(AddressFamily::Ipv4, "192.0.2.1").unwrap();
        let parsed_v6 = parse_numeric_address(AddressFamily::Ipv6, "fe80::1%5").unwrap();
        let invalid_host = parse_numeric_address(AddressFamily::Ipv4, "localhost");
        let wrong_family = parse_numeric_address(AddressFamily::Ipv6, "192.0.2.1");

        let actual = [
            trace("get-v4", address_bytes(ipv4, true)),
            trace("get-v4-mapped", address_bytes(mapped, true)),
            trace("get-v6-scope", address_bytes(scoped, true)),
            trace("parse-v4", address_bytes(parsed_v4, true)),
            trace("parse-v6-scope", address_bytes(parsed_v6, true)),
            format!("parse-host {}", if invalid_host.is_err() { -1 } else { 0 }),
            format!(
                "parse-v6-v4-literal {}",
                if wrong_family.is_err() { -1 } else { 0 }
            ),
        ]
        .join("\n")
            + "\n";
        assert_eq!(
            actual,
            include_str!("../testdata/netutils_oracle.expected.txt")
        );
    }
}
