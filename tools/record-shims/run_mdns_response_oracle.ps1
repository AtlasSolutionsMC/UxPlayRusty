[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [switch] $Record
)

$ErrorActionPreference = 'Stop'
$targetRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceRoot = Join-Path (Split-Path $targetRoot -Parent) 'UxPlay'
$mdnsDirectory = Join-Path $referenceRoot 'lib\mdnsd'
$oracleSource = Join-Path $PSScriptRoot 'mdns_response_oracle.c'
$expectedPath = Join-Path $PSScriptRoot 'mdns_response_oracle.expected.txt'
$executable = Join-Path $env:TEMP "mdns_response_oracle_$PID.exe"

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found: $Compiler"
}
if (-not (Test-Path -LiteralPath (Join-Path $mdnsDirectory 'mdnsd.c') -PathType Leaf)) {
    throw "Sibling UxPlay mDNS source not found under: $referenceRoot"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$(Split-Path -Parent $Compiler);$oldPath"
    $compileArguments = @(
        '-std=c11', '-DWIN32', '-ffunction-sections', '-fdata-sections',
        '-Wno-unused-function', '-Wno-unused-variable', "-I$mdnsDirectory",
        $oracleSource, '-Wl,--gc-sections', '-lws2_32', '-liphlpapi',
        '-o', $executable
    )
    & $Compiler @compileArguments
    if ($LASTEXITCODE -ne 0) {
        throw "mDNS response C oracle compilation failed with exit code $LASTEXITCODE."
    }

    $actual = @(& $executable)
    if ($LASTEXITCODE -ne 0) {
        throw "mDNS response C oracle exited with code $LASTEXITCODE."
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $executable) {
        Remove-Item -LiteralPath $executable -Force
    }
}

if ($Record) {
    $actual | Set-Content -LiteralPath $expectedPath -Encoding utf8NoBOM
    Write-Output "Recorded $($actual.Count) source mDNS response fixtures."
    return
}
if (-not (Test-Path -LiteralPath $expectedPath -PathType Leaf)) {
    throw "mDNS response fixtures are missing; use -Record to create them."
}
$expected = @(Get-Content -LiteralPath $expectedPath)
if ($actual.Count -ne $expected.Count) {
    throw "mDNS response oracle row count mismatch: got $($actual.Count), expected $($expected.Count)."
}
for ($index = 0; $index -lt $actual.Count; $index++) {
    if ($actual[$index] -cne $expected[$index]) {
        throw "mDNS response oracle fixture mismatch at row ${index}: $($actual[$index])"
    }
}
Write-Output "C mDNS response oracle matches $($expected.Count) recorded rows."
