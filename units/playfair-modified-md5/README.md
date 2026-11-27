# modified-md5-rs

Safe Rust port of the scalar and block-processing helpers in `lib/playfair/modified_md5.c`.

## Exports

- `F`, `G`, `H`, `I`, `rol`: bridged scalar exports; recorded and checked through `rust-anything`.
- `swap`: safe `swap_pair` helper plus a narrow C ABI wrapper for two `uint32_t` pointers.
- `modified_md5`: safe slice helper for a 64-byte block and 16-byte key, plus a narrow C ABI wrapper that writes 16 output bytes. Native-endian key/output words and the source's round-31 word swaps are preserved.

The generic C recorder does not capture pointer-based exports. Those functions are covered by five C-generated fixtures in `fixtures/reference-vectors.txt`, unit tests, and the linked smoke harness in `fixtures/ffi_link_smoke.c`. See `fixtures/README.md` for source hash and regeneration details.

## Proof commands

```powershell
rust-anything record . --count 0 --inputs record-inputs.jsonl
rust-anything build .
rust-anything diff .
rust-anything bench .
rust-anything gate .
```

The Rust crate tests also compare all 64 round constants and each pointer export against the C fixtures.
