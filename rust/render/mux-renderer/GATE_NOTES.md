# rust-anything gate evidence and limitation

The C unit and its recorded fixture are isolated under `record_shims/`. The
whole-app rust-anything recorder was also attempted with the bridge generated
for `renderers/mux_renderer.c`:

```powershell
rust-anything record . --inputs .\record_shims\abi_cases.jsonl
```

That recorder compiles every C source in the original UxPlay tree, without the
selected unit's GStreamer `pkg-config` flags. It fails on the optional
`dns_sd.h`, missing `gst/gst.h` include flags, and an unrelated libplist API
compile error in `lib/http_handlers.h`. No `cases.jsonl` is produced, so
`rust-anything gate` reports all static checks and Rust tests passing, then fails
the behavior check with `no recorded cases`.

The unit has a separate reproducible C differential instead: the runner compiles
the unchanged `mux_renderer.c` with UCRT64 C/GStreamer flags, starts the real
GStreamer pipelines, and captures logger/pipeline output plus appsrc-pad PTS,
DTS, and duration. It matches all 45 checked-in rows. Rust tests replay the
lifecycle rows and inspect buffer timestamps through the same installed
GStreamer runtime (7 unit tests and 2 C-oracle integration tests passed).

The native Windows GNU build and clippy pass. Linux/macOS/FreeBSD `cargo check`
passes when using the host UCRT64 `.pc` metadata with `PKG_CONFIG_ALLOW_CROSS=1`;
these only type-check Rust bindings and do not prove native linking or plugin
availability. Actual encoded media playback/finalization and client behavior
remain unverified. Keep the unit partial until the whole-source recorder or an
equivalent registered rust-anything case harness can gate the behavior.
