param(
    [string]$ReferenceRoot = (Join-Path $PSScriptRoot '..\..\..\..\UxPlay')
)
$ErrorActionPreference = 'Stop'
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path (Join-Path $toolchain 'bin\gcc.exe')) {
    $env:Path = "$toolchain\bin;$env:Path"
    $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $toolchain 'bin\gcc.exe'
}
$referenceRoot = (Resolve-Path $ReferenceRoot).Path
$buildDir = Join-Path $PSScriptRoot '..\target'
New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
$exe = Join-Path $buildDir 'stream_layout_oracle.exe'
$cOracle = Join-Path $PSScriptRoot 'stream_layout_oracle.c'
$include = Join-Path $referenceRoot 'lib'
& gcc -std=c11 -Wall -Wextra -Werror "-I$include" $cOracle -o $exe
if ($LASTEXITCODE -ne 0) { throw "C layout oracle exited with $LASTEXITCODE" }
$cRows = @(& $exe)
if ($LASTEXITCODE -ne 0) { throw "C layout oracle exited with $LASTEXITCODE" }
$rustRows = @(& cargo run --quiet --manifest-path (Join-Path $PSScriptRoot '..\Cargo.toml') --example layout)
if ($LASTEXITCODE -ne 0) { throw "Rust layout report exited with $LASTEXITCODE" }
$delta = Compare-Object -ReferenceObject $cRows -DifferenceObject $rustRows
if ($delta) { $delta | Format-Table | Out-String | Write-Error }
Write-Output "stream.h layout oracle matches $($cRows.Count) rows"
