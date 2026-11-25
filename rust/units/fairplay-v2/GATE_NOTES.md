# Rust Anything gate notes

## Unit and source

- Unit: `lib/fairplay_v2/fairplay_v2.c` (five C exports with an opaque mutable pointer handle).
- Reference source and header remain read-only at `D:\Save\Uxplay\UxPlay\lib\fairplay_v2\`.
- The rust-anything `bridge` was attempted with `force: true`; it rejected the unit because every export passes or returns `fairplay_v2_t *` and asks for an ownership design. The crate therefore keeps the C ABI manually and ports the implementation behind that boundary.
- Secure random bytes reuse the completed `uxplay-crypto` facade. Digest, HMAC, AES-ECB, and constant-time equality use the same OpenSSL crate/runtime directly where the existing facade does not expose the required operation.
- `record` cannot safely call these functions from generated JSON scalar cases: setup, handshake, and decrypt require fixed-size buffers plus a live stateful context, while init returns the pointer needed by every following call. The C recorder has no lifecycle/opaque-pointer scenario model for this interface. Running it with empty parameter descriptions would not characterize the original protocol.

## Verification

- `cargo +stable-x86_64-pc-windows-gnu fmt -- --check`: passed.
- `cargo +stable-x86_64-pc-windows-gnu clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo +stable-x86_64-pc-windows-gnu test`: passed (2 tests).
- `cargo +stable-x86_64-pc-windows-gnu build`: passed.
- `tests/run_oracle.ps1`: C reference and Rust transcripts match for all four selector modes, setup responses, successful handshake and media-key decryption, invalid key and handshake MACs, zeroed key output, invalid setup output preservation, and handshake rejection after state reset.
- `tests/run_c_abi_smoke.ps1`: passed against the Rust DLL and copied C header.

The rust-anything gate was run with `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`
and the MSYS2 OpenSSL environment. It passed fmt, Clippy, cargo tests, unsafe
comment audit, build, and the source/error/size checks. Its only failing check
is `behavior: no recorded cases`. The desktop gate tool's default invocation
uses the MSVC toolchain, which has no `cargo-fmt`, `cargo-clippy`, or `link.exe`
installed here; selecting the installed GNU toolchain resolves those checks.

The source's OpenSSL build policy is retained. This local proof used the
MSYS2 `ucrt64` shared OpenSSL installation (`OPENSSL_STATIC=0`). Other platform
toolchains and static OpenSSL packaging were not exercised in this unit task.

Per the skill, this unit is not marked as rust-anything gate-complete: the
manual C differential and ABI proofs pass, but the official behavior gate
cannot represent this pointer-lifecycle unit yet.
