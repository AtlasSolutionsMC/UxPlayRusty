$ErrorActionPreference = 'Stop'

$compiler = $env:UXPLAY_C_COMPILER
if ([string]::IsNullOrWhiteSpace($compiler)) {
    $compiler = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
}
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
    throw "C compiler not found at '$compiler'. Set UXPLAY_C_COMPILER to a C compiler path."
}

$targetRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$referenceRoot = Join-Path (Split-Path -Parent $targetRoot) 'UxPlay'
$referenceLib = Join-Path $referenceRoot 'lib'
$source = Join-Path $PSScriptRoot 'netutils_oracle.c'
$expectedPath = Join-Path $PSScriptRoot 'netutils_oracle.expected.txt'
$executable = Join-Path $env:TEMP "netutils_oracle_$PID.exe"
$compilerDirectory = Split-Path -Parent $compiler
$oldPath = $env:PATH

try {
    $env:PATH = "$compilerDirectory;$oldPath"
    $compileArgs = @(
        '-std=c11', '-O2', '-DWIN32', '-I', $referenceLib,
        $source, (Join-Path $referenceLib 'netutils.c'),
        '-lws2_32', '-o', $executable
    )
    & $compiler @compileArgs
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

Write-Output "C netutils behavior matches $($expected.Count) fixtures."
