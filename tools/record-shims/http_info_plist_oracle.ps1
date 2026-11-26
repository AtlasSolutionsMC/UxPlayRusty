$ErrorActionPreference = 'Stop'
$cc = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
$bin = Split-Path $cc
$source = Join-Path $PSScriptRoot 'http_info_plist_oracle.c'
$exe = Join-Path $env:TEMP "http_info_plist_oracle_$PID.exe"
$expected = Join-Path $PSScriptRoot 'http_info_plist_oracle.expected.txt'

if (-not (Test-Path -LiteralPath $cc)) {
    throw "UCRT64 compiler not found at $cc"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$bin;$oldPath"
    $compileArgs = @('-std=c11', '-Wall', '-Wextra', '-Werror', $source, '-lplist-2.0', '-o', $exe)
    & $cc @compileArgs
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle compile failed with exit code $LASTEXITCODE"
    }

    $actual = & $exe
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle failed with exit code $LASTEXITCODE"
    }
    $actual | Set-Content -LiteralPath $expected -Encoding utf8NoBOM
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $exe) {
        Remove-Item -LiteralPath $exe -Force
    }
}

Write-Output "Recorded libplist oracle at $expected"
