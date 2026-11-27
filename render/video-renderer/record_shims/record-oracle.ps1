$ErrorActionPreference = 'Stop'
$crateRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$originalRoot = (Resolve-Path (Join-Path $crateRoot '..\..\..\UxPlay')).Path
$env:Path = "$env:LOCALAPPDATA\Programs\MSYS2\ucrt64\bin;$env:Path"
$pkg = (& pkg-config --cflags --libs gstreamer-1.0 gstreamer-app-1.0 gstreamer-video-1.0 glib-2.0) -split '\s+' | Where-Object { $_ }
$oracle = Join-Path $PSScriptRoot 'video_oracle.exe'

& gcc -std=c11 -D_GNU_SOURCE -DWIN32 `
    -I (Join-Path $originalRoot 'renderers') `
    -I (Join-Path $originalRoot 'lib') `
    (Join-Path $PSScriptRoot 'video_oracle.c') `
    (Join-Path $originalRoot 'lib\logger.c') `
    @pkg -o $oracle
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the reference video renderer oracle.' }

$cases = @(
    @{ Name = 'h264-default'; Args = @('0', '0', '1', '0', '0', '-', '-') },
    @{ Name = 'mirror-h265-jpeg-flip'; Args = @('1', '1', '1', '3', '1', '-', '-') },
    @{ Name = 'rtp-h265-jpeg'; Args = @('1', '1', '0', '0', '0', 'pt=96 ! fakesink', '-') },
    @{ Name = 'sink-options'; Args = @('0', '0', '0', '0', '0', '-', ' qos=true') }
)

$expected = [System.Collections.Generic.List[string]]::new()
foreach ($case in $cases) {
    $lines = & $oracle @($case.Args)
    if ($LASTEXITCODE -ne 0) { throw "Reference oracle failed for $($case.Name)." }
    foreach ($line in $lines) {
        if ($line -match '^C_ORACLE_PIPELINE_(\d+)\t(.*)$') {
            $expected.Add("$($case.Name)|$($Matches[1])|$($Matches[2])")
        }
    }
}
Set-Content -LiteralPath (Join-Path $PSScriptRoot 'video_oracle_expected.txt') -Value $expected -Encoding UTF8
$flagsLine = & $oracle '0' '0' '1' '0' '0' '-' '-' 'https://example.invalid/stream.m3u8' |
    Where-Object { $_ -match '^C_ORACLE_FLAGS=(-?\d+)$' } | Select-Object -First 1
if (-not $flagsLine) { throw 'Reference oracle did not report the playbin flags.' }
$flagValue = [regex]::Match($flagsLine, '^C_ORACLE_FLAGS=(-?\d+)$').Groups[1].Value
$flagValue | Set-Content -LiteralPath (Join-Path $PSScriptRoot 'video_oracle_flags.txt') -Encoding ASCII
Write-Output "Recorded $($expected.Count) reference pipeline descriptions."
