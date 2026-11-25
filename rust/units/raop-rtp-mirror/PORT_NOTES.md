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
- Windows GNU `cargo test --all-targets` passes 14/14. Tests cover source-oracle callbacks,
  fragmented listener input, re-keying, and source-shaped unexpected-NAL/invalid-framing diagnostics.
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
- `ClientStreamingReport` delivers stripped binary plist bytes. When client-FPS reporting is
  enabled, `RendererMirrorCallbacks` parses the plist and logs source-shaped XML at INFO. The app
  integration regression `mirror_runtime_logs_keep_source_levels_and_streaming_report_xml`
  checks the XML bytes and level.
- App callback wiring, renderer integration, H.264 dump output, and mux input are implemented,
  but no Apple sender has exercised encrypted live video, and native Linux/macOS/BSD runtime
  checks remain open.
- Unexpected H.264 NAL types and invalid NAL framing now preserve the source INFO/DEBUG messages,
  values, and parser offsets. The full per-NAL debug hex dumps for SPS/PPS/SEI remain omitted;
  packet lifecycle, timing, FPS plist, timestamp mismatch, and receiver errors have coverage.
- No generic `rust-anything gate` was generated for this stateful socket/callback API. The checked-in
  CMake oracle directly executes the unchanged source instead.

Keep this unit partial until the outstanding real-world and cross-platform acceptance evidence is
available.
