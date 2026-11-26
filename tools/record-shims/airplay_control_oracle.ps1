$ErrorActionPreference = 'Stop'

$compiler = $env:UXPLAY_C_COMPILER
if ([string]::IsNullOrWhiteSpace($compiler)) {
    $compiler = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64\bin\cc.exe'
}
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
    throw "C compiler not found at '$compiler'. Set UXPLAY_C_COMPILER to a C11 compiler path."
}
$compilerDirectory = Split-Path -Parent $compiler
$env:PATH = "$compilerDirectory;$env:PATH"

$source = Join-Path $PSScriptRoot 'airplay_control_oracle.c'
$expectedPath = Join-Path $PSScriptRoot 'airplay_control_oracle.expected.txt'
$executable = Join-Path $env:TEMP "airplay_control_oracle_$PID.exe"

& $compiler -std=c11 -O2 $source '-o' $executable
if ($LASTEXITCODE -ne 0) {
    throw "C oracle compilation failed with exit code $LASTEXITCODE."
}

$actual = @(& $executable)
if ($LASTEXITCODE -ne 0) {
    throw "C oracle exited with code $LASTEXITCODE."
}
$expected = @(Get-Content -LiteralPath $expectedPath | Where-Object {
    $_.Trim() -and -not $_.TrimStart().StartsWith('#')
})
if ($actual.Count -ne $expected.Count) {
    throw "C oracle row count mismatch: got $($actual.Count), expected $($expected.Count)."
}

for ($index = 0; $index -lt $expected.Count; $index++) {
    $actualFields = @($actual[$index] -split '\s+')
    $expectedFields = @($expected[$index] -split '\s+')
    if ($actualFields[0] -cne $expectedFields[0]) {
        throw "C oracle label mismatch at row ${index}: '$($actualFields[0])' vs '$($expectedFields[0])'."
    }
    if ($actualFields[0] -ceq 'nan-payload') {
        foreach ($bitsText in @($actualFields[1], $actualFields[2])) {
            $bits = [Convert]::ToUInt32($bitsText, 16)
            if (($bits -band 0x7f800000) -ne 0x7f800000 -or ($bits -band 0x007fffff) -eq 0) {
                throw "C runtime returned a non-NaN bit pattern for nan-payload: $bitsText."
            }
        }
        continue
    }
    if ($actualFields[1] -cne $expectedFields[1] -or $actualFields[2] -cne $expectedFields[2]) {
        throw "C oracle value mismatch at row $index ($($actualFields[0]))."
    }
}

Remove-Item -LiteralPath $executable -Force
Write-Output "C control conversions match $($expected.Count) fixtures."
