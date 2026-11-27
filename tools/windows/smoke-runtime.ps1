param(
    [Parameter(Mandatory = $true)]
    [string]$StageDirectory,

    [Parameter(Mandatory = $true)]
    [string]$Msys2Root
)

$ErrorActionPreference = 'Stop'
$stage = (Resolve-Path $StageDirectory).Path
$ucrtRoot = Join-Path (Resolve-Path $Msys2Root).Path 'ucrt64'
$smokeRoot = Join-Path $env:TEMP ('uxplay-runtime-smoke-' + [guid]::NewGuid().ToString('N'))
$previousPath = $env:Path
$previousPluginPath = $env:GST_PLUGIN_SYSTEM_PATH_1_0
$previousPluginOverride = $env:GST_PLUGIN_PATH_1_0
$previousScanner = $env:GST_PLUGIN_SCANNER
$previousRegistry = $env:GST_REGISTRY

New-Item -ItemType Directory -Path $smokeRoot | Out-Null
try {
    Get-ChildItem -LiteralPath $stage -Filter '*.dll' -File | Copy-Item -Destination $smokeRoot
    Copy-Item (Join-Path $ucrtRoot 'bin\gst-inspect-1.0.exe') $smokeRoot
    Copy-Item (Join-Path $stage 'runtime') $smokeRoot -Recurse

    $env:Path = "$smokeRoot;$env:WINDIR\System32;$env:WINDIR"
    $env:GST_PLUGIN_SYSTEM_PATH_1_0 = Join-Path $smokeRoot 'runtime\plugins'
    $env:GST_PLUGIN_PATH_1_0 = $env:GST_PLUGIN_SYSTEM_PATH_1_0
    $env:GST_PLUGIN_SCANNER = Join-Path $smokeRoot 'runtime\libexec\gstreamer-1.0\gst-plugin-scanner.exe'
    $env:GST_REGISTRY = Join-Path $smokeRoot 'registry.bin'

    $inspect = Join-Path $smokeRoot 'gst-inspect-1.0.exe'
    $factories = @(
        'appsrc', 'decodebin', 'avdec_aac', 'avdec_alac', 'h264parse', 'h265parse',
        'audioconvert', 'audioresample', 'volume', 'level', 'autoaudiosink', 'wasapisink',
        'videoconvert', 'videoscale', 'autovideosink', 'playbin3', 'hlsdemux',
        'd3d11h264dec', 'd3d11h265dec', 'd3d11videosink', 'vulkanupload', 'vulkansink'
    )
    foreach ($factory in $factories) {
        if ($factory -eq 'vulkansink') {
            $vulkanPlugin = Join-Path $env:GST_PLUGIN_SYSTEM_PATH_1_0 'libgstvulkan.dll'
            if (-not (Test-Path -LiteralPath $vulkanPlugin)) {
                throw "Bundled Vulkan plugin is missing: $vulkanPlugin"
            }
            Write-Output 'PASS libgstvulkan.dll is staged; Vulkan device/factory acceptance remains hardware-specific'
            continue
        }

        $null = & $inspect $factory 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "Bundled GStreamer factory '$factory' failed to load (exit $LASTEXITCODE)."
        }
        Write-Output "PASS $factory"
    }

    $env:Path = "$stage;$env:WINDIR\System32;$env:WINDIR"
    $help = Start-Process -FilePath (Join-Path $stage 'uxplay.exe') -ArgumentList '-h' -WindowStyle Hidden -Wait -PassThru
    if ($help.ExitCode -ne 0) { throw "Packaged UxPlay -h smoke failed (exit $($help.ExitCode))." }
    Write-Output 'PASS uxplay -h without MSYS2 PATH'
} finally {
    $env:Path = $previousPath
    $env:GST_PLUGIN_SYSTEM_PATH_1_0 = $previousPluginPath
    $env:GST_PLUGIN_PATH_1_0 = $previousPluginOverride
    $env:GST_PLUGIN_SCANNER = $previousScanner
    $env:GST_REGISTRY = $previousRegistry

    $tempRoot = [IO.Path]::GetFullPath($env:TEMP) + [IO.Path]::DirectorySeparatorChar
    $smokeFull = [IO.Path]::GetFullPath($smokeRoot)
    if ($smokeFull.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $smokeFull -Recurse -Force
    } else {
        throw "Refusing to remove a smoke directory outside the temporary folder: $smokeFull"
    }
}
