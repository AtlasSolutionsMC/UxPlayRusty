# Windows release packaging

This procedure builds the Windows GNU/MSYS2 UCRT64 executables, bundles their runtime dependencies, and creates a self-contained Rust `Setup.exe`.

## Requirements

- Rust stable for `x86_64-pc-windows-gnu`
- MSYS2 UCRT64 with the GStreamer runtime/plugins and OpenSSL required by UxPlay

## Build

From the repository root in PowerShell:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
cargo run --release --manifest-path packaging/windows-tools/Cargo.toml -- package-release $msysRoot
```

The script rebuilds `uxplay.exe` and `uxplay-beacon.exe`, stages the runtime, and writes `Setup.exe` under `dist/windows/`. Build outputs are ignored by Git. The bundled runtime uses embedded mDNS and includes the GStreamer plugins selected by the packaging script. Custom pipelines may need additional plugins.

The Rust installer embeds a compressed copy of the staged runtime in `Setup.exe`. It supports current-user installation, Start Menu shortcut, optional desktop shortcut, optional sign-in startup, and uninstall. Run `Setup.exe` for confirmation dialogs or pass `--quiet --install-dir PATH` for scripted installs. Add `--autostart` or `--desktop-shortcut` to enable those options. Configuration and logs are kept under the user's profile and preserved when the application is uninstalled.

## Smoke checks

Check the staged runtime and CLI without relying on MSYS2 being on the child process path:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
cargo run --release --manifest-path packaging/windows-tools/Cargo.toml -- smoke-runtime .\dist\windows\stage $msysRoot
```

The Rust install smoke installs the package, checks startup/configuration and shortcuts, runs the uninstaller, and checks cleanup:

```powershell
cargo run --release --manifest-path packaging/windows-tools/Cargo.toml -- test-install .\dist\windows\Setup.exe
```

The install smoke refuses to run when a UxPlay installation or running `uxplay.exe` is already present for the current user. These automated checks do not replace validation with a physical Apple device, visible tray interaction, GPU-backed renderer lifecycle, or BLE radio scanner.

## Distribution note

Treat locally generated installers as build artifacts until the bundled runtime's license notices and corresponding source availability have been reviewed for the exact package versions.
