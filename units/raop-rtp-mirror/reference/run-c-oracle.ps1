param(
    [switch]$Record,
    [string]$ReferenceRoot = (Join-Path $PSScriptRoot '..\..\..\..\UxPlay')
)
$ErrorActionPreference = 'Stop'
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$toolchain\bin;$env:Path"
$env:PKG_CONFIG_PATH = "$toolchain\lib\pkgconfig"
$referenceRoot = (Resolve-Path $ReferenceRoot).Path
$build = Join-Path $PSScriptRoot 'build-ninja'
& cmake -S $PSScriptRoot -B $build -G Ninja `
    "-DUXPLAY_REFERENCE=$referenceRoot" `
    "-DCMAKE_PREFIX_PATH=$toolchain" `
    -DCMAKE_BUILD_TYPE=Release
if ($LASTEXITCODE -ne 0) { throw "CMake configure exited with $LASTEXITCODE" }
& cmake --build $build --target raop_rtp_mirror_oracle --config Release
if ($LASTEXITCODE -ne 0) { throw "C oracle build exited with $LASTEXITCODE" }
$oracle = Join-Path $build 'raop_rtp_mirror_oracle.exe'
$actual = & $oracle
if ($LASTEXITCODE -ne 0) { throw "C oracle exited with $LASTEXITCODE" }
$fixture = Join-Path $PSScriptRoot 'raop_rtp_mirror.expected.txt'
if ($Record) {
    Set-Content -LiteralPath $fixture -Value $actual -Encoding ascii
} else {
    $expected = Get-Content -LiteralPath $fixture
    $delta = Compare-Object -ReferenceObject $expected -DifferenceObject $actual
    if ($delta) { $delta | Format-Table | Out-String | Write-Error }
    Write-Output "raop_rtp_mirror C oracle matches $($expected.Count) rows"
}
