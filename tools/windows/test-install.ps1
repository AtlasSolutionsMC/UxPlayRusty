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
    $launchLogToml = $launchLog.Replace('\', '/')
    Set-Content -LiteralPath $launchConfig -Encoding utf8 -Value @(
        '[server]'
        'name = "UxPlay-Install-Smoke"'
        ''
        '[ble]'
        'enabled = false'
        ''
        '[logging]'
        'level = "debug"'
        "file = `"$launchLogToml`""
    )
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $appPath
    $startInfo.WorkingDirectory = $install
    $startInfo.UseShellExecute = $false
    $startInfo.Environment['UXPLAY_CONFIG'] = $launchConfig
    $guiProcess = [Diagnostics.Process]::Start($startInfo)
    try {
        Start-Sleep -Seconds 8
        $guiProcess.Refresh()
        if ($guiProcess.HasExited) {
            throw "Installed tray app exited during launch smoke (exit $($guiProcess.ExitCode))."
        }
        if (-not (Test-Path -LiteralPath $launchLog)) {
            throw 'Installed tray app did not create its isolated launch log.'
        }
        $startupFailure = Select-String -LiteralPath $launchLog -Pattern 'UxPlay Rust: server startup failed:'
        if ($startupFailure) {
            throw "Installed tray app could not start the AirPlay server: $($startupFailure.Line)"
        }
        Write-Output 'PASS installed tray app starts the AirPlay server'
    } finally {
        $guiProcess.Refresh()
        if (-not $guiProcess.HasExited) {
            Stop-Process -Id $guiProcess.Id -Force
            $guiProcess.WaitForExit(5000) | Out-Null
        }
        $guiProcess.Dispose()
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
