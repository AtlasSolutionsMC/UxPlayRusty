param(
    [Parameter(Mandatory = $true)]
    [string]$Msys2Root,

    [string]$OutputDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent | Split-Path -Parent) 'dist\windows'),

    [string]$InnoSetupCompiler = (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$msysRoot = (Resolve-Path $Msys2Root).Path
$ucrtRoot = Join-Path $msysRoot 'ucrt64'
$binaryRoot = Join-Path $ucrtRoot 'bin'
$pluginRoot = Join-Path $ucrtRoot 'lib\gstreamer-1.0'
$scanner = Join-Path $ucrtRoot 'libexec\gstreamer-1.0\gst-plugin-scanner.exe'
$objdump = Join-Path $binaryRoot 'objdump.exe'
$pacman = Join-Path $msysRoot 'usr\bin\pacman.exe'
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
$expectedOutputPrefix = [IO.Path]::GetFullPath((Join-Path $repoRoot 'dist\windows')) + [IO.Path]::DirectorySeparatorChar
if (-not ($outputRoot.Equals($expectedOutputPrefix.TrimEnd([IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase) -or
        $outputRoot.StartsWith($expectedOutputPrefix, [StringComparison]::OrdinalIgnoreCase))) {
    throw "Output directory must be inside '$expectedOutputPrefix'."
}
foreach ($required in @($binaryRoot, $pluginRoot, $scanner, $objdump, $pacman)) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "Required MSYS2 UCRT64 file is missing: $required"
    }
}

$stage = Join-Path $outputRoot 'stage'
$stageFull = [IO.Path]::GetFullPath($stage)
if (-not $stageFull.StartsWith($expectedOutputPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace an output outside dist/windows: $stageFull"
}
if (Test-Path -LiteralPath $stageFull) {
    Remove-Item -LiteralPath $stageFull -Recurse -Force
}
$runtime = Join-Path $stage 'runtime'
$stagePlugins = Join-Path $runtime 'plugins'
$stageScanner = Join-Path $runtime 'libexec\gstreamer-1.0'
New-Item -ItemType Directory -Path $stagePlugins,$stageScanner -Force | Out-Null

$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:Path = "$binaryRoot;$env:Path"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $binaryRoot 'gcc.exe'
$env:PKG_CONFIG_PATH = "$ucrtRoot\lib\pkgconfig;$ucrtRoot\share\pkgconfig"
$rustRoot = Join-Path $repoRoot 'rust'
Push-Location $rustRoot
try {
    & cargo build --manifest-path app/Cargo.toml --release
    if ($LASTEXITCODE -ne 0) { throw "Release app build failed with exit code $LASTEXITCODE" }
    & cargo build --manifest-path beacon/Cargo.toml --release
    if ($LASTEXITCODE -ne 0) { throw "Release beacon build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

Copy-Item (Join-Path $repoRoot 'rust\app\target\release\uxplay.exe') (Join-Path $stage 'uxplay.exe')
Copy-Item (Join-Path $repoRoot 'rust\beacon\target\release\uxplay-beacon.exe') (Join-Path $stage 'uxplay-beacon.exe')
Copy-Item (Join-Path $repoRoot 'rust\app\assets\config.example.toml') (Join-Path $stage 'config.example.toml')
Copy-Item (Join-Path $repoRoot 'README.md') (Join-Path $stage 'README.md')
Copy-Item (Join-Path $repoRoot 'LICENSE') (Join-Path $stage 'LICENSE')
Copy-Item $scanner (Join-Path $stageScanner 'gst-plugin-scanner.exe')

# Keep the runtime focused on UxPlay's default audio/video paths and the
# optional Windows renderers. The dependency walk below copies all imported
# UCRT64 DLLs for these plugins and both executables.
$pluginNames = @(
    'libgstapp.dll', 'libgstcoreelements.dll', 'libgsttypefindfunctions.dll',
    'libgstplayback.dll', 'libgstlibav.dll', 'libgstautodetect.dll',
    'libgstvideoparsersbad.dll', 'libgstvideoconvertscale.dll',
    'libgstaudioconvert.dll', 'libgstaudioresample.dll', 'libgstvolume.dll',
    'libgstwasapi.dll', 'libgstd3d11.dll', 'libgstvulkan.dll', 'libgsthls.dll',
    'libgstsoup.dll', 'libgstrtp.dll', 'libgstudp.dll', 'libgstisomp4.dll',
    'libgstmpegtsdemux.dll', 'libgstsubparse.dll', 'libgstjpeg.dll',
    'libgstvideorate.dll', 'libgsttcp.dll'
)
$pluginFiles = foreach ($name in $pluginNames) {
    $path = Join-Path $pluginRoot $name
    if (-not (Test-Path -LiteralPath $path)) { throw "Required GStreamer plugin is missing: $name" }
    Copy-Item -LiteralPath $path -Destination (Join-Path $stagePlugins $name)
    $path
}

$binaryIndex = @{}
Get-ChildItem -LiteralPath $binaryRoot -Filter '*.dll' -File | ForEach-Object {
    $binaryIndex[$_.Name.ToLowerInvariant()] = $_.FullName
}
$pending = [Collections.Generic.Queue[string]]::new()
foreach ($path in @((Join-Path $stage 'uxplay.exe'),(Join-Path $stage 'uxplay-beacon.exe'),$scanner) + $pluginFiles) {
    $pending.Enqueue($path)
}
$visited = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$bundled = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$system32 = Join-Path $env:WINDIR 'System32'
while ($pending.Count -gt 0) {
    $current = $pending.Dequeue()
    if (-not $visited.Add($current)) { continue }
    $imports = & $objdump -p $current 2>$null | Select-String 'DLL Name:'
    foreach ($line in $imports) {
        $name = ($line.Line -split ':',2)[1].Trim()
        if ($name -match '^(api-ms-win-|ext-ms-win-)' -or (Test-Path -LiteralPath (Join-Path $system32 $name))) { continue }
        $dependency = $binaryIndex[$name.ToLowerInvariant()]
        if (-not $dependency) { throw "Unresolved non-system DLL '$name' imported by '$current'." }
        if ($bundled.Add($name)) {
            Copy-Item -LiteralPath $dependency -Destination (Join-Path $stage $name)
            $pending.Enqueue($dependency)
        }
    }
}

$share = Join-Path $ucrtRoot 'share\gstreamer-1.0'
if (Test-Path -LiteralPath $share) {
    Copy-Item -LiteralPath $share -Destination (Join-Path $runtime 'share\gstreamer-1.0') -Recurse
}
$schemas = Join-Path $ucrtRoot 'share\glib-2.0\schemas'
if (Test-Path -LiteralPath $schemas) {
    Copy-Item -LiteralPath $schemas -Destination (Join-Path $runtime 'share\glib-2.0\schemas') -Recurse
}
$ssl = Join-Path $ucrtRoot 'etc\ssl'
if (Test-Path -LiteralPath $ssl) {
    Copy-Item -LiteralPath $ssl -Destination (Join-Path $runtime 'ssl') -Recurse
}

# Include the license texts belonging to every redistributed MSYS2 DLL/plugin.
$owners = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$runtimeFiles = @($pluginFiles) + @($bundled | ForEach-Object { $binaryIndex[$_.ToLowerInvariant()] })
$pacmanPaths = @($runtimeFiles | Sort-Object -Unique | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($ucrtRoot, $_).Replace('\', '/')
    "/ucrt64/$relative"
})
$ownership = & $pacman -Qoq $pacmanPaths 2>$null
if ($LASTEXITCODE -ne 0 -or $ownership.Count -ne $pacmanPaths.Count) {
    throw 'Could not identify an MSYS2 package owner for each redistributed runtime file.'
}
foreach ($owner in $ownership) { [void]$owners.Add($owner.Trim()) }
$licenseRoot = Join-Path $ucrtRoot 'share\licenses'
$noticeRoot = Join-Path $stage 'third-party-licenses'
New-Item -ItemType Directory -Path $noticeRoot -Force | Out-Null
foreach ($owner in $owners) {
    $shortName = $owner -replace '^mingw-w64-ucrt-x86_64-', ''
    $candidates = @(
        (Join-Path $licenseRoot $owner),
        (Join-Path $licenseRoot $shortName),
        (Join-Path (Join-Path $msysRoot 'usr\share\licenses') $shortName)
    )
    $license = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    $destination = Join-Path $noticeRoot $owner
    if ($license) {
        Copy-Item -LiteralPath $license -Destination $destination -Recurse
    } else {
        # MSYS2 package metadata remains authoritative when its installed package
        # omitted a standalone license file. Keep a per-package pointer instead
        # of silently treating the application license as the library license.
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $destination 'LICENSE-REFERENCE.txt') -Encoding utf8 -Value @(
            "Package: $owner"
            'The installed MSYS2 package did not provide a separate license text.'
            'See THIRD-PARTY-NOTICES.txt for the package SPDX identifier and upstream URL.'
        )
    }
}
$packageInfo = & $pacman -Qi $owners
if ($LASTEXITCODE -ne 0) { throw 'Could not record the MSYS2 runtime package metadata.' }
Set-Content -LiteralPath (Join-Path $stage 'THIRD-PARTY-NOTICES.txt') -Encoding utf8 -Value @(
    'UxPlayRusty includes runtime DLLs and GStreamer plugins from these MSYS2 UCRT64 packages.'
    'Package versions, upstream URLs and SPDX license identifiers follow.'
    ''
    $packageInfo
)

$revision = (& git -C $repoRoot rev-parse --short HEAD).Trim()
$gstVersion = (& (Join-Path $binaryRoot 'gst-inspect-1.0.exe') --version | Select-Object -First 2) -join ' '
Set-Content -LiteralPath (Join-Path $stage 'BUILD-INFO.txt') -Encoding utf8 -Value @(
    "UxPlayRusty Windows bundle"
    "Source revision: $revision"
    "Rust toolchain: $(& rustc -V)"
    "GStreamer runtime: $gstVersion"
    "MSYS2 UCRT64 runtime files: $($bundled.Count) DLLs, $($pluginFiles.Count) plugins"
)

$compilerPath = (Resolve-Path -LiteralPath $InnoSetupCompiler -ErrorAction SilentlyContinue).Path
if (-not $compilerPath) { throw "Inno Setup 6 compiler not found at '$InnoSetupCompiler'." }
& $compilerPath (Join-Path $PSScriptRoot 'UxPlay.iss')
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed with exit code $LASTEXITCODE" }

$setup = Join-Path $outputRoot 'Setup.exe'
if (-not (Test-Path -LiteralPath $setup)) { throw "Inno Setup did not produce '$setup'." }
Get-Item -LiteralPath $setup | Select-Object FullName,Length,LastWriteTime
