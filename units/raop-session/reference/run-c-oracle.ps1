$ErrorActionPreference = 'Stop'
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$toolchain\bin;$env:Path"
$source = Join-Path $PSScriptRoot 'session_oracle.c'
$binary = Join-Path $PSScriptRoot 'session_oracle.exe'
& (Join-Path $toolchain 'bin\gcc.exe') -std=c99 -Wall -Wextra -Werror $source -o $binary
if ($LASTEXITCODE -ne 0) { throw "C oracle compile exited with $LASTEXITCODE" }
$actual = & $binary
if ($LASTEXITCODE -ne 0) { throw "C oracle exited with $LASTEXITCODE" }
$expected = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'session_oracle.expected.txt')
$delta = Compare-Object -ReferenceObject $expected -DifferenceObject $actual
if ($delta) { $delta | Format-Table | Out-String | Write-Error }
Write-Output "RAOP scalar C oracle matches $($expected.Count) rows"
