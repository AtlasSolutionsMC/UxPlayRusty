# Local build tools (personal workstation)

These notes record the Windows build setup on my development PC. They are
machine-specific; the MSYS2 paths below may differ on another computer.

## Installed tools

- Rust toolchain: `stable-x86_64-pc-windows-gnu` (MinGW target installed).
- MSYS2 UCRT64: `C:\msys64\ucrt64`.
- OpenSSL: 4.1.0, MinGW/UCRT64 build. Headers and libraries are under
  `C:\msys64\ucrt64\include\openssl` and `C:\msys64\ucrt64\lib`.
- GStreamer: 1.28.9 development files available through UCRT64 `pkg-config`.
- libplist: 2.7.0 development files available through UCRT64 `pkg-config`.
- `pkg-config.exe`, `gcc.exe`, `windres.exe`, and `objdump.exe` are in
  `C:\msys64\ucrt64\bin`.

Versions above were read from this workstation on 2026-11-27.

## Build the Windows GNU target

Run from the repository root in PowerShell:

```powershell
$env:PKG_CONFIG_PATH = 'C:\msys64\ucrt64\lib\pkgconfig'
$env:PATH = 'C:\msys64\ucrt64\bin;' + $env:PATH
cargo +stable-x86_64-pc-windows-gnu build --release --manifest-path app/Cargo.toml
```

The application executable is written to:

```text
app\target\release\uxplay.exe
```

For a faster development build, remove `--release`; Cargo writes that executable
to `app\target\debug\uxplay.exe`.

## Why use the GNU toolchain here

The OpenSSL installed on this PC is built for MinGW and supplies `.a` libraries.
The default Rust MSVC target expects MSVC-compatible OpenSSL `.lib` files, so a
plain MSVC build reports that it cannot find OpenSSL even though the GNU
installation is present. Use the GNU Rust toolchain with the UCRT64 libraries
for local builds. This produces a Windows x64 GNU executable; it is not an MSVC
build.

The app build also probes GStreamer and libplist through `pkg-config`, so keep
`PKG_CONFIG_PATH` and the UCRT64 `bin` directory in the build environment.

## Quick version checks

```powershell
rustc +stable-x86_64-pc-windows-gnu --version
openssl version
$env:PKG_CONFIG_PATH = 'C:\msys64\ucrt64\lib\pkgconfig'
pkg-config --modversion openssl gstreamer-1.0 libplist-2.0
```
