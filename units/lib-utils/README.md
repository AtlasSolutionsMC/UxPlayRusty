# `lib/utils.c` Rust unit

Safe implementations of every export in the isolated `utils.c` source are in `src/port.rs`. The C ABI adapters are isolated in `src/ffi.rs`; they expose original C names for replacement builds and `rust_anything_*` aliases for side-by-side verification. Pointer validation and memory access stay in the adapter layer.

The rust-anything bridge analyzer records scalar `parse_int` cases. The pointer-based APIs need explicit buffers and ownership, so `tests/ffi_smoke.c` links the staged C reference into a small process, loads the Rust shadow DLL, and compares each exported API directly.

## Reference vectors

The C differential smoke uses these stable vectors:

- `utils_strsep("left::right::", "::")` yields `left`, `right`, the trailing empty token, then `NULL`.
- RAOP bytes `00 0f a0 ff` format as `000FA0FF`; AirPlay formats as `00:0f:a0:ff`.
- Hex `00aFff` decodes to bytes `00 af ff`; those bytes format as `00afff` and as data with a two-byte line width to `00 af \nff \n`.
- IPv4 bytes `c0 a8 01 07` format as `192.168.1.7`; the link-local IPv6 vector formats as `fe80::0001:0203:0405:0607%3`.
- The timestamp `1700000000123456789` is compared byte-for-byte against C for both local-time output functions, using the host's current timezone.
- Text bytes `61 0d 62 00 0d` become `61 20 62 00 00 00`; file bytes `00 01 0d ff`, multiline plist XML, `parse_int("+42") == 42`, and the GMT suffix/shape are also checked in-process.

## Verification

On Windows with the Rust GNU toolchain, MSYS2 UCRT64 GCC, and Python available to rust-anything:

```powershell
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --features shadow
cargo build --release --features shadow
gcc -Wall -Wextra tests/ffi_smoke.c -o tests/ffi_smoke.exe
.\tests\ffi_smoke.exe (Resolve-Path target/release/utils_rs.dll).Path
rust-anything gate <absolute-path-to-this-crate>
```

The C smoke frees Rust-owned allocations with the matching `msvcrt.dll` allocator used by this GNU-target DLL. Timestamp formatting uses the host local timezone. `gmt_time_string` emits the C-locale English `%c` layout; a non-C process locale can produce a different string in the original C function.
