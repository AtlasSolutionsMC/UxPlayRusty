param([switch] $Record)

$ErrorActionPreference = 'Stop'
$targetRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceRoot = Join-Path (Split-Path -Parent $targetRoot) 'UxPlay'
$compiler = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
$library = Join-Path $referenceRoot 'lib'
$source = Join-Path $PSScriptRoot 'logger_oracle.c'
$referenceSource = Join-Path $library 'logger.c'
$expected = Join-Path $PSScriptRoot 'logger_oracle.expected.txt'
$executable = Join-Path $env:TEMP "uxplay_logger_oracle_$PID.exe"

if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
    throw "UCRT64 compiler not found: $compiler"
}
if (-not (Test-Path -LiteralPath $referenceSource -PathType Leaf)) {
    throw "Reference logger not found: $referenceSource"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$(Split-Path -Parent $compiler);$oldPath"
    & $compiler '-std=c11' '-O2' '-Wall' '-Wextra' '-Werror' '-pthread' '-DWIN32' `
        '-I' $library $source $referenceSource '-o' $executable
    if ($LASTEXITCODE -ne 0) {
        throw "Logger C oracle compilation failed with exit code $LASTEXITCODE."
    }

    $actual = @(& $executable)
    if ($LASTEXITCODE -ne 0) {
        throw "Logger C oracle exited with code $LASTEXITCODE."
    }
    if ($Record) {
        $actual | Set-Content -LiteralPath $expected -Encoding utf8NoBOM
        Write-Output "Recorded $($actual.Count) C logger events in $expected"
        return
    }

    $recorded = @(Get-Content -LiteralPath $expected)
    if (($actual -join "`n") -cne ($recorded -join "`n")) {
        throw "C logger output differs from the recorded $($recorded.Count)-event fixture."
    }
    Write-Output "C logger oracle matches $($actual.Count) recorded events."
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $executable -PathType Leaf) {
        Remove-Item -LiteralPath $executable -Force
    }
}
