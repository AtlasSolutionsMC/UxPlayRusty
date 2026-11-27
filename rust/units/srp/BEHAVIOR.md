# SRP source behavior record

Source: the original `lib/srp.c` and `lib/srp.h` (MIT licensed; original notice retained in the copied header).

The active header enables `APPLE_VARIANT`. It exposes ten server-side functions around five hash enums (SHA-1, SHA-224, SHA-256, SHA-384, SHA-512), one active RFC 5054 group (2048-bit, generator 2), and custom hexadecimal groups. The SRP user/client implementation is under `#if 0` and is not a source export. `hash_reset` has external linkage in the C file but takes the private `HashCTX_t`, is not declared by `srp.h`, and is only an implementation helper; it is excluded from the public C API port.

Recorded semantics:

- `srp_random_seed` marks the library initialized and passes non-null seed bytes to OpenSSL's RNG. Null bytes skip the seed call.
- Salted-verifier creation generates a `BN_rand(..., 128, -1, 0)` salt, hashes `username || ':' || password`, computes `x = H(s || H(username:password))`, then `v = g^x mod N`. The salt is Apple variant 128-bit width, encoded as minimal unsigned big-endian bytes.
- Numeric values used in hashes are generally minimal unsigned big-endian. RFC 5054 `H(N || g)` and `H(A || B)` inputs are each padded to the byte width of N. In non-RFC mode the multiplier and public value use the original unpadded formulas.
- The Apple server ephemeral API computes `B = (k*v mod N + g^b mod N) mod N` in RFC mode and `B = k*v + g^b mod N` in original mode. The supplied server secret makes this operation deterministic.
- `srp_verifier_new` rejects `A mod N == 0`, computes B, u, S, a two-digest Apple session key (`H(S || 00000000) || H(S || 00000001)`), client proof M, and server proof HAMK. It copies the username and owns the returned B bytes until verifier deletion.
- A matching proof sets authentication and returns a pointer to verifier-owned HAMK; a mismatch returns null and leaves authentication state unchanged. Authentication remains true after a later mismatch.
- Session-key bytes and HAMK are verifier-owned pointers valid until deletion. Registration and standalone ephemeral output buffers are C-allocator-owned and caller-freed.

`rust/units/srp/tests/srp_c_diff.c` records/diffs deterministic server and verifier paths against the untouched C implementation, then checks both a rejected proof and successful proof using the C source's computed M as the oracle. The random registration export is checked for Apple salt/verifier shape, then those same C-generated salt/verifier bytes feed both implementations. This avoids treating random outputs as fixed vectors.

Invalid pointer/length combinations are rejected at the Rust boundary. They are not run through the C oracle because the original API dereferences them without validation (undefined behavior). Invalid public values A=0 and A=N are compared directly because the C implementation has a defined SRP-6a rejection path.

Known failure-policy difference: OpenSSL hash/backend errors return the Rust wrapper's null/zero result; the C hash helpers call `handle_error` and terminate the process. This operational failure path was not injected in the differential harness.
