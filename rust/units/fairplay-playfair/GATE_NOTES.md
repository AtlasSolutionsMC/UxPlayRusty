# Rust Anything gate notes

## Unit and source

- Combined unit: `lib/fairplay_playfair.c`, `lib/playfair/playfair.c`, and the
  required algorithm/tables from `lib/playfair/omg_hax.c` and `omg_hax.h`.
- Reference files remain read-only under `D:\Save\Uxplay\UxPlay\lib\`.
- This is a manually assembled C ABI bridge. Rust Anything's bridge and recorder
  cannot express the opaque context pointer plus fixed-size request/response
  buffers whose values depend on earlier setup and handshake calls.
- The runtime implementation is Rust. Original C files are compiled only by the
  differential oracle; they are not included in the Rust crate or final app.
- Selector 2 calls the existing Rust FairPlay-v2 crate. The v3 key derivation
  calls the Rust modified-MD5 and SAP-hash crates; SAP-hash calls hand-garble.

## Verification and remaining gate limitation

The targeted C oracle and Rust transcript match across all four legacy modes,
invalid selector-2 setup, and adapter reset/error paths. The Rust suite has five
passing tests, and the C ABI smoke passes. Rust Anything's fmt, clippy, test,
unsafe-comment, lockfile, panic, and build checks pass. The recorder does not
support this lifecycle surface, so no `cases.jsonl` can be generated without an
unsafe fake scalar API.
The gate therefore reports `behavior: no recorded cases` even when compilation,
tests, and the direct C differential pass. Its size check also counts only the
manifest's single `unit` C file (the 106-code-line adapter), not the dependent
`playfair.c` and `omg_hax.c` implementation included in this combined unit. The
651 Rust lines include the complete safe port, FFI boundary, oracle runner, and
tests; the combined C source scope is larger than that one-file denominator.
This is a manifest limitation, not a per-file translation ratio for the combined
unit. The unit must not be reported as official gate-complete until Rust Anything
can record opaque stateful fixed-buffer C calls and represent multiple files in
one unit for its size check.

The local build proof uses MSYS2 UCRT64 OpenSSL and the Windows GNU Rust target;
other supported operating systems have not been built in this isolated unit.
