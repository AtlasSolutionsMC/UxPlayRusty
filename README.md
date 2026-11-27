# UxPlayRusty

UxPlayRusty is a standalone Rust AirPlay receiver for Windows, Linux, and macOS. It includes the receiver application and a separate BLE companion.

## Windows features

- Optional low-latency video queues and D3D11 or Vulkan renderer selection
- Editable `config.toml` with command-line overrides
- Notification-area controls for configuration, restart, disconnect, and exit
- Optional sign-in startup and BLE companion lifecycle management
- Windows installer and bundled GStreamer runtime build scripts

Apple-device streaming and hardware-specific renderer and BLE behavior still require hands-on validation. The repository does not claim full AirPlay compatibility based only on builds or synthetic tests.

## Build on Windows

Install Rust stable for `x86_64-pc-windows-gnu`, MSYS2 UCRT64 with the required GStreamer runtime and development packages, and OpenSSL. In PowerShell, configure the toolchain and build the receiver:

```powershell
$msysUcrt = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msysUcrt\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $msysUcrt 'bin\gcc.exe'
$env:PKG_CONFIG_PATH = "$msysUcrt\lib\pkgconfig;$msysUcrt\share\pkgconfig"
cargo build --manifest-path rust/app/Cargo.toml --release
cargo build --manifest-path rust/beacon/Cargo.toml --release
```

Run `uxplay.exe -h` for command-line options. Focused crate tests can be run with their manifest, for example:

```powershell
cargo test --manifest-path rust/units/playfair-sap-hash/Cargo.toml
```

## Windows configuration and installer

See [Windows configuration](docs/development/WindowsConfig.md) for every supported setting, [Windows tray controls](docs/development/WindowsTray.md) for desktop behavior, and [Windows release packaging](docs/development/WindowsRelease.md) to build and smoke-test the installer. Build output is written under the ignored `dist/windows/` directory and is not included in Git.