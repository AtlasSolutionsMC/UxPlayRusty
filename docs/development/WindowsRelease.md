# Windows release packaging

The Windows installer is assembled from the GNU/MSYS2 UCRT64 release build.
The packager builds both executables, follows PE DLL imports for the app,
beacon, GStreamer plugins and plugin scanner, collects the required runtime
files, records MSYS2 package metadata and available license texts, then compiles
Inno Setup's installer.

## Build

Install Rust stable for `x86_64-pc-windows-gnu`, MSYS2 UCRT64 with the GStreamer
runtime/plugins used by UxPlay, and Inno Setup 6. From the repository root run:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
.\tools\windows\package-release.ps1 -Msys2Root $msysRoot
```

The script rebuilds `uxplay.exe` and `uxplay-beacon.exe` and writes the staging
directory and `Setup.exe` below the ignored `dist/windows/` directory. The
Windows release uses embedded mDNS. It does not require Bonjour at runtime.
The staged runtime contains the GStreamer plugins needed for default audio,
H.264/H.265 mirror playback, HLS, D3D11 and Vulkan paths; arbitrary custom
GStreamer pipelines can require plugins beyond this bundle.

The installer adds a Start Menu shortcut, an optional desktop shortcut, an
optional current-user sign-in startup task, an Add/Remove Programs entry and an
uninstaller. It installs under the current user's Programs directory, keeps
configuration and logs under the user's profile, and preserves config/logs on
uninstall. Startup selection updates both `config.toml` and the current-user
Run value.

## Runtime and installer smoke

Run the bundled-runtime smoke with MSYS2 removed from the child process path:

```powershell
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'
.\tools\windows\smoke-runtime.ps1 -StageDirectory .\dist\windows\stage -Msys2Root $msysRoot
```

The smoke loads required GStreamer factories from the staged bundle and checks
the packaged app's CLI startup. It does not test media from an Apple device,
tray interaction, GPU rendering quality, BLE radio behavior or a clean Windows
installation.

The install smoke installs silently to a temporary directory, checks the
Start Menu shortcut, startup config and Run registration, uninstalls, and
checks cleanup:

```powershell
.\tools\windows\test-install.ps1 -Setup .\dist\windows\Setup.exe
```

It refuses to run if the current user already has a UxPlay installation or a
running `uxplay.exe`; it temporarily backs up and restores the current user's
config, BLE handoff, startup value and UxPlay shortcut. This checks installer
plumbing on the development machine, not the required clean-machine acceptance
flow. The final release still needs installation, sign-in startup, tray/media
operation and uninstall on a clean Windows system.

`third-party-licenses/` contains license texts installed by MSYS2 when
available. Some installed packages supply SPDX metadata but no separate text;
those packages receive a `LICENSE-REFERENCE.txt`, and their package version,
SPDX declaration and upstream URL are recorded in `THIRD-PARTY-NOTICES.txt`.
Review notices and any source-distribution obligations before publishing the
installer.
