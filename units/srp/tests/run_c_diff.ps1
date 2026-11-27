$ErrorActionPreference = 'Stop'

$crate = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$sourceLib = Join-Path (Join-Path (Split-Path -Parent $repoRoot) 'UxPlay') 'lib'
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$msysBin = Join-Path $msysRoot 'bin'
$env:PATH = "$msysBin;$env:PATH"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:PKG_CONFIG_PATH = Join-Path $msysRoot 'lib\pkgconfig'
$manifest = Join-Path $crate 'Cargo.toml'
& cargo build --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { throw 'Building the Rust SRP library failed.' }

$cflags = (& pkg-config --cflags openssl) -split '\s+'
$libs = (& pkg-config --libs openssl) -split '\s+'
$exe = Join-Path $crate 'target\debug\srp_c_diff.exe'
$rustImportLib = Join-Path $crate 'target\debug\libuxplay_srp.dll.a'
& gcc @cflags "-I$sourceLib" "-I$(Join-Path $crate 'include')" `
    (Join-Path $PSScriptRoot 'srp_c_diff.c') $rustImportLib @libs -o $exe
if ($LASTEXITCODE -ne 0) { throw 'Compiling the original C and Rust differential harness failed.' }

$env:PATH = "$(Join-Path $crate 'target\debug');$msysBin;$env:PATH"
& $exe
if ($LASTEXITCODE -ne 0) { throw 'SRP C differential checks failed.' }
