# Cryptography and pairing research

## Scope found in the reference source

The crypto layer is broader than “hashing” and is on the AirPlay wire path:

| Source | Operations that the Rust layer must preserve |
| --- | --- |
| lib/crypto.c | AES-128-CTR, AES-128-CBC, AES-128-GCM; SHA-512 and MD5 digest contexts; Ed25519 key generation/import/export/sign/verify; X25519 key generation/raw public-key import/export/shared-secret derivation; random bytes. |
| lib/pairing.c | SHA-512-based key derivation, pairing/session state, Ed25519 signatures and X25519 handshake through crypto.c, AES block encryption/decryption, digest authentication. |
| lib/srp.c | SRP-6a verifier and authentication state, BIGNUM arithmetic and modular exponentiation, SHA-1/224/256/384/512, random salt/ephemeral values. It has an Apple pairing variant and a Windows CryptoAPI path for seeding. |
| lib/fairplay_v2/fairplay_v2.c | Protocol-specific transform plus OpenSSL SHA-1, MD5, HMAC-SHA1, AES-128-ECB, random bytes, and constant-time comparisons. |
| lib/fairplay_playfair.c and lib/playfair/* | Legacy FairPlay handshake/decrypt logic and fixed protocol tables. |

The source CMake requires OpenSSL 1.1.1 or later for the crypto layer on Windows and Unix; macOS also links OpenSSL, with source comments documenting static OpenSSL builds. The target has no crypto tests today, so behavior needs recorded cases and deterministic known-answer tests around the crypto facade.

## Recommendation

Start with a single Rust crypto facade backed by the openssl crate, version 0.10, and preserve the source algorithms and call semantics first. This is the closest mapping to the existing implementation: the crate exposes safe wrappers for EVP cipher/digest contexts, PKey/key I/O/signatures, BigNum arithmetic, random bytes, and constant-time comparison. The documented BigNum mod_exp corresponds directly to BN_mod_exp; BigNum also exposes set_const_time for secret exponent values. This avoids replacing several protocol primitives at once while preserving the existing OpenSSL dependency and the existing 1.1.1 minimum.

Keep the facade narrow and explicit: digest/update/final, AES context init/update/final/reset, AEAD tag set/get, raw and PEM key conversion, Ed25519 sign/verify, X25519 derive, constant-time equality, secure randomness, and SRP integer operations. Add tests against captured outputs before replacing any caller. Do not use panic-based error handling for peer-controlled inputs.

For any SRP secret exponent or secret intermediate that reaches modular exponentiation, explicitly review constant-time handling and secret lifetime. The Rust OpenSSL wrapper exposes BigNum::set_const_time and BigNum::new_secure, but choosing those flags must be based on the original call site and OpenSSL behavior rather than applied mechanically. Use openssl::memcmp::eq for fixed-length authentication/tag comparisons corresponding to CRYPTO_memcmp.

### Build and platform tradeoff

Using openssl preserves the project’s existing native libcrypto requirement; it removes UxPlay’s C implementation from the Rust target but does not make the cryptographic backend a pure-Rust implementation. The openssl crate can use a system OpenSSL found through pkg-config/Homebrew/vcpkg, or its vendored feature. Vendoring compiles OpenSSL from source and requires a C compiler, Perl, and make; it therefore increases build time and still uses native C code. Since the accepted goal preserves all existing platforms and the source already requires OpenSSL, prefer the system library initially and test the platform matrix. Use vendoring only if packaging requires it and each target can build it.

If “pure Rust” is intended to mean no native OpenSSL dependency at all, RustCrypto is a plausible later replacement for several primitives, but it is a larger compatibility project: AES modes, MD5/SHA variants, HMAC, Ed25519, X25519, PEM handling, and SRP big-number arithmetic span separate APIs. SRP constant-time arithmetic and serialized byte behavior remain the gating items. Do not replace libcrypto solely to make Cargo.toml look Rust-only.

## Alternatives and fit

| Candidate | Fit | What it covers / what remains |
| --- | --- | --- |
| openssl 0.10 | Best phase-one fit | Same OpenSSL API family and broadest coverage. Still requires OpenSSL or vendored C build; platform packaging remains. |
| ring | Partial | Docs expose AES-GCM, X25519, Ed25519, SHA-1 (legacy), SHA-2 and HMAC. It is not a drop-in for CBC, CTR, MD5, PEM key-file behavior, or SRP BIGNUM arithmetic. |
| RustCrypto crates | Partial, best route if native OpenSSL must eventually go | aes-gcm is pure Rust; ed25519-dalek and x25519-dalek provide pure-Rust Ed25519/X25519. CBC/CTR, MD5/SHA-1, HMAC, PEM, and SRP still need separate crates and exact byte-level integration. |
| Generic big-integer crates | Unknown for SRP secrets | Do not select an integer implementation until its constant-time and zeroization properties are verified for the operations used by SRP. Avoid treating ordinary arbitrary-precision arithmetic as suitable for secret exponents by default. |

## Semantic traps to record and test

- AES-CTR has no padding; CBC uses EVP’s padding behavior unless the caller disables it; FairPlay ECB explicitly disables padding. Preserve each mode’s exact init/update/final/reset sequence and output length.
- GCM’s IV length, tag length, and tag placement are protocol-visible. Authentication failure must not leave unauthenticated plaintext available to callers.
- EVP digest calls are streaming and sometimes reset/reuse a context. Hash concatenation order and fixed prefixes/suffixes in FairPlay are exact protocol data.
- SRP has multiple hash algorithms and group constants, plus the Apple variant. Integer byte width matters: some hashes consume minimal-width BIGNUM bytes while RFC-style formulas pad operands to group width. Preserve exact leading-zero behavior.
- X25519 and Ed25519 use raw 32-byte public/secret values in protocol frames, while persisted Ed25519 keys are PEM. The source has a device-ID-derived Ed25519 path as well as random key generation and file load/save.
- Random outputs make whole-process captures differ. Keep production randomness cryptographically secure; use deterministic test vectors or a test-only random provider for unit proofs, never a fixed production seed.
- The C SRP source is MIT-licensed; retain its notice if ported code is derived from it. Review the FairPlay and Playfair data/code notices before copying constants or logic.
- The C analyzer’s “dynamic code (eval/new Function/computed imports)” blocker is not present in the reviewed crypto files: a targeted search for eval, dynamic loader calls, or computed imports returned no matches. Treat that generic flag as unverified for C, not as evidence that the crypto module is impossible to port.

## Primary sources

- [OpenSSL crate API and build modes](https://docs.rs/openssl/latest/openssl/)
- [OpenSSL crate BigNum API](https://docs.rs/openssl/latest/openssl/bn/struct.BigNum.html)
- [OpenSSL crate BigNumContext](https://docs.rs/openssl/latest/openssl/bn/struct.BigNumContext.html)
- [OpenSSL crate CipherCtx](https://docs.rs/openssl/latest/openssl/cipher_ctx/struct.CipherCtx.html)
- [OpenSSL crate constant-time comparison](https://docs.rs/openssl/latest/openssl/memcmp/index.html)
- [OpenSSL crate source for BigNum constant-time flags](https://docs.rs/openssl/latest/src/openssl/bn.rs.html)
- [ring algorithm inventory](https://docs.rs/ring/latest/ring/all.html)
- [RustCrypto AES-GCM API and security notes](https://docs.rs/aes-gcm/latest/aes_gcm/)
- [ed25519-dalek SigningKey API](https://docs.rs/ed25519-dalek/latest/ed25519_dalek/struct.SigningKey.html)
- [x25519-dalek StaticSecret API](https://docs.rs/x25519-dalek/latest/x25519_dalek/struct.StaticSecret.html)
