[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [string] $OutputPath = (Join-Path $env:TEMP 'uxplay-http-request-target-byte-oracle.exe')
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$upstreamRoot = Join-Path (Split-Path $repoRoot -Parent) 'UxPlay'
$sourceLib = Join-Path $upstreamRoot 'lib'
$llhttpDir = Join-Path $sourceLib 'llhttp'
$oracle = Join-Path $PSScriptRoot 'http_request_target_byte_oracle.c'
$expected = Join-Path $PSScriptRoot 'http_request_target_byte_oracle.expected.tsv'
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found: $Compiler. Pass -Compiler with the path to MSYS2 UCRT64 cc.exe."
}
if (-not (Test-Path -LiteralPath (Join-Path $sourceLib 'http_request.c') -PathType Leaf)) {
    throw "Sibling UxPlay source not found under: $upstreamRoot"
}

$env:PATH = "$(Split-Path -Parent $Compiler);$env:PATH"
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

& $outputFullPath | Set-Content -LiteralPath $expected -Encoding utf8
if ($LASTEXITCODE -ne 0) {
    throw "C oracle execution failed with exit code $LASTEXITCODE"
}
Write-Output "Wrote source-derived parser classifications to $expected"
