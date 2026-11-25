# FairPlay selector-2 port checkpoints

- Read the Rust Anything skill, crypto research, and project ADRs 001–008.
- Reviewed the source and header; isolated all five public exports and their
  fixed record sizes, state reset rules, selector transforms, and crypto inputs.
- Attempted rust-anything `bridge`; it refused opaque pointer lifecycle exports.
- Created an isolated Rust crate with safe protocol logic, C ABI wrappers,
  copied public header, and OpenSSL-backed SHA/HMAC/ECB/random operations.
- Added deterministic C oracle and Rust protocol tests for modes 0–3, handshake,
  key unwrap/authentication, invalid MAC, and state reset.
- Ran the GNU/MSYS2 fmt, Clippy, tests, build, C differential, and C ABI smoke;
  all pass. rust-anything gate passes its source, build, and static checks with
  the GNU toolchain selected; its behavior gate remains blocked by the
  recorder's opaque-pointer limitation.
