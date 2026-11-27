param(
    [Parameter(Mandatory = $true)]
    [string]$Setup
)

$ErrorActionPreference = 'Stop'
$setupPath = (Resolve-Path $Setup).Path
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}_is1'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Test-Path $uninstallKey) { throw 'A UxPlay installation already exists for this user.' }
if (Get-Process -Name uxplay -ErrorAction SilentlyContinue) { throw 'Stop UxPlay before running the installer smoke test.' }

$tempRoot = [IO.Path]::GetFullPath($env:TEMP) + [IO.Path]::DirectorySeparatorChar
$install = Join-Path $env:TEMP ('UxPlay-Install-Smoke-' + [guid]::NewGuid().ToString('N'))
if (-not ([IO.Path]::GetFullPath($install)).StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Install test destination is outside TEMP: $install"
}
$config = Join-Path $env:APPDATA 'UxPlay\config.toml'
$handoff = Join-Path $env:APPDATA 'UxPlay\uxplay.ble'
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\UxPlay\UxPlay.lnk'
$backupRoot = Join-Path $env:TEMP ('UxPlay-Install-Backup-' + [guid]::NewGuid().ToString('N'))

function Get-BleSmokeCompanionState {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId,
        [Parameter(Mandatory = $true)]
        [string]$HandoffPath
    )
    $companion = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    $handoffExists = Test-Path -LiteralPath $HandoffPath
    if ($companion -and $handoffExists) { return 'running' }
    if (-not $companion -and -not $handoffExists) { return 'exited-cleanly' }
    return $null
}

function Write-BleSmokeExitDiagnostics {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Stage,
        [Parameter(Mandatory = $true)]
        [string]$LogPath
    )
    # The app supervisor joins the companion's stdout/stderr readers after observing exit.
    Start-Sleep -Milliseconds 500
    Write-Output "BLE companion exited before the $Stage action; relevant app log follows:"
    if (-not (Test-Path -LiteralPath $LogPath)) {
        Write-Output '<no app log>'
        return
    }
    $lines = @(Get-Content -LiteralPath $LogPath | Where-Object {
        $_ -match '(?i)BLE companion|WinRT|Bluetooth|advertis|publisher|beacon|failed|error|exited'
    } | Select-Object -Last 40)
    if ($lines.Count -eq 0) {
        Write-Output '<no BLE-related log lines>'
        return
    }
    foreach ($line in $lines) { Write-Output "BLE-LOG $line" }
}

$hadConfig = Test-Path $config
$hadHandoff = Test-Path $handoff
$hadShortcut = Test-Path $shortcut
$oldRun = (Get-ItemProperty $runKey -Name UxPlay -ErrorAction SilentlyContinue).UxPlay
New-Item -ItemType Directory -Path $backupRoot | Out-Null
if ($hadConfig) { Copy-Item $config (Join-Path $backupRoot 'config.toml') }
if ($hadHandoff) { Copy-Item $handoff (Join-Path $backupRoot 'uxplay.ble') }
if ($hadShortcut) { Copy-Item $shortcut (Join-Path $backupRoot 'UxPlay.lnk') }

try {
    $installer = Start-Process -FilePath $setupPath -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=$install", '/TASKS=autostart'
    ) -Wait -PassThru
    if ($installer.ExitCode -ne 0) { throw "Setup exited with code $($installer.ExitCode)." }

    $uninstaller = Join-Path $install 'unins000.exe'
    if (-not (Test-Path $uninstaller)) { throw 'The installed uninstaller is missing.' }
    if (-not (Test-Path $shortcut)) { throw 'The Start Menu shortcut is missing.' }
    $startup = (Get-ItemProperty $runKey -Name UxPlay -ErrorAction Stop).UxPlay
    $configText = Get-Content -LiteralPath $config -Raw
    $expectedSuffix = '\' + (Split-Path $install -Leaf) + '\uxplay.exe'
    if (-not (Test-Path -LiteralPath $startup.Trim('"')) -or
        -not $startup.Trim('"').EndsWith($expectedSuffix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Startup Run value does not point to this installation: $startup"
    }
    if ($configText -notmatch '(?m)^enabled = true\s*$') {
        throw 'The installer startup checkbox did not persist startup.enabled = true.'
    }
    Write-Output 'PASS install files, Start Menu shortcut, startup Run value, and config setting'

    $appPath = Join-Path $install 'uxplay.exe'
    $originalPath = $env:Path
    try {
        $env:Path = ($originalPath -split ';' | Where-Object {
            $_ -notmatch '(?i)(\\|/)MSYS2(\\|/)|^[A-Z]:\\msys64(\\|$)'
        }) -join ';'
        $help = Start-Process -FilePath $appPath -ArgumentList '-h' -WindowStyle Hidden -Wait -PassThru
        $helpExitCode = $help.ExitCode
    } finally {
        $env:Path = $originalPath
    }
    if ($helpExitCode -ne 0) {
        throw "Installed uxplay.exe -h failed without MSYS2 PATH (exit $helpExitCode)."
    }
    Write-Output 'PASS installed uxplay -h without MSYS2 PATH'

    $launchLog = Join-Path $backupRoot 'launch.log'
    $launchConfig = Join-Path $backupRoot 'launch-config.toml'
    $bleHandoff = Join-Path $backupRoot 'ble-startup-smoke.ble'
    $launchLogToml = $launchLog.Replace('\', '/')
    $bleHandoffToml = $bleHandoff.Replace('\', '/')
    Set-Content -LiteralPath $launchConfig -Encoding utf8 -Value @(
        '[server]'
        'name = "UxPlay-Install-Smoke"'
        ''
        '[ble]'
        'enabled = true'
        "handoff_path = `"$bleHandoffToml`""
        ''
        '[startup]'
        'enabled = true'
        ''
        '[logging]'
        'level = "debug"'
        "file = `"$launchLogToml`""
    )
    $beaconPath = Join-Path $install 'uxplay-beacon.exe'
    $disabledBeaconPath = Join-Path $install 'uxplay-beacon.exe.smoke-disabled'
    $installPrefix = [IO.Path]::GetFullPath($install).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    foreach ($candidatePath in @($beaconPath, $disabledBeaconPath)) {
        if (-not [IO.Path]::GetFullPath($candidatePath).StartsWith($installPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to move a BLE companion outside the temporary installation: $candidatePath"
        }
    }
    if (-not (Test-Path -LiteralPath $beaconPath)) { throw 'The installed BLE companion is missing.' }
    if (Test-Path -LiteralPath $disabledBeaconPath) { throw 'A stale BLE smoke marker already exists.' }
    Move-Item -LiteralPath $beaconPath -Destination $disabledBeaconPath
    try {
        $startInfo = [Diagnostics.ProcessStartInfo]::new()
        $startInfo.FileName = $appPath
        $startInfo.WorkingDirectory = $install
        $startInfo.UseShellExecute = $false
        $startInfo.Environment['UXPLAY_CONFIG'] = $launchConfig
        $guiProcess = [Diagnostics.Process]::Start($startInfo)
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(20)
            $airplayReadyAfterBleFailure = $false
            do {
                $guiProcess.Refresh()
                if ($guiProcess.HasExited) {
                    throw "Installed tray app exited during launch smoke (exit $($guiProcess.ExitCode))."
                }
                if (-not (Test-Path -LiteralPath $launchLog)) {
                    Start-Sleep -Milliseconds 250
                    continue
                }
                $logText = Get-Content -LiteralPath $launchLog -Raw
                $startupFailure = Select-String -LiteralPath $launchLog -Pattern 'UxPlay Rust: server startup failed:'
                if ($startupFailure) {
                    throw "Installed tray app could not start the AirPlay server: $($startupFailure.Line)"
                }
                $listener = @(Get-NetTCPConnection -State Listen -OwningProcess $guiProcess.Id -ErrorAction SilentlyContinue)
                $bleCompanionMissing = $logText -match 'BLE companion .* is missing; AirPlay will continue without BLE discovery'
                if ($listener.Count -gt 0 -and $bleCompanionMissing -and -not (Test-Path -LiteralPath $bleHandoff)) {
                    $airplayReadyAfterBleFailure = $true
                    break
                }
                Start-Sleep -Milliseconds 250
            } while ([DateTime]::UtcNow -lt $deadline)

            if (-not $airplayReadyAfterBleFailure) {
                throw 'AirPlay did not open a TCP listener and clean the BLE handoff after the missing-companion failure.'
            }
            Write-Output 'PASS AirPlay listener stays active when the BLE companion is missing; handoff is cleaned up'

            if (-not ('UxPlaySessionEndProbe' -as [type])) {
                Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class UxPlaySessionEndProbe {
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr FindWindow(string className, string windowName);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool PostMessage(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);
}
'@
            }
            $trayWindow = [UxPlaySessionEndProbe]::FindWindow('UxPlayRustyTrayHost', 'UxPlay')
            if ($trayWindow -eq [IntPtr]::Zero) { throw 'Could not find the installed tray host window.' }
            [uint32]$windowProcessId = 0
            [void][UxPlaySessionEndProbe]::GetWindowThreadProcessId($trayWindow, [ref]$windowProcessId)
            if ($windowProcessId -ne $guiProcess.Id) {
                throw "Tray host belongs to PID $windowProcessId instead of installed app PID $($guiProcess.Id)."
            }
            [UIntPtr]$queryResult = [UIntPtr]::Zero
            $querySent = [UxPlaySessionEndProbe]::SendMessageTimeout(
                $trayWindow, 0x0011, [UIntPtr]::Zero, [IntPtr]::Zero, 0x0002, 5000, [ref]$queryResult
            )
            if ($querySent -eq [IntPtr]::Zero -or $queryResult.ToUInt64() -ne 1) {
                throw 'Tray host did not accept WM_QUERYENDSESSION.'
            }
            if (-not [UxPlaySessionEndProbe]::PostMessage($trayWindow, 0x0016, [UIntPtr]::new(1), [IntPtr]::Zero)) {
                throw 'Could not post WM_ENDSESSION to the tray host.'
            }
            if (-not $guiProcess.WaitForExit(30000)) {
                throw 'Installed app did not shut down within 30 seconds after WM_ENDSESSION.'
            }
            if ($guiProcess.ExitCode -ne 0) {
                throw "Installed app exited with code $($guiProcess.ExitCode) after WM_ENDSESSION."
            }
            if (@(Get-NetTCPConnection -State Listen -OwningProcess $guiProcess.Id -ErrorAction SilentlyContinue).Count -gt 0) {
                throw 'AirPlay listener remained after Windows session shutdown.'
            }
            if (Test-Path -LiteralPath $bleHandoff) { throw 'BLE handoff remained after Windows session shutdown.' }
            Write-Output 'PASS Windows session shutdown exits cleanly and releases AirPlay and BLE state'
        } finally {
            $guiProcess.Refresh()
            if (-not $guiProcess.HasExited) {
                Stop-Process -Id $guiProcess.Id -Force
                $guiProcess.WaitForExit(5000) | Out-Null
            }
            $guiProcess.Dispose()
        }
    } finally {
        if (Test-Path -LiteralPath $disabledBeaconPath) {
            if (Test-Path -LiteralPath $beaconPath) {
                throw 'Refusing to restore the BLE companion over an existing installed file.'
            }
            Move-Item -LiteralPath $disabledBeaconPath -Destination $beaconPath
        }
    }

    $activeBleLog = Join-Path $backupRoot 'ble-active-launch.log'
    $activeBleConfig = Join-Path $backupRoot 'ble-active-config.toml'
    $activeBleHandoff = Join-Path $backupRoot 'ble-active.ble'
    $activeBleConfigText = @(
        '[server]'
        'name = "UxPlay-Install-BLE-Smoke"'
        ''
        '[ble]'
        'enabled = true'
        "handoff_path = `"$($activeBleHandoff.Replace('\', '/'))`""
        ''
        '[startup]'
        'enabled = true'
        ''
        '[logging]'
        'level = "debug"'
        "file = `"$($activeBleLog.Replace('\', '/'))`""
    )
    Set-Content -LiteralPath $activeBleConfig -Encoding utf8 -Value $activeBleConfigText
    $activeBleStartInfo = [Diagnostics.ProcessStartInfo]::new()
    $activeBleStartInfo.FileName = $appPath
    $activeBleStartInfo.WorkingDirectory = $install
    $activeBleStartInfo.UseShellExecute = $false
    $activeBleStartInfo.Environment['UXPLAY_CONFIG'] = $activeBleConfig
    $activeBleProcess = [Diagnostics.Process]::Start($activeBleStartInfo)
    try {
        $activeBleDeadline = [DateTime]::UtcNow.AddSeconds(30)
        $companionPid = 0
        do {
            $activeBleProcess.Refresh()
            if ($activeBleProcess.HasExited) {
                throw "Installed tray app exited during BLE companion launch (exit $($activeBleProcess.ExitCode))."
            }
            if (Test-Path -LiteralPath $activeBleLog) {
                $activeBleText = Get-Content -LiteralPath $activeBleLog -Raw
                $activeBleFailure = Select-String -LiteralPath $activeBleLog -Pattern 'UxPlay Rust: server startup failed:'
                if ($activeBleFailure) {
                    throw "Installed tray app could not start with BLE enabled: $($activeBleFailure.Line)"
                }
                $companionStart = [regex]::Match($activeBleText, 'Started Windows BLE companion \(pid (?<pid>\d+)\)\.')
                if ($companionStart.Success) { $companionPid = [int]$companionStart.Groups['pid'].Value }
            }
            if ($companionPid -ne 0 -and
                @(Get-NetTCPConnection -State Listen -OwningProcess $activeBleProcess.Id -ErrorAction SilentlyContinue).Count -gt 0) {
                break
            }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $activeBleDeadline)

        if ($companionPid -eq 0) {
            throw 'The installed app did not launch its packaged BLE companion.'
        }
        $initialCompanionStateDeadline = [DateTime]::UtcNow.AddSeconds(10)
        $initialCompanionState = $null
        do {
            $initialCompanionState = Get-BleSmokeCompanionState -ProcessId $companionPid -HandoffPath $activeBleHandoff
            if ($initialCompanionState) { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $initialCompanionStateDeadline)
        if (-not $initialCompanionState) {
            $activeBleText = if (Test-Path -LiteralPath $activeBleLog) { Get-Content -LiteralPath $activeBleLog -Raw } else { '<no app log>' }
            throw "BLE companion process and handoff did not settle to a consistent state: $activeBleText"
        }
        Write-Output "PASS installed app launches packaged BLE companion (PID $companionPid); state=$initialCompanionState with AirPlay listening"
        if ($initialCompanionState -eq 'exited-cleanly') {
            Write-BleSmokeExitDiagnostics -Stage 'tray Config' -LogPath $activeBleLog
        }

        $activeTrayWindow = [UxPlaySessionEndProbe]::FindWindow('UxPlayRustyTrayHost', 'UxPlay')
        if ($activeTrayWindow -eq [IntPtr]::Zero) { throw 'Could not find tray host during active BLE shutdown smoke.' }
        [uint32]$activeWindowProcessId = 0
        [void][UxPlaySessionEndProbe]::GetWindowThreadProcessId($activeTrayWindow, [ref]$activeWindowProcessId)
        if ($activeWindowProcessId -ne $activeBleProcess.Id) {
            throw "Active BLE tray window belongs to PID $activeWindowProcessId instead of $($activeBleProcess.Id)."
        }
        if (-not [UxPlaySessionEndProbe]::PostMessage($activeTrayWindow, 0x800B, [UIntPtr]::new(1001), [IntPtr]::Zero)) {
            throw 'Could not dispatch the tray Config command.'
        }
        $configEditorDeadline = [DateTime]::UtcNow.AddSeconds(10)
        $configEditor = $null
        do {
            $configPattern = [regex]::Escape($activeBleConfig)
            $configEditor = Get-CimInstance Win32_Process -Filter "Name = 'notepad.exe'" -ErrorAction SilentlyContinue |
                Where-Object { $_.CommandLine -match $configPattern } | Select-Object -First 1
            if ($configEditor) { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $configEditorDeadline)
        if (-not $configEditor) { throw 'Tray Config did not open the active BLE config in Notepad.' }
        Stop-Process -Id $configEditor.ProcessId -Force
        Write-Output 'PASS tray Config opens the active user config in Notepad'

        if (-not [UxPlaySessionEndProbe]::PostMessage($activeTrayWindow, 0x800B, [UIntPtr]::new(1002), [IntPtr]::Zero)) {
            throw 'Could not dispatch the tray Restart command.'
        }
        $restartDeadline = [DateTime]::UtcNow.AddSeconds(30)
        $restartedCompanionPid = 0
        $restartedCompanionState = $null
        do {
            $activeBleProcess.Refresh()
            if ($activeBleProcess.HasExited) {
                throw "Installed tray app exited during Restart (exit $($activeBleProcess.ExitCode))."
            }
            if (Test-Path -LiteralPath $activeBleLog) {
                $activeBleText = Get-Content -LiteralPath $activeBleLog -Raw
                $companionStarts = [regex]::Matches($activeBleText, 'Started Windows BLE companion \(pid (?<pid>\d+)\)\.')
                if ($companionStarts.Count -ge 2) {
                    $restartedCompanionPid = [int]$companionStarts[$companionStarts.Count - 1].Groups['pid'].Value
                    $listenerAfterRestart = @(Get-NetTCPConnection -State Listen -OwningProcess $activeBleProcess.Id -ErrorAction SilentlyContinue)
                    if ($listenerAfterRestart.Count -gt 0) {
                        $restartedCompanionState = Get-BleSmokeCompanionState -ProcessId $restartedCompanionPid -HandoffPath $activeBleHandoff
                        if ($restartedCompanionState) { break }
                    }
                }
            }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $restartDeadline)
        if ($restartedCompanionPid -eq 0 -or -not $restartedCompanionState) {
            $activeBleText = if (Test-Path -LiteralPath $activeBleLog) { Get-Content -LiteralPath $activeBleLog -Raw } else { '<no app log>' }
            throw "Tray Restart did not reach a consistent second companion state: $activeBleText"
        }
        $oldChildExitDeadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            $oldCompanion = Get-Process -Id $companionPid -ErrorAction SilentlyContinue
            if ($companionPid -eq $restartedCompanionPid) { $oldCompanion = $null }
            if (-not $oldCompanion) { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $oldChildExitDeadline)
        if ($oldCompanion) { throw "Previous BLE companion process $companionPid remained after Restart." }
        $companionPid = $restartedCompanionPid
        Write-Output "PASS tray Restart manages second BLE companion (PID $companionPid, state=$restartedCompanionState) and keeps AirPlay listening"
        if ($restartedCompanionState -eq 'exited-cleanly') {
            Write-BleSmokeExitDiagnostics -Stage 'tray Exit' -LogPath $activeBleLog
        }

        if (-not [UxPlaySessionEndProbe]::PostMessage($activeTrayWindow, 0x800B, [UIntPtr]::new(1004), [IntPtr]::Zero)) {
            throw 'Could not dispatch the tray Exit command.'
        }
        if (-not $activeBleProcess.WaitForExit(30000)) {
            throw 'Installed app did not shut down within 30 seconds after tray Exit with BLE active.'
        }
        if ($activeBleProcess.ExitCode -ne 0) {
            throw "Installed app exited with code $($activeBleProcess.ExitCode) after tray Exit."
        }
        $childExitDeadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            $remainingCompanion = Get-Process -Id $companionPid -ErrorAction SilentlyContinue
            if (-not $remainingCompanion) { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $childExitDeadline)
        if ($remainingCompanion) { throw "BLE companion process $companionPid remained after app shutdown." }
        if (Test-Path -LiteralPath $activeBleHandoff) { throw 'Active BLE handoff remained after app shutdown.' }
        if (@(Get-NetTCPConnection -State Listen -OwningProcess $activeBleProcess.Id -ErrorAction SilentlyContinue).Count -gt 0) {
            throw 'AirPlay listener remained after active BLE app shutdown.'
        }
        Write-Output 'PASS tray Exit leaves no packaged BLE companion or handoff and releases AirPlay'
    } finally {
        $activeBleProcess.Refresh()
        if (-not $activeBleProcess.HasExited) {
            Stop-Process -Id $activeBleProcess.Id -Force
            $activeBleProcess.WaitForExit(5000) | Out-Null
        }
        $activeBleProcess.Dispose()
    }

    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    ) -Wait -PassThru
    if ($uninstall.ExitCode -ne 0) { throw "Uninstaller exited with code $($uninstall.ExitCode)." }
    if (Test-Path $install) { throw 'Installation files remain after uninstall.' }
    if ((Get-ItemProperty $runKey -Name UxPlay -ErrorAction SilentlyContinue).UxPlay) {
        throw 'The startup Run value remains after uninstall.'
    }
    if (Test-Path $uninstallKey) { throw 'The Add/Remove Programs entry remains after uninstall.' }
    if (-not $hadShortcut -and (Test-Path $shortcut)) { throw 'The Start Menu shortcut remains after uninstall.' }
    Write-Output 'PASS uninstall files, startup cleanup, and Add/Remove Programs cleanup'
} finally {
    $installFull = [IO.Path]::GetFullPath($install)
    if (Test-Path -LiteralPath $installFull) {
        if (-not $installFull.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to clean an install directory outside TEMP: $installFull"
        }
        $cleanupUninstaller = Join-Path $installFull 'unins000.exe'
        if (Test-Path -LiteralPath $cleanupUninstaller) {
            $cleanup = Start-Process -FilePath $cleanupUninstaller -ArgumentList @(
                '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
            ) -Wait -PassThru
            if ($cleanup.ExitCode -ne 0) {
                throw "Cleanup uninstaller exited with code $($cleanup.ExitCode)."
            }
        }
        if (Test-Path -LiteralPath $installFull) {
            throw "Temporary install directory remains after cleanup: $installFull"
        }
    }
    if ($hadConfig -and (Test-Path (Join-Path $backupRoot 'config.toml'))) {
        Copy-Item (Join-Path $backupRoot 'config.toml') $config -Force
    }
    if ($hadHandoff -and (Test-Path (Join-Path $backupRoot 'uxplay.ble'))) {
        Copy-Item (Join-Path $backupRoot 'uxplay.ble') $handoff -Force
    }
    if ($hadShortcut -and (Test-Path (Join-Path $backupRoot 'UxPlay.lnk'))) {
        Copy-Item (Join-Path $backupRoot 'UxPlay.lnk') $shortcut -Force
    }
    if ($oldRun) {
        New-ItemProperty $runKey -Name UxPlay -Value $oldRun -PropertyType String -Force | Out-Null
    } else {
        Remove-ItemProperty $runKey -Name UxPlay -ErrorAction SilentlyContinue
    }
    $backupFull = [IO.Path]::GetFullPath($backupRoot)
    if ($backupFull.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $backupFull -Recurse -Force
    } else {
        throw "Refusing to remove a backup outside TEMP: $backupFull"
    }
}
