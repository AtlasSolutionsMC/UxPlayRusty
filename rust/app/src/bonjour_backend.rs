//! External DNS-SD registration used by Avahi/mDNSResponder, Apple Bonjour,
//! and the Windows Bonjour DLL.

use crate::discovery_services::ServiceRecord;
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SocketEventAction {
    Continue,
    ProcessResult,
    Stop,
}

fn socket_event_action(readable: bool, faulted: bool) -> SocketEventAction {
    if faulted {
        SocketEventAction::Stop
    } else if readable {
        SocketEventAction::ProcessResult
    } else {
        SocketEventAction::Continue
    }
}

#[cfg(any(test, all(any(unix, windows), feature = "external-dns-sd")))]
fn watch_spawn_result<T>(result: io::Result<T>) -> Option<T> {
    // The source ignores a zero return from g_unix_fd_add/g_io_add_watch and
    // leaves the DNS-SD registration alive without a servicing watch.
    result.ok()
}

#[cfg(any(test, all(any(unix, windows), feature = "external-dns-sd")))]
fn service_socket_failure_message(fd: i32, service: &str) -> String {
    let service_id = if service == "raop" { 0 } else { 1 };
    format!("dnssd: stopped servicing the DNS-SD socket (fd {fd}) for service {service_id}")
}

#[cfg(any(test, all(any(unix, windows), feature = "external-dns-sd")))]
fn log_service_socket_failure(logger: &crate::logger::Logger, fd: i32, service: &str) {
    logger.log(
        crate::logger::ERR,
        service_socket_failure_message(fd, service),
    );
}

const INCLUDE_P2P: u32 = 0x0002_0000;
const INCLUDE_AWDL: u32 = 0x0010_0000;
const INTERFACE_INDEX_ANY: u32 = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationRequest {
    instance_name: Vec<u8>,
    service_type: Vec<u8>,
    interface_index: u32,
    flags: u32,
    port_network_order: u16,
    txt_record: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    all(unix, feature = "external-dns-sd", not(test)),
    expect(
        dead_code,
        reason = "Only the Windows Bonjour loader constructs these errors."
    )
)]
enum DnsSdInitError {
    LibraryNotFound,
    ProcedureNotFound,
}

impl DnsSdInitError {
    const fn source_code(self) -> i32 {
        match self {
            Self::LibraryNotFound => 3,
            Self::ProcedureNotFound => 4,
        }
    }

    #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
    fn as_io_error(self) -> io::Error {
        io::Error::from_raw_os_error(-self.source_code())
    }
}

#[cfg(any(windows, test))]
const REQUIRED_WINDOWS_DNS_SD_EXPORTS: [&[u8]; 7] = [
    b"DNSServiceRegister\0",
    b"DNSServiceRefDeallocate\0",
    b"TXTRecordCreate\0",
    b"TXTRecordSetValue\0",
    b"TXTRecordGetLength\0",
    b"TXTRecordGetBytesPtr\0",
    b"TXTRecordDeallocate\0",
];

#[cfg(any(windows, test))]
fn missing_windows_dns_sd_exports(mut has_export: impl FnMut(&[u8]) -> bool) -> bool {
    REQUIRED_WINDOWS_DNS_SD_EXPORTS
        .iter()
        .any(|name| !has_export(name))
}

#[cfg(all(unix, feature = "external-dns-sd"))]
type DnsServiceRegisterReplyFn = unsafe extern "C" fn(
    *mut std::ffi::c_void,
    u32,
    i32,
    *const std::ffi::c_char,
    *const std::ffi::c_char,
    *const std::ffi::c_char,
    *mut std::ffi::c_void,
);

#[cfg(all(windows, feature = "external-dns-sd"))]
type DnsServiceRegisterReplyFn = unsafe extern "system" fn(
    *mut std::ffi::c_void,
    u32,
    i32,
    *const std::ffi::c_char,
    *const std::ffi::c_char,
    *const std::ffi::c_char,
    *mut std::ffi::c_void,
);

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
macro_rules! define_dns_service_function_types {
    ($abi:literal) => {
        #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
        type DnsServiceRegisterFn = unsafe extern $abi fn(
            *mut *mut std::ffi::c_void,
            u32,
            u32,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            u16,
            u16,
            *const std::ffi::c_void,
            Option<DnsServiceRegisterReplyFn>,
            *mut std::ffi::c_void,
        ) -> i32;

        #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
        type DnsServiceRefDeallocateFn = unsafe extern $abi fn(*mut std::ffi::c_void);

        #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
        type DnsServiceRefSockFdFn = unsafe extern $abi fn(*mut std::ffi::c_void) -> i32;

        #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
        type DnsServiceProcessResultFn = unsafe extern $abi fn(*mut std::ffi::c_void) -> i32;
    };
}

#[cfg(all(unix, feature = "external-dns-sd"))]
define_dns_service_function_types!("C");

#[cfg(all(windows, feature = "external-dns-sd"))]
define_dns_service_function_types!("system");

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
struct DnsSdApi {
    register: DnsServiceRegisterFn,
    deallocate: DnsServiceRefDeallocateFn,
    sock_fd: Option<DnsServiceRefSockFdFn>,
    process_result: Option<DnsServiceProcessResultFn>,
    #[cfg(windows)]
    module: windows_sys::Win32::Foundation::HMODULE,
}

#[cfg(all(windows, feature = "external-dns-sd"))]
// SAFETY: the loaded module and immutable function pointers are shared only
// while the module handle remains owned by this Arc; each DNS-SD reference is
// serviced by one worker and deallocated after that worker joins.
unsafe impl Send for DnsSdApi {}

#[cfg(all(windows, feature = "external-dns-sd"))]
// SAFETY: see the Send implementation above; no mutable global API state is
// exposed through this function table.
unsafe impl Sync for DnsSdApi {}

#[cfg(all(unix, feature = "external-dns-sd"))]
impl DnsSdApi {
    fn load() -> Result<Self, DnsSdInitError> {
        Ok(Self {
            register: DNSServiceRegister,
            deallocate: DNSServiceRefDeallocate,
            sock_fd: Some(DNSServiceRefSockFD),
            process_result: Some(DNSServiceProcessResult),
        })
    }
}

#[cfg(all(windows, feature = "external-dns-sd"))]
impl DnsSdApi {
    fn load() -> Result<Self, DnsSdInitError> {
        use windows_sys::Win32::Foundation::FreeLibrary;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

        // This is the exact DLL name loaded by dnssd_private_init on Windows.
        let module = unsafe { LoadLibraryA(c"dnssd.dll".as_ptr().cast()) };
        if module.is_null() {
            return Err(DnsSdInitError::LibraryNotFound);
        }

        let has_export =
            |name: &[u8]| unsafe { GetProcAddress(module, name.as_ptr().cast()).is_some() };
        if missing_windows_dns_sd_exports(has_export) {
            unsafe { FreeLibrary(module) };
            return Err(DnsSdInitError::ProcedureNotFound);
        }

        let register_raw = unsafe { GetProcAddress(module, c"DNSServiceRegister".as_ptr().cast()) }
            .expect("required DNSServiceRegister export was checked");
        let deallocate_raw =
            unsafe { GetProcAddress(module, c"DNSServiceRefDeallocate".as_ptr().cast()) }
                .expect("required DNSServiceRefDeallocate export was checked");
        // SAFETY: GetProcAddress resolved the exact named exports; their ABIs
        // and signatures are declared by Apple's dns_sd.h and dns_sd.c.
        let register = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, DnsServiceRegisterFn>(
                register_raw,
            )
        };
        // SAFETY: see the matching GetProcAddress lookup above.
        let deallocate = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, DnsServiceRefDeallocateFn>(
                deallocate_raw,
            )
        };
        // These two exports are optional in the source DLL loader. If either
        // is absent, keep the registration alive without a servicing worker.
        let sock_fd =
            unsafe { GetProcAddress(module, c"DNSServiceRefSockFD".as_ptr().cast()) }.map(
                |procedure| {
                    // SAFETY: the optional export has the signature declared by dns_sd.h.
                    unsafe {
                        std::mem::transmute::<
                            unsafe extern "system" fn() -> isize,
                            DnsServiceRefSockFdFn,
                        >(procedure)
                    }
                },
            );
        let process_result =
            unsafe { GetProcAddress(module, c"DNSServiceProcessResult".as_ptr().cast()) }.map(
                |procedure| {
                    // SAFETY: the optional export has the signature declared by dns_sd.h.
                    unsafe {
                        std::mem::transmute::<
                            unsafe extern "system" fn() -> isize,
                            DnsServiceProcessResultFn,
                        >(procedure)
                    }
                },
            );

        Ok(Self {
            register,
            deallocate,
            sock_fd,
            process_result,
            module,
        })
    }
}

#[cfg(all(windows, feature = "external-dns-sd"))]
impl Drop for DnsSdApi {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::FreeLibrary(self.module) };
    }
}

fn registration_request(
    service: &ServiceRecord,
    peer_to_peer: bool,
) -> io::Result<RegistrationRequest> {
    let mut qualified_suffix = Vec::with_capacity(service.service_type.len() + 1);
    qualified_suffix.push(b'.');
    qualified_suffix.extend_from_slice(&service.service_type);
    let instance_name = service
        .name
        .strip_suffix(qualified_suffix.as_slice())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "DNS-SD instance name does not end in its service type",
            )
        })?;
    let service_type = service
        .service_type
        .strip_suffix(b".local")
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "DNS-SD service type is not local",
            )
        })?;

    Ok(RegistrationRequest {
        instance_name: instance_name.to_vec(),
        service_type: service_type.to_vec(),
        interface_index: INTERFACE_INDEX_ANY,
        flags: if peer_to_peer {
            INCLUDE_P2P | INCLUDE_AWDL
        } else {
            0
        },
        port_network_order: service.port.to_be(),
        txt_record: service.txt.clone(),
    })
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
pub(super) struct DnsSdFactory {
    peer_to_peer: bool,
    logger: std::sync::Arc<crate::logger::Logger>,
    api: Result<std::sync::Arc<DnsSdApi>, DnsSdInitError>,
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl DnsSdFactory {
    pub(super) fn new(peer_to_peer: bool, logger: std::sync::Arc<crate::logger::Logger>) -> Self {
        Self {
            peer_to_peer,
            logger,
            api: DnsSdApi::load().map(std::sync::Arc::new),
        }
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl crate::discovery_publisher::ResponderFactory for DnsSdFactory {
    fn initialization_error_code(&self) -> Option<i32> {
        self.api.as_ref().err().map(|error| error.source_code())
    }

    fn start(
        &self,
        _services: crate::discovery_services::DiscoveryServices,
    ) -> io::Result<Box<dyn crate::discovery_publisher::ResponderBackend>> {
        let api = self
            .api
            .as_ref()
            .map(std::sync::Arc::clone)
            .map_err(|error| error.as_io_error())?;
        Ok(Box::new(BonjourResponder {
            peer_to_peer: self.peer_to_peer,
            logger: std::sync::Arc::clone(&self.logger),
            api,
            raop: None,
            airplay: None,
            last_raop: None,
            last_airplay: None,
        }))
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
struct BonjourResponder {
    peer_to_peer: bool,
    logger: std::sync::Arc<crate::logger::Logger>,
    api: std::sync::Arc<DnsSdApi>,
    raop: Option<RegisteredService>,
    airplay: Option<RegisteredService>,
    last_raop: Option<ServiceRecord>,
    last_airplay: Option<ServiceRecord>,
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl crate::discovery_publisher::ResponderBackend for BonjourResponder {
    fn start_services(
        &mut self,
        services: crate::discovery_services::DiscoveryServices,
    ) -> io::Result<()> {
        self.update_services(services)
    }

    fn update_services(
        &mut self,
        services: crate::discovery_services::DiscoveryServices,
    ) -> io::Result<()> {
        let mut first_error = None;
        update_registration(
            &mut self.raop,
            &mut self.last_raop,
            services.raop(),
            self.peer_to_peer,
            &self.api,
            std::sync::Arc::clone(&self.logger),
            &mut first_error,
        );
        update_registration(
            &mut self.airplay,
            &mut self.last_airplay,
            services.airplay(),
            self.peer_to_peer,
            &self.api,
            std::sync::Arc::clone(&self.logger),
            &mut first_error,
        );
        first_error.map_or(Ok(()), Err)
    }

    fn announce(&self, _ttl: u32) {}

    fn goodbye(&self, _service: &ServiceRecord) {}

    fn stop(&mut self) {
        self.raop = None;
        self.airplay = None;
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
fn update_registration(
    current: &mut Option<RegisteredService>,
    previous: &mut Option<ServiceRecord>,
    service: &ServiceRecord,
    peer_to_peer: bool,
    api: &std::sync::Arc<DnsSdApi>,
    logger: std::sync::Arc<crate::logger::Logger>,
    first_error: &mut Option<io::Error>,
) {
    if previous.as_ref() == Some(service) {
        return;
    }

    *current = None;
    *previous = Some(service.clone());
    if !service.registered {
        return;
    }

    match register_service(service, peer_to_peer, std::sync::Arc::clone(api), logger) {
        Ok(registered) => *current = Some(registered),
        Err(error) if first_error.is_none() => *first_error = Some(error),
        Err(_) => {}
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
struct RegisteredService {
    reference: std::ptr::NonNull<std::ffi::c_void>,
    api: std::sync::Arc<DnsSdApi>,
    pump: Option<RegistrationPump>,
    // Keep the exact DNS-SD strings and TXT bytes alive for the lifetime of
    // the registration, as the source retains its TXTRecordRef until teardown.
    _request: RegistrationRequest,
    _instance_name: std::ffi::CString,
    _service_type: std::ffi::CString,
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl Drop for RegisteredService {
    fn drop(&mut self) {
        // Stop the socket reader before releasing the DNS-SD reference it uses.
        self.pump.take();
        // SAFETY: the DNSServiceRef came from a successful DNSServiceRegister
        // call and is uniquely owned by this registration guard.
        unsafe { (self.api.deallocate)(self.reference.as_ptr()) };
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
struct RegistrationPump {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl RegistrationPump {
    fn start(
        reference: std::ptr::NonNull<std::ffi::c_void>,
        api: &std::sync::Arc<DnsSdApi>,
        service: &'static str,
        logger: std::sync::Arc<crate::logger::Logger>,
    ) -> Option<Self> {
        use std::sync::atomic::AtomicBool;
        // SAFETY: `reference` is a live DNS-SD registration and the returned
        // descriptor remains owned by that reference until after this worker joins.
        let sock_fd = api.sock_fd?;
        let process_result = api.process_result?;
        let fd = unsafe { sock_fd(reference.as_ptr()) };
        if fd < 0 {
            // The C main loop skips services whose DNS-SD implementation has no
            // processable socket, so preserve that behavior.
            return None;
        }

        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let worker_stop = std::sync::Arc::clone(&stop);
        let worker_logger = std::sync::Arc::clone(&logger);
        #[cfg(windows)]
        let worker_api = std::sync::Arc::clone(api);
        let reference_address = reference.as_ptr() as usize;
        let worker = watch_spawn_result(
            std::thread::Builder::new()
                .name(format!("uxplay-dnssd-{service}"))
                .spawn(move || {
                    #[cfg(unix)]
                    run_unix_registration_pump(
                        fd,
                        service,
                        worker_stop,
                        reference_address,
                        process_result,
                        worker_logger,
                    );
                    #[cfg(windows)]
                    run_windows_registration_pump(
                        fd,
                        service,
                        worker_stop,
                        reference_address,
                        worker_api,
                        process_result,
                        worker_logger,
                    );
                }),
        );
        let worker = worker?;

        Some(Self {
            stop,
            worker: Some(worker),
        })
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
impl Drop for RegistrationPump {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Release;

        self.stop.store(true, Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(all(unix, feature = "external-dns-sd"))]
fn run_unix_registration_pump(
    fd: i32,
    service: &'static str,
    worker_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reference_address: usize,
    process_result: DnsServiceProcessResultFn,
    logger: std::sync::Arc<crate::logger::Logger>,
) {
    use std::sync::atomic::Ordering::Acquire;

    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    while !worker_stop.load(Acquire) {
        // A finite wait lets shutdown join promptly without a cross-thread
        // DNSServiceRef operation.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 250) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            log_service_socket_failure(&logger, fd, service);
            break;
        }
        if ready == 0 {
            continue;
        }

        let events = descriptor.revents;
        descriptor.revents = 0;
        let action = socket_event_action(
            events & libc::POLLIN != 0,
            events & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0,
        );
        match action {
            SocketEventAction::Continue => {}
            SocketEventAction::Stop => {
                log_service_socket_failure(&logger, fd, service);
                break;
            }
            SocketEventAction::ProcessResult => {
                // SAFETY: this worker exclusively services the reference until
                // it exits; RegisteredService joins it before deallocation.
                let result = unsafe { process_result(reference_address as *mut std::ffi::c_void) };
                if result != 0 {
                    log_service_socket_failure(&logger, fd, service);
                    break;
                }
            }
        }
    }
}

#[cfg(all(windows, feature = "external-dns-sd"))]
fn run_windows_registration_pump(
    fd: i32,
    service: &'static str,
    worker_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reference_address: usize,
    _api: std::sync::Arc<DnsSdApi>,
    process_result: DnsServiceProcessResultFn,
    logger: std::sync::Arc<crate::logger::Logger>,
) {
    use std::sync::atomic::Ordering::Acquire;
    use windows_sys::Win32::Networking::WinSock::{
        POLLERR, POLLHUP, POLLIN, POLLNVAL, SOCKET, WSAEINTR, WSAGetLastError, WSAPOLLFD, WSAPoll,
    };

    let mut descriptor = WSAPOLLFD {
        fd: fd as SOCKET,
        events: POLLIN,
        revents: 0,
    };
    while !worker_stop.load(Acquire) {
        // A finite wait lets shutdown join promptly without a cross-thread
        // DNSServiceRef operation. Bonjour exports these APIs only when its
        // Windows backend can be serviced through a Winsock socket.
        let ready = unsafe { WSAPoll(&mut descriptor, 1, 250) };
        if ready < 0 {
            if unsafe { WSAGetLastError() } == WSAEINTR {
                continue;
            }
            log_service_socket_failure(&logger, fd, service);
            break;
        }
        if ready == 0 {
            continue;
        }

        let events = descriptor.revents;
        descriptor.revents = 0;
        let action = socket_event_action(
            events & POLLIN != 0,
            events & (POLLHUP | POLLERR | POLLNVAL) != 0,
        );
        match action {
            SocketEventAction::Continue => {}
            SocketEventAction::Stop => {
                log_service_socket_failure(&logger, fd, service);
                break;
            }
            SocketEventAction::ProcessResult => {
                // SAFETY: this worker exclusively services the reference until
                // it exits; RegisteredService joins it before deallocation.
                let result = unsafe { process_result(reference_address as *mut std::ffi::c_void) };
                if result != 0 {
                    log_service_socket_failure(&logger, fd, service);
                    break;
                }
            }
        }
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
fn register_service(
    service: &ServiceRecord,
    peer_to_peer: bool,
    api: std::sync::Arc<DnsSdApi>,
    logger: std::sync::Arc<crate::logger::Logger>,
) -> io::Result<RegisteredService> {
    use std::ffi::{CString, c_void};
    use std::ptr::{self, NonNull};

    let request = registration_request(service, peer_to_peer)?;
    let instance_name = CString::new(request.instance_name.as_slice()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "DNS-SD name contains a NUL byte",
        )
    })?;
    let service_type = CString::new(request.service_type.as_slice()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "DNS-SD service type contains a NUL byte",
        )
    })?;
    let txt_len = u16::try_from(request.txt_record.len()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "DNS-SD TXT record is too long")
    })?;
    let mut reference = ptr::null_mut();

    // SAFETY: all strings and the TXT buffer remain owned by `request` and its
    // CStrings across the call; the out-pointer is valid; null domain and host
    // select the system defaults, matching the C registration calls.
    let result = unsafe {
        (api.register)(
            &mut reference,
            request.flags,
            request.interface_index,
            instance_name.as_ptr(),
            service_type.as_ptr(),
            ptr::null(),
            ptr::null(),
            request.port_network_order,
            txt_len,
            request.txt_record.as_ptr().cast::<c_void>(),
            Some(discard_registration_reply),
            ptr::null_mut(),
        )
    };
    if result != 0 {
        if let Some(reference) = NonNull::new(reference) {
            // SAFETY: a non-null reference returned alongside an error is still
            // owned by this call and must be released.
            unsafe { (api.deallocate)(reference.as_ptr()) };
        }
        return Err(io::Error::from_raw_os_error(result.saturating_neg()));
    }
    let reference = NonNull::new(reference).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "DNSServiceRegister succeeded without returning a service reference",
        )
    })?;
    let pump = RegistrationPump::start(reference, &api, service_name(service), logger);
    Ok(RegisteredService {
        reference,
        api,
        pump,
        _request: request,
        _instance_name: instance_name,
        _service_type: service_type,
    })
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
fn service_name(service: &ServiceRecord) -> &'static str {
    if service.service_type == b"_raop._tcp.local" {
        "raop"
    } else {
        "airplay"
    }
}

#[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
macro_rules! define_discard_registration_reply {
    ($abi:literal) => {
        #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
        unsafe extern $abi fn discard_registration_reply(
            _reference: *mut std::ffi::c_void,
            _flags: u32,
            _error_code: i32,
            _name: *const std::ffi::c_char,
            _service_type: *const std::ffi::c_char,
            _domain: *const std::ffi::c_char,
            _context: *mut std::ffi::c_void,
        ) {
        }
    };
}

#[cfg(all(unix, feature = "external-dns-sd"))]
define_discard_registration_reply!("C");

#[cfg(all(windows, feature = "external-dns-sd"))]
define_discard_registration_reply!("system");

#[cfg(all(unix, feature = "external-dns-sd"))]
#[link(name = "dns_sd")]
unsafe extern "C" {
    fn DNSServiceRegister(
        reference: *mut *mut std::ffi::c_void,
        flags: u32,
        interface_index: u32,
        name: *const std::ffi::c_char,
        service_type: *const std::ffi::c_char,
        domain: *const std::ffi::c_char,
        host: *const std::ffi::c_char,
        port_network_order: u16,
        txt_len: u16,
        txt_record: *const std::ffi::c_void,
        callback: Option<
            unsafe extern "C" fn(
                *mut std::ffi::c_void,
                u32,
                i32,
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                *mut std::ffi::c_void,
            ),
        >,
        context: *mut std::ffi::c_void,
    ) -> i32;

    fn DNSServiceRefDeallocate(reference: *mut std::ffi::c_void);
    fn DNSServiceRefSockFD(reference: *mut std::ffi::c_void) -> libc::c_int;
    fn DNSServiceProcessResult(reference: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::{
        DnsSdInitError, INCLUDE_AWDL, INCLUDE_P2P, REQUIRED_WINDOWS_DNS_SD_EXPORTS,
        SocketEventAction, missing_windows_dns_sd_exports, registration_request,
        service_socket_failure_message, socket_event_action, watch_spawn_result,
    };
    use crate::discovery_services::ServiceRecord;

    #[cfg(all(windows, feature = "external-dns-sd"))]
    #[derive(Default)]
    struct RegistrationObservation {
        flags: u32,
        interface_index: u32,
        instance_name: Vec<u8>,
        service_type: Vec<u8>,
        domain_is_null: bool,
        host_is_null: bool,
        port_network_order: u16,
        txt_record: Vec<u8>,
        callback_supplied: bool,
        deallocations: usize,
    }

    #[cfg(all(windows, feature = "external-dns-sd"))]
    static REGISTRATION_OBSERVATION: std::sync::Mutex<Option<RegistrationObservation>> =
        std::sync::Mutex::new(None);

    #[cfg(all(windows, feature = "external-dns-sd"))]
    unsafe extern "system" fn capture_dns_service_registration(
        reference: *mut *mut std::ffi::c_void,
        flags: u32,
        interface_index: u32,
        instance_name: *const std::ffi::c_char,
        service_type: *const std::ffi::c_char,
        domain: *const std::ffi::c_char,
        host: *const std::ffi::c_char,
        port_network_order: u16,
        txt_len: u16,
        txt_record: *const std::ffi::c_void,
        callback: Option<super::DnsServiceRegisterReplyFn>,
        _context: *mut std::ffi::c_void,
    ) -> i32 {
        use std::ffi::CStr;

        // SAFETY: register_service passes valid NUL-terminated strings and a
        // live TXT buffer for the duration of this synchronous test callback.
        let observation = RegistrationObservation {
            flags,
            interface_index,
            instance_name: unsafe { CStr::from_ptr(instance_name) }.to_bytes().to_vec(),
            service_type: unsafe { CStr::from_ptr(service_type) }.to_bytes().to_vec(),
            domain_is_null: domain.is_null(),
            host_is_null: host.is_null(),
            port_network_order,
            // SAFETY: the test's DNS-SD record has a nonempty TXT byte vector
            // and txt_len is the exact length passed by register_service.
            txt_record: unsafe {
                std::slice::from_raw_parts(txt_record.cast::<u8>(), usize::from(txt_len)).to_vec()
            },
            callback_supplied: callback.is_some(),
            deallocations: 0,
        };
        *REGISTRATION_OBSERVATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observation);
        if !reference.is_null() {
            // SAFETY: this test API transfers the allocated sentinel to the
            // RegisteredService guard, which calls the matching deallocator.
            unsafe { *reference = Box::into_raw(Box::new(1_u8)).cast() };
        }
        if let Some(callback) = callback {
            // SAFETY: the source callback is a no-op and these pointers are not
            // dereferenced by it; this exercises the callback ABI path.
            unsafe {
                callback(
                    std::ptr::null_mut(),
                    0,
                    0,
                    instance_name,
                    service_type,
                    domain,
                    std::ptr::null_mut(),
                )
            }
        }
        0
    }

    #[cfg(all(windows, feature = "external-dns-sd"))]
    unsafe extern "system" fn deallocate_dns_service_reference(reference: *mut std::ffi::c_void) {
        if !reference.is_null() {
            // SAFETY: the paired test registration created this Box exactly once.
            unsafe { drop(Box::from_raw(reference.cast::<u8>())) };
        }
        if let Some(observation) = REGISTRATION_OBSERVATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            observation.deallocations += 1;
        }
    }

    #[cfg(all(windows, feature = "external-dns-sd"))]
    fn mock_dns_sd_api() -> super::DnsSdApi {
        super::DnsSdApi {
            register: capture_dns_service_registration,
            deallocate: deallocate_dns_service_reference,
            sock_fd: None,
            process_result: None,
            #[cfg(windows)]
            module: std::ptr::null_mut(),
        }
    }

    fn record(service_type: &[u8], name: &[u8]) -> ServiceRecord {
        ServiceRecord {
            service_type: service_type.to_vec(),
            name: name.to_vec(),
            port: 7000,
            txt: vec![3, b'k', b'=', b'v'],
            registered: true,
        }
    }

    #[test]
    fn p2p_registrations_use_any_interface_and_both_apple_flags() {
        assert_eq!(INCLUDE_P2P, 0x0002_0000);
        assert_eq!(INCLUDE_AWDL, 0x0010_0000);
        let request = registration_request(
            &record(b"_airplay._tcp.local", b"Living Room._airplay._tcp.local"),
            true,
        )
        .unwrap();
        assert_eq!(request.instance_name, b"Living Room");
        assert_eq!(request.service_type, b"_airplay._tcp");
        assert_eq!(request.interface_index, 0);
        assert_eq!(request.flags, 0x0012_0000);
        assert_eq!(request.port_network_order, 7000u16.to_be());
        assert_eq!(request.txt_record, [3, b'k', b'=', b'v']);
    }

    #[test]
    fn default_bonjour_registration_keeps_standard_flags() {
        let request = registration_request(
            &record(b"_airplay._tcp.local", b"Living Room._airplay._tcp.local"),
            false,
        )
        .unwrap();
        assert_eq!(request.interface_index, 0);
        assert_eq!(request.flags, 0);
    }

    #[test]
    fn raop_registration_preserves_the_mac_prefixed_service_instance() {
        let request = registration_request(
            &record(
                b"_raop._tcp.local",
                b"001122AABBCC@Living Room._raop._tcp.local",
            ),
            false,
        )
        .unwrap();
        assert_eq!(request.instance_name, b"001122AABBCC@Living Room");
        assert_eq!(request.service_type, b"_raop._tcp");
        assert_eq!(request.flags, 0);
    }

    #[test]
    fn dns_sd_socket_watch_processes_readability_and_stops_on_errors() {
        assert_eq!(
            socket_event_action(true, false),
            SocketEventAction::ProcessResult
        );
        assert_eq!(
            socket_event_action(false, false),
            SocketEventAction::Continue
        );
        assert_eq!(socket_event_action(true, true), SocketEventAction::Stop);
    }

    #[test]
    fn dns_sd_watch_creation_failure_leaves_registration_without_a_worker() {
        assert_eq!(watch_spawn_result(Ok(17)), Some(17));
        assert_eq!(
            watch_spawn_result::<i32>(Err(std::io::Error::other("injected thread failure"))),
            None
        );
    }

    #[test]
    fn dns_sd_socket_failure_message_matches_c_log_body_and_service_ids() {
        assert_eq!(
            service_socket_failure_message(17, "raop"),
            "dnssd: stopped servicing the DNS-SD socket (fd 17) for service 0"
        );
        assert_eq!(
            service_socket_failure_message(18, "airplay"),
            "dnssd: stopped servicing the DNS-SD socket (fd 18) for service 1"
        );
    }

    #[test]
    fn dns_sd_socket_failures_use_the_source_error_level_and_logger_filter() {
        use std::sync::{Arc, Mutex};

        let logger = crate::logger::Logger::new();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&messages);
        logger.set_callback(Some(Arc::new(move |level, message| {
            captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((level, message.to_vec()));
        })));

        logger.set_level(crate::logger::WARNING);
        super::log_service_socket_failure(&logger, 17, "raop");
        logger.set_level(crate::logger::CRIT);
        super::log_service_socket_failure(&logger, 18, "airplay");

        assert_eq!(
            *messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [(
                crate::logger::ERR,
                b"dnssd: stopped servicing the DNS-SD socket (fd 17) for service 0".to_vec()
            )]
        );
    }

    #[test]
    fn windows_loader_requires_the_same_seven_exports_as_the_c_backend() {
        assert_eq!(
            REQUIRED_WINDOWS_DNS_SD_EXPORTS,
            [
                b"DNSServiceRegister\0".as_slice(),
                b"DNSServiceRefDeallocate\0".as_slice(),
                b"TXTRecordCreate\0".as_slice(),
                b"TXTRecordSetValue\0".as_slice(),
                b"TXTRecordGetLength\0".as_slice(),
                b"TXTRecordGetBytesPtr\0".as_slice(),
                b"TXTRecordDeallocate\0".as_slice(),
            ]
        );
        assert!(!missing_windows_dns_sd_exports(|_| true));
        for missing in REQUIRED_WINDOWS_DNS_SD_EXPORTS {
            assert!(missing_windows_dns_sd_exports(
                |candidate| candidate != missing
            ));
        }
    }

    #[test]
    fn windows_loader_errors_preserve_the_c_dns_sd_error_codes() {
        assert_eq!(DnsSdInitError::LibraryNotFound.source_code(), 3);
        assert_eq!(DnsSdInitError::ProcedureNotFound.source_code(), 4);
    }

    #[cfg(all(windows, feature = "external-dns-sd"))]
    #[test]
    fn windows_initialization_failures_reach_the_publisher_preflight_codes() {
        use crate::discovery_publisher::ResponderFactory;

        for (error, expected_code) in [
            (DnsSdInitError::LibraryNotFound, 3),
            (DnsSdInitError::ProcedureNotFound, 4),
        ] {
            let factory = super::DnsSdFactory {
                peer_to_peer: false,
                logger: crate::logger::stdout_logger(crate::logger::INFO),
                api: Err(error),
            };
            assert_eq!(factory.initialization_error_code(), Some(expected_code));
            assert_eq!(error.as_io_error().raw_os_error(), Some(-expected_code));
        }
    }

    #[cfg(all(windows, feature = "external-dns-sd"))]
    #[test]
    fn dns_sd_registration_passes_source_arguments_and_releases_the_reference() {
        use std::sync::Arc;

        *REGISTRATION_OBSERVATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let registered = super::register_service(
            &record(
                b"_raop._tcp.local",
                b"001122AABBCC@Living Room._raop._tcp.local",
            ),
            false,
            Arc::new(mock_dns_sd_api()),
            crate::logger::stdout_logger(crate::logger::INFO),
        )
        .unwrap();
        assert!(registered.pump.is_none());

        {
            let observation = REGISTRATION_OBSERVATION
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let observation = observation.as_ref().unwrap();
            assert_eq!(observation.flags, 0);
            assert_eq!(observation.interface_index, 0);
            assert_eq!(observation.instance_name, b"001122AABBCC@Living Room");
            assert_eq!(observation.service_type, b"_raop._tcp");
            assert!(observation.domain_is_null);
            assert!(observation.host_is_null);
            assert_eq!(observation.port_network_order, 7000_u16.to_be());
            assert_eq!(observation.txt_record, [3, b'k', b'=', b'v']);
            assert!(observation.callback_supplied);
            assert_eq!(observation.deallocations, 0);
        }

        drop(registered);
        assert_eq!(
            REGISTRATION_OBSERVATION
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .unwrap()
                .deallocations,
            1
        );
    }
}
