# Video renderer port evidence

## Implemented behavior

- H.264, H.265, JPEG cover-art, and HLS pipeline ownership and GStreamer state transitions.
- Parser, decoder, converter, sink, RTP suffix, sink options, codec-name rewriting, videoflip mappings, live appsrc caps, buffer timestamps, pause/resume, seek, volume, metadata overlay, EOS, bus watch, plugin lookup, and `GstVideoOverlay` window handles.
- The original `X_DISPLAY_FIX` X11 window discovery/fullscreen/key handling remains outside this crate. `waiting_for_x11_window()` returns false without that optional adapter.
- The safe Rust API accepts a logger callback. The transitional C initializer currently ignores its opaque `logger_t*`; its logging callback semantics are not represented by this crate's C ABI.

## Differential evidence

`record_shims/record-oracle.ps1` compiles the unmodified reference source and `lib/logger.c` with the installed MSYS2 UCRT64 GCC and GStreamer 1.28.9 libraries. It records eight pipeline descriptions covering default H.264, H.265 plus JPEG, orientation, RTP mode, and sink options. It also records the original playbin flags value (`1943`) for playbin3.

The Rust tests compare all eight pipeline strings and the playbin flags value to those C outputs. The pipeline tests pass. The C source tree remains read-only.

## Checks run

- `cargo fmt --check` with `stable-x86_64-pc-windows-gnu`: passed.
- `cargo clippy --all-targets -- -D warnings` with `stable-x86_64-pc-windows-gnu`: passed.
- `cargo test -- --test-threads=1` with `stable-x86_64-pc-windows-gnu`: 7 passed, including local GStreamer pipeline construction, codec selection, and playbin flags.
- `cargo build --release` with `stable-x86_64-pc-windows-gnu`: passed.

## rust-anything gate limits

- `rust-anything record` could not start its generic C recorder: `running cc (original sources): program not found`. The custom C oracle above provides focused differential cases instead; `rust-anything` still has no `cases.jsonl` for this crate.
- `rust-anything gate` ran under the default `stable-x86_64-pc-windows-msvc` toolchain. That toolchain has no installed `cargo-fmt`/`cargo-clippy` and no `link.exe`, so its fmt, clippy, test, and build checks could not run. Direct checks under the installed GNU/MinGW toolchain passed.
- The `rust-anything gate` behavior check therefore remains incomplete. Do not mark this unit gate-passed until recorded behavior is accepted by that gate and the X11 path is addressed or explicitly excluded by the root migration decision.
