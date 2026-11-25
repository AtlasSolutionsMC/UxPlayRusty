# Pairing gate evidence and limits

## Completed checks

- Windows GNU `cargo test` passes 9/9 unit tests. `cargo clippy --all-targets -- -D warnings` and `cargo fmt -- --check` pass.
- The C harness compiles the unchanged `pairing.c`, `crypto.c`, `srp.c`, and `utils.c` against MSYS2 UCRT OpenSSL. Its checked-in 10-row oracle matches deterministic identity creation, public key, status values, untouched output buffer, deterministic nonce, and valid/invalid-password digest authentication.
- The app connection adapter now wires `/pair-setup`, `/pair-setup-pin`, and both `/pair-verify` steps to `uxplay_pairing`/`uxplay_srp`; successful setup state is passed to RAOP SETUP, where the source-compatible callback effects persist the paired client key. A protocol regression preserves C's conditional PIN-mode registration check: Initial/Finished sessions check registration, while a session already in Setup proceeds to Pair Verify and is registered at SETUP.
- `protocol_connection::tests::pin_pairing_completes_crypto_setup_and_encrypted_audio_delivery` runs a valid in-process PIN client through SRP M1/HAMK, both AES-GCM identity messages, both Pair Verify signatures and the source CTR fake round, FairPlay setup/handshake, then first SETUP/RECORD. The decrypted FairPlay key matches the unchanged-C mode-2 oracle fixture; Pair Verify ECDH mixing reaches RAOP session state. A resulting encrypted RTP datagram reaches the app receiver, is decrypted and written via the audio callback to an ALAC dump whose bytes are asserted; FLUSH and audio TEARDOWN are also checked. Registration callback carries the exact client id, base64 key and name. The app gate passes 497/497 by default and 499/499 with `external-dns-sd`; both Clippy variants and formatting pass. The feature check and optimized build passed before this test-only extension. GStreamer playback remains disabled/unverified in this headless transcript.
- The oracle intentionally avoids malformed digest strings: the C implementation dereferences missing parsed fields and crashes for that input. The Rust API returns `false` instead.
- Windows GNU test/build needs `cargo:rustc-link-arg=-lbcrypt` appended after OpenSSL static libraries. Target-specific `build.rs` files now provide this for the crypto, SRP, and pairing crates.
- The original source tree was only read; the oracle runner links it directly from `D:\Save\Uxplay\UxPlay\lib`.

## Remaining gate limits

- `rust-anything gate` is not complete. It requires bridge-generated `rust-anything.json`, but the current C bridge generator cannot scaffold this pointer/stateful unit. No metadata was fabricated for the manually authored safe API.
- C differential coverage is focused on deterministic, stable outputs; randomized X25519/SRP internals and Pair Setup error exits still rely on Rust tests and source review. The new client/server transcript is internal simulation evidence, not a C transcript differential or proof of live Apple-client interoperability.
- Only Windows GNU runtime tests ran here. Native Linux, macOS and BSD OpenSSL builds and real Apple-client end-to-end pairing/session acceptance remain open.

The unit and app route are behavior-tested with a C library oracle and local connection-level tests, but remain partial: the generic `rust-anything` gate is incomplete for this pointer/state API, and real Apple pairing/session interoperability is unverified.
