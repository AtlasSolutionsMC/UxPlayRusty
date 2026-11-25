$ErrorActionPreference = 'Stop'

$crateRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$referenceRoot = (Resolve-Path (Join-Path $crateRoot '../../../../UxPlay')).Path
$opensslDir = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path -LiteralPath (Join-Path $opensslDir 'include\openssl\evp.h')) {
    $env:OPENSSL_DIR = $opensslDir
    $env:OPENSSL_STATIC = '0'
    $env:PATH = (Join-Path $opensslDir 'bin') + ';' + $env:PATH
}
$cc = $env:CC
if (-not $cc) {
    $ccPath = Join-Path $opensslDir 'bin\cc.exe'
    if (Test-Path -LiteralPath $ccPath) { $cc = $ccPath }
}
if (-not $cc) { throw 'Set CC to an MSYS2 C compiler configured with OpenSSL.' }

$exePath = Join-Path $env:TEMP ("fairplay-playfair-oracle-{0}.exe" -f [guid]::NewGuid())
$lib = Join-Path $referenceRoot 'lib'
$playfair = Join-Path $lib 'playfair'
$v2 = Join-Path $lib 'fairplay_v2'
$sourceFiles = @(
    (Join-Path $PSScriptRoot 'fairplay_playfair_oracle.c'),
    (Join-Path $lib 'fairplay_playfair.c'),
    (Join-Path $playfair 'playfair.c'),
    (Join-Path $playfair 'omg_hax.c'),
    (Join-Path $playfair 'modified_md5.c'),
    (Join-Path $playfair 'sap_hash.c'),
    (Join-Path $playfair 'hand_garble.c'),
    (Join-Path $v2 'fairplay_v2.c')
)
try {
    & $cc @sourceFiles "-I$lib" "-I$playfair" "-I$v2" '-lcrypto' '-lbcrypt' '-o' $exePath
    if ($LASTEXITCODE -ne 0) { throw "C oracle compilation failed with exit code $LASTEXITCODE" }
    $cOutput = (& $exePath) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "C oracle exited with code $LASTEXITCODE" }
    $rustOutput = (& cargo +stable-x86_64-pc-windows-gnu run --quiet --manifest-path (Join-Path $crateRoot 'Cargo.toml') --features oracle --bin fairplay_playfair_oracle) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "Rust transcript command failed with exit code $LASTEXITCODE" }
    if ($cOutput -ne $rustOutput) {
        $cPath = Join-Path $crateRoot 'target\c-oracle.txt'
        $rustPath = Join-Path $crateRoot 'target\rust-oracle.txt'
        Set-Content -LiteralPath $cPath -Value $cOutput
        Set-Content -LiteralPath $rustPath -Value $rustOutput
        throw "C and Rust transcripts differ. Compare $cPath and $rustPath."
    }
    'C oracle and Rust transcripts match for all four v3 modes, invalid selector-2 adapter setup, setup/handshake/decrypt framing, and reset/error state paths.'
} finally {
    Remove-Item -LiteralPath $exePath -Force -ErrorAction SilentlyContinue
}
