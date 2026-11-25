# UxPlayRusty

Standalone Rust replacement for UxPlay. The reference source stays outside this repository at `D:\Save\Uxplay\UxPlay` and is used only for behavior recording and comparison.

## Current status

The migration remains active; valid startup options enter the standalone Rust server by default. The C reference stays in the separate reference checkout and is not launched by this binary. Windows CLI arguments use C's active ANSI code page. All 118 recorded CLI cases have Rust-only evidence, including startup failures when required GStreamer plugins are missing. The Rust server handles discovery, pairing and the staged protocol stack. Its per-connection runtime starts RAOP timing, audio RTP and video mirror workers, connects callbacks to the GStreamer renderers, forwards volume/progress/DMAP metadata/cover art, and returns the actual ports bound during `SETUP`. The Rust media callbacks support `-admp`, `-vdmp` and `-mp4`, including codec-based filenames, dump limits, SPS rollover and MP4 packet timestamps. A Rust integration test exercises a complete local `SETUP`, ephemeral media sockets and disconnect cleanup. This is not yet an Apple-client playback test or full protocol replay. The server also handles `/pair-setup`, `/pair-verify`, `/pair-pin-start`, `/pair-setup-pin`, `/scrub`, `/rate` and `/stop`; raw `/info` and `/pair-setup` matched C byte-for-byte. POST `/reverse` upgrades to PTTH/1.0 and sends FCUP requests over the reverse socket. HLS `/play` and `/action` now sequence FCUP fetches, store and rewrite master/media playlists, filter custom and Raspberry Pi profiles, select requested/system audio and subtitle languages, and switch the video renderer to playbin. Fixed and random `-pw` modes now use RFC 2617 Digest challenges on initial RAOP `SETUP`; random mode displays a four-digit PIN and rotates it after three prompts. Rust `-ble` writes the source-shaped PID/port/process handoff file and removes it on shutdown for the separate `uxplay-beacon` process. Windows mDNS runtime smoke received a 533-byte `_raop._tcp.local` PTR reply. BLE radio runtime, full device playback, media-output acceptance and complete standalone cutover remain open.

Current Windows GNU app checks pass 526 all-target tests by default and 528 with `external-dns-sd` (411/413 main, 10 AirPlay controls, 98 HTTP transport and 7 RAOP-info), plus 23 HTTP-handler unit tests and 5 source-derived oracle tests. The Windows compatibility crate passes its C-oracle test and now supplies startup and optional IPv6 listener diagnostics. Formatting and Clippy with `-D warnings` pass both modes. Windows CLI now applies the active ANSI code page like the C runtime; all 118 recorded cases have Rust-only evidence, and GStreamer missing-plugin startup now matches the C exit code and stopping message. The primary Rust listener binds IPv4 and optional IPv6 on the same selected port, following the source's backlog of five. Invalid `/play` requests send 400, disconnect and trigger a server restart after the source's 100 ms reset interval. RAOP `POST /feedback` now returns the source response and resets a watchdog; `-reset n` bounds missed feedback while connections are active, then restarts the listener. Finished HLS playback now stops/restores the renderer, clears cached playlists and closes HLS/EVENT/AirPlay connections after the reply while keeping RAOP active. The raw-byte C/Rust HTTP differential passes 8/8, normalizing only `Date`; a focused `/server-info` app replay passes 1/1 at 0 fallback. The generic 8-case HTTP replay is 5/8 because its reader resets on source-persistent connections; raw-byte capture still matches C 8/8. Video pipeline options `-bt709` and `-srgb` now build the source-shaped caps fragments; `-async` and `-vsync` apply codec-specific delays to audio render timestamps. Password checks cover a fixed-password challenge, digest success/rejection and nonce consumption, random PIN display/rotation, and an initial-SETUP integration flow. The BLE handoff writer has a byte-layout test; the external advertiser still needs radio/runtime validation. HLS transformations and the `/play` → FCUP → `/action` adapter flow have focused tests; recent `/action` checks preserve C-string matching and playlist-removal resume/cache state. Actual streaming from an Apple client and Raspberry Pi hardware behavior remain unverified. The Windows mDNS and local RAOP SETUP tests prove socket paths, not real AirPlay playback or output-file acceptance. Linux linking remains blocked by the missing GCC/OpenSSL sysroot; macOS/BSD and non-Windows runtime paths remain unverified. Earlier C differential evidence covers RTSP parsing, DNS-SD, mDNS, controls, logger, networking, RAOP parameters, plist responses and the cleartext queue. See [Checkpoints.md](Checkpoints.md) and [rust/MAP.md](rust/MAP.md) for current evidence and remaining work.

## Development

The baseline CMake build and toolchain notes are in [Checkpoints.md](Checkpoints.md). The migration map, accepted decisions, glossary and research are in [rust/](rust/).

Build the current Rust front door with:

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
