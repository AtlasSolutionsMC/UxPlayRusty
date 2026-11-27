# FairPlay selector-2 unit

This crate ports `lib/fairplay_v2/fairplay_v2.c` and exports the five functions
from the matching `include/fairplay_v2.h`. Protocol state and byte handling live
in safe Rust; the C ABI uses a narrow pointer wrapper.

The SAP transformation uses the reference's fixed mode-specific round keys,
input/output masks, and chaining masks. SHA-1, MD5, HMAC-SHA1, AES-128-ECB,
and constant-time equality use the OpenSSL crate. Random bytes reuse the
completed `uxplay-crypto` facade, which also uses OpenSSL.

## Local proof

The crate tests all four selector modes, handshake confirmation, wrapped media
key authentication, invalid MAC zeroing, and state reset behavior. The C
oracle compiles the untouched reference source with a deterministic test-only
random provider and compares complete response transcripts against Rust.

On Windows GNU/MSYS2 with OpenSSL installed under `ucrt64`, run:

```powershell
$env:CC = "$env:LOCALAPPDATA\Programs\MSYS2\ucrt64\bin\cc.exe"
cargo +stable-x86_64-pc-windows-gnu fmt -- --check
cargo +stable-x86_64-pc-windows-gnu clippy --all-targets --all-features -- -D warnings
cargo +stable-x86_64-pc-windows-gnu test
cargo +stable-x86_64-pc-windows-gnu build
.\tests\run_oracle.ps1
.\tests\run_c_abi_smoke.ps1
```

The two scripts set `OPENSSL_DIR` and `OPENSSL_STATIC=0` for the selected GNU
OpenSSL installation.
