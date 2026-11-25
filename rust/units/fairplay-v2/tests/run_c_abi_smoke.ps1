$ErrorActionPreference = 'Stop'

$crateRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$opensslDir = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path -LiteralPath (Join-Path $opensslDir 'include\openssl\evp.h')) {
    $env:OPENSSL_DIR = $opensslDir
    $env:OPENSSL_STATIC = '0'
    $env:PATH = (Join-Path $crateRoot 'target\debug') + ';' + (Join-Path $opensslDir 'bin') + ';' + $env:PATH
}
$cc = $env:CC
if (-not $cc) {
    $ccCommand = Get-Command cc.exe -ErrorAction SilentlyContinue
    if ($ccCommand) {
        $cc = $ccCommand.Source
    } else {
        $cc = Join-Path $opensslDir 'bin\cc.exe'
    }
}
if (-not (Test-Path -LiteralPath $cc)) { throw 'Set CC to the C compiler used for the GNU OpenSSL target.' }

& cargo +stable-x86_64-pc-windows-gnu build --manifest-path (Join-Path $crateRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "Rust cdylib build failed with exit code $LASTEXITCODE" }

$exe = Join-Path $crateRoot 'target\c_abi_smoke.exe'
& $cc (Join-Path $PSScriptRoot 'c_abi_smoke.c') '-I' (Join-Path $crateRoot 'include') '-L' (Join-Path $crateRoot 'target\debug') '-lfairplay_v2' '-o' $exe
if ($LASTEXITCODE -ne 0) { throw "C ABI smoke compilation failed with exit code $LASTEXITCODE" }
& $exe
if ($LASTEXITCODE -ne 0) { throw "Rust C ABI smoke failed with exit code $LASTEXITCODE" }
'Rust C ABI exports match the header and pass setup/reset/invalid-state checks.'
