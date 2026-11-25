# RAOP session focused gate

This is a stateful Rust API rather than a rust-anything generated FFI module, so the generated `rust-anything gate` does not apply. Run the focused checks from this crate directory:

```powershell
$toolchain = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$toolchain\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $toolchain 'bin\gcc.exe'
$env:PKG_CONFIG_PATH = Join-Path $toolchain 'lib\pkgconfig'
$env:OPENSSL_DIR = $toolchain
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build
.\reference\run-c-oracle.ps1
```

The oracle runs unchanged C expressions transcribed into `reference/session_oracle.c`; it is not an end-to-end handler comparison. The app-level integration gate remains outstanding.
