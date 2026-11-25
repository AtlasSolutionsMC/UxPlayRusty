$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceRoot = 'D:\Save\Uxplay\UxPlay'
$msysBin = 'C:\Users\Alphacomputer\AppData\Local\Programs\MSYS2\ucrt64\bin'
$cc = Join-Path $msysBin 'cc.exe'
$shimDir = $PSScriptRoot
$libDir = Join-Path $referenceRoot 'lib'
$outputDir = Join-Path $repoRoot 'rust\app\target\debug'
$cExe = Join-Path $outputDir 'raop_buffer_oracle.exe'
$rustExe = Join-Path $outputDir 'raop_buffer_rust_oracle.exe'
$expectedFile = Join-Path $shimDir 'raop_buffer_oracle.expected.txt'

if (-not (Test-Path -LiteralPath $cc)) {
    throw "MSYS2 UCRT64 C compiler not found: $cc"
}

$env:PATH = "$msysBin;$env:PATH"

$cArgs = @(
    '-std=c11',
    '-Wall',
    '-Wextra',
    '-Werror',
    '-include', (Join-Path $shimDir 'raop_buffer_test_preinclude.h'),
    "-I$libDir",
    (Join-Path $libDir 'raop_buffer.c'),
    (Join-Path $libDir 'byteutils.c'),
    (Join-Path $shimDir 'raop_buffer_test_stubs.c'),
    (Join-Path $shimDir 'raop_buffer_oracle.c'),
    '-lws2_32',
    '-o', $cExe
)

& $cc @cArgs
if ($LASTEXITCODE -ne 0) {
    throw "C oracle compilation failed with exit code $LASTEXITCODE"
}

& rustup run stable-x86_64-pc-windows-gnu rustc --edition=2024 -Awarnings `
    (Join-Path $shimDir 'raop_buffer_rust_oracle.rs') -o $rustExe
if ($LASTEXITCODE -ne 0) {
    throw "Rust oracle compilation failed with exit code $LASTEXITCODE"
}

$cTrace = @(& $cExe)
if ($LASTEXITCODE -ne 0) {
    throw "C oracle failed with exit code $LASTEXITCODE"
}

$rustTrace = @(& $rustExe)
if ($LASTEXITCODE -ne 0) {
    throw "Rust oracle failed with exit code $LASTEXITCODE"
}

$expectedTrace = @(Get-Content -LiteralPath $expectedFile)
$cDiff = Compare-Object -ReferenceObject $expectedTrace -DifferenceObject $cTrace
if ($cDiff) {
    $cDiff | Format-Table -AutoSize | Out-String | Write-Output
    throw 'The current C reference trace differs from the checked-in fixture.'
}

$rustDiff = Compare-Object -ReferenceObject $cTrace -DifferenceObject $rustTrace
if ($rustDiff) {
    $rustDiff | Format-Table -AutoSize | Out-String | Write-Output
    throw 'The Rust queue trace differs from the C reference.'
}

Write-Output "C and Rust traces match ($($cTrace.Count) lines)."
