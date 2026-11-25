$ErrorActionPreference = 'Stop'

$crateRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$opensslDir = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path -LiteralPath (Join-Path $opensslDir 'include\openssl\evp.h')) {
    $env:OPENSSL_DIR = $opensslDir
    $env:OPENSSL_STATIC = '0'
    $env:PATH = (Join-Path $crateRoot 'target\debug') + ';' + (Join-Path $opensslDir 'bin') + ';' + $env:PATH
}
$cc = $env:CC
if (-not $cc) { $cc = Join-Path $opensslDir 'bin\cc.exe' }
if (-not (Test-Path -LiteralPath $cc)) { throw 'Set CC to the MSYS2 C compiler used for GNU OpenSSL.' }

& cargo +stable-x86_64-pc-windows-gnu build --manifest-path (Join-Path $crateRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "Rust cdylib build failed with exit code $LASTEXITCODE" }

$exe = Join-Path $crateRoot 'target\c_abi_smoke.exe'
& $cc (Join-Path $PSScriptRoot 'c_abi_smoke.c') '-I' (Join-Path $crateRoot 'include') '-L' (Join-Path $crateRoot 'target\debug') '-lfairplay_playfair' '-lcrypto' '-lbcrypt' '-o' $exe
if ($LASTEXITCODE -ne 0) { throw "C ABI smoke compilation failed with exit code $LASTEXITCODE" }
& $exe
if ($LASTEXITCODE -ne 0) { throw "Rust FairPlay C ABI smoke failed with exit code $LASTEXITCODE" }
'Rust C ABI exports match fairplay.h and pass setup/handshake/decrypt/destroy.'
