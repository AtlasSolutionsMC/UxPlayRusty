# UxPlayRusty

Standalone Rust implementation of UxPlay. The unchanged C/C++/Python reference lives outside this repository at `D:\Save\Uxplay\UxPlay` and is used for behavior comparisons.

## Project status

The Rust server is the default and no longer launches the reference executable. Windows GNU is the most fully exercised platform: the current app tests pass 550 cases by default and 552 with `external-dns-sd`; both Clippy modes and optimized builds pass. The Windows BLE companion passes 73 tests, and its 10-case source replay matches. The C/Rust HTTP response differential passes 8/8 routes after normalizing only the `Date` header. The generic server replay groups three connection-reset responses and reports 5/8 at zero fallback; those three routes pass individually. See [Checkpoints.md](Checkpoints.md) and [rust/MAP.md](rust/MAP.md) for commands and evidence.

The project is not yet accepted as a complete AirPlay replacement. Apple-device streaming and encrypted video still need real-device validation; physical BLE behavior and native non-Windows runtimes are also open. The user cannot provide the iPhone capture needed to diagnose the remaining video interoperability issue, so no speculative crypto/parser change is in progress.

## Project layout

```text
.
├── .github/workflows/       # Linux and macOS CI
├── README.md                # Project entry point and build notes
├── Checkpoints.md           # Resume state and historical progress
├── TestingBeforeResume.md   # Latest evidence to read before continuing
└── rust/
    ├── app/                 # Main UxPlay server binary
    ├── beacon/              # Separate BLE beacon binary
    ├── core/                # Shared BLE payload and handoff types
    ├── render/              # Audio, video, and mux renderers
    ├── units/               # Protocol and compatibility libraries
    ├── app-cases/           # CLI and server replay fixtures
    ├── research/            # Protocol/platform feasibility notes
    ├── MAP.md               # Port status and acceptance matrix
    ├── DECISIONS.md         # Accepted design choices
    └── GLOSSARY.md          # Shared protocol terminology
```

Build output (`target/`), the local C baseline, and generated binaries are excluded from Git. Keep the reference checkout separate and unchanged.

## Why there are many `Cargo.toml` files

There are 25 Rust packages in this repository. Each `Cargo.toml` belongs to a crate or executable package, such as the main app, BLE companion, renderer library, or a separately gated protocol unit. **Individual functions do not need their own `Cargo.toml`**; functions are grouped into Rust modules under `src/`.

The smaller crates preserve useful boundaries for source comparisons, focused tests, and reuse. They are built by pointing Cargo at their manifest. A Cargo workspace could centralize membership and dependency locking, but it would not remove the package manifests and would change dependency resolution; that cleanup is deliberately separate from this file-organization pass.

## Build on Windows

The supported local build uses the Windows GNU toolchain and MSYS2 UCRT64. Set up the compiler and GStreamer runtime paths, then build the app:

```powershell
$msysUcrt = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msysUcrt\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $msysUcrt 'bin\gcc.exe'
$env:GST_PLUGIN_PATH_1_0 = Join-Path $msysUcrt 'lib\gstreamer-1.0'
$env:GST_PLUGIN_SCANNER_1_0 = Join-Path $msysUcrt 'libexec\gstreamer-1.0\gst-plugin-scanner.exe'
cargo build --manifest-path rust/app/Cargo.toml
```

For the BLE companion, use `cargo build --manifest-path rust/beacon/Cargo.toml`. Run `uxplay.exe -h` for the application options.

The build needs the native GStreamer runtime/plugins and OpenSSL selected in [rust/DECISIONS.md](rust/DECISIONS.md). Windows `external-dns-sd` builds compile without the Bonjour SDK but need Bonjour's `dnssd.dll` on `PATH` at runtime. Builds without that feature use embedded mDNS. On Linux/BSD, the external feature links `dns_sd`; macOS uses Bonjour by default for source parity, and `apple-p2p` enables the optional `-p2p` mode.

Build an individual library or run its focused gate with its own manifest, for example:

```powershell
cargo test --manifest-path rust/units/playfair-sap-hash/Cargo.toml
```

## Migration records

- [Migration map](rust/MAP.md)
- [Design decisions](rust/DECISIONS.md)
- [Progress checkpoints](Checkpoints.md)
- [Resume/testing notes](TestingBeforeResume.md)
- [iPhone Mirroring and Vulkan feasibility](rust/research/iphone-mirroring-vulkan-feasibility.md)
