# RAOP video mirror port notes

## Current scope

`src/lib.rs` ports packet interpretation from `lib/raop_rtp_mirror.c`: the 128-byte header,
remote timestamp adjustment, H.264 and H.265 codec setup, SPS/PPS/VPS capture, video suspend and
resume markers, AES-CTR payload decryption, length-prefixed NAL validation, Annex-B start codes,
parameter-set prepending, invalid-payload marking, and the 25,000-byte client-report trailer.

`src/runtime.rs` owns the IPv4/IPv6 TCP listener and its worker lifecycle. It accepts fragmented
header and payload writes, uses the source's short receive timeout, preserves the single accepted
stream at a time, handles reconnects, and calls typed callbacks for codec, frame, reset and running
state.

## Evidence

- `reference/run-c-oracle.ps1` builds the unmodified CMake project out of tree and exercises the
  original `raop_rtp_mirror_init`, AES setup, TCP listener, codec packet and encrypted H.264 frame.
  Rust matches its two callback rows byte-for-byte in the unit test fixture.
- Windows GNU `cargo test --all-targets` passes 6/6. One test opens the Rust listener and sends the
  same packet shapes in 11-byte fragments.
- Windows GNU `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass.
- Linux and FreeBSD `cargo check` source/type checks pass using the installed Windows GCC only to
  preprocess OpenSSL headers; they are not native target builds or runtime checks. The macOS check
  could not run because this host has no Clang/Mac SDK and its GCC rejects Apple's `-arch` flags.
- The C oracle and live listener were run on this Windows host; none of these checks are native
  Linux, macOS, or BSD runtime validation.

## App integration and remaining acceptance

- The crate is wired through `rust/app/src/raop_media_runtime.rs`: RAOP SETUP's `MirrorSetup`
  constructs a `MirrorReceiver` with the FairPlay-derived audio key; the stream connection ID
  initializes mirror AES; `StartMirror` binds the negotiated TCP port; and codec, frame, reset,
  and client-report events flow through the app's renderer/output callbacks.
- `ClientStreamingReport` delivers stripped binary plist bytes; the source logs XML when
  client-FPS reporting is enabled. The app's current callback path does not yet reproduce that
  XML diagnostic.
- App callback wiring, renderer integration, H.264 dump output, and mux input are implemented,
  but no Apple sender has exercised encrypted live video, and native Linux/macOS/BSD runtime
  checks remain open.
- The source's extensive debug/error log wording is not reproduced here.
- No generic `rust-anything gate` was generated for this stateful socket/callback API. The checked-in
  CMake oracle directly executes the unchanged source instead.

Keep this unit partial until the outstanding real-world and cross-platform acceptance evidence is
available.
