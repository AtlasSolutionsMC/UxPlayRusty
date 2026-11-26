param(
    [switch] $Record,
    [string] $ReferenceRoot,
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\gcc.exe')
)

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $ReferenceRoot) {
    $ReferenceRoot = Join-Path (Split-Path -Parent $repo) 'UxPlay'
}
$source = (Resolve-Path $ReferenceRoot).Path
$gcc = $Compiler
$executable = Join-Path $env:TEMP 'uxplay_compat_oracle.exe'
$actual = Join-Path $env:TEMP 'uxplay_compat_oracle.actual.txt'
$expected = Join-Path $PSScriptRoot 'compat_oracle.expected.txt'

& $gcc -DWIN32 -I (Join-Path $source 'lib') `
    (Join-Path $PSScriptRoot 'compat_oracle.c') `
    (Join-Path $source 'lib\compat.c') -o $executable
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& $executable | Set-Content -LiteralPath $actual -Encoding ascii
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if ($Record) {
    Copy-Item -LiteralPath $actual -Destination $expected -Force
    Write-Output "Recorded $((Get-Content -LiteralPath $expected).Count) C oracle rows."
    exit 0
}

if (-not (Test-Path -LiteralPath $expected)) {
    Write-Error 'Fixture missing; run with -Record once to create it.'
    exit 1
}

$expectedText = [IO.File]::ReadAllText($expected)
$actualText = [IO.File]::ReadAllText($actual)
if ($actualText -cne $expectedText) {
    Write-Error "C oracle differs from $expected."
    Compare-Object ($expectedText -split "`n") ($actualText -split "`n")
    exit 1
}

Write-Output "C compat oracle matches $((Get-Content -LiteralPath $expected).Count) rows."
