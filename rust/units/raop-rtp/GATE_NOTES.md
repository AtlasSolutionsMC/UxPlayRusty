# Gate notes

The source `raop_rtp.c` owns a long-lived socket/session state machine with callbacks and pointer-backed packet buffers. The available rust-anything C recorder cannot express this stateful surface as recorded calls, so a generic `rust-anything gate` result is not available. No bridge metadata was fabricated.

Behavior checks currently are:

- `reference/run-rtp-time-oracle.ps1` extracts the unchanged C implementation of the private RTP-to-client-NTP helper, compiles it, and compares three checked-in rows.
- Windows GNU: 9 Rust tests pass; fmt, Clippy (`-D warnings`), and release build pass.
- The loopback test exercises two ephemeral UDP sockets, a source-format sync packet, an RTP audio packet, and callback values. This is local Windows networking, not an AirPlay-device replay.

`cargo check --target x86_64-unknown-linux-gnu` was attempted from Windows and stopped in `openssl-sys`, which needs a Linux C compiler and an OpenSSL target sysroot. The installed Windows OpenSSL headers are not a Linux sysroot. macOS and FreeBSD native/link acceptance remain open.
