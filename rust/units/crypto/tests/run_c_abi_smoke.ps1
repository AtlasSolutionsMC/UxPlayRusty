$ErrorActionPreference = 'Stop'

$crate = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
$sourceLib = Join-Path (Join-Path (Split-Path -Parent $repoRoot) 'UxPlay') 'lib'
$output = Join-Path $crate 'target\debug'
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$msysBin = Join-Path $msysRoot 'bin'
$env:PATH = "$msysBin;$env:PATH"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:PKG_CONFIG_PATH = Join-Path $msysRoot 'lib\pkgconfig'
$manifest = Join-Path $crate 'Cargo.toml'
& cargo build --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { throw 'Building the Rust C ABI library failed.' }
$cflags = (& pkg-config --cflags openssl) -split '\s+'
$libs = (& pkg-config --libs openssl) -split '\s+'
$renames = @(
    'aes_ctr_init=ref_aes_ctr_init', 'aes_ctr_reset=ref_aes_ctr_reset',
    'aes_ctr_encrypt=ref_aes_ctr_encrypt', 'aes_ctr_decrypt=ref_aes_ctr_decrypt',
    'aes_ctr_start_fresh_block=ref_aes_ctr_start_fresh_block', 'aes_ctr_destroy=ref_aes_ctr_destroy',
    'aes_cbc_init=ref_aes_cbc_init', 'aes_cbc_reset=ref_aes_cbc_reset',
    'aes_cbc_encrypt=ref_aes_cbc_encrypt', 'aes_cbc_decrypt=ref_aes_cbc_decrypt',
    'aes_cbc_destroy=ref_aes_cbc_destroy', 'x25519_key_generate=ref_x25519_key_generate',
    'x25519_key_from_raw=ref_x25519_key_from_raw', 'x25519_key_get_raw=ref_x25519_key_get_raw',
    'x25519_key_destroy=ref_x25519_key_destroy', 'get_random_bytes=ref_get_random_bytes',
    'pk_to_base64=ref_pk_to_base64', 'x25519_derive_secret=ref_x25519_derive_secret',
    'gcm_encrypt=ref_gcm_encrypt', 'gcm_decrypt=ref_gcm_decrypt',
    'ed25519_key_generate=ref_ed25519_key_generate', 'ed25519_key_from_raw=ref_ed25519_key_from_raw',
    'ed25519_key_get_raw=ref_ed25519_key_get_raw', 'ed25519_key_copy=ref_ed25519_key_copy',
    'ed25519_key_destroy=ref_ed25519_key_destroy', 'ed25519_sign=ref_ed25519_sign',
    'ed25519_verify=ref_ed25519_verify', 'sha_init=ref_sha_init', 'sha_update=ref_sha_update',
    'sha_final=ref_sha_final', 'sha_reset=ref_sha_reset', 'sha_destroy=ref_sha_destroy',
    'md5_init=ref_md5_init', 'md5_update=ref_md5_update', 'md5_final=ref_md5_final',
    'md5_reset=ref_md5_reset', 'md5_destroy=ref_md5_destroy', 'get_md5=ref_get_md5'
)
$defines = $renames | ForEach-Object { "-D$_" }
$referenceObject = Join-Path $output 'crypto_reference.o'
$smokeExe = Join-Path $output 'c_abi_smoke.exe'

& gcc @cflags "-I$sourceLib" @defines -c (Join-Path $sourceLib 'crypto.c') -o $referenceObject
if ($LASTEXITCODE -ne 0) { throw 'Compiling untouched C reference failed.' }
& gcc @cflags "-I$sourceLib" "-I$(Join-Path $crate 'include')" `
    (Join-Path $PSScriptRoot 'c_abi_smoke.c') $referenceObject @libs -o $smokeExe
if ($LASTEXITCODE -ne 0) { throw 'Compiling C ABI smoke harness failed.' }

Push-Location $output
try {
    & $smokeExe
    if ($LASTEXITCODE -ne 0) { throw 'C ABI smoke test failed.' }

    function Invoke-SmokeProcess([string]$argument) {
        $start = New-Object System.Diagnostics.ProcessStartInfo
        $start.FileName = $smokeExe
        $start.Arguments = $argument
        $start.WorkingDirectory = $output
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $process = [System.Diagnostics.Process]::Start($start)
        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        return @{ ExitCode = $process.ExitCode; Stdout = $stdout; Stderr = $stderr }
    }

    $sourceError = Invoke-SmokeProcess '--source-error'
    $rustError = Invoke-SmokeProcess '--rust-error'
    if ($sourceError.ExitCode -ne 1 -or $rustError.ExitCode -ne 1) {
        throw "Operational error exit mismatch: source=$($sourceError.ExitCode), Rust=$($rustError.ExitCode)."
    }
    # The smoke build renames C exports with macros, which also changes __func__;
    # restore the source symbol name before comparing the observable log line.
    $normalizedSourceError = $sourceError.Stderr -replace 'Crypto error at ref_ed25519_sign:', 'Crypto error at ed25519_sign:'
    $normalizedSourceError = $normalizedSourceError -replace "`r`n", "`n"
    $normalizedRustError = $rustError.Stderr -replace "`r`n", "`n"
    if ($normalizedSourceError -cne $normalizedRustError) {
        throw "Operational error logging mismatch:`nsource: $($sourceError.Stderr)`nRust: $($rustError.Stderr)"
    }
    if ($rustError.Stderr -notmatch '^Crypto error at ed25519_sign: error:') {
        throw "Rust operational error location/format mismatch: $($rustError.Stderr)"
    }
    if ($sourceError.Stdout -cne $rustError.Stdout) {
        throw 'Operational error stdout mismatch.'
    }
    Write-Host 'Operational error exit and stderr match C reference.'
}
finally { Pop-Location }
