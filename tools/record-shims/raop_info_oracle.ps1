[CmdletBinding()]
param(
    [switch] $Record
)

$ErrorActionPreference = 'Stop'
$cc = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
$ccDir = Split-Path -Parent $cc
$source = Join-Path $PSScriptRoot 'raop_info_oracle.c'
$exe = Join-Path $env:TEMP "raop_info_oracle_$PID.exe"
$expected = Join-Path $PSScriptRoot 'raop_info_oracle.expected.txt'

if (-not (Test-Path -LiteralPath $cc -PathType Leaf)) {
    throw "UCRT64 compiler not found at $cc"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$ccDir;$oldPath"
    & $cc -std=c11 -Wall -Wextra -Werror $source '-lplist-2.0' '-o' $exe
    if ($LASTEXITCODE -ne 0) {
        throw "RAOP info oracle compile failed with exit code $LASTEXITCODE"
    }
    $actual = @(& $exe)
    if ($LASTEXITCODE -ne 0) {
        throw "RAOP info oracle exited with code $LASTEXITCODE"
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $exe) {
        Remove-Item -LiteralPath $exe -Force
    }
}

if ($Record) {
    $actual | Set-Content -LiteralPath $expected -Encoding utf8NoBOM
    Write-Output "Recorded RAOP info libplist fixtures at $expected"
    return
}

if (-not (Test-Path -LiteralPath $expected -PathType Leaf)) {
    throw "Expected fixture file not found at $expected; use -Record to create it"
}

$expectedRows = @(Get-Content -LiteralPath $expected)
if ($actual.Count -ne $expectedRows.Count) {
    throw "RAOP info oracle row count differs: C=$($actual.Count), fixture=$($expectedRows.Count)"
}

for ($index = 0; $index -lt $actual.Count; $index++) {
    if ($actual[$index] -cne $expectedRows[$index]) {
        throw "RAOP info oracle differs at row $($index + 1): C='$($actual[$index])', fixture='$($expectedRows[$index])'"
    }
}

Write-Output "RAOP info oracle matches $($actual.Count) libplist fixture rows"
