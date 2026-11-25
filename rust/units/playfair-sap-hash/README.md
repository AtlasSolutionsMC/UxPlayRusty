# sap-hash-rs

Safe Rust implementation of `lib/playfair/sap_hash.c` and a narrow C ABI wrapper for the original 64-byte input / 16-byte output `sap_hash` contract. It uses the safe fixed-array `garble` API from `playfair-hand-garble`.

## Proof

- `rol8` and `rol8x` pass the `rust-anything` scalar gate (90/90 recorded cases).
- `sap_hash` matches five C-generated vectors in `sap-hash-cases.tsv` and a linked C ABI smoke test against the original `sap_hash.c` plus `hand_garble.c`.
- The C reference vectors use MSYS2 UCRT64 GCC 16.2.0, x86_64 MinGW-w64. Source SHA-256: `sap_hash.c` `B6366CF49DEA6CFDAEA10806EE4B93E615106A255CDF4ED5532E7F59C0A03DEF`; `hand_garble.c` `A6E3F762D8E9925323A364B34239432C9E480A6473D067F124A178880FF33D17`.
- `cargo fmt --check`, Clippy with `-D warnings`, `cargo test`, and `rust-anything gate` pass with overflow checks explicitly enabled.

The `rust-anything` bridge metadata records only the scalar exports because the generic bridge cannot model the original pointer API; the safe wrapper and pointer behavior are proved by the fixtures and linked C smoke test.
