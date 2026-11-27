param(
    [string]$BeaconExecutable = (Join-Path (Split-Path $PSScriptRoot -Parent | Split-Path -Parent) 'rust\beacon\target\release\uxplay-beacon.exe'),

    [string]$Ipv4Address = '192.0.2.18',

    [ValidateRange(100, 65535)]
    [int]$AirPlayPort = 7101,

    [ValidateRange(1, 120)]
    [int]$StartupTimeoutSeconds = 20,

    [ValidateRange(1, 120)]
    [int]$ShutdownTimeoutSeconds = 12
)

$ErrorActionPreference = 'Stop'
$beacon = (Resolve-Path -LiteralPath $BeaconExecutable).Path
$tempRoot = (Resolve-Path -LiteralPath $env:TEMP).Path
$scratch = Join-Path $tempRoot ('uxplay-winrt-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
$handoff = Join-Path $scratch 'handoff.ble'
$stdout = Join-Path $scratch 'beacon.stdout.log'
$stderr = Join-Path $scratch 'beacon.stderr.log'
$process = $null

try {
    $ownerPid = [uint32]$PID
    $ownerName = (Get-Process -Id $PID).ProcessName + '.exe'
    $handoffBytes = [System.Collections.Generic.List[byte]]::new()
    $handoffBytes.AddRange([BitConverter]::GetBytes([uint16]$AirPlayPort))
    $handoffBytes.AddRange([BitConverter]::GetBytes($ownerPid))
    $handoffBytes.AddRange([Text.Encoding]::ASCII.GetBytes($ownerName))
    [IO.File]::WriteAllBytes($handoff, $handoffBytes.ToArray())

    $arguments = @(
        '--ipv4', $Ipv4Address,
        '--path', "`"$handoff`"",
        '--exit-when-stopped'
    )
    $process = Start-Process `
        -FilePath $beacon `
        -ArgumentList $arguments `
        -WindowStyle Hidden `
        -PassThru `
        -RedirectStandardOutput $stdout `
        -RedirectStandardError $stderr

    $startupDeadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $registered = $false
    while ([DateTime]::UtcNow -lt $startupDeadline) {
        $process.Refresh()
        $log = if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout -Raw } else { '' }
        if ($log -match ' registered') {
            $registered = $true
            break
        }
        if ($process.HasExited -or $log -match 'did not reach STOPPED|could not initialize the WinRT MTA|Failed to start beacon') {
            break
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $registered) {
        if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout }
        if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr }
        $failure = if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout -Raw } else { '' }
        if ($failure -match 'did not reach STOPPED') {
            throw 'WinRT beacon preflight failed: Stop returned, but the publisher remained STARTED.'
        }
        throw 'Beacon did not start advertising before the startup timeout.'
    }

    Remove-Item -LiteralPath $handoff -Force
    $shutdownDeadline = [DateTime]::UtcNow.AddSeconds($ShutdownTimeoutSeconds)
    while ([DateTime]::UtcNow -lt $shutdownDeadline) {
        $process.Refresh()
        if ($process.HasExited) { break }
        Start-Sleep -Milliseconds 250
    }
    $process.Refresh()

    if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout }
    if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr }
    if (-not $process.HasExited) {
        throw 'Beacon remained alive after handoff removal and the shutdown timeout.'
    }
    if ($process.ExitCode -ne 0) {
        throw "Beacon exited with code $($process.ExitCode) after handoff removal."
    }

    Write-Output "PASS WinRT beacon advertised and exited cleanly after handoff removal (PID $($process.Id))."
} finally {
    if (Test-Path -LiteralPath $handoff) {
        Remove-Item -LiteralPath $handoff -Force
    }
    if ($process) {
        $process.Refresh()
        if (-not $process.HasExited) {
            $live = Get-CimInstance Win32_Process -Filter "ProcessId = $($process.Id)"
            if ($live -and $live.ExecutablePath -eq $beacon) {
                Stop-Process -Id $process.Id -Force
                Wait-Process -Id $process.Id -Timeout 5 -ErrorAction SilentlyContinue
            }
        }
    }

    $tempPrefix = [IO.Path]::GetFullPath($tempRoot) + [IO.Path]::DirectorySeparatorChar
    $scratchFull = [IO.Path]::GetFullPath($scratch)
    if ($scratchFull.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $scratchFull -Recurse -Force
    } else {
        throw "Refusing to remove test scratch directory outside TEMP: $scratchFull"
    }
}
