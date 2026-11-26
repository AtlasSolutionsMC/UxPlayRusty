# UxPlayRusty

Standalone Rust replacement for UxPlay. The reference source stays outside this repository at `D:\Save\Uxplay\UxPlay` and is used only for behavior recording and comparison.

## Current status

The migration remains active; valid startup options enter the standalone Rust server by default. The C reference stays in the separate reference checkout and is not launched by this binary. Windows CLI arguments use C's active ANSI code page. All 118 recorded CLI cases have Rust-only evidence, including startup failures when required GStreamer plugins are missing. The Rust server handles discovery, pairing and the staged protocol stack. Its per-connection runtime starts RAOP timing, audio RTP and video mirror workers, connects callbacks to the GStreamer renderers, forwards volume/progress/DMAP metadata/cover art, and returns the actual ports bound during `SETUP`. The Rust media callbacks support `-admp`, `-vdmp` and `-mp4`, including codec-based filenames, dump limits, SPS rollover and MP4 packet timestamps. A Rust integration test exercises a complete local `SETUP`, ephemeral media sockets and disconnect cleanup. This is not yet an Apple-client playback test or full protocol replay. The server also handles `/pair-setup`, `/pair-verify`, `/pair-pin-start`, `/pair-setup-pin`, `/scrub`, `/rate` and `/stop`; raw `/info` and `/pair-setup` matched C byte-for-byte. POST `/reverse` upgrades to PTTH/1.0 and sends FCUP requests over the reverse socket. HLS `/play` and `/action` now sequence FCUP fetches, store and rewrite master/media playlists, filter custom and Raspberry Pi profiles, select requested/system audio and subtitle languages, and switch the video renderer to playbin. Fixed and random `-pw` modes now use RFC 2617 Digest challenges on initial RAOP `SETUP`; random mode displays a four-digit PIN and rotates it after three prompts. Rust `-ble` writes the source-shaped PID/port/process handoff file and removes it on shutdown for the separate `uxplay-beacon` process. Windows mDNS runtime smoke received a 533-byte `_raop._tcp.local` PTR reply. The C launcher/fallback has been removed; Apple playback, media output, BLE radio, and non-Windows runtime acceptance remain open.

Latest Windows GNU all-target gates pass 433 app tests by default and 435 with `external-dns-sd`, plus 10/99/7 tests across auxiliary targets; Clippy with warnings denied and rustfmt pass both modes. The 776-case unchanged-C HTTP differential covers every byte in URL targets, header names and header values and passes Linux run `45833787778`. Rust port-list validation follows the upstream fix for unique decimal port lists, with CLI fixtures and direct binary checks for range errors and duplicates; latest Linux run `45836629441` passes. GitHub compare against `FDH2/UxPlay:master` reports 0 commits behind. RAOP RTP passes 17 tests; mirror RTP passes 14; mirror buffer passes its 3 source-oracle tests. A fresh empty-target optimized Windows GNU release build and the `external-dns-sd` release build passed at the previous acceptance boundary; `uxplay.exe -h` exits 0. All 118 recorded Windows CLI cases have Rust-only evidence. Raw-byte C/Rust HTTP responses match for 8/8 routes after normalizing only `Date`; the broader replay reports 5/8 at zero fallback because three reader-reset cases pass individually 1/1. A macOS 15 workflow now gates native Apple Bonjour/P2P build, tests, Clippy and release; Linux run `45837953213` passed, while native macOS run `45837953181` is still queued. Both CI workflows now also gate the Rust BLE companion. Windows GNU BLE tests pass 68 library + 5 binary tests; the Python 3.13.17 source replay matches all ten CLI/config cases and the BlueZ callback fixture, and target checks pass for macOS, Linux and FreeBSD. Physical BLE and Bonjour behavior remain open. The live GitHub compare confirms 0 commits behind `FDH2/UxPlay:master`; the previously shared 95/3 view was stale. Native Unix/BSD runtime, Bonjour registration visibility, Apple audio/video playback, BLE radio, real-device mirroring and packaging acceptance remain open. The Vulkan and iPhone Mirroring feasibility assessment is in [rust/research/iphone-mirroring-vulkan-feasibility.md](rust/research/iphone-mirroring-vulkan-feasibility.md). See [Checkpoints.md](Checkpoints.md) and [rust/MAP.md](rust/MAP.md) for detailed evidence and remaining work.

## Development

The baseline CMake build and toolchain notes are in [Checkpoints.md](Checkpoints.md). The migration map, accepted decisions, glossary and research are in [rust/](rust/).

Build the standalone Rust application with:

```powershell
$msysUcrt = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msysUcrt\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $msysUcrt 'bin\gcc.exe'
cargo build --manifest-path rust/app/Cargo.toml
```

On Linux and BSD, enable the optional external DNS-SD backend with
`external-dns-sd` when matching `cmake -DUSE_DNS_SD=1`; install a compatible
`libdns_sd` implementation such as Avahi's DNS-SD compatibility library or
mDNSResponder. Windows can use the same feature to load Bonjour's `dnssd.dll`
at runtime. macOS default-build parity uses Bonjour DNS-SD; the source's
`-DUSE_MDNS=1` selects embedded mDNS instead. The `apple-p2p` feature also
enables the optional `-p2p` mode:

```sh
# Linux/BSD with libdns_sd available
cargo build --manifest-path rust/app/Cargo.toml --features external-dns-sd

# Windows with Bonjour installed and dnssd.dll on the loader path
cargo build --manifest-path rust/app/Cargo.toml --features external-dns-sd

# macOS default Bonjour backend, including optional -p2p support
cargo build --manifest-path rust/app/Cargo.toml --features apple-p2p
```

On Unix, `external-dns-sd` links `dns_sd`; Linux/BSD must provide it through the
selected system DNS-SD implementation, and macOS gets it from the SDK. On
Windows, the feature dynamically loads `dnssd.dll` and checks for the required
Bonjour exports, so no Bonjour SDK is needed to compile. Install Bonjour on the
Windows host and keep its DLL on the loader path to run the backend. Missing
DLL/export errors are reported during startup; native Windows registration and
visibility have not been runtime-tested. The Apple feature selects Bonjour
whether or not `-p2p` is set. On macOS, `-p2p` adds Apple's `IncludeP2P` and
`IncludeAWDL` registration flags and the AirPlay Receiver setting guard. Builds
without the relevant feature use embedded mDNS.

Build and run an individual port from its crate directory; for example:

```powershell
cargo test --manifest-path rust/units/playfair-sap-hash/Cargo.toml
```

The final build will require the native GStreamer runtime/plugins and OpenSSL selected in `rust/DECISIONS.md`.
