param(
    [switch]$Record,
    [string]$ReferenceRoot = (Join-Path $PSScriptRoot '..\..\..\..\UxPlay')
)
$ErrorActionPreference = 'Stop'
$referenceRoot = (Resolve-Path $ReferenceRoot).Path
$sourcePath = Join-Path $referenceRoot 'lib\raop_rtp.c'
$source = Get-Content -LiteralPath $sourcePath -Raw
$signature = 'static uint64_t rtp_time_to_client_ntp('
$start = $source.IndexOf($signature, [StringComparison]::Ordinal)
if ($start -lt 0) { throw 'Could not locate the source RTP time conversion function' }
$openBrace = $source.IndexOf('{', $start)
if ($openBrace -lt 0) { throw 'Could not locate the source RTP time function body' }
$depth = 0
$end = -1
for ($index = $openBrace; $index -lt $source.Length; $index++) {
    if ($source[$index] -eq '{') { $depth++ }
    elseif ($source[$index] -eq '}') {
        $depth--
        if ($depth -eq 0) { $end = $index + 1; break }
    }
}
if ($end -lt 0) { throw 'Could not find the end of the source RTP time function' }
$functionText = $source.Substring($start, $end - $start)
$code = @"
#include <stdbool.h>
#include <stdint.h>
#include <limits.h>
#include <stdio.h>
typedef struct raop_rtp_s {
    bool initial_sync;
    uint32_t rtp_sync;
    uint64_t client_ntp_sync;
    double rtp_clock_rate;
} raop_rtp_t;
$functionText
int main(void) {
    raop_rtp_t session = {0};
    session.initial_sync = true;
    session.rtp_clock_rate = (double)(1000000000U / 44100U);
    session.rtp_sync = UINT32_MAX - 2U;
    session.client_ntp_sync = UINT64_C(123456789);
    printf("wrap-forward,%llu\n", (unsigned long long)rtp_time_to_client_ntp(&session, 3U));
    session.rtp_sync = 3U;
    session.client_ntp_sync = UINT64_C(1000);
    printf("wrap-negative-clamp,%llu\n", (unsigned long long)rtp_time_to_client_ntp(&session, UINT32_MAX - 2U));
    session.initial_sync = false;
    session.client_ntp_sync = UINT64_C(77);
    printf("before-sync,%llu\n", (unsigned long long)rtp_time_to_client_ntp(&session, 42U));
    return 0;
}
"@
$tempC = Join-Path $env:TEMP "uxplay_rtp_time_oracle_$PID.c"
$binary = Join-Path $env:TEMP "uxplay_rtp_time_oracle_$PID.exe"
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
if (Test-Path (Join-Path $toolchain 'bin\gcc.exe')) {
    $env:Path = "$(Join-Path $toolchain 'bin');$env:Path"
}
try {
    Set-Content -LiteralPath $tempC -Value $code -Encoding ascii
    & gcc -std=c11 -Wall -Wextra -Werror $tempC -o $binary
    if ($LASTEXITCODE -ne 0) { throw "gcc exited with $LASTEXITCODE" }
    $actual = & $binary
    if ($LASTEXITCODE -ne 0) { throw "oracle exited with $LASTEXITCODE" }
    $fixture = Join-Path $PSScriptRoot 'rtp_time.expected.txt'
    if ($Record) {
        Set-Content -LiteralPath $fixture -Value $actual -Encoding ascii
    } else {
        $expected = Get-Content -LiteralPath $fixture
        $delta = Compare-Object -ReferenceObject $expected -DifferenceObject $actual
        if ($delta) { $delta | Format-Table | Out-String | Write-Error }
        Write-Output "raop_rtp time oracle matches $($expected.Count) rows"
    }
} finally {
    Remove-Item -LiteralPath $tempC -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $binary -Force -ErrorAction SilentlyContinue
}
