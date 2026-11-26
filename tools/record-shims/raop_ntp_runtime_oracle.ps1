$ErrorActionPreference = 'Stop'

$compiler = $env:UXPLAY_C_COMPILER
if ([string]::IsNullOrWhiteSpace($compiler)) {
    $compiler = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
}
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
    throw "C compiler not found at '$compiler'. Set UXPLAY_C_COMPILER to a C compiler path."
}

$targetRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceLib = Join-Path (Split-Path -Parent $targetRoot) 'UxPlay\lib'
$source = Join-Path $PSScriptRoot 'raop_ntp_runtime_oracle.c'
$byteutils = Join-Path $referenceLib 'byteutils.c'
$expectedPath = Join-Path $PSScriptRoot 'raop_ntp_runtime_oracle.expected.txt'
$executable = Join-Path $env:TEMP "raop_ntp_runtime_oracle_$PID.exe"
$oldPath = $env:PATH

try {
    $env:PATH = "$(Split-Path -Parent $compiler);$oldPath"
    & $compiler -std=c11 -Wall -Wextra -Werror -I $referenceLib $source $byteutils -lws2_32 -o $executable
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle compilation failed with exit code $LASTEXITCODE."
    }

    $actual = @(& $executable)
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle exited with code $LASTEXITCODE."
    }
    $expected = @(Get-Content -LiteralPath $expectedPath | Where-Object { $_.Trim() })
    if ($actual.Count -ne $expected.Count) {
        throw "C oracle row count mismatch: got $($actual.Count), expected $($expected.Count)."
    }
    for ($index = 0; $index -lt $expected.Count; $index++) {
        if ($actual[$index] -cne $expected[$index]) {
            throw "C oracle mismatch at row ${index}: '$($actual[$index])' vs '$($expected[$index])'."
        }
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $executable -PathType Leaf) {
        Remove-Item -LiteralPath $executable -Force
    }
}

Write-Output "C byteutils timing packet behavior matches $($expected.Count) fixtures."
