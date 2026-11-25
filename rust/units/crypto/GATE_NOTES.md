# Rust Anything behavior-gate exception

`rust-anything gate rust/units/crypto` currently fails only its `behavior` check because this
crate has no `cases.jsonl`. The C recorder cannot produce usable cases for this unit:

- Its source build links the staged C files with `-lm` but omits OpenSSL (`-lcrypto`), so
  `rust-anything record rust/units/crypto --count 1` stops at link time with unresolved
  OpenSSL symbols such as `BIO_new_fp`, `EVP_MD_CTX_new`, and `RAND_bytes`.
- The generated manifest has empty `params` arrays for all 38 exports, although their native
  prototypes take buffers and opaque context/key pointers. The generic C recorder builds its
  ctypes signatures from those arrays, so it cannot describe or replay this stateful pointer API
  as JSON scalar arguments.

The linked C ABI reference harness at `tests/c_abi_smoke.c`, run by
`tests/run_c_abi_smoke.ps1`, compiles untouched `lib/crypto.c` against OpenSSL and compares the
Rust library's exported symbols and deterministic behavior. It also checks the full GCM output
buffer after authentication failure and compares the exit code and OpenSSL error log for an
operational failure. The Rust unit tests cover deterministic NIST/RFC vectors; random outputs
are checked by invariants and key-file round trips.

Latest native proof on Windows GNU/OpenSSL: `cargo fmt -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build`, and
`tests/run_c_abi_smoke.ps1` all pass. The rust-anything gate reports every check green except
`behavior: no recorded cases` for the recorder limitations above.

For consumers linking this crate on Windows GNU, `build.rs` appends `-lbcrypt` after the
OpenSSL archives because OpenSSL 4.1's Windows random provider calls `BCryptGenRandom`.
