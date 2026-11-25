[CmdletBinding()]
param(
    [string] $Compiler = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'),
    [switch] $Record
)

$ErrorActionPreference = 'Stop'
$unitRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = (Resolve-Path (Join-Path $unitRoot '..\..\..')).Path
$referenceRoot = Join-Path (Split-Path $targetRoot -Parent) 'UxPlay'
$compilerDir = Split-Path -Parent $Compiler
$oracleSource = Join-Path $PSScriptRoot 'mux_renderer_oracle.c'
$expectedPath = Join-Path $PSScriptRoot 'mux_renderer_oracle.expected.txt'
$exe = Join-Path $env:TEMP "mux_renderer_oracle_$PID.exe"
$workDir = Join-Path $env:TEMP "mux_renderer_oracle_$PID"
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([char]'\')
$cases = @(
    @{ name = 'h264-aac'; base = 'mux-h264-aac' },
    @{ name = 'h265-video'; base = 'mux-h265-video' },
    @{ name = 'alac-video-config'; base = 'mux-alac-video-config' },
    @{ name = 'codec-switch'; base = 'mux-codec-switch' },
    @{ name = 'both-disabled'; base = 'mux-both-disabled' },
    @{ name = 'timestamps'; base = 'mux-timestamps' }
)

if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "C compiler not found: $Compiler"
}
$resolvedWorkDir = [System.IO.Path]::GetFullPath($workDir)
$relativeWorkDir = [System.IO.Path]::GetRelativePath($tempRoot, $resolvedWorkDir)
if ([System.IO.Path]::IsPathRooted($relativeWorkDir) -or
    $relativeWorkDir -eq '..' -or
    $relativeWorkDir.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Oracle work directory escaped TEMP: $resolvedWorkDir"
}
New-Item -ItemType Directory -Path $resolvedWorkDir | Out-Null

$oldPath = $env:PATH
$oldDebug = $env:GST_DEBUG
$lines = [System.Collections.Generic.List[string]]::new()
foreach ($case in $cases) {
    $baseName = $case.base
    $expectedFiles = @(
        "$baseName.1.H264.AAC.mp4",
        "$baseName.1.H265.mp4",
        "$baseName.1.ALAC.mp4",
        "$baseName.1.H265.AAC.mp4",
        "$baseName.2.H264.AAC.mp4",
        "$baseName.1.H264.AAC.mp4",
        "$baseName.1.mp4",
        '(NULL).1.mp4'
    )
    Push-Location $resolvedWorkDir
    try {
        $env:PATH = "$compilerDir;$oldPath"
        $env:GST_DEBUG = '0'
        $pkgFlagsText = & pkg-config --cflags --libs gstreamer-app-1.0
        if ($LASTEXITCODE -ne 0) {
            throw 'pkg-config failed for gstreamer-app-1.0'
        }
        $pkgFlags = ([string]$pkgFlagsText) -split '\s+'
        $compile = @(
            '-std=c11', '-DWIN32', '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter',
            "-I$(Join-Path $referenceRoot 'renderers')",
            "-I$(Join-Path $referenceRoot 'lib')",
            $oracleSource,
            (Join-Path $referenceRoot 'lib\logger.c'),
            '-o', $exe
        ) + $pkgFlags
        & $Compiler @compile
        if ($LASTEXITCODE -ne 0) {
            throw "Mux renderer C oracle compile failed with exit code $LASTEXITCODE"
        }
        $scenarioOutput = @(& $exe $case.name $case.base 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "Mux renderer C oracle scenario '$($case.name)' exited with code $LASTEXITCODE"
        }
        $lines.Add("CASE|$($case.name)")
        foreach ($line in $scenarioOutput) {
            $lines.Add([string]$line)
        }
        $expectedFiles | ForEach-Object {
            $file = Join-Path $resolvedWorkDir $_
            if (Test-Path -LiteralPath $file -PathType Leaf) {
                Remove-Item -LiteralPath $file -Force
            }
        }
    } finally {
        Pop-Location
        $env:PATH = $oldPath
        $env:GST_DEBUG = $oldDebug
        if (Test-Path -LiteralPath $exe -PathType Leaf) {
            Remove-Item -LiteralPath $exe -Force
        }
    }
}

try {
    if ($Record) {
        $lines | Set-Content -LiteralPath $expectedPath -Encoding utf8NoBOM
        Write-Output "Recorded $($lines.Count) mux renderer C behavior rows."
        return
    }
    if (-not (Test-Path -LiteralPath $expectedPath -PathType Leaf)) {
        throw "Expected fixture missing at $expectedPath; run with -Record to create it."
    }
    $expected = @(Get-Content -LiteralPath $expectedPath)
    if ($lines.Count -ne $expected.Count) {
        throw "Mux renderer oracle row count differs: C=$($lines.Count), fixture=$($expected.Count)"
    }
    for ($index = 0; $index -lt $lines.Count; $index++) {
        if ($lines[$index] -cne $expected[$index]) {
            throw "Mux renderer oracle differs at row $($index + 1): C='$($lines[$index])', fixture='$($expected[$index])'"
        }
    }
    Write-Output "Mux renderer C oracle matches $($lines.Count) recorded rows."
} finally {
    if ((Test-Path -LiteralPath $resolvedWorkDir -PathType Container) -and
        -not (Get-ChildItem -LiteralPath $resolvedWorkDir -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $resolvedWorkDir -Force
    }
}
