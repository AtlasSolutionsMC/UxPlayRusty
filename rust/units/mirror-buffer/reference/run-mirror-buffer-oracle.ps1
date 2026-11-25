param(
    [switch]$Record,
    [string]$ReferenceRoot = (Join-Path $PSScriptRoot '..\..\..\..\..\UxPlay')
)
$ErrorActionPreference = 'Stop'
$referenceRoot = (Resolve-Path $ReferenceRoot).Path
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path (Join-Path $toolchain 'bin\gcc.exe')) {
    $env:Path = "$(Join-Path $toolchain 'bin');$env:Path"
}
$oracle = Join-Path $PSScriptRoot 'mirror_buffer_oracle.c'
$binary = Join-Path $PSScriptRoot 'mirror_buffer_oracle.exe'
$include = Join-Path $referenceRoot 'lib'
$sourceMirror = Join-Path $referenceRoot 'lib\mirror_buffer.c'
$sourceCrypto = Join-Path $referenceRoot 'lib\crypto.c'
& gcc -std=c11 -DWIN32 -Wall -Wextra -Werror "-I$include" $oracle $sourceMirror $sourceCrypto -lcrypto -lbcrypt -o $binary
if ($LASTEXITCODE -ne 0) { throw "gcc exited with $LASTEXITCODE" }
$actual = & $binary
if ($LASTEXITCODE -ne 0) { throw "oracle exited with $LASTEXITCODE" }
$fixture = Join-Path $PSScriptRoot 'mirror_buffer.expected.txt'
if ($Record) {
    Set-Content -LiteralPath $fixture -Value $actual -Encoding ascii
} else {
    $expected = Get-Content -LiteralPath $fixture
    $delta = Compare-Object -ReferenceObject $expected -DifferenceObject $actual
    if ($delta) { $delta | Format-Table | Out-String | Write-Error }
    Write-Output "mirror_buffer oracle matches $($expected.Count) rows"
}
