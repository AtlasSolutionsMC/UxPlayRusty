# RAOP session handler port

## Scope

`src/lib.rs` models the state used by `raop_conn_t` for RAOP SETUP, RECORD, FLUSH, and TEARDOWN. It parses binary plists, stores timing/audio/mirror session state and negotiated ports/keys, creates binary-plist SETUP replies, and emits ordered `SessionEffect` values for socket, pairing, FairPlay, and renderer work.

- SETUP validates the 16-byte EIV and 72-byte EKEY, decrypts with the staged FairPlay unit, applies the paired-client key step as the first 16 bytes of SHA-512(clear FairPlay key || ECDH secret) when applicable (`sha_init()` in the unchanged C crypto facade is SHA-512), and records timing/audio/mirror initialization requests.
- SETUP handles type 96 audio and type 110 mirror streams, including local and remote ports, stream connection IDs, audio format inputs, HLS cleanup, unknown stream disconnects, and the response plist fields.
- RECORD reproduces the source latency calculation and both response headers.
- FLUSH reproduces the `RTP-Info: seq=` prefix check and `strtol` behavior before emitting the audio flush effect.
- TEARDOWN preserves type 96 precedence, the HLS reset reason, audio cover-art stop, mirror stop, full session destruction, and HLS connection reset behavior.
- `AudioPorts`, FairPlay decryption, pairing session data, and the mirror listener constructor reuse the staged `raop-rtp`, `fairplay-playfair`, `pairing`, and `raop-rtp-mirror` crates.

## Evidence

- `reference/run-c-oracle.ps1` compiles and runs `reference/session_oracle.c` with Windows GNU GCC. Its ten output rows cover the source C latency expression, flush parsing expression, and teardown precedence. This is a focused scalar oracle, not an invocation of the static RAOP handler functions.
- Seven Rust unit tests cover SETUP plist/key/port behavior, bad key sizes, RECORD arithmetic, FLUSH parsing, and the three TEARDOWN branches.
- On Windows GNU, `cargo test --all-targets` passes 7/7, `cargo clippy --all-targets -- -D warnings` passes, and `cargo build` passes. `cargo fmt` was applied; rerun `cargo fmt --check` as part of integration.

## Integration limits

- The app must run authentication and client-admission callbacks synchronously and pass their results through `SetupDecisions`; authorization challenges are passed through `authorization_challenge`. This crate does not implement digest authentication, random PIN management, or the callback itself.
- The returned `GetAudioFormat` and following `StartAudio` effects are ordered. The app effect runner must carry callback-mutated `AudioFormat` values into `StartAudio`.
- The current transition uses the configured local ports in its reply. If a local port is zero or a socket bind fails, the app must resolve the mirror/audio runtime result before sending the C-compatible negotiated-port reply and disconnect state. The current one-shot transition cannot await those effect results.
- `InitializeAudio`/`InitializeMirror` effects represent requested initialization; a later integration layer must feed back construction failures before finalizing SETUP's disconnect behavior.
- Binary-plist dictionaries are semantically equivalent, but their serialized key ordering may differ from libplist. Compare parsed values during app replay rather than requiring byte identity.
- The oracle checks scalar C expressions only. It does not exercise the original `raop_handler_setup` function, real UDP/TCP sockets, digest authentication, callback order, or platform-specific renderer behavior.

Keep this unit staged until the app adapter resolves the synchronous callback/effect results and replays real SETUP/TEARDOWN sessions against the unchanged C server.
