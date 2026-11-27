//! DNS-SD response construction and query routing from `lib/mdnsd/mdnsd.c`.
//!
//! The module is deliberately transport-free: a socket owner supplies a
//! received query and sends the returned packets to multicast and, when the
//! source requested it, back to the peer.

use crate::discovery_services::{DiscoveryServices, ServiceRecord};
use crate::mdns_wire::{
    DNS_TYPE_A, DNS_TYPE_AAAA, DNS_TYPE_PTR, DNS_TYPE_SRV, DNS_TYPE_TXT, MAX_PACKET, PacketBuilder,
    WireError, name_equals, query_type_matches, read_name, read_u16,
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const SERVICE_TTL: u32 = 4500;
pub const HOST_TTL: u32 = 120;
pub const DNS_PORT: u16 = 5353;
const COMBINED_PACKET_MAX: usize = 1200;
const DNS_HEADER_SIZE: usize = 12;
const DNS_RESPONSE_AUTHORITATIVE: u16 = 0x8400;
const DNS_UNICAST_RESPONSE: u16 = 0x8000;
const SERVICE_ENUMERATION_NAME: &[u8] = b"_services._dns-sd._udp.local";
const MULTICAST_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MULTICAST_IPV6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);
const SOCKET_POLL_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostAddresses {
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6: Option<[u8; 16]>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResponseSelection {
    pub airplay: bool,
    pub raop: bool,
    pub host: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryResponsePlan {
    /// Packets sent to the family multicast group for every matching query.
    pub multicast: Vec<Vec<u8>>,
    /// A copy of the same packets, sent to the query peer when requested.
    pub unicast: Vec<Vec<u8>>,
}

/// Select the same default IPv4/IPv6 host addresses used by the source
/// responder. Windows deliberately publishes only its filtered IPv4 choice.
pub fn default_host_addresses() -> (HostAddresses, u32) {
    let ipv4 = default_ipv4_address();
    #[cfg(unix)]
    let (ipv6, scope_id) = default_ipv6_address_scope();
    #[cfg(not(unix))]
    let (ipv6, scope_id) = (None, 0);
    (HostAddresses { ipv4, ipv6 }, scope_id)
}

#[cfg(unix)]
fn default_ipv4_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket
        .connect(SocketAddrV4::new(MULTICAST_IPV4, DNS_PORT))
        .ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) => Some(address),
        std::net::IpAddr::V6(_) => None,
    }
}

#[cfg(windows)]
fn default_ipv4_address() -> Option<Ipv4Addr> {
    windows_network::default_ipv4_address()
}

#[cfg(not(any(unix, windows)))]
fn default_ipv4_address() -> Option<Ipv4Addr> {
    None
}

#[cfg(unix)]
fn default_ipv6_address_scope() -> (Option<[u8; 16]>, u32) {
    use std::ptr;

    let mut head = ptr::null_mut();
    // SAFETY: `head` is a valid out-pointer and is freed below on success.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return (None, 0);
    }

    let mut current = head;
    let mut found = None;
    let mut found_scope = 0;
    while !current.is_null() {
        // SAFETY: `current` traverses the linked list returned by getifaddrs.
        let interface = unsafe { &*current };
        let is_ipv6 = if interface.ifa_addr.is_null() {
            false
        } else {
            // SAFETY: getifaddrs provides a valid sockaddr pointer for this node.
            unsafe {
                (*interface.ifa_addr.cast::<libc::sockaddr>()).sa_family as i32 == libc::AF_INET6
            }
        };
        if is_ipv6
            && interface.ifa_flags & (libc::IFF_UP as u32 | libc::IFF_MULTICAST as u32)
                == (libc::IFF_UP as u32 | libc::IFF_MULTICAST as u32)
            && interface.ifa_flags & libc::IFF_LOOPBACK as u32 == 0
        {
            // SAFETY: the address family was checked above.
            let address = unsafe { &*interface.ifa_addr.cast::<libc::sockaddr_in6>() };
            let octets = address.sin6_addr.s6_addr;
            let parsed = std::net::Ipv6Addr::from(octets);
            if !parsed.is_unspecified() && !parsed.is_loopback() {
                let link_local = parsed.is_unicast_link_local();
                if found.is_none() || link_local {
                    let mut scope = address.sin6_scope_id;
                    if scope == 0 {
                        // SAFETY: ifa_name is a NUL-terminated interface name from getifaddrs.
                        scope = unsafe { libc::if_nametoindex(interface.ifa_name) };
                    }
                    found = Some(octets);
                    found_scope = scope;
                    if link_local {
                        break;
                    }
                }
            }
        }
        current = interface.ifa_next;
    }

    // SAFETY: `head` came from a successful getifaddrs call.
    unsafe { libc::freeifaddrs(head) };
    if found_scope == 0 {
        (None, 0)
    } else {
        (found, found_scope)
    }
}

#[cfg(windows)]
fn default_ipv6_address_scope() -> (Option<[u8; 16]>, u32) {
    (None, 0)
}

#[cfg(all(not(unix), not(windows)))]
fn default_ipv6_address_scope() -> (Option<[u8; 16]>, u32) {
    (None, 0)
}

#[cfg(windows)]
mod windows_network {
    use std::mem::size_of;
    use std::net::Ipv4Addr;
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GetIfEntry2, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
        MIB_IF_ROW2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    const INITIAL_BUFFER_LENGTH: u32 = 15_000;
    pub(super) fn default_ipv4_address() -> Option<Ipv4Addr> {
        let mut buffer_length = INITIAL_BUFFER_LENGTH;
        let mut buffer = allocate_buffer(buffer_length);
        // SAFETY: the buffer is aligned and sized for the length passed to the API.
        let mut result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET),
                0,
                ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut buffer_length,
            )
        };
        if result == ERROR_BUFFER_OVERFLOW {
            buffer = allocate_buffer(buffer_length);
            // SAFETY: the resized buffer matches the updated size passed to the API.
            result = unsafe {
                GetAdaptersAddresses(
                    u32::from(AF_INET),
                    0,
                    ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut buffer_length,
                )
            };
        }
        if result != 0 {
            return None;
        }

        let mut candidates = Vec::new();
        let mut adapter = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            // SAFETY: API-provided adapter nodes remain valid while `buffer` is alive.
            let current = unsafe { &*adapter };
            if current.OperStatus == IfOperStatusUp
                && current.IfType != IF_TYPE_SOFTWARE_LOOPBACK
                && !current.FirstUnicastAddress.is_null()
            {
                // SAFETY: the unicast list is owned by the API buffer and first node is non-null.
                let unicast = unsafe { &*current.FirstUnicastAddress };
                if !unicast.Address.lpSockaddr.is_null() {
                    // SOCKADDR_IN.sin_addr begins at byte offset 4 on supported Windows ABIs.
                    // SAFETY: GetAdaptersAddresses(AF_INET) returns SOCKADDR_IN here.
                    let octets = unsafe {
                        ptr::read_unaligned(
                            unicast
                                .Address
                                .lpSockaddr
                                .cast::<u8>()
                                .add(4)
                                .cast::<[u8; 4]>(),
                        )
                    };
                    let address = Ipv4Addr::from(octets);
                    if !address.is_unspecified()
                        && !address.is_loopback()
                        && !address.is_multicast()
                    {
                        let hardware = is_hardware_interface(current.Luid).unwrap_or(false);
                        candidates.push((hardware, current.Ipv4Metric, address));
                    }
                }
            }
            adapter = current.Next;
        }
        candidates
            .into_iter()
            .min_by_key(|(hardware, metric, _)| interface_priority(*hardware, *metric))
            .map(|(_, _, address)| address)
    }

    fn allocate_buffer(bytes: u32) -> Vec<u64> {
        vec![0; (bytes as usize).div_ceil(size_of::<u64>())]
    }

    fn is_hardware_interface(
        luid: windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH,
    ) -> Option<bool> {
        let mut row = MIB_IF_ROW2 {
            InterfaceLuid: luid,
            ..Default::default()
        };
        // SAFETY: GetIfEntry2 receives a writable row initialized with its interface LUID.
        if unsafe { GetIfEntry2(&mut row) } != 0 {
            return None;
        }
        // MIB_IF_ROW2's first status bit is HardwareInterface.
        Some(row.InterfaceAndOperStatusFlags._bitfield & 1 != 0)
    }

    fn interface_priority(hardware: bool, metric: u32) -> (bool, u32) {
        (!hardware, metric)
    }

    #[cfg(test)]
    mod tests {
        use super::interface_priority;

        #[test]
        fn physical_interfaces_precede_virtual_and_use_metric_for_ties() {
            assert!(interface_priority(true, 25) < interface_priority(false, 1));
            assert!(interface_priority(true, 25) < interface_priority(true, 35));
        }
    }
}

/// Live multicast responder. Address discovery remains owned by the app's
/// interface-selection layer, while this type owns the family sockets and
/// query workers just as `mdnsd_start`/`mdnsd_stop` do in the reference.
pub struct MdnsResponder {
    services: Arc<Mutex<DiscoveryServices>>,
    addresses: HostAddresses,
    ipv6_scope_id: u32,
    shutdown: Arc<AtomicBool>,
    socket4: Option<Arc<UdpSocket>>,
    socket6: Option<Arc<UdpSocket>>,
    workers: Vec<JoinHandle<()>>,
}

impl MdnsResponder {
    /// Bind whichever IPv4/IPv6 mDNS sockets are available. Like the source,
    /// startup succeeds when either family is ready and fails only if both
    /// sockets cannot be opened.
    pub fn start(
        services: DiscoveryServices,
        addresses: HostAddresses,
        ipv6_scope_id: u32,
    ) -> io::Result<Self> {
        let (socket4, error4) = match open_socket4(addresses.ipv4) {
            Ok(socket) => (Some(Arc::new(socket)), None),
            Err(error) => (None, Some(error)),
        };
        let (socket6, error6) = match open_socket6(ipv6_scope_id) {
            Ok(socket) => (socket.map(Arc::new), None),
            Err(error) => (None, Some(error)),
        };
        if socket4.is_none() && socket6.is_none() {
            return Err(error4.or(error6).unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "no mDNS family is available",
                )
            }));
        }

        let services = Arc::new(Mutex::new(services));
        let shutdown = Arc::new(AtomicBool::new(false));
        let workers = start_query_workers(
            socket4.is_some(),
            socket6.is_some(),
            Arc::clone(&services),
            Arc::clone(&shutdown),
            |family| match family {
                AddressFamily::Ipv4 => spawn_query_worker(
                    Arc::clone(socket4.as_ref().expect("IPv4 worker has a socket")),
                    Arc::clone(&services),
                    addresses,
                    0,
                    family,
                    Arc::clone(&shutdown),
                ),
                AddressFamily::Ipv6 => spawn_query_worker(
                    Arc::clone(socket6.as_ref().expect("IPv6 worker has a socket")),
                    Arc::clone(&services),
                    addresses,
                    ipv6_scope_id,
                    family,
                    Arc::clone(&shutdown),
                ),
            },
        )?;

        Ok(Self {
            services,
            addresses,
            ipv6_scope_id,
            shutdown,
            socket4,
            socket6,
            workers,
        })
    }

    pub fn update_services(&self, services: DiscoveryServices) {
        *lock_unpoisoned(&self.services) = services;
    }

    /// Send the source's current-service announcement on each available family.
    pub fn announce(&self, ttl: u32) {
        let services = lock_unpoisoned(&self.services);
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        let selection = ResponseSelection {
            airplay: true,
            raop: true,
            host: true,
        };
        if let Some(socket) = &self.socket4 {
            let packets = build_responses(
                &services,
                self.addresses,
                selection,
                AddressFamily::Ipv4,
                ttl,
            );
            send_multicast(socket, AddressFamily::Ipv4, 0, &packets);
        }
        if let Some(socket) = &self.socket6 {
            let packets = build_responses(
                &services,
                self.addresses,
                selection,
                AddressFamily::Ipv6,
                ttl,
            );
            send_multicast(socket, AddressFamily::Ipv6, self.ipv6_scope_id, &packets);
        }
    }

    /// Send a zero-TTL goodbye for the record supplied by the unregister action.
    pub fn goodbye(&self, service: &ServiceRecord) {
        let services = lock_unpoisoned(&self.services);
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        let host_name = services.host_name();
        let (packet4, packet6) = build_goodbye_packets(service, host_name, self.addresses);
        if let (Some(socket), Some(packet)) = (&self.socket4, packet4) {
            send_multicast(
                socket,
                AddressFamily::Ipv4,
                0,
                std::slice::from_ref(&packet),
            );
        }
        if let (Some(socket), Some(packet)) = (&self.socket6, packet6) {
            send_multicast(
                socket,
                AddressFamily::Ipv6,
                self.ipv6_scope_id,
                std::slice::from_ref(&packet),
            );
        }
    }

    pub fn stop(&mut self) {
        // `mdnsd_stop()` takes the service mutex before setting `running = 0`.
        // Query and announcement sends use that same mutex, so once this store
        // completes no later callback can send through the worker-owned socket.
        signal_shutdown(&self.services, &self.shutdown);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        // `mdnsd_stop()` closes both family sockets before returning. The
        // workers own temporary Arcs, so release the responder's handles once
        // they have joined.
        drop(self.socket4.take());
        drop(self.socket6.take());
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_socket4(interface: Option<Ipv4Addr>) -> io::Result<UdpSocket> {
    let interface = interface.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = socket.set_reuse_address(true);
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);
    let _ = socket.set_multicast_ttl_v4(255);
    let _ = socket.set_multicast_loop_v4(true);
    if !interface.is_unspecified() {
        let _ = socket.set_multicast_if_v4(&interface);
    }
    socket.bind(&SockAddr::from(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        DNS_PORT,
    )))?;
    let socket: UdpSocket = socket.into();
    if let Err(error) = socket.join_multicast_v4(&MULTICAST_IPV4, &interface)
        && (interface.is_unspecified()
            || socket
                .join_multicast_v4(&MULTICAST_IPV4, &Ipv4Addr::UNSPECIFIED)
                .is_err())
    {
        return Err(error);
    }
    socket.set_read_timeout(Some(SOCKET_POLL_TIMEOUT))?;
    Ok(socket)
}

fn open_socket6(scope_id: u32) -> io::Result<Option<UdpSocket>> {
    if scope_id == 0 {
        return Ok(None);
    }
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = socket.set_reuse_address(true);
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);
    // The C responder attempts IPV6_V6ONLY but deliberately ignores failure.
    let _ = socket.set_only_v6(true);
    let _ = socket.set_multicast_hops_v6(255);
    let _ = socket.set_multicast_loop_v6(true);
    let _ = socket.set_multicast_if_v6(scope_id);
    socket.bind(&SockAddr::from(SocketAddrV6::new(
        Ipv6Addr::UNSPECIFIED,
        DNS_PORT,
        0,
        0,
    )))?;
    let socket: UdpSocket = socket.into();
    socket.join_multicast_v6(&MULTICAST_IPV6, scope_id)?;
    socket.set_read_timeout(Some(SOCKET_POLL_TIMEOUT))?;
    Ok(Some(socket))
}

fn spawn_query_worker(
    socket: Arc<UdpSocket>,
    services: Arc<Mutex<DiscoveryServices>>,
    addresses: HostAddresses,
    scope_id: u32,
    family: AddressFamily,
    shutdown: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    let name = match family {
        AddressFamily::Ipv4 => "uxplay-mdns-ipv4",
        AddressFamily::Ipv6 => "uxplay-mdns-ipv6",
    };
    thread::Builder::new().name(name.to_owned()).spawn(move || {
        let mut buffer = [0u8; MAX_PACKET];
        while !shutdown.load(Ordering::Acquire) {
            let (length, peer) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => continue,
            };
            process_query_while_running(
                &services,
                &shutdown,
                addresses,
                family,
                peer.port(),
                &buffer[..length],
                |plan| {
                    send_multicast(&socket, family, scope_id, &plan.multicast);
                    for packet in plan.unicast {
                        let _ = socket.send_to(&packet, peer);
                    }
                },
            );
        }
    })
}

fn start_query_workers(
    start_ipv4: bool,
    start_ipv6: bool,
    services: Arc<Mutex<DiscoveryServices>>,
    shutdown: Arc<AtomicBool>,
    mut spawn: impl FnMut(AddressFamily) -> io::Result<JoinHandle<()>>,
) -> io::Result<Vec<JoinHandle<()>>> {
    let mut workers = Vec::with_capacity(2);
    for (family, should_start) in [
        (AddressFamily::Ipv4, start_ipv4),
        (AddressFamily::Ipv6, start_ipv6),
    ] {
        if !should_start {
            continue;
        }
        match spawn(family) {
            Ok(worker) => workers.push(worker),
            Err(error) => {
                // `mdnsd_start` closes every opened socket and returns -1 if
                // its single listener thread cannot be created. Our per-family
                // workers use the same source-visible failure code and all
                // already-started workers are joined before socket owners drop.
                signal_shutdown(&services, &shutdown);
                for worker in workers {
                    let _ = worker.join();
                }
                return Err(worker_spawn_error(error));
            }
        }
    }
    Ok(workers)
}

fn worker_spawn_error(error: io::Error) -> io::Error {
    io::Error::other(error)
}

fn send_multicast(socket: &UdpSocket, family: AddressFamily, scope_id: u32, packets: &[Vec<u8>]) {
    let destination = match family {
        AddressFamily::Ipv4 => SocketAddr::V4(SocketAddrV4::new(MULTICAST_IPV4, DNS_PORT)),
        AddressFamily::Ipv6 => {
            SocketAddr::V6(SocketAddrV6::new(MULTICAST_IPV6, DNS_PORT, 0, scope_id))
        }
    };
    for packet in packets {
        let _ = socket.send_to(packet, destination);
    }
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn signal_shutdown(services: &Mutex<DiscoveryServices>, shutdown: &AtomicBool) {
    let _services = lock_unpoisoned(services);
    shutdown.store(true, Ordering::Release);
}

fn process_query_with_services(
    services: &Mutex<DiscoveryServices>,
    addresses: HostAddresses,
    family: AddressFamily,
    source_port: u16,
    query: &[u8],
    send: impl FnOnce(QueryResponsePlan),
) {
    let current = lock_unpoisoned(services);
    let plan = respond_to_query(&current, addresses, family, source_port, query);
    // C holds `mdnsd->mutex` through response selection and both packet sends.
    send(plan);
}

fn process_query_while_running(
    services: &Mutex<DiscoveryServices>,
    shutdown: &AtomicBool,
    addresses: HostAddresses,
    family: AddressFamily,
    source_port: u16,
    query: &[u8],
    send: impl FnOnce(QueryResponsePlan),
) {
    let current = lock_unpoisoned(services);
    if shutdown.load(Ordering::Acquire) {
        return;
    }
    let plan = respond_to_query(&current, addresses, family, source_port, query);
    // Keep the service lock through sends, just like mdns_handle_query. Stop
    // takes this lock before setting shutdown, making the send/stop order clear.
    send(plan);
}

/// Build response packets in the source's order, combining both services only
/// when the combined packet fits the source's 1200-byte ceiling.
pub fn build_responses(
    services: &DiscoveryServices,
    addresses: HostAddresses,
    selection: ResponseSelection,
    family: AddressFamily,
    ttl: u32,
) -> Vec<Vec<u8>> {
    let airplay = services.airplay();
    let raop = services.raop();

    if selection.airplay
        && selection.raop
        && airplay.registered
        && raop.registered
        && let Ok(Some(packet)) = build_packet(
            &[airplay, raop],
            selection.host,
            services.host_name(),
            addresses,
            family,
            ttl,
        )
        && packet.len() <= COMBINED_PACKET_MAX
    {
        return vec![packet];
    }

    let mut packets = Vec::with_capacity(2);
    if selection.airplay
        && airplay.registered
        && let Ok(Some(packet)) = build_packet(
            &[airplay],
            selection.host,
            services.host_name(),
            addresses,
            family,
            ttl,
        )
    {
        packets.push(packet);
    }
    if selection.raop
        && raop.registered
        && let Ok(Some(packet)) = build_packet(
            &[raop],
            selection.host,
            services.host_name(),
            addresses,
            family,
            ttl,
        )
    {
        packets.push(packet);
    }
    if !selection.airplay
        && !selection.raop
        && selection.host
        && let Ok(Some(packet)) =
            build_packet(&[], true, services.host_name(), addresses, family, ttl)
    {
        packets.push(packet);
    }
    packets
}

/// Turn one DNS query into the multicast and optional duplicate unicast sends
/// used by `mdns_handle_query`.
pub fn respond_to_query(
    services: &DiscoveryServices,
    addresses: HostAddresses,
    family: AddressFamily,
    source_port: u16,
    query: &[u8],
) -> QueryResponsePlan {
    let Some((selection, unicast_requested)) = select_query(services, query) else {
        return QueryResponsePlan::default();
    };
    if !selection.airplay && !selection.raop && !selection.host {
        return QueryResponsePlan::default();
    }

    let multicast = build_responses(services, addresses, selection, family, SERVICE_TTL);
    if multicast.is_empty() {
        return QueryResponsePlan::default();
    }
    let unicast = if unicast_requested || source_port != DNS_PORT {
        multicast.clone()
    } else {
        Vec::new()
    };
    QueryResponsePlan { multicast, unicast }
}

fn select_query(services: &DiscoveryServices, query: &[u8]) -> Option<(ResponseSelection, bool)> {
    if query.len() < DNS_HEADER_SIZE {
        return None;
    }
    let flags = read_u16(&query[2..4]).ok()?;
    if flags & 0x8000 != 0 {
        return None;
    }
    let question_count = read_u16(&query[4..6]).ok()?;
    let mut offset = DNS_HEADER_SIZE;
    let mut selection = ResponseSelection::default();
    let mut unicast_requested = false;

    for _ in 0..question_count {
        let name = match read_name(query, &mut offset, 256) {
            Ok(name) => name,
            Err(_) => break,
        };
        if offset.checked_add(4).is_none_or(|end| end > query.len()) {
            break;
        }
        let Ok(record_type) = read_u16(&query[offset..offset + 2]) else {
            break;
        };
        let Ok(class) = read_u16(&query[offset + 2..offset + 4]) else {
            break;
        };
        offset += 4;
        unicast_requested |= class & DNS_UNICAST_RESPONSE != 0;
        select_question(services, &name, record_type, &mut selection);
    }

    Some((selection, unicast_requested))
}

fn select_question(
    services: &DiscoveryServices,
    name: &[u8],
    record_type: u16,
    selection: &mut ResponseSelection,
) {
    let airplay = services.airplay();
    let raop = services.raop();
    if name_equals(name, SERVICE_ENUMERATION_NAME) && query_type_matches(record_type, DNS_TYPE_PTR)
    {
        selection.airplay |= airplay.registered;
        selection.raop |= raop.registered;
        return;
    }
    if airplay.registered
        && ((name_equals(name, &airplay.service_type)
            && query_type_matches(record_type, DNS_TYPE_PTR))
            || (name_equals(name, &airplay.name)
                && (query_type_matches(record_type, DNS_TYPE_TXT)
                    || query_type_matches(record_type, DNS_TYPE_SRV))))
    {
        selection.airplay = true;
        selection.host = true;
        return;
    }
    if raop.registered
        && ((name_equals(name, &raop.service_type)
            && query_type_matches(record_type, DNS_TYPE_PTR))
            || (name_equals(name, &raop.name)
                && (query_type_matches(record_type, DNS_TYPE_TXT)
                    || query_type_matches(record_type, DNS_TYPE_SRV))))
    {
        selection.raop = true;
        selection.host = true;
        return;
    }
    if name_equals(name, services.host_name())
        && (query_type_matches(record_type, DNS_TYPE_A)
            || query_type_matches(record_type, DNS_TYPE_AAAA))
    {
        selection.host = true;
    }
}

fn build_packet(
    records: &[&ServiceRecord],
    include_host: bool,
    host_name: &[u8],
    addresses: HostAddresses,
    family: AddressFamily,
    ttl: u32,
) -> Result<Option<Vec<u8>>, WireError> {
    let mut answer_records = PacketBuilder::new();
    let mut answer_count = 0u16;
    let host_ttl = if ttl == 0 { 0 } else { HOST_TTL };

    for service in records {
        append_service(&mut answer_records, service, host_name, ttl)?;
        answer_count = answer_count.saturating_add(4);
    }
    if include_host {
        match family {
            AddressFamily::Ipv4 => {
                if let Some(address) = addresses.ipv4.filter(|value| !value.is_unspecified()) {
                    answer_records.add_a(
                        host_name,
                        u32::from_ne_bytes(address.octets()),
                        host_ttl,
                    )?;
                    answer_count = answer_count.saturating_add(1);
                }
            }
            AddressFamily::Ipv6 => {
                if let Some(address) = addresses
                    .ipv6
                    .filter(|value| value.iter().any(|byte| *byte != 0))
                {
                    answer_records.add_aaaa(host_name, &address, host_ttl)?;
                    answer_count = answer_count.saturating_add(1);
                }
            }
        }
    }
    if answer_count == 0 {
        return Ok(None);
    }

    let mut packet = PacketBuilder::new();
    packet.put_u16(0)?;
    packet.put_u16(u32::from(DNS_RESPONSE_AUTHORITATIVE))?;
    packet.put_u16(0)?;
    packet.put_u16(u32::from(answer_count))?;
    packet.put_u16(0)?;
    packet.put_u16(0)?;
    packet.put_bytes(answer_records.bytes())?;
    if packet.len() > MAX_PACKET {
        return Ok(None);
    }
    Ok(Some(packet.bytes().to_vec()))
}

fn build_goodbye_packets(
    service: &ServiceRecord,
    host_name: &[u8],
    addresses: HostAddresses,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let build = |family| {
        build_packet(&[service], false, host_name, addresses, family, 0)
            .ok()
            .flatten()
    };
    (build(AddressFamily::Ipv4), build(AddressFamily::Ipv6))
}

fn append_service(
    packet: &mut PacketBuilder,
    service: &ServiceRecord,
    host_name: &[u8],
    ttl: u32,
) -> Result<(), WireError> {
    packet.add_ptr(SERVICE_ENUMERATION_NAME, &service.service_type, ttl)?;
    packet.add_ptr(&service.service_type, &service.name, ttl)?;
    packet.add_srv(
        &service.name,
        service.port,
        host_name,
        if ttl == 0 { 0 } else { HOST_TTL },
    )?;
    packet.add_txt(&service.name, &service.txt, ttl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery_services::{ServiceNames, ServiceTxtRecords};
    use crate::mdns_wire::{DNS_CLASS_IN, DNS_TYPE_ANY};

    const ORACLE: &str = include_str!("../testdata/mdns_response_oracle.expected.txt");

    fn oracle_row(label: &str) -> Vec<&str> {
        ORACLE
            .lines()
            .find_map(|line| {
                let (entry, fields) = line.split_once('|')?;
                (entry == label).then(|| fields.split('|').collect())
            })
            .expect("oracle row")
    }

    fn expected_packet(label: &str) -> Vec<u8> {
        let fields = oracle_row(label);
        assert_eq!(fields[0], "0", "C source must build {label}");
        let hex = fields[1];
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    fn expected_packet_length(label: &str) -> usize {
        let fields = oracle_row(label);
        assert_eq!(fields[0], "0", "C source must build {label}");
        fields[1].parse().unwrap()
    }

    fn fixture_services() -> DiscoveryServices {
        let names = ServiceNames {
            host_name: b"UxPlay.local".to_vec(),
            raop_name: b"AABBCCDDEEFF@Living Room._raop._tcp.local".to_vec(),
            airplay_name: b"Living Room._airplay._tcp.local".to_vec(),
        };
        let txt = ServiceTxtRecords {
            airplay: encode_txt(&[
                (b"deviceid", b"010203040506"),
                (b"features", b"0x5A7FFFF7,0x1E"),
            ]),
            raop: encode_txt(&[(b"txtvers", b"1"), (b"ch", b"2")]),
        };
        DiscoveryServices::new_with_txt(names, txt)
    }

    fn registered_services() -> DiscoveryServices {
        let mut services = fixture_services();
        let airplay_txt = services.airplay().txt.clone();
        let raop_txt = services.raop().txt.clone();
        services.register_airplay(7000, &airplay_txt, 0).unwrap();
        services.register_raop(5000, &raop_txt, 0).unwrap();
        services
    }

    #[test]
    fn stop_releases_the_socket_like_mdnsd_stop() {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut responder = MdnsResponder {
            services: Arc::new(Mutex::new(fixture_services())),
            addresses: HostAddresses::default(),
            ipv6_scope_id: 0,
            shutdown: Arc::new(AtomicBool::new(false)),
            socket4: Some(Arc::new(socket)),
            socket6: None,
            workers: Vec::new(),
        };

        responder.stop();

        assert!(responder.socket4.is_none());
        assert!(responder.socket6.is_none());
        let rebound = UdpSocket::bind(address).unwrap();
        assert_eq!(rebound.local_addr().unwrap(), address);
    }

    #[test]
    fn stop_waits_for_a_locked_query_and_later_queries_do_not_send() {
        use std::sync::TryLockError;
        use std::sync::mpsc;

        let services = Arc::new(Mutex::new(registered_services()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut responder = MdnsResponder {
            services: Arc::clone(&services),
            addresses: addresses(),
            ipv6_scope_id: 0,
            shutdown: Arc::clone(&shutdown),
            socket4: None,
            socket6: None,
            workers: Vec::new(),
        };

        let (send_started, send_started_rx) = mpsc::channel();
        let (allow_send_finish, allow_send_finish_rx) = mpsc::channel();
        let worker_services = Arc::clone(&services);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::spawn(move || {
            let packet = query(SERVICE_ENUMERATION_NAME, DNS_TYPE_PTR, DNS_CLASS_IN, 0);
            process_query_while_running(
                &worker_services,
                &worker_shutdown,
                addresses(),
                AddressFamily::Ipv4,
                DNS_PORT,
                &packet,
                |plan| {
                    assert_eq!(plan.multicast, [expected_packet("services-only4")]);
                    send_started.send(()).unwrap();
                    allow_send_finish_rx.recv().unwrap();
                },
            );
        });

        send_started_rx.recv().unwrap();
        assert!(matches!(services.try_lock(), Err(TryLockError::WouldBlock)));

        let (stop_started, stop_started_rx) = mpsc::channel();
        let (stop_finished, stop_finished_rx) = mpsc::channel();
        let stopper = thread::spawn(move || {
            stop_started.send(()).unwrap();
            responder.stop();
            stop_finished.send(()).unwrap();
        });
        stop_started_rx.recv().unwrap();
        let stop_waited_for_query = stop_finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err();
        let shutdown_deferred_until_send_finished = !shutdown.load(Ordering::Acquire);

        allow_send_finish.send(()).unwrap();
        worker.join().unwrap();
        stop_finished_rx.recv().unwrap();
        stopper.join().unwrap();
        assert!(stop_waited_for_query);
        assert!(shutdown_deferred_until_send_finished);
        assert!(shutdown.load(Ordering::Acquire));

        let send_after_stop = AtomicBool::new(false);
        let packet = query(SERVICE_ENUMERATION_NAME, DNS_TYPE_PTR, DNS_CLASS_IN, 0);
        process_query_while_running(
            &services,
            &shutdown,
            addresses(),
            AddressFamily::Ipv4,
            DNS_PORT,
            &packet,
            |_| send_after_stop.store(true, Ordering::Release),
        );
        assert!(!send_after_stop.load(Ordering::Acquire));
    }

    fn addresses() -> HostAddresses {
        HostAddresses {
            ipv4: Some(Ipv4Addr::new(192, 168, 1, 77)),
            ipv6: Some([
                0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x77,
            ]),
        }
    }

    fn encode_txt(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut result = Vec::new();
        for (key, value) in entries {
            let mut item = key.to_vec();
            item.push(b'=');
            item.extend_from_slice(value);
            result.push(item.len() as u8);
            result.extend_from_slice(&item);
        }
        result
    }

    fn query(name: &[u8], record_type: u16, class: u16, flags: u16) -> Vec<u8> {
        let mut packet = PacketBuilder::new();
        packet.put_u16(0x1234).unwrap();
        packet.put_u16(u32::from(flags)).unwrap();
        packet.put_u16(1).unwrap();
        packet.put_u16(0).unwrap();
        packet.put_u16(0).unwrap();
        packet.put_u16(0).unwrap();
        packet.put_name(name).unwrap();
        packet.put_u16(u32::from(record_type)).unwrap();
        packet.put_u16(u32::from(class)).unwrap();
        packet.bytes().to_vec()
    }

    #[test]
    fn combined_and_family_specific_announcements_match_c_bytes() {
        let services = registered_services();
        let selected = ResponseSelection {
            airplay: true,
            raop: true,
            host: true,
        };
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                selected,
                AddressFamily::Ipv4,
                SERVICE_TTL
            ),
            [expected_packet("announcement4")]
        );
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                selected,
                AddressFamily::Ipv6,
                SERVICE_TTL
            ),
            [expected_packet("announcement6")]
        );
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                ResponseSelection {
                    host: false,
                    ..selected
                },
                AddressFamily::Ipv4,
                SERVICE_TTL
            ),
            [expected_packet("services-only4")]
        );
    }

    #[test]
    fn single_service_goodbye_and_host_responses_match_c_bytes() {
        let services = registered_services();
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                ResponseSelection {
                    airplay: true,
                    host: true,
                    ..ResponseSelection::default()
                },
                AddressFamily::Ipv4,
                SERVICE_TTL
            ),
            [expected_packet("airplay-response4")]
        );
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                ResponseSelection {
                    airplay: true,
                    ..ResponseSelection::default()
                },
                AddressFamily::Ipv4,
                0
            ),
            [expected_packet("airplay-goodbye4")]
        );
        let (goodbye4, goodbye6) =
            build_goodbye_packets(services.airplay(), services.host_name(), addresses());
        assert_eq!(goodbye4, Some(expected_packet("airplay-goodbye4")));
        assert_eq!(goodbye6, Some(expected_packet("airplay-goodbye6")));
        assert_eq!(goodbye4, goodbye6);
        assert_eq!(
            build_responses(
                &services,
                addresses(),
                ResponseSelection {
                    host: true,
                    ..ResponseSelection::default()
                },
                AddressFamily::Ipv4,
                SERVICE_TTL
            ),
            [expected_packet("host-only4")]
        );
    }

    #[test]
    fn oversized_combined_response_falls_back_to_source_single_service_packets() {
        let mut services = fixture_services();
        let value = vec![b'a'; 240];
        let airplay_txt = encode_txt(&[(b"x", &value), (b"y", &value)]);
        let raop_txt = encode_txt(&[(b"x", &value), (b"y", &value)]);
        services.register_airplay(7000, &airplay_txt, 0).unwrap();
        services.register_raop(5000, &raop_txt, 0).unwrap();
        let packets = build_responses(
            &services,
            addresses(),
            ResponseSelection {
                airplay: true,
                raop: true,
                host: true,
            },
            AddressFamily::Ipv4,
            SERVICE_TTL,
        );
        assert_eq!(oracle_row("large-combined")[0], "-1");
        assert_eq!(
            oracle_row("large-combined")[1].parse::<usize>().unwrap(),
            1498
        );
        assert_eq!(
            packets.iter().map(Vec::len).collect::<Vec<_>>(),
            [
                expected_packet_length("large-airplay"),
                expected_packet_length("large-raop"),
            ]
        );
    }

    #[test]
    fn question_matching_matches_c_registration_and_type_rules() {
        let services = registered_services();
        let cases = [
            (
                "match-enumeration",
                b"_services._dns-sd._udp.local".as_slice(),
                DNS_TYPE_PTR,
            ),
            (
                "match-airplay-type",
                b"_airplay._tcp.local".as_slice(),
                DNS_TYPE_PTR,
            ),
            (
                "match-airplay-txt",
                b"Living Room._airplay._tcp.local".as_slice(),
                DNS_TYPE_TXT,
            ),
            (
                "match-raop-srv",
                b"AABBCCDDEEFF@Living Room._raop._tcp.local".as_slice(),
                DNS_TYPE_SRV,
            ),
            ("match-host-a", b"UxPlay.local".as_slice(), DNS_TYPE_A),
            ("match-host-any", b"uxplay.LOCAL".as_slice(), DNS_TYPE_ANY),
            ("match-unknown", b"other.local".as_slice(), DNS_TYPE_PTR),
        ];
        for (label, name, record_type) in cases {
            let mut selection = ResponseSelection::default();
            select_question(&services, name, record_type, &mut selection);
            let expected = oracle_row(label)
                .iter()
                .map(|value| value.parse::<i32>().unwrap() != 0)
                .collect::<Vec<_>>();
            assert_eq!(
                [
                    selection.airplay || selection.raop || selection.host,
                    selection.airplay,
                    selection.raop,
                    selection.host,
                ],
                expected.as_slice(),
                "{label}"
            );
        }

        let mut unregistered = services;
        unregistered.unregister_airplay();
        let mut selection = ResponseSelection::default();
        select_question(
            &unregistered,
            b"_airplay._tcp.local",
            DNS_TYPE_PTR,
            &mut selection,
        );
        let expected = oracle_row("match-unregistered")
            .iter()
            .map(|value| value.parse::<i32>().unwrap() != 0)
            .collect::<Vec<_>>();
        assert_eq!(
            [
                selection.airplay || selection.raop || selection.host,
                selection.airplay,
                selection.raop,
                selection.host
            ],
            expected.as_slice()
        );
    }

    #[test]
    fn query_routing_matches_multicast_and_unicast_duplicate_rules() {
        let services = registered_services();
        let enumeration = query(SERVICE_ENUMERATION_NAME, DNS_TYPE_PTR, DNS_CLASS_IN, 0);
        let multicast = respond_to_query(
            &services,
            addresses(),
            AddressFamily::Ipv4,
            DNS_PORT,
            &enumeration,
        );
        assert_eq!(multicast.multicast, [expected_packet("services-only4")]);
        assert!(multicast.unicast.is_empty());

        let host_query = query(b"UxPlay.local", DNS_TYPE_A, DNS_CLASS_IN | 0x8000, 0);
        let unicast = respond_to_query(
            &services,
            addresses(),
            AddressFamily::Ipv4,
            DNS_PORT,
            &host_query,
        );
        assert_eq!(unicast.multicast, [expected_packet("host-only4")]);
        assert_eq!(unicast.unicast, unicast.multicast);

        let legacy_source_port = respond_to_query(
            &services,
            addresses(),
            AddressFamily::Ipv4,
            5354,
            &query(b"UxPlay.local", DNS_TYPE_A, DNS_CLASS_IN, 0),
        );
        assert_eq!(legacy_source_port.unicast, legacy_source_port.multicast);
    }

    #[test]
    fn valid_questions_before_a_truncated_question_still_route_a_response() {
        let services = registered_services();
        let mut packet = query(SERVICE_ENUMERATION_NAME, DNS_TYPE_PTR, DNS_CLASS_IN, 0);
        packet[4..6].copy_from_slice(&2_u16.to_be_bytes());
        // mdns_handle_query stops parsing on the second malformed name but
        // retains the service selection made by its first valid question.
        packet.extend_from_slice(&[4, b'x', b'y']);

        let plan = respond_to_query(
            &services,
            addresses(),
            AddressFamily::Ipv4,
            DNS_PORT,
            &packet,
        );
        assert_eq!(plan.multicast, [expected_packet("services-only4")]);
        assert!(plan.unicast.is_empty());
    }

    #[test]
    fn query_selection_keeps_the_state_lock_until_packets_are_sent() {
        use std::sync::TryLockError;
        use std::sync::mpsc;

        let services = Arc::new(Mutex::new(registered_services()));
        let worker_services = Arc::clone(&services);
        let (send_started, send_started_rx) = mpsc::channel();
        let (allow_send_finish, allow_send_finish_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let query = query(SERVICE_ENUMERATION_NAME, DNS_TYPE_PTR, DNS_CLASS_IN, 0);
            process_query_with_services(
                &worker_services,
                addresses(),
                AddressFamily::Ipv4,
                DNS_PORT,
                &query,
                |plan| {
                    assert_eq!(plan.multicast, [expected_packet("services-only4")]);
                    send_started.send(()).unwrap();
                    allow_send_finish_rx.recv().unwrap();
                },
            );
        });

        send_started_rx.recv().unwrap();
        let state_lock_was_held = matches!(services.try_lock(), Err(TryLockError::WouldBlock));
        allow_send_finish.send(()).unwrap();
        worker.join().unwrap();
        assert!(state_lock_was_held);
    }

    #[test]
    fn response_packets_and_malformed_queries_are_ignored() {
        let services = registered_services();
        let response = query(b"_airplay._tcp.local", DNS_TYPE_PTR, DNS_CLASS_IN, 0x8000);
        assert_eq!(
            respond_to_query(
                &services,
                addresses(),
                AddressFamily::Ipv4,
                DNS_PORT,
                &response
            ),
            QueryResponsePlan::default()
        );
        assert_eq!(
            respond_to_query(
                &services,
                addresses(),
                AddressFamily::Ipv4,
                DNS_PORT,
                &[0; 11]
            ),
            QueryResponsePlan::default()
        );
        assert_eq!(
            respond_to_query(
                &services,
                addresses(),
                AddressFamily::Ipv4,
                DNS_PORT,
                &query(b"other.local", DNS_TYPE_PTR, DNS_CLASS_IN, 0)
            ),
            QueryResponsePlan::default()
        );
    }

    #[test]
    fn worker_spawn_error_discards_the_platform_code() {
        let error = worker_spawn_error(io::Error::from_raw_os_error(11));
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(error.raw_os_error(), None);
    }

    #[test]
    fn partial_worker_start_failure_stops_and_joins_prior_workers() {
        use std::sync::mpsc;

        let services = Arc::new(Mutex::new(registered_services()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let (worker_stopped, worker_stopped_rx) = mpsc::channel();
        let error = start_query_workers(
            true,
            true,
            Arc::clone(&services),
            Arc::clone(&shutdown),
            move |family| match family {
                AddressFamily::Ipv4 => {
                    let worker_shutdown = Arc::clone(&worker_shutdown);
                    let worker_stopped = worker_stopped.clone();
                    Ok(thread::spawn(move || {
                        while !worker_shutdown.load(Ordering::Acquire) {
                            thread::yield_now();
                        }
                        worker_stopped.send(()).unwrap();
                    }))
                }
                AddressFamily::Ipv6 => Err(io::Error::from_raw_os_error(11)),
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(error.raw_os_error(), None);
        assert!(shutdown.load(Ordering::Acquire));
        // The startup helper returns only after it has joined the earlier
        // worker, which observes shutdown and reports completion here.
        worker_stopped_rx.try_recv().unwrap();
    }
}
