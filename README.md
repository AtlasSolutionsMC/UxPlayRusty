<table>
<tr>
<td width="220" align="center" valign="middle">
<img src="assets/uxplayrusty-logo-transparent.png" alt="UxPlayRusty logo" width="190">
</td>
<td valign="middle">
<h1>UxPlayRusty</h1>
<p><strong>A Rust AirPlay receiver for Windows, Linux, and macOS</strong></p>
<p>Stream audio and video from Apple devices over your local network. Includes the receiver application and an optional BLE companion.</p>
</td>
</tr>
</table>

---

## About

UxPlayRusty is a Rust implementation of the UxPlay receiver, organized into modules for the main application, network service discovery, media rendering, and the BLE service-discovery beacon.

> **Status:** Under active development. Builds and automated checks do not confirm AirPlay compatibility with physical Apple devices or hardware-specific behavior.

## Features

| Area | Details |
| --- | --- |
| AirPlay receiver | `uxplay` application for Windows, Linux, and macOS |
| Video | Windows renderer selection: `auto`, `d3d11`, or `vulkan` |
| Configuration | Windows `config.toml` with command-line overrides |
| Notification area | Windows menu for Config, Restart, Disconnect, and Exit |
| BLE companion | `uxplay-beacon` advertises the AirPlay service over Bluetooth LE; enable it from Windows configuration |
| Windows packaging | Scripts for creating a bundled runtime and `Setup.exe` installer |

## Getting started on Windows

### Requirements

- Rust stable for the `x86_64-pc-windows-gnu` target
- MSYS2 UCRT64 with GStreamer runtime and development packages, plus OpenSSL

### Build

From the repository root in PowerShell, run:

```powershell
$msysUcrt = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msysUcrt\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $msysUcrt 'bin\gcc.exe'
$env:PKG_CONFIG_PATH = "$msysUcrt\lib\pkgconfig;$msysUcrt\share\pkgconfig"

cargo build --manifest-path app/Cargo.toml --release
cargo build --manifest-path beacon/Cargo.toml --release
```

The executables are written to `app/target/release/uxplay.exe` and `beacon/target/release/uxplay-beacon.exe`. To see the available command-line options, run:

```powershell
app\target\release\uxplay.exe -h
```

### Configuration

On first launch, the app creates `%APPDATA%\UxPlay\config.toml`. Use it to configure the receiver name, video renderer, low-latency mode, BLE, and logging.

- [Example configuration](app/assets/config.example.toml)
- [Configuration options and networking](docs/development/WindowsConfig.md)
- [Notification-area controls](docs/development/WindowsTray.md)
- [Build the installer and run smoke checks](docs/development/WindowsRelease.md)

## Build on Linux and macOS

In addition to Rust, install the native development packages for GStreamer, OpenSSL, and libplist. Linux also requires a DNS-SD/Avahi development package. Then build with:

```sh
cargo build --manifest-path app/Cargo.toml --release
cargo build --manifest-path beacon/Cargo.toml --release
```

See the [Linux](.github/workflows/rust-linux.yml) and [macOS](.github/workflows/rust-macos.yml) CI workflows for the packages used by the project.

## Project structure

```text
app/       AirPlay receiver application
beacon/    BLE service-discovery companion
core/      Shared foundational code
render/    Audio, video, and mux renderers
units/     Protocol modules and supporting components
packaging/ Windows packaging tools
docs/      Development and configuration guides
```

## License

This project is distributed under the [GNU General Public License v3.0](LICENSE). Review dependency license notices and the licenses of bundled packages before redistributing a runtime bundle.

