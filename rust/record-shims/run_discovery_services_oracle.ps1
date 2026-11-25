[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [switch] $Record
)

$ErrorActionPreference = 'Stop'
$targetRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceRoot = Join-Path (Split-Path $targetRoot -Parent) 'UxPlay'
$libDirectory = Join-Path $referenceRoot 'lib'
$mdnsDirectory = Join-Path $libDirectory 'mdnsd'
$oracleSource = Join-Path $PSScriptRoot 'discovery_services_oracle.c'
$expectedPath = Join-Path $PSScriptRoot 'discovery_services_oracle.expected.txt'
$executable = Join-Path $env:TEMP "discovery_services_oracle_$PID.exe"

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found: $Compiler"
}
if (-not (Test-Path -LiteralPath (Join-Path $mdnsDirectory 'dnssd_mdnsd.c') -PathType Leaf)) {
    throw "Sibling UxPlay mDNS source not found under: $referenceRoot"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$(Split-Path -Parent $Compiler);$oldPath"
    $compileArguments = @(
        '-std=c11', '-DWIN32', '-ffunction-sections', '-fdata-sections',
        '-Wno-unused-function', '-Wno-unused-variable', '-Wno-unused-parameter',
        '-Wno-error=implicit-function-declaration',
        "-I$libDirectory", "-I$mdnsDirectory", $oracleSource,
        '-Wl,--gc-sections', '-lws2_32', '-liphlpapi', '-o', $executable
    )
    & $Compiler @compileArguments
    if ($LASTEXITCODE -ne 0) {
        throw "DNS-SD services C oracle compilation failed with exit code $LASTEXITCODE."
    }

    $actual = @(& $executable)
    if ($LASTEXITCODE -ne 0) {
        throw "DNS-SD services C oracle exited with code $LASTEXITCODE."
    }

    if ($Record -or -not (Test-Path -LiteralPath $expectedPath)) {
        $actual | Set-Content -LiteralPath $expectedPath -Encoding utf8NoBOM
        Write-Output "Recorded $($actual.Count) DNS-SD service fixtures."
    } else {
        $expected = @(Get-Content -LiteralPath $expectedPath)
        if ($actual.Count -ne $expected.Count) {
            throw "DNS-SD service oracle row count mismatch: got $($actual.Count), expected $($expected.Count)."
        }
        for ($index = 0; $index -lt $actual.Count; $index++) {
            if ($actual[$index] -cne $expected[$index]) {
                throw "DNS-SD service oracle fixture mismatch at row ${index}: $($actual[$index])"
            }
        }
        Write-Output "C DNS-SD service oracle matches $($expected.Count) recorded rows."
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $executable) {
        Remove-Item -LiteralPath $executable -Force
    }
}
