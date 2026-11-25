# SRP unit checkpoints

1. Read `rust/research/crypto.md` and inspect `lib/srp.c` / `lib/srp.h`. Recorded the active Apple-variant server exports, byte encoding, hash behavior, RFC compatibility branch, and the disabled `SRPUser` scope in `BEHAVIOR.md`.
2. Added an isolated `uxplay-srp-unit` crate with safe SRP operations and a narrow C ABI adapter. Secret OpenSSL big numbers are cleared on drop, returned session/proof material is cleansed, and client proof comparison is constant-time.
3. Added the original `srp.h` header and `tests/srp_c_diff.c`, which links the untouched C implementation beside the Rust library. The differential passed all five hashes in both encoding modes on a custom group, plus standard 2048-bit SHA-512 RFC mode, successful/wrong proof behavior, session key/HAMK, rejected `A=0` / `A=N`, and Rust-boundary null/nonempty pointer guards.
4. Native Rust proof passed with the installed `stable-x86_64-pc-windows-gnu` toolchain: `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build`, and `tests/run_c_diff.ps1`.
5. Ran `rust-anything gate`. Its unsafe and manifest checks pass. The tool's process uses the default `stable-x86_64-pc-windows-msvc` toolchain, which lacks rustfmt, clippy, and `link.exe`; its generic behavior check also has no JSON cases because this opaque pointer/state API cannot be safely recorded by its scalar C recorder. The custom C differential is the unit behavior gate; generic gate status remains incomplete for those environment/recorder limitations.

This unit does not include pairing-handler dispatch, request/session integration, or the disabled client-side SRP API.
