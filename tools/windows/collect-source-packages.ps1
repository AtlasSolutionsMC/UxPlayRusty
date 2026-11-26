param(
    [string]$StageDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent | Split-Path -Parent) 'dist\windows\stage'),
    [string]$OutputDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent | Split-Path -Parent) 'dist\windows\source-packages'),
    [string]$Msys2Root = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2'),
    [string[]]$PackageName,
    [string[]]$MirrorRoot = @(
        'https://mirrors.tuna.tsinghua.edu.cn/msys2/mingw/sources',
        'https://mirrors.nju.edu.cn/msys2/mingw/sources',
        'https://mirrors.bfsu.edu.cn/msys2/mingw/sources'
    ),
    [switch]$CreateTar,
    [switch]$PlanOnly
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$stageFull = (Resolve-Path $StageDirectory).Path
$outputFull = [IO.Path]::GetFullPath($OutputDirectory)
$expectedOutputPrefix = [IO.Path]::GetFullPath((Join-Path $repoRoot 'dist\windows')) + [IO.Path]::DirectorySeparatorChar
if (-not $outputFull.StartsWith($expectedOutputPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Output directory must be inside '$expectedOutputPrefix'."
}

$noticeFile = Join-Path $stageFull 'THIRD-PARTY-NOTICES.txt'
$zstd = Join-Path $Msys2Root 'ucrt64\bin\zstd.exe'
if (-not (Test-Path -LiteralPath $noticeFile)) { throw "Runtime notices are missing: $noticeFile" }
if (-not $PlanOnly -and -not (Test-Path -LiteralPath $zstd)) { throw "MSYS2 zstd is missing: $zstd" }

$sourceBaseOverrides = @{
    'mingw-w64-ucrt-x86_64-gettext-runtime' = 'mingw-w64-gettext'
    'mingw-w64-ucrt-x86_64-gst-plugins-bad-libs' = 'mingw-w64-gst-plugins-bad'
    'mingw-w64-ucrt-x86_64-libgcc' = 'mingw-w64-gcc'
    'mingw-w64-ucrt-x86_64-libgomp' = 'mingw-w64-gcc'
    'mingw-w64-ucrt-x86_64-libstdc++' = 'mingw-w64-gcc'
    'mingw-w64-ucrt-x86_64-libwinpthread' = 'mingw-w64-winpthreads'
    'mingw-w64-ucrt-x86_64-libx264' = 'mingw-w64-x264'
}

$noticeText = Get-Content -LiteralPath $noticeFile -Raw
$blocks = $noticeText -split '(?=Name\s+: )' | Where-Object { $_ -match '^Name\s+:' }
$packages = foreach ($block in $blocks) {
    $name = ([regex]::Match($block, '(?m)^Name\s+:\s+(.*)$')).Groups[1].Value.Trim()
    $version = ([regex]::Match($block, '(?m)^Version\s+:\s+(.*)$')).Groups[1].Value.Trim()
    if (-not $name.StartsWith('mingw-w64-ucrt-x86_64-', [StringComparison]::Ordinal)) {
        throw "Unsupported source package name in notices: $name"
    }
    if (-not $version) { throw "Package version is missing in notices: $name" }

    $sourceBase = $sourceBaseOverrides[$name]
    if (-not $sourceBase) { $sourceBase = $name -replace '^mingw-w64-ucrt-x86_64-', 'mingw-w64-' }
    [pscustomobject]@{
        Name = $name
        Version = $version
        SourceBase = $sourceBase
        Url = "https://mirror.msys2.org/mingw/sources/$sourceBase-$version.src.tar.zst"
    }
}

if ($PackageName) {
    $requested = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $PackageName) { [void]$requested.Add($name) }
    $packages = @($packages | Where-Object { $requested.Contains($_.Name) })
    $missingNames = @($requested | Where-Object { $_ -notin $packages.Name })
    if ($missingNames.Count -gt 0) { throw "Requested package(s) are not in the staged runtime notices: $($missingNames -join ', ')" }
}

if ($PlanOnly) {
    $packages | Sort-Object Name | Format-Table Name, Version, SourceBase, Url -AutoSize
    "Source packages planned: $($packages.Count)"
    return
}

New-Item -ItemType Directory -Path $outputFull -Force | Out-Null
Copy-Item -LiteralPath $noticeFile -Destination (Join-Path $outputFull 'THIRD-PARTY-NOTICES.txt') -Force
$manifest = [Collections.Generic.List[string]]::new()
$manifest.Add('UxPlayRusty Windows runtime source package set')
$manifest.Add("Generated: $([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss')) UTC")
$manifest.Add('Runtime notices: THIRD-PARTY-NOTICES.txt')
$manifest.Add('Each archive is the exact MSYS2 package source release matching one redistributed runtime package version.')
$manifest.Add('This set does not replace the UxPlayRusty application source or the notices shipped beside the installer.')
$manifest.Add('')

foreach ($package in ($packages | Sort-Object Name)) {
    $archiveName = "$($package.SourceBase)-$($package.Version).src.tar.zst"
    $archivePath = Join-Path $outputFull $archiveName
    if (-not (Test-Path -LiteralPath $archivePath)) {
        $partialPath = "$archivePath.partial"
        $downloaded = $false
        try {
            foreach ($mirror in $MirrorRoot) {
                Remove-Item -LiteralPath $partialPath -Force -ErrorAction SilentlyContinue
                $sourceUrl = "$($mirror.TrimEnd('/'))/$archiveName"
                & curl.exe --fail --location --silent --show-error --retry 2 --max-time 180 --output $partialPath $sourceUrl
                if ($LASTEXITCODE -eq 0) {
                    $downloaded = $true
                    break
                }
            }
            if (-not $downloaded) { throw "Could not download $archiveName from any configured MSYS2 mirror." }
            & $zstd -t $partialPath | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Downloaded source archive failed zstd validation: $archiveName" }
            Move-Item -LiteralPath $partialPath -Destination $archivePath -Force
        } finally {
            if (Test-Path -LiteralPath $partialPath) { Remove-Item -LiteralPath $partialPath -Force }
        }
    }

    & $zstd -t $archivePath | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Source archive failed zstd validation: $archivePath" }
    $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest.Add("$($package.Name) $($package.Version)")
    $manifest.Add("  Source package: $archiveName")
    $manifest.Add("  URL: $($package.Url)")
    $manifest.Add("  SHA256: $hash")
}

Set-Content -LiteralPath (Join-Path $outputFull 'SOURCE-PACKAGES.txt') -Value $manifest -Encoding utf8
"Source packages verified: $($packages.Count)"
"Manifest: $(Join-Path $outputFull 'SOURCE-PACKAGES.txt')"

if ($CreateTar) {
    $tarPath = Join-Path (Split-Path $outputFull -Parent) 'source-packages.tar'
    & tar.exe -cf $tarPath -C $outputFull .
    if ($LASTEXITCODE -ne 0) { throw "Could not create source package archive: $tarPath" }
    $archive = Get-Item -LiteralPath $tarPath
    "Source package archive: $($archive.FullName) ($($archive.Length) bytes)"
}
