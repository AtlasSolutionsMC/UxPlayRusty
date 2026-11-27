$ErrorActionPreference = 'Stop'

$crateRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$opensslDir = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path -LiteralPath (Join-Path $opensslDir 'include\openssl\evp.h')) {
    $env:OPENSSL_DIR = $opensslDir
    $env:OPENSSL_STATIC = '0'
    $env:PATH = (Join-Path $opensslDir 'bin') + ';' + $env:PATH
}
$cc = $env:CC
if (-not $cc) {
    $ccCommand = Get-Command cc.exe -ErrorAction SilentlyContinue
    if ($ccCommand) {
        $cc = $ccCommand.Source
    } else {
        $msysCc = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
        if (Test-Path -LiteralPath $msysCc) { $cc = $msysCc }
    }
}
if (-not $cc) { throw 'Set CC to a C compiler configured with OpenSSL headers and libcrypto.' }

$tempExe = Join-Path $env:TEMP ("fairplay-v2-oracle-{0}.exe" -f [guid]::NewGuid())
try {
    & $cc (Join-Path $PSScriptRoot 'fairplay_v2_oracle.c') -o $tempExe -lcrypto
    if ($LASTEXITCODE -ne 0) { throw "C oracle compilation failed with exit code $LASTEXITCODE" }

    $cOutput = (& $tempExe) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "C oracle exited with code $LASTEXITCODE" }
    $rustOutput = (& cargo +stable-x86_64-pc-windows-gnu run --quiet --manifest-path (Join-Path $crateRoot 'Cargo.toml') --features oracle --bin fairplay_v2_oracle) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "Rust transcript command failed with exit code $LASTEXITCODE" }
    if ($cOutput -ne $rustOutput) {
        $cPath = Join-Path $crateRoot 'target\c-oracle.txt'
        $rustPath = Join-Path $crateRoot 'target\rust-oracle.txt'
        Set-Content -LiteralPath $cPath -Value $cOutput
        Set-Content -LiteralPath $rustPath -Value $rustOutput
        throw "C and Rust protocol transcripts differ. Compare $cPath and $rustPath."
    }
    'C oracle and Rust transcript match across four selector modes and the key success/error paths.'
} finally {
    Remove-Item -LiteralPath $tempExe -Force -ErrorAction SilentlyContinue
}
