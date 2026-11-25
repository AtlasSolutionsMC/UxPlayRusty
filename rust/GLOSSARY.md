# UxPlayRusty glossary

Use these terms consistently in modules, logs and tests.

| Term | Meaning in UxPlay | Rust name / location |
|---|---|---|
| AirPlay server | Receiver process advertised to Apple clients | `uxplay` binary, `protocol::Server` |
| AirPlay mirroring | Screen/video stream and control session | `protocol::mirror` |
| RAOP | AirPlay audio transport/control path | `protocol::raop` |
| FairPlay | Legacy Apple stream-key/decryption handshake | `crypto::fairplay` |
| SAP hash | Proprietary Playfair key transform helper family | `crypto::playfair::sap_hash` |
| Pairing | Ed25519/X25519 setup and session keys | `crypto::pairing::PairingSession` |
| SRP | Secure Remote Password authentication | `crypto::srp::SrpSession` |
| mDNS / DNS-SD | Local-link service discovery and service records | `discovery::mdns` / `discovery::dns_sd` |
| BLE beacon | Independent 46-byte legacy advertisement carrying IPv4 address and port | `beacon::Advertisement` |
| BlueZ | Linux Bluetooth daemon and D-Bus adapter | `beacon::bluez` |
| WinRT | Windows Bluetooth advertisement API | `beacon::winrt` |
| HCI | Raw Bluetooth Host Controller Interface backend | `beacon::hci` |
| BleuIO | BLE USB modem controlled through serial AT commands | `beacon::bleuio` |
| HLS | HTTP Live Streaming playback path, currently documented for the iOS YouTube app | `render::hls` |
| renderer | Per-client GStreamer playback/output pipeline | `render::{audio,video,mux}` |
| appsrc | GStreamer element receiving encoded or decoded buffers from AirPlay | `render::AppSource` adapter |
| rc file | Startup option file selected by `UXPLAYRC` or home-directory conventions | `config::RcFile` |
| AWDL | Apple Wireless Direct Link peer-to-peer connection mode | `discovery::awdl` (macOS/Bonjour only) |
| fallback | Temporary call from the Rust front door to the original executable | `app::original::run` |
