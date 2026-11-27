# Windows release packaging

This procedure builds the Windows GNU/MSYS2 UCRT64 executables, bundles their runtime dependencies, and creates `Setup.exe` with Inno Setup 6.

## Requirements

- Rust stable for `x86_64-pc-windows-gnu`
- MSYS2 UCRT64 with the GStreamer runtime/plugins and OpenSSL required by UxPlay
- Inno Setup 6

## Build

From the repository root in PowerShell:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
.\packaging\windows\package-release.ps1 -Msys2Root $msysRoot
```

The script rebuilds `uxplay.exe` and `uxplay-beacon.exe`, stages the runtime, and writes `Setup.exe` under `dist/windows/`. Build outputs are ignored by Git. The bundled runtime uses embedded mDNS and includes the GStreamer plugins selected by the packaging script. Custom pipelines may need additional plugins.

The installer supports a current-user installation, Start Menu shortcut, optional desktop shortcut, optional sign-in startup, and uninstall. Configuration and logs are kept under the user's profile and preserved when the application is uninstalled.

## Smoke checks

Check the staged runtime and CLI without relying on MSYS2 being on the child process path:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
.\packaging\windows\smoke-runtime.ps1 -StageDirectory .\dist\windows\stage -Msys2Root $msysRoot
```

The install smoke installs the package, checks startup/configuration and tray command routing, launches the app, uninstalls it, and checks cleanup:

```powershell
.\packaging\windows\test-install.ps1 -Setup .\dist\windows\Setup.exe
```

The install smoke refuses to run when a UxPlay installation or running `uxplay.exe` is already present for the current user. These automated checks do not replace validation with a physical Apple device, visible tray interaction, GPU-backed renderer lifecycle, or BLE radio scanner.

## Distribution note

Treat locally generated installers as build artifacts until the bundled runtime's license notices and corresponding source availability have been reviewed for the exact package versions. The build script can collect matching MSYS2 source packages with `packaging/windows/collect-source-packages.ps1`; these archives are separate from `Setup.exe` and are not added to Git automatically.