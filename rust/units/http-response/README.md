# http-response-rs

Rust port of the opaque `http_response_t` byte builder from UxPlay's `lib/http_response.c`.

## Compatibility surface

- Keeps the original nine C ABI symbols and opaque-handle ownership contract.
- Emits HTTP and reverse-HTTP lines, headers and binary bodies byte-for-byte, including CRLFs.
- Preserves the Content-Length behavior for nonempty bodies and the source's Content-Type search
  before adding `Content-Length: 0` to an empty response.
- Preserves disconnect state and the fact that reinitialization resets only the data length, not the
  `complete` or `disconnect` flags.

The Rust API and FFI implementation are in `src/port.rs` and `src/lib.rs`. Four unit tests cover
status/body bytes, empty-body variants, reinitialization/state and capacity growth. The linked C
smoke harness in `.rust-anything/ffi_link_smoke.c` compares the Rust shared library directly with a
symbol-renamed copy of the original C implementation for four end-to-end scenarios. The reference
copy in `../../staging/http-response/` is test-only and is not linked into the Rust library.

## Source provenance

- `D:\Save\Uxplay\UxPlay\lib\http_response.c`, SHA-256
  `6159594A4C2D2D5482DFB920A930B09CA60C17850A6F368BD606734689DBCEE9`
- `D:\Save\Uxplay\UxPlay\lib\http_response.h`, SHA-256
  `E9B14B46EBD6FBF8991E731CA77DBCFB87C5AF944436958DE88CA2B9BA53CAF5`

`rust-anything bridge` refused this unit because every export takes or returns opaque pointers;
structured ownership and multi-call state cannot be represented by its JSON bridge. The recorded
byte fixtures and linked C ABI comparison are the behavior proof for this manually designed unit.

## Verification

```powershell
cargo fmt --manifest-path rust/units/http-response/Cargo.toml --check
cargo clippy --manifest-path rust/units/http-response/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/units/http-response/Cargo.toml
cargo build --manifest-path rust/units/http-response/Cargo.toml --release
```

The C smoke harness is built with MSYS2 UCRT64 `cc`, linking the release DLL, then run with that
directory on `PATH`. It reports `4 C ABI scenarios match the original HTTP response builder`.
