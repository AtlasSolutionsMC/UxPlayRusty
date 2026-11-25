# RAOP RTP audio session port

`src/lib.rs` ports the session behavior from `lib/raop_rtp.c` into a safe Rust API:

- numeric IPv4/IPv6 remote validation and negotiated UDP bind with ephemeral-port writeback;
- control/data receive loop, first control-peer capture, resend requests, and control sync packets;
- ALAC/AAC-ELD startup behavior, including the early AAC timing estimate and ALAC format-packet skip;
- RTP timestamp to client-NTP conversion, using the source's integer division for the sample clock;
- a 256-slot audio queue with per-packet AES-CBC decryption, clear no-data marker handling, missing-packet wait/skip, and resend-range selection;
- coalesced volume, flush, metadata, cover-art, DACP ID, and progress callbacks in source order;
- source-shaped audio startup and local control/data socket descriptor/port logs and receive/resend error code messages;
- control packet type/resend/sync diagnostics, short-packet dumps, resend request diagnostics, and per-frame DEBUG_DATA timing lines;
- start, stop, join, and current-running state.

The timing service and audio callbacks are injected through `TimingClock` and `AudioCallbacks`. The app integrates this crate with its RAOP NTP runtime and renderer. The app sets the detailed per-frame diagnostic gate from the configured logger level, matching the source's `LOGGER_DEBUG_DATA` check.

## Evidence

- C oracle extracted the private `rtp_time_to_client_ntp` implementation from the unchanged source and matched all 3 checked-in vectors.
- 15 Rust tests pass on Windows GNU, including AES-CBC fixture decryption, resend-window behavior, sync packet handling, ephemeral UDP start/stop, source-shaped socket and error diagnostics, per-frame DEBUG_DATA formatting, and a loopback sync-plus-audio exchange through the callback.
- `cargo fmt --check`, Clippy with `-D warnings`, and release build pass on Windows GNU.
- Linux cross-target `cargo check` stops in `openssl-sys`: this machine has no Linux OpenSSL sysroot/pkg-config setup. macOS and FreeBSD linked/source checks were not run for this isolated crate.

## Remaining integration/acceptance

- No real AirPlay sender, audible hardware output, or long-running playback session was available; loopback and local renderer tests do not prove Apple interoperability or audible playback.
- The adjacent app NTP runtime supplies Unix `SO_TIMESTAMP` and Windows QPC/`SIO_TIMESTAMPING` kernel receive-time support. Real Windows RxAll delivery and timing under adapter load remain unverified.
- RTP packet/error diagnostic parity is partial: source-shaped packet/timing text, receive/resend error code formats, and Windows error text through the shared C-compatible `wsa_strerror` port are covered. This nonblocking receive loop has no independent `select` error path.
- A generic `rust-anything gate` was not produced for this pointer/stateful C unit. The checked-in C oracle and Rust tests are current local behavior evidence; this unit remains partial until all accepted platform/runtime checks pass.
