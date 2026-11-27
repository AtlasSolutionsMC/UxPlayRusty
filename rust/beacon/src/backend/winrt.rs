//! Windows Bluetooth LE advertising through the WinRT publisher API.

use crate::{BeaconBackend, BeaconOptions};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisement, BluetoothLEAdvertisementPublisher,
    BluetoothLEAdvertisementPublisherStatus,
    BluetoothLEAdvertisementPublisherStatusChangedEventArgs, BluetoothLEManufacturerData,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataWriter;

const APPLE_COMPANY_ID: u16 = 0x004c;
const PUBLISHER_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Builds the manufacturer-specific payload used by UxPlay's WinRT module.
pub fn manufacturer_payload(address: Ipv4Addr, port: u16) -> Result<Vec<u8>, String> {
    if port == 0 {
        return Err("AirPlay port must be greater than zero".to_owned());
    }
    let mut payload = Vec::with_capacity(10);
    payload.extend_from_slice(&[0x09, 0x08, 0x13, 0x30]);
    payload.extend_from_slice(&address.octets());
    payload.extend_from_slice(&port.to_be_bytes());
    Ok(payload)
}

pub struct WinRtBackend {
    publisher: Option<PublisherLease<BluetoothLEAdvertisementPublisher>>,
    advertised_port: Option<u16>,
    advertised_address: Option<Ipv4Addr>,
    current_publisher_stopped: Arc<AtomicBool>,
    quiet: Arc<AtomicBool>,
}

struct PublisherLease<T> {
    publisher: T,
    stopped: Arc<AtomicBool>,
}

impl<T> PublisherLease<T> {
    fn new(publisher: T, stopped: Arc<AtomicBool>) -> Self {
        Self { publisher, stopped }
    }
}

impl WinRtBackend {
    pub fn new() -> Self {
        Self {
            publisher: None,
            advertised_port: None,
            advertised_address: None,
            current_publisher_stopped: Arc::new(AtomicBool::new(false)),
            quiet: Arc::new(AtomicBool::new(false)),
        }
    }

    fn create_publisher(
        payload: &[u8],
        quiet: Arc<AtomicBool>,
        current_publisher_stopped: Arc<AtomicBool>,
    ) -> windows::core::Result<PublisherLease<BluetoothLEAdvertisementPublisher>> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(payload)?;

        let manufacturer_data = BluetoothLEManufacturerData::new()?;
        manufacturer_data.SetCompanyId(APPLE_COMPANY_ID)?;
        manufacturer_data.SetData(&writer.DetachBuffer()?)?;

        let advertisement = BluetoothLEAdvertisement::new()?;
        advertisement
            .ManufacturerData()?
            .Append(&manufacturer_data)?;

        let publisher = BluetoothLEAdvertisementPublisher::Create(&advertisement)?;
        let callback_stopped = Arc::clone(&current_publisher_stopped);
        let status_changed = TypedEventHandler::<
            BluetoothLEAdvertisementPublisher,
            BluetoothLEAdvertisementPublisherStatusChangedEventArgs,
        >::new(move |sender, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let status = args.Status()?;
            if !quiet.load(Ordering::Relaxed) {
                crate::source_println!("Publisher status change to: {}", status_name(status));
            }
            if status == BluetoothLEAdvertisementPublisherStatus::Aborted {
                crate::source_println!(
                    "Publisher was aborted after starting: perhaps no Bluetooth interface is available?"
                );
                crate::source_println!("Stopping");
                std::process::exit(1);
            }
            if status == BluetoothLEAdvertisementPublisherStatus::Stopped {
                // Python's callback clears the module-level publisher on STOPPED,
                // even when this event came from an older publisher instance.
                publisher_status_stopped(&callback_stopped);
            }
            let _ = sender;
            Ok(())
        });
        let _status_token = publisher.StatusChanged(&status_changed)?;
        Ok(PublisherLease::new(publisher, current_publisher_stopped))
    }
}

impl Default for WinRtBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl BeaconBackend for WinRtBackend {
    fn start(&mut self, options: &BeaconOptions, port: u16) -> Result<u16, String> {
        discard_stopped_publisher(&mut self.publisher);
        if self.publisher.is_some() {
            return Err("code error, should not happen".to_owned());
        }
        if options.advmin.is_some() || options.advmax.is_some() || options.index.is_some() {
            return Err(
                "uxplay_beacon_module_winrt: advmin, advmax, index were not all None".to_owned(),
            );
        }
        let payload = manufacturer_payload(options.ipv4, port)?;

        let quiet = port == 1;
        self.quiet.store(quiet, Ordering::Relaxed);
        self.current_publisher_stopped
            .store(false, Ordering::Relaxed);
        if quiet {
            crate::source_println!("beacon test");
        }
        let publisher = Self::create_publisher(
            &payload,
            Arc::clone(&self.quiet),
            Arc::clone(&self.current_publisher_stopped),
        )
        .map_err(|error| error.to_string())?;
        self.publisher = Some(publisher);
        self.advertised_port = Some(port);
        self.advertised_address = Some(options.ipv4);

        if let Err(error) = self
            .publisher
            .as_ref()
            .expect("publisher was stored before Start")
            .publisher
            .Start()
        {
            let status = self
                .publisher
                .as_ref()
                .expect("publisher remains stored after a failed Start")
                .publisher
                .Status()
                .map(status_name)
                .unwrap_or("UNKNOWN");
            crate::source_println!("Failed to start Publisher: {error}");
            crate::source_println!("Publisher Status: {status}");
            self.advertised_address = None;
            self.advertised_port = None;
            return Err("Failed to start beacon:\ngiving up, check Bluetooth adapter".to_owned());
        }

        if !quiet {
            crate::source_println!(
                "AirPlay Service_Discovery Advertisement ({}:{port}) registered",
                options.ipv4
            );
        }
        Ok(port)
    }

    fn stop(&mut self) -> Result<(), String> {
        let publisher_was_released = discard_stopped_publisher(&mut self.publisher);
        if let Some(error) = stopped_publisher_stop_error(publisher_was_released) {
            return Err(error.to_owned());
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| "WinRT publisher is not active".to_owned())?;
        let stop_result = publisher
            .publisher
            .Stop()
            .map_err(|error| error.to_string());
        if let Err(error) = stop_result {
            return finish_stop(
                &mut self.publisher,
                &mut self.advertised_port,
                &mut self.advertised_address,
                Err(error),
            );
        }

        let publisher = self.publisher.as_ref().expect("publisher remains active");
        let stopped = wait_for_stopped_status(
            || {
                publisher.stopped.load(Ordering::Acquire)
                    || publisher.publisher.Status().is_ok_and(|status| {
                        matches!(
                            status,
                            BluetoothLEAdvertisementPublisherStatus::Stopped
                                | BluetoothLEAdvertisementPublisherStatus::Aborted
                        )
                    })
            },
            PUBLISHER_STOP_TIMEOUT,
        );
        let status = publisher
            .publisher
            .Status()
            .map(status_name)
            .unwrap_or("UNKNOWN");
        crate::source_println!("Publisher stop completed with status: {status}");
        if !stopped {
            return Err(format!(
                "WinRT publisher did not reach STOPPED within {} seconds (current status: {status})",
                PUBLISHER_STOP_TIMEOUT.as_secs()
            ));
        }
        publisher.stopped.store(true, Ordering::Release);

        finish_stop(
            &mut self.publisher,
            &mut self.advertised_port,
            &mut self.advertised_address,
            Ok(()),
        )
    }

    fn preflight(&mut self, _options: &BeaconOptions) -> Result<bool, String> {
        // A fake port-1 advertisement immediately followed by Stop can leave the
        // WinRT publisher in STARTED while its asynchronous transition completes.
        // Defer adapter validation to the real handoff advertisement instead.
        Ok(false)
    }
}

fn wait_for_stopped_status(mut is_stopped: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if is_stopped() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

fn discard_stopped_publisher<T>(publisher: &mut Option<PublisherLease<T>>) -> bool {
    if publisher
        .as_ref()
        .is_some_and(|active| active.stopped.load(Ordering::Relaxed))
    {
        publisher.take();
        true
    } else {
        false
    }
}

fn stopped_publisher_stop_error(released_by_status_callback: bool) -> Option<&'static str> {
    released_by_status_callback.then_some(crate::WINRT_STOPPED_PUBLISHER_ERROR)
}

fn publisher_status_stopped(current_publisher_stopped: &AtomicBool) {
    current_publisher_stopped.store(true, Ordering::Relaxed);
}

fn finish_stop<T>(
    publisher: &mut Option<PublisherLease<T>>,
    advertised_port: &mut Option<u16>,
    advertised_address: &mut Option<Ipv4Addr>,
    stop_result: Result<(), String>,
) -> Result<(), String> {
    match stop_result {
        Ok(()) => {
            publisher.take();
            *advertised_port = None;
            *advertised_address = None;
            Ok(())
        }
        Err(error) => {
            // A STOPPED status event clears Python's global publisher even when a
            // caller's stop operation reports failure; retain it otherwise.
            discard_stopped_publisher(publisher);
            Err(error)
        }
    }
}

fn status_name(status: BluetoothLEAdvertisementPublisherStatus) -> &'static str {
    match status {
        BluetoothLEAdvertisementPublisherStatus::Aborted => "ABORTED",
        BluetoothLEAdvertisementPublisherStatus::Created => "CREATED",
        BluetoothLEAdvertisementPublisherStatus::Started => "STARTED",
        BluetoothLEAdvertisementPublisherStatus::Stopping => "STOPPING",
        BluetoothLEAdvertisementPublisherStatus::Stopped => "STOPPED",
        BluetoothLEAdvertisementPublisherStatus::Waiting => "WAITING",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manufacturer_payload_matches_source_bytes_and_port_order() {
        assert_eq!(
            manufacturer_payload(Ipv4Addr::new(192, 168, 1, 10), 7100).unwrap(),
            [0x09, 0x08, 0x13, 0x30, 192, 168, 1, 10, 0x1b, 0xbc]
        );
    }

    #[test]
    fn zero_port_is_rejected() {
        assert_eq!(
            manufacturer_payload(Ipv4Addr::LOCALHOST, 0).unwrap_err(),
            "AirPlay port must be greater than zero"
        );
    }

    #[test]
    fn stopped_status_wait_returns_when_publisher_is_stopped() {
        assert!(wait_for_stopped_status(|| true, PUBLISHER_STOP_TIMEOUT));
    }

    #[test]
    fn stopped_status_wait_is_bounded() {
        assert!(!wait_for_stopped_status(|| false, Duration::ZERO));
    }

    #[test]
    fn stopped_status_releases_the_publisher_for_a_later_start() {
        let stopped = Arc::new(AtomicBool::new(false));
        let mut publisher = Some(PublisherLease::new("first publisher", Arc::clone(&stopped)));

        assert!(!discard_stopped_publisher(&mut publisher));
        assert!(publisher.is_some());

        stopped.store(true, Ordering::Relaxed);
        assert!(discard_stopped_publisher(&mut publisher));
        assert!(publisher.is_none());

        publisher = Some(PublisherLease::new(
            "replacement publisher",
            Arc::new(AtomicBool::new(false)),
        ));
        assert!(!discard_stopped_publisher(&mut publisher));
        assert_eq!(
            publisher.as_ref().map(|active| active.publisher),
            Some("replacement publisher")
        );
    }

    #[test]
    fn old_stopped_callback_clears_the_current_global_publisher() {
        let current_stopped = Arc::new(AtomicBool::new(false));
        let old_publisher_callback = Arc::clone(&current_stopped);
        let mut current_publisher = Some(PublisherLease::new(
            "new publisher",
            Arc::clone(&current_stopped),
        ));

        // Python's on_status_changed ignores sender identity and sets the
        // module-global publisher to None for every STOPPED event.
        publisher_status_stopped(&old_publisher_callback);

        let released = discard_stopped_publisher(&mut current_publisher);
        assert!(released);
        assert_eq!(
            stopped_publisher_stop_error(released),
            Some(crate::WINRT_STOPPED_PUBLISHER_ERROR)
        );
        assert!(current_publisher.is_none());
    }

    #[test]
    fn stop_error_retains_publisher_and_advertisement_state() {
        let mut publisher = Some(PublisherLease::new(
            "active publisher",
            Arc::new(AtomicBool::new(false)),
        ));
        let mut advertised_port = Some(7100);
        let mut advertised_address = Some(Ipv4Addr::new(192, 168, 1, 10));

        let result = finish_stop(
            &mut publisher,
            &mut advertised_port,
            &mut advertised_address,
            Err("Stop failed".to_owned()),
        );

        assert_eq!(result, Err("Stop failed".to_owned()));
        assert_eq!(
            publisher.as_ref().map(|active| active.publisher),
            Some("active publisher")
        );
        assert_eq!(advertised_port, Some(7100));
        assert_eq!(advertised_address, Some(Ipv4Addr::new(192, 168, 1, 10)));

        assert_eq!(
            finish_stop(
                &mut publisher,
                &mut advertised_port,
                &mut advertised_address,
                Ok(()),
            ),
            Ok(())
        );
        assert!(publisher.is_none());
        assert_eq!(advertised_port, None);
        assert_eq!(advertised_address, None);
    }

    #[test]
    fn stopped_callback_releases_publisher_without_clearing_failed_stop_fields() {
        let stopped = Arc::new(AtomicBool::new(true));
        let mut publisher = Some(PublisherLease::new("publisher", stopped));
        let mut advertised_port = Some(7100);
        let mut advertised_address = Some(Ipv4Addr::new(192, 168, 1, 10));

        assert_eq!(
            finish_stop(
                &mut publisher,
                &mut advertised_port,
                &mut advertised_address,
                Err("Stop failed".to_owned()),
            ),
            Err("Stop failed".to_owned())
        );
        assert!(publisher.is_none());
        assert_eq!(advertised_port, Some(7100));
        assert_eq!(advertised_address, Some(Ipv4Addr::new(192, 168, 1, 10)));
    }

    #[test]
    fn winrt_preflight_defers_to_the_real_handoff_advertisement() {
        let options = BeaconOptions {
            ble_type: crate::BleType::WinRt,
            file: None,
            path: "beacon.ble".into(),
            ipv4: Ipv4Addr::new(192, 0, 2, 8),
            advmin: None,
            advmax: None,
            index: None,
            device: None,
            exit_when_stopped: false,
            notices: Vec::new(),
        };
        let mut backend = WinRtBackend::new();

        assert_eq!(backend.preflight(&options), Ok(false));
        assert!(backend.publisher.is_none());
    }
}
