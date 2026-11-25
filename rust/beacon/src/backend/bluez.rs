//! Linux BLE advertising through BlueZ's D-Bus API.

use crate::{BeaconBackend, BeaconOptions};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::io::Write;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::runtime::{Builder, Runtime};
use uxplay_core::beacon::manufacturer_data;
use zbus::wire::{Array, OwnedVariant, Type, Variant};
use zbus::{Connection, OwnedObjectPath, Proxy, interface};

const BLUEZ_SERVICE: &str = "org.bluez";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const ADVERTISING_MANAGER_INTERFACE: &str = "org.bluez.LEAdvertisingManager1";
const ADVERTISEMENT_PATH_BASE: &str = "/org/bluez/airplay_service_discovery_advertisement";
const APPLE_COMPANY_ID: u16 = 0x004c;
const SERVICE_UNAVAILABLE: &str =
    "Error: Bluetooth D-Bus service not running on host.\nStopping ...";
const ADAPTER_NOT_FOUND: &str = "Error: Bluetooth adapter not found\nStopping ...";
const REGISTRATION_FAILED: &str = "Failed to start beacon:\ngiving up, check Bluetooth adapter";
#[cfg(test)]
const SOURCE_CALLBACK_FIXTURE: &str =
    include_str!("../../tests/fixtures/source-bluez-callbacks.txt");

#[cfg(test)]
fn source_callback_line(prefix: &str) -> &'static str {
    SOURCE_CALLBACK_FIXTURE
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("source BlueZ fixture is missing {prefix:?}"))
}

#[derive(serde::Deserialize, Type)]
#[serde(transparent)]
#[zbus(signature = "a{oa{sa{sv}}}")]
struct ManagedObjects(ManagedObjectMap);

type ManagedObjectMap = IndexMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>>;

#[derive(Debug, Default, Eq, PartialEq)]
struct AdvertisementState {
    status: RegistrationStatus,
    manager_available: bool,
    advertised_port: Option<u16>,
    advertised_address: Option<Ipv4Addr>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RegistrationStatus {
    #[default]
    Idle,
    Preflight,
    Registering,
    Registered,
    Failed,
}

impl AdvertisementState {
    fn begin(&mut self, address: Ipv4Addr, port: u16) {
        self.status = RegistrationStatus::Registering;
        self.manager_available = true;
        self.advertised_port = Some(port);
        self.advertised_address = Some(address);
    }

    fn preflight(&mut self) {
        self.status = RegistrationStatus::Preflight;
        self.manager_available = false;
        self.advertised_port = None;
        self.advertised_address = None;
    }

    fn registration_succeeded(&mut self) -> String {
        if self.status == RegistrationStatus::Registering {
            self.status = RegistrationStatus::Registered;
        }
        let address = self
            .advertised_address
            .map_or_else(|| "None".to_owned(), |address| address.to_string());
        let port = self
            .advertised_port
            .map_or_else(|| "None".to_owned(), |port| port.to_string());
        format!("AirPlay Service_Discovery Advertisement ({address}:{port}) registered")
    }

    fn registration_failed(&mut self) {
        if self.status == RegistrationStatus::Registering {
            self.status = RegistrationStatus::Failed;
        }
        self.manager_available = false;
        self.advertised_port = None;
        self.advertised_address = None;
    }

    fn stopped(&mut self) {
        self.status = RegistrationStatus::Idle;
        self.manager_available = false;
        // Python assigns `advertised_Port` (capital P), leaving the source
        // global `advertised_port` stale after an otherwise successful stop.
        self.advertised_address = None;
    }
}

/// A D-Bus service interface exported back to BlueZ for each advertisement.
struct AdvertisementObject {
    path: String,
    manufacturer_data: Vec<u8>,
    min_interval: u32,
    max_interval: u32,
}

impl AdvertisementObject {
    fn new(
        path: String,
        ipv4: Ipv4Addr,
        port: u16,
        advmin: Option<i64>,
        advmax: Option<i64>,
    ) -> Result<Self, String> {
        let manufacturer_data = manufacturer_data(ipv4, port).to_vec();
        Ok(Self {
            path,
            manufacturer_data,
            min_interval: interval(advmin, "advmin")?,
            max_interval: interval(advmax, "advmax")?,
        })
    }

    fn advertisement_type(&self) -> String {
        "broadcast".to_owned()
    }

    fn manufacturer_data_property(&self) -> zbus::fdo::Result<HashMap<u16, OwnedVariant>> {
        let bytes = Array::from(self.manufacturer_data.clone());
        let value = OwnedVariant::try_from(Variant::Array(bytes))
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        Ok(HashMap::from([(APPLE_COMPANY_ID, value)]))
    }

    fn min_interval_property(&self) -> u32 {
        self.min_interval
    }

    fn max_interval_property(&self) -> u32 {
        self.max_interval
    }
}

#[interface(name = "org.bluez.LEAdvertisement1")]
impl AdvertisementObject {
    fn release(&self) {
        release_advertisement(&self.path);
    }

    #[zbus(property, name = "Type")]
    fn ad_type(&self) -> String {
        self.advertisement_type()
    }

    #[zbus(property, name = "ManufacturerData")]
    fn manufacturer_data(&self) -> zbus::fdo::Result<HashMap<u16, OwnedVariant>> {
        self.manufacturer_data_property()
    }

    #[zbus(property, name = "MinInterval")]
    fn ad_min_interval(&self) -> u32 {
        self.min_interval_property()
    }

    #[zbus(property, name = "MaxInterval")]
    fn ad_max_interval(&self) -> u32 {
        self.max_interval_property()
    }
}

/// Owns the D-Bus connection and its exported advertisement interface.
pub struct BluezBackend {
    runtime: Runtime,
    connection: Connection,
    adapter_path: Option<OwnedObjectPath>,
    advertisement_path: Option<OwnedObjectPath>,
    preflight_start: bool,
    state: Arc<Mutex<AdvertisementState>>,
}

impl BluezBackend {
    /// Connects to BlueZ. Adapter selection and power-on happen for each setup/start.
    pub fn find_device() -> Result<Self, String> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot create BlueZ runtime: {error}"))?;
        let connection = runtime
            .block_on(Connection::system())
            .map_err(|_| SERVICE_UNAVAILABLE.to_owned())?;
        Ok(Self {
            runtime,
            connection,
            adapter_path: None,
            advertisement_path: None,
            preflight_start: false,
            state: Arc::new(Mutex::new(AdvertisementState::default())),
        })
    }
}

async fn find_adapter(connection: &Connection) -> Result<OwnedObjectPath, String> {
    let proxy = Proxy::new(connection, BLUEZ_SERVICE, "/", OBJECT_MANAGER_INTERFACE)
        .await
        .map_err(|error| {
            if is_service_unknown(&error) {
                SERVICE_UNAVAILABLE.to_owned()
            } else {
                format!("Error: Bluetooth adapter not found: {error}\nStopping ...")
            }
        })?;
    let objects: ManagedObjects = proxy
        .call("GetManagedObjects", &())
        .await
        .map_err(|error| {
            if is_service_unknown(&error) {
                SERVICE_UNAVAILABLE.to_owned()
            } else {
                format!("Error: cannot discover Bluetooth adapters: {error}\nStopping ...")
            }
        })?;

    first_advertising_adapter(&objects.0).ok_or_else(|| ADAPTER_NOT_FOUND.to_owned())
}

fn first_advertising_adapter(objects: &ManagedObjectMap) -> Option<OwnedObjectPath> {
    objects
        .iter()
        .find(|(_, interfaces)| interfaces.contains_key(ADVERTISING_MANAGER_INTERFACE))
        .map(|(path, _)| path.clone())
}

async fn set_adapter_powered(
    connection: &Connection,
    adapter_path: &OwnedObjectPath,
) -> zbus::Result<()> {
    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        adapter_path.as_str(),
        ADAPTER_INTERFACE,
    )
    .await?;
    proxy.set_property("Powered", true).await?;
    Ok(())
}

fn discover_and_power_adapter(
    find: impl FnOnce() -> Result<OwnedObjectPath, String>,
    power: impl FnOnce(&OwnedObjectPath) -> Result<(), String>,
) -> Result<OwnedObjectPath, String> {
    let adapter_path = find()?;
    power(&adapter_path)?;
    Ok(adapter_path)
}

async fn register_advertisement(
    connection: &Connection,
    adapter_path: &OwnedObjectPath,
    advertisement_path: &OwnedObjectPath,
) -> zbus::Result<()> {
    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        adapter_path.as_str(),
        ADVERTISING_MANAGER_INTERFACE,
    )
    .await?;
    let options: HashMap<String, OwnedVariant> = HashMap::new();
    let _: () = proxy
        .call("RegisterAdvertisement", &(advertisement_path, options))
        .await?;
    Ok(())
}

async fn unregister_advertisement(
    connection: &Connection,
    adapter_path: &OwnedObjectPath,
    advertisement_path: &OwnedObjectPath,
) -> zbus::Result<()> {
    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        adapter_path.as_str(),
        ADVERTISING_MANAGER_INTERFACE,
    )
    .await?;
    let _: () = proxy
        .call("UnregisterAdvertisement", &(advertisement_path,))
        .await?;
    Ok(())
}

fn is_service_unknown(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::FDO(error)
            if matches!(error.as_ref(), zbus::fdo::Error::ServiceUnknown(_))
    )
}

/// Builds the exact source advertisement path. `index` is a suffix; it does not select hciN.
fn advertisement_path(index: Option<i64>) -> String {
    format!("{ADVERTISEMENT_PATH_BASE}{}", index.unwrap_or(0))
}

fn interval(value: Option<i64>, name: &str) -> Result<u32, String> {
    let value = value.ok_or_else(|| format!("{name} was not set"))?.max(100);
    u32::try_from(value).map_err(|_| format!("{name} is outside the BlueZ uint32 range"))
}

fn lock_state(state: &Mutex<AdvertisementState>) -> MutexGuard<'_, AdvertisementState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn release_lines(path: &str) -> [String; 2] {
    [
        format!("{path}: D-Bus Released! (Bluetooth USB adapter removed?)"),
        "Stopping ...".to_owned(),
    ]
}

fn release_advertisement(path: &str) -> ! {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    for line in release_lines(path) {
        let _ = writeln!(output, "{line}");
    }
    let _ = output.flush();
    std::process::exit(1);
}

impl BeaconBackend for BluezBackend {
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String> {
        if self.advertisement_path.is_some()
            || lock_state(&self.state).status != RegistrationStatus::Idle
        {
            return Err("code error, should not happen".to_owned());
        }
        if port == 0 {
            return Err("AirPlay port must be greater than zero".to_owned());
        }
        let runtime = &self.runtime;
        let connection = &self.connection;
        let adapter_path = discover_and_power_adapter(
            || runtime.block_on(find_adapter(connection)),
            |path| {
                runtime
                    .block_on(set_adapter_powered(connection, path))
                    .map_err(|error| format!("Error: cannot power Bluetooth adapter: {error}"))
            },
        )?;
        self.adapter_path = Some(adapter_path.clone());

        let path = advertisement_path(options.index);
        let object_path = OwnedObjectPath::try_from(path.as_str())
            .map_err(|error| format!("{REGISTRATION_FAILED}: {error}"))?;
        let advertisement =
            AdvertisementObject::new(path, options.ipv4, port, options.advmin, options.advmax)?;
        let registered = self
            .runtime
            .block_on(
                self.connection
                    .object_server()
                    .at(object_path.as_str(), advertisement),
            )
            .map_err(|error| format!("{REGISTRATION_FAILED}: {error}"))?;
        if registered.is_some() {
            return Err(REGISTRATION_FAILED.to_owned());
        }

        self.advertisement_path = Some(object_path.clone());
        if port == 1 {
            // Port 1 is a preflight sentinel only in the script's explicit probe. A real
            // handoff on port 1 makes Python's `beacon_on()` return None and abort startup.
            lock_state(&self.state).preflight();
            return crate::bluez_port_one_start_result(self.preflight_start);
        }

        lock_state(&self.state).begin(options.ipv4, port);

        let connection = self.connection.clone();
        let adapter_path = adapter_path.clone();
        let state = Arc::clone(&self.state);
        self.runtime.spawn(async move {
            match register_advertisement(&connection, &adapter_path, &object_path).await {
                Ok(()) => {
                    let message = lock_state(&state).registration_succeeded();
                    crate::source_println!("{message}");
                }
                Err(_) => lock_state(&state).registration_failed(),
            }
        });

        Ok(port)
    }

    fn stop(&mut self) -> Result<(), String> {
        let Some(advertisement_path) = self.advertisement_path.as_ref() else {
            return Ok(());
        };

        if lock_state(&self.state).manager_available {
            let adapter_path = self
                .adapter_path
                .as_ref()
                .ok_or_else(|| "BlueZ adapter was not selected".to_owned())?;
            self.runtime
                .block_on(unregister_advertisement(
                    &self.connection,
                    adapter_path,
                    advertisement_path,
                ))
                .map_err(|error| format!("Failed to unregister BlueZ advertisement: {error}"))?;
            crate::source_println!("AirPlay Service-Discovery beacon advertisement unregistered");
        }

        self.runtime
            .block_on(
                self.connection
                    .object_server()
                    .remove::<AdvertisementObject, _>(advertisement_path.as_str()),
            )
            .map_err(|error| format!("Failed to remove BlueZ advertisement object: {error}"))?;
        self.advertisement_path = None;
        lock_state(&self.state).stopped();
        Ok(())
    }

    fn preflight(&mut self, options: &BeaconOptions) -> Result<bool, String> {
        self.preflight_start = true;
        let start_result = self.start(options, 1);
        self.preflight_start = false;
        let port = start_result?;
        self.stop()?;
        Ok(port == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[test]
    fn advertisement_path_uses_the_source_index_suffix() {
        assert_eq!(
            advertisement_path(Some(3)),
            "/org/bluez/airplay_service_discovery_advertisement3"
        );
        assert_eq!(
            advertisement_path(None),
            "/org/bluez/airplay_service_discovery_advertisement0"
        );
    }

    #[test]
    fn property_map_matches_source_names_values_and_wire_signatures() {
        let object = AdvertisementObject::new(
            advertisement_path(Some(2)),
            Ipv4Addr::new(192, 168, 1, 10),
            7100,
            Some(250),
            Some(400),
        )
        .unwrap();

        assert_eq!(object.advertisement_type(), "broadcast");
        assert_eq!(object.min_interval_property(), 250);
        assert_eq!(object.max_interval_property(), 400);
        assert_eq!(
            <String as Type>::SIGNATURE.to_string(),
            "s",
            "Type property must serialize as a D-Bus string"
        );
        assert_eq!(
            <HashMap<u16, OwnedVariant> as Type>::SIGNATURE.to_string(),
            "a{qv}",
            "ManufacturerData must be a{{qv}}"
        );
        assert_eq!(<u32 as Type>::SIGNATURE.to_string(), "u");

        let manufacturer = object.manufacturer_data_property().unwrap();
        assert_eq!(manufacturer.len(), 1);
        let value = &manufacturer[&APPLE_COMPANY_ID];
        assert_eq!(value.value_signature().to_string(), "v");
        let Variant::Array(bytes) = &**value else {
            panic!("ManufacturerData variant must contain a byte array")
        };
        let bytes = bytes
            .iter()
            .map(|byte| match byte {
                Variant::U8(byte) => *byte,
                _ => panic!("manufacturer data array contains a non-byte value"),
            })
            .collect::<Vec<_>>();
        assert_eq!(bytes, [0x09, 0x08, 0x13, 0x30, 192, 168, 1, 10, 0x1b, 0xbc]);
    }

    #[test]
    fn intervals_clamp_independently_and_reject_missing_or_unrepresentable_values() {
        let object = AdvertisementObject::new(
            advertisement_path(Some(0)),
            Ipv4Addr::LOCALHOST,
            7000,
            Some(20),
            Some(90),
        )
        .unwrap();
        assert_eq!(object.min_interval_property(), 100);
        assert_eq!(object.max_interval_property(), 100);
        assert_eq!(interval(None, "advmin").unwrap_err(), "advmin was not set");
        assert_eq!(
            interval(Some(i64::MAX), "advmax").unwrap_err(),
            "advmax is outside the BlueZ uint32 range"
        );
    }

    #[test]
    fn first_adapter_uses_object_manager_iteration_order() {
        let adapter_one = OwnedObjectPath::try_from("/org/bluez/hci1").unwrap();
        let adapter_zero = OwnedObjectPath::try_from("/org/bluez/hci0").unwrap();
        let mut first_interfaces = HashMap::new();
        first_interfaces.insert(ADVERTISING_MANAGER_INTERFACE.to_owned(), HashMap::new());
        let mut objects = ManagedObjectMap::new();
        objects.insert(adapter_one.clone(), first_interfaces);
        objects.insert(adapter_zero, HashMap::new());

        assert_eq!(first_advertising_adapter(&objects), Some(adapter_one));
    }

    #[test]
    fn each_start_discovers_the_adapter_before_powering_it_on() {
        let expected = OwnedObjectPath::try_from("/org/bluez/hci0").unwrap();
        let operations = RefCell::new(Vec::new());
        let selected = discover_and_power_adapter(
            || {
                operations.borrow_mut().push("discover");
                Ok(expected.clone())
            },
            |path| {
                assert_eq!(path, &expected);
                operations.borrow_mut().push("power");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(selected, expected);
        assert_eq!(*operations.borrow(), ["discover", "power"]);
    }

    #[test]
    fn registration_error_clears_advertised_state_without_exiting() {
        let mut state = AdvertisementState::default();
        let address = Ipv4Addr::new(192, 168, 1, 10);
        state.begin(address, 7100);
        assert_eq!(state.status, RegistrationStatus::Registering);

        state.registration_failed();

        assert_eq!(state.status, RegistrationStatus::Failed);
        assert!(!state.manager_available);
        assert_eq!(state.advertised_port, None);
        assert_eq!(state.advertised_address, None);
    }

    #[test]
    fn registration_success_and_stop_follow_source_state_transitions() {
        let mut state = AdvertisementState::default();
        let address = Ipv4Addr::LOCALHOST;
        state.begin(address, 7000);
        assert_eq!(
            state.registration_succeeded(),
            format!("AirPlay Service_Discovery Advertisement ({address}:7000) registered")
        );
        assert_eq!(state.status, RegistrationStatus::Registered);
        assert!(state.manager_available);

        state.stopped();

        assert_eq!(state.status, RegistrationStatus::Idle);
        assert!(!state.manager_available);
        assert_eq!(state.advertised_port, Some(7000));
        assert_eq!(state.advertised_address, None);
    }

    #[test]
    fn late_registration_error_after_stop_leaves_backend_ready_for_next_start() {
        let mut state = AdvertisementState::default();
        state.begin(Ipv4Addr::LOCALHOST, 7000);
        state.stopped();
        assert_eq!(state.advertised_port, Some(7000));

        // Python's error callback always clears its globals, even when the
        // Unregister call already stopped this advertisement.
        state.registration_failed();

        assert_eq!(state, AdvertisementState::default());
        assert_eq!(
            source_callback_line("state after late error:"),
            "state after late error: port=None, address=None"
        );
        state.begin(Ipv4Addr::new(192, 168, 1, 10), 7100);
        assert_eq!(state.status, RegistrationStatus::Registering);
        assert_eq!(state.advertised_port, Some(7100));
    }

    #[test]
    fn late_registration_success_logs_python_globals_after_stop() {
        let mut state = AdvertisementState::default();
        state.begin(Ipv4Addr::new(192, 0, 2, 8), 7100);
        state.stopped();

        assert_eq!(
            state.registration_succeeded(),
            source_callback_line("AirPlay Service_Discovery Advertisement")
        );
        assert_eq!(state.status, RegistrationStatus::Idle);
        assert_eq!(state.advertised_port, Some(7100));
        assert_eq!(state.advertised_address, None);
        assert!(
            SOURCE_CALLBACK_FIXTURE.contains(source_callback_line("state after late success:"))
        );
    }

    #[test]
    fn release_callback_logs_source_messages_before_exit() {
        let lines = SOURCE_CALLBACK_FIXTURE
            .lines()
            .skip_while(|line| !line.starts_with("/org/bluez/"))
            .take(3)
            .collect::<Vec<_>>();
        assert_eq!(
            release_lines("/org/bluez/airplay_service_discovery_advertisement2"),
            [lines[0].to_owned(), lines[1].to_owned()]
        );
        assert_eq!(lines[2], "Release exit: 1");
    }
}
