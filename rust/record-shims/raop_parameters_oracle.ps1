[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [switch] $Record
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found at '$Compiler'. Pass -Compiler with a C11 compiler path."
}

$oldPath = $env:PATH
$env:PATH = "$(Split-Path -Parent $Compiler);$oldPath"
$source = Join-Path $PSScriptRoot 'raop_parameters_oracle.c'
$expected = Join-Path $PSScriptRoot 'raop_parameters_oracle.expected.txt'
$executable = Join-Path $env:TEMP "raop_parameters_oracle_$PID.exe"

try {
    & $Compiler -std=c11 -O2 -Wall -Wextra -Werror $source '-o' $executable
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle compilation failed with exit code $LASTEXITCODE."
    }

    $lines = @(& $executable)
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle exited with code $LASTEXITCODE."
    }
    if ($Record) {
        $lines | Set-Content -LiteralPath $expected -Encoding utf8NoBOM
        Write-Output "Recorded $($lines.Count) GET/SET_PARAMETER libc fixtures."
    } else {
        if (-not (Test-Path -LiteralPath $expected -PathType Leaf)) {
            throw "Expected fixture not found: $expected. Use -Record only to create it intentionally."
        }
        $recorded = @(Get-Content -LiteralPath $expected)
        if ($lines.Count -ne $recorded.Count) {
            throw "GET/SET_PARAMETER oracle row count mismatch: got $($lines.Count), expected $($recorded.Count)."
        }
        for ($index = 0; $index -lt $lines.Count; $index++) {
            if ($lines[$index] -cne $recorded[$index]) {
                throw "GET/SET_PARAMETER oracle mismatch at row ${index}: got '$($lines[$index])', expected '$($recorded[$index])'."
            }
        }
        Write-Output "C GET/SET_PARAMETER oracle matches $($recorded.Count) recorded rows."
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $executable) {
        Remove-Item -LiteralPath $executable -Force
    }
}

