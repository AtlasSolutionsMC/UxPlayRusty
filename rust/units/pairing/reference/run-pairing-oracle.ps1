param([switch]$Record)

$source = 'D:\Save\Uxplay\UxPlay\lib'
$msys = 'C:\Users\Alphacomputer\AppData\Local\Programs\MSYS2\ucrt64'
$gcc = Join-Path $msys 'bin\gcc.exe'
$pkgConfig = Join-Path $msys 'bin\pkg-config.exe'
$exe = Join-Path $env:TEMP 'uxplay_pairing_oracle.exe'
$actual = Join-Path $env:TEMP 'uxplay_pairing_oracle.actual.txt'
$expected = Join-Path $PSScriptRoot 'pairing_oracle.expected.txt'

$env:Path = "$(Join-Path $msys 'bin');$env:Path"
$env:PKG_CONFIG_PATH = Join-Path $msys 'lib\pkgconfig'
$opensslArgs = @((& $pkgConfig --cflags --libs openssl) -split '\s+')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& $gcc -DWIN32 -I $source `
    (Join-Path $PSScriptRoot 'pairing_oracle.c') `
    (Join-Path $source 'pairing.c') `
    (Join-Path $source 'crypto.c') `
    (Join-Path $source 'srp.c') `
    (Join-Path $source 'utils.c') `
    @opensslArgs -lbcrypt -o $exe
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& $exe | Set-Content -LiteralPath $actual -Encoding ascii
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if ($Record) {
    Copy-Item -LiteralPath $actual -Destination $expected -Force
    Write-Output "Recorded $((Get-Content -LiteralPath $expected).Count) pairing C oracle rows."
    exit 0
}

if (-not (Test-Path -LiteralPath $expected)) {
    Write-Error 'Fixture missing; run with -Record once to create it.'
    exit 1
}

$expectedText = [IO.File]::ReadAllText($expected)
$actualText = [IO.File]::ReadAllText($actual)
if ($actualText -cne $expectedText) {
    Write-Error "Pairing C oracle differs from $expected."
    Compare-Object ($expectedText -split "`n") ($actualText -split "`n")
    exit 1
}

Write-Output "Pairing C oracle matches $((Get-Content -LiteralPath $expected).Count) rows."
