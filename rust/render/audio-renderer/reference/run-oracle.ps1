param(
    [string]$OriginalRoot = (Resolve-Path (Join-Path $PSScriptRoot '../../../../../UxPlay')).Path,
    [string]$Ucrt64 = (Join-Path $env:LOCALAPPDATA 'Programs/MSYS2/ucrt64')
)

$ErrorActionPreference = 'Stop'
$bin = Join-Path $Ucrt64 'bin'
$env:PATH = "$bin;$env:PATH"
$env:PKG_CONFIG_PATH = Join-Path $Ucrt64 'lib/pkgconfig'

$cflags = (& (Join-Path $bin 'pkg-config.exe') --cflags gstreamer-app-1.0 gstreamer-1.0 glib-2.0 gobject-2.0).Trim()
$libs = (& (Join-Path $bin 'pkg-config.exe') --libs gstreamer-app-1.0 gstreamer-1.0 glib-2.0 gobject-2.0).Trim()
$cflagArgs = $cflags.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
$libArgs = $libs.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
$source = Join-Path $OriginalRoot 'renderers/audio_renderer.c'
$harness = Join-Path $PSScriptRoot 'audio_renderer_oracle.c'
$output = Join-Path $env:TEMP 'audio_renderer_oracle.exe'

& (Join-Path $bin 'cc.exe') `
    '-I' (Join-Path $OriginalRoot 'renderers') `
    '-I' (Join-Path $OriginalRoot 'lib') `
    @cflagArgs $source $harness @libArgs '-o' $output
if ($LASTEXITCODE -ne 0) {
    throw "C reference build failed with exit code $LASTEXITCODE"
}

& $output
if ($LASTEXITCODE -ne 0) {
    throw "C reference run failed with exit code $LASTEXITCODE"
}
