# `lib/stream.h` port notes

## Scope

`src/lib.rs` defines `#[repr(C)]` Rust counterparts for `video_decode_struct` and
`audio_decode_struct`. The original header only declares callback data layouts; it has no exported
functions to record or diff. No app code consumes these structs yet.

## Evidence

- `reference/run-layout-oracle.ps1` compiles a C program against the unchanged reference
  `lib/stream.h` and compares size, alignment, and every field offset with the Rust layouts on the
  same target.
- Rust tests cover zero/default initialization. The layout comparison is the source differential.
- This header-only unit has no callable API for a `rust-anything gate`; do not mark it complete
  based on the local type test alone. Re-run the C oracle on native Linux, macOS, BSD, and Windows
  toolchains as those hosts become available.

## Integration

The structs retain raw pointer fields solely to match the source layout. They do not provide pointer
dereference helpers; app/session code should use owned or borrowed Rust frame types. The application
does not use this crate yet.
