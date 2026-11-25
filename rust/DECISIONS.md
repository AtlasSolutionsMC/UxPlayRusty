# Architecture decisions

## ADR-001: Build a standalone Rust replacement
Status: accepted (2026-11-21)

Context: UxPlay is a whole-application C/C++ project plus a Python BLE companion. The rust-anything analyzer supports C and Python but not the C++11 command-line entry point. The original remains the reference and must stay unchanged.

Decision: implement new application code in Rust from an empty target directory. Use rust-anything to characterize and gate bridgeable C/Python units; manually port C++ and unsupported units. Keep the original tree read-only and build its comparison executable under `baseline/`.

Consequences: do not copy source implementations into the final Rust app. Preserve required upstream license notices. Native libraries such as GStreamer, libplist, OpenSSL and OS Bluetooth services may remain runtime/build dependencies; “Rust replacement” means UxPlay-owned application code is Rust, not that every codec or OS service is reimplemented.

## ADR-002: Keep the two executable entry points
Status: accepted (2026-11-21)

Context: `uxplay` is a persistent AirPlay server; `uxplay-beacon` is an independent helper that advertises the server over BLE and can be run separately.

Decision: produce two Rust binaries, `uxplay` and `uxplay-beacon`. Keep their separate command-line surfaces and files/environment contracts. Do not combine BLE advertising into the AirPlay process.

Consequences: each binary needs its own app-level record/replay cases, install entry and usage documentation. The primary app must preserve `-ble` file production and beacon interoperability.

## ADR-003: Preserve observable behavior
Status: accepted (2026-11-21)

Context: the user approved retaining the original CLI, configuration/environment behavior, AirPlay protocol behavior, output and exit codes.

Decision: treat flags, rc-file precedence, protocol messages and serialized plist/crypto bytes, discovery records, media pipeline strings, generated files, stdout/stderr and exit status as compatibility requirements. Record the original before replacing a unit. Do not “fix” source behavior without a new decision and explicit evidence.

Consequences: every unit must pass `rust-anything gate`; app cases must pass `rust-anything app replay`. Random/session-dependent fields require narrowly named normalizers in the recorded cases, never broad wildcarding.

## ADR-004: Retain all supported operating systems
Status: accepted (2026-11-21)

Context: the accepted target matrix is Linux, macOS, BSD/Unix and Windows. Some feature backends are OS-specific (AWDL, BlueZ, WinRT, raw HCI, Bonjour and serial BLE).

Decision: preserve the original platform-specific feature gates and build a per-OS adapter for discovery, networking, rendering, screensaver inhibition and BLE. The Windows development build uses the GNU/MinGW ABI, matching the original's documented MSYS2 build; other OS builds must use their native Rust target and dependency manager.

Consequences: local Windows success is not evidence of BSD/macOS/Linux acceptance. Each target remains open until built on that OS or a compatible CI runner and its native integration is exercised.

## ADR-005: Reuse native crypto and media engines first
Status: accepted for phase one (2026-11-21)

Context: the source uses OpenSSL for EVP, Ed25519, X25519, SRP big integers and FairPlay primitives. Rendering and HLS use GStreamer, GLib and installed codec/sink plugins. Primary-source research found safe Rust bindings with close semantic coverage.

Decision: use the Rust `openssl` crate for the phase-one crypto facade and `gstreamer`, `gstreamer-app`, `gstreamer-video` and `glib` bindings for rendering. Keep the same native runtime engines and plugin contracts; do not introduce a Rust-only crypto or media rewrite during protocol migration.

Consequences: packaging still requires libcrypto/OpenSSL and GStreamer with the needed plugins. Current gstreamer-rs bindings require GStreamer >=1.14 while the source CMake floor is >=1.4. The Rust target will declare and verify the newer floor unless later primary-source work proves a safe older-binding/FFI route. This minimum-version difference remains a compatibility risk to report and test on all four OS families.

## ADR-006: Preserve selectable discovery backends
Status: accepted (2026-11-21)

Context: UxPlay 1.74 supports an in-tree minimal mDNS responder, optional external DNS-SD/Bonjour/Avahi and a separate BLE beacon. macOS also has an AWDL/Bonjour path. `mdns-sd` research did not establish support for every BSD target.

Decision: keep explicit discovery adapters and the existing selection semantics. Port the in-tree responder unless a Rust responder proves behavior and platform support on every accepted target. Keep Bonjour/Avahi integration optional and retain BLE as a distinct discovery option.

Consequences: do not silently substitute a Linux-only or mDNS-only implementation. Verify service type, TXT records, ports, addresses, TTL/withdrawal, interface selection and AWDL behavior.

## ADR-007: Migrate in reversible increments
Status: accepted (2026-11-21)

Context: the user chose incremental migration with side-by-side comparison before cutover.

Decision: keep a Rust front door with a logged original fallback while units are being ported. Wire a unit only after its behavior cases and gate pass. Lift/remove fallback only after every case and unit is green; retain the original project as rollback/reference.

Consequences: the front door's measured fallback rate is tracked in `rust/MAP.md` and must keep decreasing as units wire in. Any nonzero fallback means migration is incomplete. Cutover instructions must include rollback to the original binary.

## ADR-008: Preserve the existing protocol scope
Status: accepted (2026-11-21)

Context: the reference supports AirPlay mirroring, AirPlay audio/RAOP, supported HLS playback, pairing/FairPlay, selectable discovery, media rendering/recording, RTP forwarding and BLE discovery. The README says AirPlay 2 multi-room audio is not supported.

Decision: port the reference's supported feature set and its existing exclusions. Do not claim AirPlay 2 multi-room support or extend to new protocol features as part of compatibility migration.

Consequences: acceptance cases will cover supported endpoints and documented unsupported cases, so the Rust binary does not accidentally advertise functionality that is not implemented.

## ADR-009: Make the Rust server the normal startup path
Status: accepted (2026-11-26)

Context: the user asked to continue the migration with essential functionality first and shorten the path to a safe cutover. The Rust server now owns discovery, pairing, protocol sessions and media delivery; its current Windows GNU all-target suite passes, the focused server replay passes with zero fallback, and the raw C/Rust HTTP differential matches all eight recorded responses.

Decision: successfully parsed ASCII startup options launch the Rust server by default. `UXPLAY_REFERENCE=1` explicitly runs the unchanged reference as a rollback during migration. Existing `UXPLAY_RUST_SERVER=1` is no longer required. This advances the normal path only; it does not reduce the accepted feature, platform or compatibility scope in ADR-003/004/008.

Consequences: Unicode CLI text and other inputs that the front door cannot parse may still reach the reference temporarily and must be recorded and ported. Do not claim the migration complete until residual fallback is removed, full CLI/server and unit acceptance passes, and the platform/device requirements in MAP are addressed. This supersedes only ADR-007's staged default-start choice; its full-gate and final-lift requirements remain in force.
