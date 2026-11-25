# FairPlay PlayFair unit

This crate ports the stateful adapter in `lib/fairplay_playfair.c`, the legacy
`playfair_decrypt` path, and only the required portions of `lib/playfair/omg_hax.c`.
Its exported C ABI retains `fairplay_init`, `fairplay_setup`, `fairplay_handshake`,
`fairplay_decrypt`, and `fairplay_destroy`. Selector 2 delegates to the Rust
`fairplay-v2` crate. The v3 decrypt path reuses the completed modified-MD5 and SAP
hash crates; SAP hash reuses the hand-garble crate.

Protocol lookup tables are extracted without editing the read-only reference by
`tools/generate_tables.ps1`. That script writes `tables.bin` and the small Rust
index module in `src/tables.rs`.

## Proof

`tests/run_oracle.ps1` compiles the untouched C reference and compares the full
transcript with the Rust port across all four legacy modes, including setup,
handshake, key decrypt, invalid selector-2 setup, invalid input, and state reset.
`tests/run_c_abi_smoke.ps1` checks the exported ABI from C. Rust unit tests cover
framing and state transitions.

On Windows GNU/MSYS2 with OpenSSL in `ucrt64`:

```powershell
$env:CC = "$env:LOCALAPPDATA\Programs\MSYS2\ucrt64\bin\cc.exe"
cargo +stable-x86_64-pc-windows-gnu fmt -- --check
cargo +stable-x86_64-pc-windows-gnu clippy --all-targets --all-features -- -D warnings
cargo +stable-x86_64-pc-windows-gnu test
cargo +stable-x86_64-pc-windows-gnu build
.\tests\run_oracle.ps1
.\tests\run_c_abi_smoke.ps1
```

The source license notices are recorded in `LICENSE-NOTICE.md` and the copied
PlayFair GPL text. Rust Anything's generic recorder cannot model opaque C handles
with fixed-size buffers and cross-call state; see `GATE_NOTES.md` for that gate
limitation.
