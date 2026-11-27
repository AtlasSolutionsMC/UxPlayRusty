# Pairing behavior record

Reference: `D:/Save/Uxplay/UxPlay/lib/pairing.c` and `lib/pairing.h` (read-only).

## Exported behavior ported

- Persistent Ed25519 identity loading/generation and raw public key extraction.
- Pairing session states `INITIAL`, `SETUP`, `HANDSHAKE`, `FINISHED`, X25519 exchange, Ed25519 signature exchange, SHA-512 key derivation, and AES-128-CTR wrapping.
- Nonce generation as SHA-512 of native-endian `uint64_t`, C-string client data, and local Ed25519 public key, truncated to caller length.
- RFC 2617 MD5 Digest authentication with source token scan order, optional qop/nc/cnonce, lower-case hex digest, and case-sensitive response comparison.
- SRP-2048/SHA-1 server offer and proof validation using the sibling `uxplay-srp` unit. The username limit is 24 bytes; salt/verifier are left-padded into protocol-width storage; server B uses the original RFC 5054 compatibility mode.
- Pair-Setup AES-GCM client key authentication and server key response. SRP state is discarded before tag verification; IV byte 15 is incremented before decrypt and again before server encrypt.
- Pair-setup client key base64 encoding and source-compatible random PIN range.

## Deliberate safe-boundary behavior

Malformed/missing digest fields and invalid short SRP proofs return a failed result rather than dereferencing null or undersized C pointers. Methods return the original numeric status values on validly representable source outcomes; OpenSSL/SRP operational failures are surfaced as `PairingError` rather than terminating the process. C-owned malloc/free pointer lifetimes are represented with Rust-owned values. Fixed-size secrets are cleansed when session/SRP state drops.

## Compatibility quirk preserved

`srp_new_user` returns a pointer to the *start* of the padded 16-byte salt array while reporting the minimal unpadded salt length. Thus a leading-zero salt is exposed as a prefix of the padded array; the Rust `SrpOffer.salt` reproduces that exact slice.

## Pending verification

Run this crate's tests with `cargo test --manifest-path units/pairing/Cargo.toml`.
Platform-specific behavior should be verified on each supported operating system.
