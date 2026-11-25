$ErrorActionPreference = 'Stop'
$cc = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
$bin = Split-Path $cc
$source = Join-Path $PSScriptRoot 'fcup_request_oracle.c'
$referenceInclude = 'D:\Save\Uxplay\UxPlay\lib'
$responseSource = Join-Path $referenceInclude 'http_response.c'
$exe = Join-Path $env:TEMP "fcup_request_oracle_$PID.exe"
$expected = Join-Path $PSScriptRoot 'fcup_request.expected.xml'
$expectedRequest = Join-Path $PSScriptRoot 'fcup_request.expected.bin'

if (-not (Test-Path -LiteralPath $cc)) {
    throw "UCRT64 compiler not found at $cc"
}

$oldPath = $env:PATH
try {
    $env:PATH = "$bin;$oldPath"
    $compileArgs = @('-std=c11', '-Wall', '-Wextra', '-Werror', '-Wno-error=sign-compare', '-DWIN32', '-I', $referenceInclude, $source, $responseSource, '-lplist-2.0', '-o', $exe)
    & $cc @compileArgs
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle compile failed with exit code $LASTEXITCODE"
    }

    & $exe $expected $expectedRequest
    if ($LASTEXITCODE -ne 0) {
        throw "C oracle failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:PATH = $oldPath
    if (Test-Path -LiteralPath $exe) {
        Remove-Item -LiteralPath $exe -Force
    }
}

Write-Output "Recorded original create_fcup_request() XML at $expected and reverse-HTTP request at $expectedRequest"
