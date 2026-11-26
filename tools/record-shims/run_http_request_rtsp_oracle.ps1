[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [string] $OutputPath = (Join-Path $env:TEMP 'uxplay-http-request-rtsp-oracle.exe')
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$upstreamRoot = Join-Path (Split-Path $repoRoot -Parent) 'UxPlay'
$sourceLib = Join-Path $upstreamRoot 'lib'
$llhttpDir = Join-Path $sourceLib 'llhttp'
$oracle = Join-Path $PSScriptRoot 'http_request_rtsp_oracle.c'
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found: $Compiler. Pass -Compiler with the path to MSYS2 UCRT64 cc.exe."
}
if (-not (Test-Path -LiteralPath $oracle -PathType Leaf)) {
    throw "RTSP oracle source not found: $oracle"
}
if (-not (Test-Path -LiteralPath (Join-Path $sourceLib 'http_request.c') -PathType Leaf)) {
    throw "Sibling UxPlay source not found under: $upstreamRoot"
}

$compilerDir = Split-Path -Parent $Compiler
$env:PATH = "$compilerDir;$env:PATH"

$sources = @(
    $oracle
    (Join-Path $sourceLib 'http_request.c')
    (Join-Path $llhttpDir 'api.c')
    (Join-Path $llhttpDir 'http.c')
    (Join-Path $llhttpDir 'llhttp.c')
)

& $Compiler -Wall -Wextra -Wno-unused-parameter "-I$sourceLib" "-I$llhttpDir" @sources -o $outputFullPath
if ($LASTEXITCODE -ne 0) {
    throw "C oracle compilation failed with exit code $LASTEXITCODE"
}

& $outputFullPath
if ($LASTEXITCODE -ne 0) {
    throw "C oracle reported mismatches with exit code $LASTEXITCODE"
}
