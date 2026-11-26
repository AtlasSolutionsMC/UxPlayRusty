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
