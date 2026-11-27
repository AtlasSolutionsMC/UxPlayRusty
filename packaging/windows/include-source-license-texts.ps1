param(
    [Parameter(Mandatory = $true)]
    [string]$StageDirectory,

    [Parameter(Mandatory = $true)]
    [string]$SourcePackageDirectory,

    [string]$Msys2Root = (Join-Path $env:LOCALAPPDATA 'Programs\MSYS2')
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$stage = (Resolve-Path $StageDirectory).Path
$sourceRoot = (Resolve-Path $SourcePackageDirectory).Path
$expectedOutputPrefix = [IO.Path]::GetFullPath((Join-Path $repoRoot 'dist\windows')) + [IO.Path]::DirectorySeparatorChar
foreach ($path in @($stage, $sourceRoot)) {
    if (-not $path.StartsWith($expectedOutputPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Stage and source-package directories must be inside '$expectedOutputPrefix'."
    }
}

$zstd = Join-Path (Resolve-Path $Msys2Root).Path 'ucrt64\bin\zstd.exe'
$tar = (Get-Command tar.exe -ErrorAction Stop).Source
$git = (Get-Command git.exe -ErrorAction Stop).Source
$noticePath = Join-Path $stage 'THIRD-PARTY-NOTICES.txt'
$sourceManifestPath = Join-Path $sourceRoot 'SOURCE-PACKAGES.txt'
foreach ($required in @($zstd, $noticePath, $sourceManifestPath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required license source input is missing: $required"
    }
}

$packageVersions = @{}
$noticeText = Get-Content -LiteralPath $noticePath -Raw
foreach ($block in ($noticeText -split '(?=Name\s+:\s)')) {
    $nameMatch = [regex]::Match($block, '(?m)^Name\s+:\s+(?<name>\S+)')
    if (-not $nameMatch.Success) { continue }
    $versionMatch = [regex]::Match($block, '(?m)^Version\s+:\s+(?<version>\S+)')
    if (-not $versionMatch.Success) { throw "Runtime package version is missing for $($nameMatch.Groups['name'].Value)." }
    $packageVersions[$nameMatch.Groups['name'].Value] = $versionMatch.Groups['version'].Value
}

$sourceRecords = @{}
$manifestLines = Get-Content -LiteralPath $sourceManifestPath
for ($index = 0; $index -lt $manifestLines.Count; $index++) {
    $packageMatch = [regex]::Match($manifestLines[$index], '^(?<name>mingw-w64-ucrt-x86_64-\S+)\s+(?<version>\S+)$')
    if (-not $packageMatch.Success) { continue }
    $sourceMatch = [regex]::Match($manifestLines[$index + 1], '^\s+Source package:\s+(?<file>\S+)$')
    $hashMatch = [regex]::Match($manifestLines[$index + 3], '^\s+SHA256:\s+(?<hash>[0-9a-fA-F]{64})$')
    if (-not $sourceMatch.Success -or -not $hashMatch.Success) {
        throw "Source manifest entry is incomplete for $($packageMatch.Groups['name'].Value)."
    }
    $sourceRecords[$packageMatch.Groups['name'].Value] = [pscustomobject]@{
        Version = $packageMatch.Groups['version'].Value
        File = $sourceMatch.Groups['file'].Value
        Sha256 = $hashMatch.Groups['hash'].Value.ToLowerInvariant()
    }
}

$licenseRoot = Join-Path $stage 'third-party-licenses'
$references = @(Get-ChildItem -LiteralPath $licenseRoot -Filter 'LICENSE-REFERENCE.txt' -File -Recurse)
if ($references.Count -eq 0) {
    Write-Output 'No license-reference placeholders found; no source license extraction was needed.'
    return
}

$verifiedHashes = @{}
$licensePattern = '^[^/]+/(?:COPYING|LICENSE|COPYRIGHT|LICENCE)(?:[._-][^/]*)?$'
$extractedCount = 0
foreach ($reference in $references) {
    $owner = $reference.Directory.Name
    $runtimeVersion = $packageVersions[$owner]
    $sourceRecord = $sourceRecords[$owner]
    if (-not $runtimeVersion -or -not $sourceRecord) {
        throw "No exact runtime/source version mapping exists for $owner."
    }
    if ($runtimeVersion -ne $sourceRecord.Version) {
        throw "Source version $($sourceRecord.Version) does not match runtime $owner $runtimeVersion."
    }

    $archivePath = Join-Path $sourceRoot $sourceRecord.File
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        throw "Exact source package is missing for $owner`: $archivePath"
    }
    if (-not $verifiedHashes.ContainsKey($archivePath)) {
        $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $sourceRecord.Sha256) {
            throw "SHA-256 mismatch for exact source package $($sourceRecord.File)."
        }
        $verifiedHashes[$archivePath] = $actualHash
    }

    $outerEntries = @(& $zstd -d -c $archivePath | & $tar -tf -)
    if ($LASTEXITCODE -ne 0) { throw "Could not list source package $($sourceRecord.File)." }
    $unsafeEntries = @($outerEntries | Where-Object { $_ -match '(^/|^[A-Za-z]:|(^|/)\.\.?(/|$))' })
    if ($unsafeEntries.Count -gt 0) {
        throw "Source package contains unsafe archive paths: $($unsafeEntries[0])"
    }
    $upstreamArchive = $outerEntries | Where-Object {
        $parts = $_ -split '/'
        $parts.Count -eq 2 -and $_ -match '\.(tar\.(xz|gz|bz2|zst)|tgz|txz)$'
    } | Select-Object -First 1

    $sourceLicensePaths = @()
    $gitDirectory = $null
    $scratch = $null
    try {
        if ($upstreamArchive) {
            $innerEntries = @(& $zstd -d -c $archivePath | & $tar -xOf - $upstreamArchive | & $tar -tf -)
            if ($LASTEXITCODE -ne 0) { throw "Could not list upstream archive $upstreamArchive for $owner." }
            $sourceLicensePaths = @($innerEntries | Where-Object { $_ -match $licensePattern })
            if ($sourceLicensePaths.Count -eq 0) {
                throw "No top-level license text was found in $upstreamArchive for $owner."
            }
        } else {
            $packEntry = $outerEntries | Where-Object { $_ -match '^[^/]+/[^/]+/objects/pack/[^/]+\.pack$' } | Select-Object -First 1
            if (-not $packEntry) { throw "No upstream tarball or Git object pack was found for $owner." }
            $repoRelative = $packEntry -replace '/objects/pack/[^/]+\.pack$', ''
            $scratch = Join-Path $env:TEMP ('uxplay-license-source-' + [guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $scratch | Out-Null
            & $zstd -d -c $archivePath | & $tar -xf - -C $scratch
            if ($LASTEXITCODE -ne 0) { throw "Could not extract source Git repository for $owner." }
            $gitDirectory = Join-Path $scratch ($repoRelative -replace '/', '\')
            $gitPaths = @(& $git "--git-dir=$gitDirectory" ls-tree -r --name-only HEAD)
            if ($LASTEXITCODE -ne 0) { throw "Could not inspect pinned source Git tree for $owner." }
            $sourceLicensePaths = @($gitPaths | Where-Object { $_ -match '^[^/]+$' -and $_ -match '^(COPYING|LICENSE|COPYRIGHT|LICENCE)(?:[._-].*)?$' })
            if ($sourceLicensePaths.Count -eq 0) { throw "No top-level license text was found in the pinned Git source for $owner." }
        }

        $sourceLicenseRoot = Join-Path $reference.Directory.FullName 'source'
        New-Item -ItemType Directory -Path $sourceLicenseRoot -Force | Out-Null
        $licenseDetails = @()
        foreach ($licensePath in $sourceLicensePaths) {
            $fileName = Split-Path -Leaf $licensePath
            $destination = Join-Path $sourceLicenseRoot $fileName
            if ($upstreamArchive) {
                & $zstd -d -c $archivePath | & $tar -xOf - $upstreamArchive | & $tar -xOf - $licensePath > $destination
                if ($LASTEXITCODE -ne 0) { throw "Could not extract $licensePath for $owner." }
            } else {
                & $git "--git-dir=$gitDirectory" show "HEAD:$licensePath" > $destination
                if ($LASTEXITCODE -ne 0) { throw "Could not extract $licensePath from pinned Git source for $owner." }
            }
            if (-not (Test-Path -LiteralPath $destination -PathType Leaf) -or (Get-Item -LiteralPath $destination).Length -eq 0) {
                throw "Extracted license text is empty: $destination"
            }
            $licenseHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
            $licenseDetails += "  $fileName (SHA256 $licenseHash; source path $licensePath)"
        }

        $referenceText = @(
            "Package: $owner"
            "Version: $runtimeVersion"
            "Source package: $($sourceRecord.File)"
            "Source package SHA-256: $($sourceRecord.Sha256)"
            'License files extracted from the exact upstream source snapshot are in the sibling source directory.'
            $licenseDetails
            'See THIRD-PARTY-NOTICES.txt for the package metadata and declared license identifiers.'
            'This file is provenance information; it is not a legal compliance determination.'
        )
        Set-Content -LiteralPath (Join-Path $reference.Directory.FullName 'LICENSE-SOURCE.txt') -Value $referenceText -Encoding utf8
        Remove-Item -LiteralPath $reference.FullName -Force
        $extractedCount++
    } finally {
        if ($scratch) {
            $tempPrefix = [IO.Path]::GetFullPath($env:TEMP) + [IO.Path]::DirectorySeparatorChar
            $scratchFull = [IO.Path]::GetFullPath($scratch)
            if ($scratchFull.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                Remove-Item -LiteralPath $scratchFull -Recurse -Force
            } else {
                throw "Refusing to remove scratch directory outside TEMP: $scratchFull"
            }
        }
    }
}

Write-Output "Source license texts extracted for $extractedCount runtime package(s)."
