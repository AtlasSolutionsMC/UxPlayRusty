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
Start Menu shortcut, startup config and Run registration, runs the installed
`uxplay.exe -h` with MSYS2 removed from `PATH`, launches the tray build with an
isolated config and checks that the AirPlay server starts, uninstalls, and
checks cleanup:

```powershell
.\tools\windows\test-install.ps1 -Setup .\dist\windows\Setup.exe
```

It refuses to run if the current user already has a UxPlay installation or a
running `uxplay.exe`; it temporarily backs up and restores the current user's
config, BLE handoff, startup value and UxPlay shortcut. This checks installer
plumbing on the development machine. The clean-runner install/uninstall and
tray/server startup gate passed in run `45898164036`; sign-in startup, tray
menus, connected-client controls and Apple-device media remain manual
acceptance checks before a final release.

The `Windows installer smoke` workflow builds a provisional installer on one
fresh GitHub-hosted Windows runner, then transfers it as a one-day workflow
artifact to a separate fresh Windows runner for install, startup-registration,
installed CLI, tray/server startup and uninstall checks. The artifact is
temporary CI handoff only; it is not a release upload. This provides a clean OS
install gate while real AirPlay devices, tray interaction and BLE radio
behavior remain separate manual acceptance checks.

`third-party-licenses/` contains license texts installed by MSYS2 when
available. Some installed packages supply SPDX metadata but no separate text;
those packages receive a `LICENSE-REFERENCE.txt`, and their package version,
SPDX declaration and upstream URL are recorded in `THIRD-PARTY-NOTICES.txt`.
The current UCRT64 bundle includes GPL-labeled FFmpeg, x264 and x265 runtime
packages; the staged `avcodec-63.dll` imports the x264 and x265 DLLs. Their
staged directories contain `LICENSE-REFERENCE.txt` rather than the package
license text or corresponding source. FFmpeg documents that enabling GPL parts
makes GPL apply to FFmpeg and describes distributing the exact matching source
and build information ([FFmpeg legal considerations](https://www.ffmpeg.org/legal.html)).
Treat `Setup.exe` as provisional: do not publish it until the exact source/build
inputs and notices are resolved for the runtime, or the runtime is rebuilt with
a reviewed license profile.
