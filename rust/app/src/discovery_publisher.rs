//! Executes DNS-SD state transitions against the live mDNS responder.
//!
//! `dnssd_mdnsd.c` keeps service metadata separate from the socket-owning
//! `mdnsd` instance. This module preserves that boundary and applies each
//! `PublisherAction` in order.

use crate::discovery_services::{
    DiscoveryServices, PublisherAction, RegistrationError, RegistrationTransition, ServiceRecord,
};
use crate::mdns_responder::{MdnsResponder, default_host_addresses};
use std::io;
use std::sync::Arc;

pub(super) trait ResponderBackend {
    fn start_services(&mut self, _services: DiscoveryServices) -> io::Result<()> {
        Ok(())
    }
    fn update_services(&mut self, services: DiscoveryServices) -> io::Result<()>;
    fn announce(&self, ttl: u32);
    fn goodbye(&self, service: &ServiceRecord);
    fn stop(&mut self);
}

pub(super) trait ResponderFactory: Send + Sync {
    fn initialization_error_code(&self) -> Option<i32> {
        None
    }

    fn start(&self, services: DiscoveryServices) -> io::Result<Box<dyn ResponderBackend>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiscoveryBackendSelection {
    ExternalDnsSd { peer_to_peer: bool },
    EmbeddedMdns,
}

fn select_discovery_backend(
    external_dns_sd_available: bool,
    peer_to_peer: bool,
    apple_p2p_available: bool,
) -> DiscoveryBackendSelection {
    if external_dns_sd_available {
        DiscoveryBackendSelection::ExternalDnsSd {
            peer_to_peer: peer_to_peer && apple_p2p_available,
        }
    } else {
        DiscoveryBackendSelection::EmbeddedMdns
    }
}

struct LiveResponderFactory;

impl ResponderFactory for LiveResponderFactory {
    fn start(&self, services: DiscoveryServices) -> io::Result<Box<dyn ResponderBackend>> {
        // The C responder refreshes its default interface selection on every
        // mdnsd_start attempt, including retries after a bind failure.
        let (addresses, ipv6_scope_id) = default_host_addresses();
        let responder = MdnsResponder::start(services, addresses, ipv6_scope_id)?;
        Ok(Box::new(responder))
    }
}

impl ResponderBackend for MdnsResponder {
    fn update_services(&mut self, services: DiscoveryServices) -> io::Result<()> {
        MdnsResponder::update_services(self, services);
        Ok(())
    }

    fn announce(&self, ttl: u32) {
        MdnsResponder::announce(self, ttl);
    }

    fn goodbye(&self, service: &ServiceRecord) {
        MdnsResponder::goodbye(self, service);
    }

    fn stop(&mut self) {
        MdnsResponder::stop(self);
    }
}

/// Owns service-registration state and starts/stops the live responder as
/// registration actions require.
pub struct DiscoveryPublisher {
    services: DiscoveryServices,
    factory: Arc<dyn ResponderFactory>,
    responder: Option<Box<dyn ResponderBackend>>,
}

impl DiscoveryPublisher {
    /// Create a publisher using the source-compatible default interface choice.
    pub fn new(services: DiscoveryServices) -> Self {
        Self::new_with_peer_to_peer(services, false)
    }

    /// Select external DNS-SD when the platform feature is enabled, matching
    /// `USE_DNS_SD`; macOS `apple-p2p` enables that feature by default for
    /// source parity. P2P/AWDL registration flags remain macOS-only.
    pub fn new_with_peer_to_peer(services: DiscoveryServices, peer_to_peer: bool) -> Self {
        Self::new_with_peer_to_peer_and_logger(
            services,
            peer_to_peer,
            crate::logger::stdout_logger(crate::logger::INFO),
        )
    }

    /// Use the process logger for external DNS-SD socket failure callbacks.
    pub fn new_with_peer_to_peer_and_logger(
        services: DiscoveryServices,
        peer_to_peer: bool,
        _logger: Arc<crate::logger::Logger>,
    ) -> Self {
        match select_discovery_backend(
            cfg!(all(any(unix, windows), feature = "external-dns-sd")),
            peer_to_peer,
            cfg!(all(target_os = "macos", feature = "apple-p2p")),
        ) {
            #[cfg(all(any(unix, windows), feature = "external-dns-sd"))]
            DiscoveryBackendSelection::ExternalDnsSd { peer_to_peer } => Self::with_factory(
                services,
                Arc::new(crate::bonjour_backend::DnsSdFactory::new(
                    peer_to_peer,
                    _logger,
                )),
            ),
            DiscoveryBackendSelection::EmbeddedMdns => {
                Self::with_factory(services, Arc::new(LiveResponderFactory))
            }
            #[cfg(not(all(any(unix, windows), feature = "external-dns-sd")))]
            DiscoveryBackendSelection::ExternalDnsSd { .. } => {
                unreachable!("external DNS-SD selection requires its build feature")
            }
        }
    }

    fn with_factory(services: DiscoveryServices, factory: Arc<dyn ResponderFactory>) -> Self {
        Self {
            services,
            factory,
            responder: None,
        }
    }

    pub fn services(&self) -> &DiscoveryServices {
        &self.services
    }

    pub fn is_running(&self) -> bool {
        self.responder.is_some()
    }

    /// Return the native DNS-SD load error, if this build selected a backend
    /// whose required library or exports are unavailable.
    pub fn initialization_error_code(&self) -> Option<i32> {
        self.factory.initialization_error_code()
    }

    /// Register RAOP, returning the same start code as `dnssd_register_raop`.
    pub fn register_raop(
        &mut self,
        port: u16,
        txt: &[u8],
    ) -> Result<RegistrationTransition, RegistrationError> {
        let mut transition = self.services.register_raop(port, txt, 0)?;
        transition.return_code = self.apply_registration(&mut transition.actions);
        Ok(transition)
    }

    /// Register AirPlay, returning the same start code as `dnssd_register_airplay`.
    pub fn register_airplay(
        &mut self,
        port: u16,
        txt: &[u8],
    ) -> Result<RegistrationTransition, RegistrationError> {
        let mut transition = self.services.register_airplay(port, txt, 0)?;
        transition.return_code = self.apply_registration(&mut transition.actions);
        Ok(transition)
    }

    pub fn unregister_raop(&mut self) {
        let actions = self.services.unregister_raop();
        self.apply_unregistration(&actions);
    }

    pub fn unregister_airplay(&mut self) {
        let actions = self.services.unregister_airplay();
        self.apply_unregistration(&actions);
    }

    /// Unregister RAOP and AirPlay in source order before stopping the
    /// responder. Dropping this publisher performs the same shutdown.
    pub fn stop(&mut self) {
        let raop_actions = self.services.unregister_raop();
        self.apply_unregistration_actions(&raop_actions, false);
        let airplay_actions = self.services.unregister_airplay();
        self.apply_unregistration_actions(&airplay_actions, false);
        if let Some(mut responder) = self.responder.take() {
            responder.stop();
        }
    }

    fn apply_registration(&mut self, actions: &mut Vec<PublisherAction>) -> i32 {
        let mut start_result = 0;
        for action in actions.iter() {
            match action {
                PublisherAction::UpdateServices { .. } => {
                    if let Some(responder) = &mut self.responder
                        && let Err(error) = responder.update_services(self.services.clone())
                    {
                        start_result = source_error_code(error);
                    }
                }
                PublisherAction::Start if start_result == 0 => start_result = self.start(),
                PublisherAction::Start => {}
                PublisherAction::Announce { ttl } if start_result == 0 => {
                    if let Some(responder) = &self.responder {
                        responder.announce(*ttl);
                    }
                }
                PublisherAction::Announce { .. }
                | PublisherAction::Goodbye(_)
                | PublisherAction::Stop => {}
            }
        }
        if start_result != 0 {
            actions.retain(|action| !matches!(action, PublisherAction::Announce { .. }));
        }
        start_result
    }

    fn apply_unregistration(&mut self, actions: &[PublisherAction]) {
        self.apply_unregistration_actions(actions, true);
    }

    fn apply_unregistration_actions(&mut self, actions: &[PublisherAction], stop_on_action: bool) {
        for action in actions {
            match action {
                PublisherAction::Goodbye(service) => {
                    if let Some(responder) = &self.responder {
                        responder.goodbye(service);
                    }
                }
                PublisherAction::UpdateServices { .. } => {
                    if let Some(responder) = &mut self.responder {
                        let _ = responder.update_services(self.services.clone());
                    }
                }
                PublisherAction::Stop if stop_on_action => self.stop(),
                PublisherAction::Stop => {}
                PublisherAction::Start | PublisherAction::Announce { .. } => {}
            }
        }
    }

    fn start(&mut self) -> i32 {
        if self.responder.is_some() {
            return 0;
        }
        match self.factory.start(self.services.clone()) {
            Ok(mut responder) => {
                let result = responder
                    .start_services(self.services.clone())
                    .map_or_else(source_error_code, |_| 0);
                self.responder = Some(responder);
                result
            }
            Err(error) => source_error_code(error),
        }
    }
}

fn source_error_code(error: io::Error) -> i32 {
    error.raw_os_error().map_or(-1, i32::saturating_neg)
}

impl Drop for DiscoveryPublisher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct TestState {
        events: Arc<Mutex<Vec<String>>>,
        starts: Arc<Mutex<VecDeque<i32>>>,
    }

    struct TestFactory(TestState);

    struct TestResponder(TestState);

    #[test]
    fn external_dns_sd_selection_keeps_apple_flags_mac_only() {
        assert_eq!(
            select_discovery_backend(true, false, true),
            DiscoveryBackendSelection::ExternalDnsSd {
                peer_to_peer: false
            }
        );
        assert_eq!(
            select_discovery_backend(true, true, true),
            DiscoveryBackendSelection::ExternalDnsSd { peer_to_peer: true }
        );
        assert_eq!(
            select_discovery_backend(false, false, false),
            DiscoveryBackendSelection::EmbeddedMdns
        );
        assert_eq!(
            select_discovery_backend(true, true, false),
            DiscoveryBackendSelection::ExternalDnsSd {
                peer_to_peer: false
            }
        );
    }

    #[test]
    fn platform_selector_uses_the_external_backend_when_feature_enabled() {
        let selection = select_discovery_backend(
            cfg!(all(any(unix, windows), feature = "external-dns-sd")),
            true,
            cfg!(all(target_os = "macos", feature = "apple-p2p")),
        );
        if cfg!(all(any(unix, windows), feature = "external-dns-sd")) {
            assert_eq!(
                selection,
                DiscoveryBackendSelection::ExternalDnsSd {
                    peer_to_peer: cfg!(all(target_os = "macos", feature = "apple-p2p")),
                }
            );
        } else {
            assert_eq!(selection, DiscoveryBackendSelection::EmbeddedMdns);
        }
    }

    impl ResponderFactory for TestFactory {
        fn start(&self, _services: DiscoveryServices) -> io::Result<Box<dyn ResponderBackend>> {
            let result = self
                .0
                .starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(0);
            self.0
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("start|{result}"));
            if result == 0 {
                Ok(Box::new(TestResponder(self.0.clone())))
            } else {
                Err(io::Error::from_raw_os_error(result.saturating_neg()))
            }
        }
    }

    impl ResponderBackend for TestResponder {
        fn update_services(&mut self, services: DiscoveryServices) -> io::Result<()> {
            self.0
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!(
                    "update|{}|{}",
                    services.airplay().registered,
                    services.raop().registered
                ));
            Ok(())
        }

        fn announce(&self, ttl: u32) {
            self.0
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("announce|{ttl}"));
        }

        fn goodbye(&self, service: &ServiceRecord) {
            self.0
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!(
                    "goodbye|{}",
                    String::from_utf8_lossy(&service.name)
                ));
        }

        fn stop(&mut self) {
            self.0
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("stop".to_owned());
        }
    }

    fn publisher(state: &TestState) -> DiscoveryPublisher {
        let names = crate::discovery_services::prepare_service_names(b"Room", &[0; 6], Ok(b"host"))
            .unwrap();
        DiscoveryPublisher::with_factory(
            DiscoveryServices::new(names),
            Arc::new(TestFactory(state.clone())),
        )
    }

    fn events(state: &TestState) -> Vec<String> {
        state
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[test]
    fn registration_starts_once_updates_services_and_announces_each_successful_service() {
        let state = TestState::default();
        let mut publisher = publisher(&state);
        let raop = publisher.register_raop(5000, b"r").unwrap();
        assert_eq!(raop.return_code, 0);
        let airplay = publisher.register_airplay(7000, b"a").unwrap();
        assert_eq!(airplay.return_code, 0);
        assert_eq!(
            events(&state),
            [
                "start|0",
                "announce|4500",
                "update|true|true",
                "announce|4500"
            ]
        );
        assert!(publisher.is_running());
    }

    #[test]
    fn failed_start_keeps_registration_state_and_does_not_announce() {
        let state = TestState::default();
        state
            .starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(-97);
        let mut publisher = publisher(&state);
        let transition = publisher.register_raop(5000, b"r").unwrap();
        assert_eq!(transition.return_code, -97);
        assert_eq!(transition.actions.len(), 2);
        assert!(publisher.services().raop().registered);
        assert!(!publisher.is_running());
        assert_eq!(events(&state), ["start|-97"]);
    }

    #[test]
    fn a_later_registration_retries_start_and_announces_both_registered_services() {
        let state = TestState::default();
        state
            .starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend([-97, 0]);
        let mut publisher = publisher(&state);

        assert_eq!(
            publisher.register_raop(5000, b"r").unwrap().return_code,
            -97
        );
        let transition = publisher.register_airplay(7000, b"a").unwrap();

        assert_eq!(transition.return_code, 0);
        assert!(publisher.services().raop().registered);
        assert!(publisher.services().airplay().registered);
        assert!(publisher.is_running());
        assert_eq!(events(&state), ["start|-97", "start|0", "announce|4500"]);
    }

    #[test]
    fn unregister_sends_goodbye_updates_then_stops_after_the_last_service() {
        let state = TestState::default();
        let mut publisher = publisher(&state);
        publisher.register_raop(5000, b"r").unwrap();
        publisher.register_airplay(7000, b"a").unwrap();
        state
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        publisher.unregister_raop();
        assert!(publisher.is_running());
        publisher.unregister_airplay();
        assert!(!publisher.is_running());
        assert_eq!(
            events(&state),
            [
                "goodbye|000000000000@Room._raop._tcp.local",
                "update|true|false",
                "goodbye|Room._airplay._tcp.local",
                "update|false|false",
                "stop"
            ]
        );
    }

    #[test]
    fn stop_unregisters_raop_then_airplay_before_stopping_and_is_idempotent() {
        let state = TestState::default();
        let mut publisher = publisher(&state);
        publisher.register_raop(5000, b"r").unwrap();
        publisher.register_airplay(7000, b"a").unwrap();
        state
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        publisher.stop();
        assert!(!publisher.is_running());
        assert!(!publisher.services().raop().registered);
        assert!(!publisher.services().airplay().registered);
        let expected = [
            "goodbye|000000000000@Room._raop._tcp.local",
            "update|true|false",
            "goodbye|Room._airplay._tcp.local",
            "update|false|false",
            "stop",
        ];
        assert_eq!(events(&state), expected);

        publisher.stop();
        assert_eq!(events(&state), expected);
    }
}
