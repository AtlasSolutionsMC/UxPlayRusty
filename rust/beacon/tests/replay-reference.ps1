#Requires -Version 7.0

param(
    [string]$SourceRoot = $env:UXPLAY_REFERENCE_ROOT,
    [string]$Python = $env:UXPLAY_PYTHON,
    [string]$RustExecutable
)

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if ([string]::IsNullOrWhiteSpace($SourceRoot)) {
    $SourceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '..\UxPlay'))
}
if ([string]::IsNullOrWhiteSpace($Python)) {
    $pythonCommand = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($pythonCommand) {
        $Python = $pythonCommand.Source
    } else {
        $Python = 'python'
    }
}
if ([string]::IsNullOrWhiteSpace($RustExecutable)) {
    $RustExecutable = Join-Path $repoRoot 'rust\beacon\target\debug\uxplay-beacon.exe'
}
if (-not (Test-Path -LiteralPath $RustExecutable -PathType Leaf)) {
    throw "Build the beacon first: cargo build --manifest-path rust/beacon/Cargo.toml"
}

$script = Join-Path $SourceRoot 'Bluetooth_LE_beacon\uxplay-beacon.py'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    throw "Reference beacon script not found: $script"
}

$tempRoot = Join-Path $env:TEMP ('uxplay-beacon-parity-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null

function Invoke-Captured(
    [string]$ProgramPath,
    [string[]]$ArgumentList,
    [string]$HomeDirectory,
    [string]$ReferenceRoot
) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ProgramPath
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['HOME'] = $HomeDirectory
    $start.Environment['USERPROFILE'] = $HomeDirectory
    $start.Environment['UXPLAY_REFERENCE_ROOT'] = $ReferenceRoot
    $start.Environment['PYTHONIOENCODING'] = 'utf-8'
    foreach ($argument in $ArgumentList) {
        [void]$start.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    [void]$process.Start()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    [PSCustomObject]@{
        ExitCode = $process.ExitCode
        Stdout = $stdout.Result
        Stderr = $stderr.Result
    }
}

$bluezOracleScript = Join-Path $PSScriptRoot 'source-bluez-callbacks.py'
$bluezSource = Join-Path $SourceRoot 'Bluetooth_LE_beacon\uxplay_beacon_module_BlueZ.py'
$bluezExpectedPath = Join-Path $PSScriptRoot 'fixtures\source-bluez-callbacks.txt'
$bluezExpected = [System.IO.File]::ReadAllText($bluezExpectedPath).Replace("`r`n", "`n")
$bluezResult = Invoke-Captured $Python @($bluezOracleScript, $bluezSource) $tempRoot $SourceRoot
$bluezActual = $bluezResult.Stdout.Replace("`r`n", "`n")
if ($bluezResult.ExitCode -ne 0 -or $bluezActual -cne $bluezExpected -or $bluezResult.Stderr.Length -ne 0) {
    throw 'BlueZ late-callback/Release source fixture no longer matches.'
}

$cases = [ordered]@{
    help = @('--help')
    missing_file = @('--file', (Join-Path $tempRoot 'absent.rc'))
    invalid_cli_ipv4 = @('--ipv4', 'nope')
    bleuio_no_device = @('BleuIO', '--ipv4', '127.0.0.1')
}
foreach ($name in @('valid', 'bad_ipv4', 'unknown', 'empty', 'comment', 'unicode_decimal')) {
    $path = Join-Path $tempRoot ($name + '.rc')
    $contents = switch ($name) {
        valid { "--ipv4 192.0.2.8`n" }
        bad_ipv4 { "--ipv4 999.0.0.1`n" }
        unknown { "--bogus value`n" }
        empty { '' }
        comment { "# only`n" }
        unicode_decimal { "--advmin ١٢٣`n" }
    }
    [System.IO.File]::WriteAllText($path, $contents, [System.Text.UTF8Encoding]::new($false))
    $cases[$name] = @('--file', $path)
}

$results = foreach ($case in $cases.GetEnumerator()) {
    $original = Invoke-Captured $Python (@($script) + $case.Value) $tempRoot $SourceRoot
    $ported = Invoke-Captured $RustExecutable $case.Value $tempRoot $SourceRoot
    [PSCustomObject]@{
        Case = $case.Key
        SameExit = $original.ExitCode -eq $ported.ExitCode
        SameStdout = $original.Stdout -ceq $ported.Stdout
        SameStderr = $original.Stderr -ceq $ported.Stderr
        OriginalExit = $original.ExitCode
        RustExit = $ported.ExitCode
    }
}

$results | Format-Table -AutoSize
if (@($results | Where-Object { -not ($_.SameExit -and $_.SameStdout -and $_.SameStderr) }).Count -ne 0) {
    exit 1
}
Write-Output 'BlueZ late-callback/Release source fixture: matched'
