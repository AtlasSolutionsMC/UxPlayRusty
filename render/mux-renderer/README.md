# mux-renderer-rs

Rust-owned port of `renderers/mux_renderer.c`. The original C implementation is
used read-only by the C oracle under `record_shims/`.

## API

- `MuxRenderer::new(MuxRendererConfig)` owns one recording lifecycle.
- `choose_audio_codec`, `choose_video_codec`, `push_audio`, and `push_video`
  mirror the source callbacks. Packet inputs are borrowed byte slices; timestamps
  are NTP nanoseconds as in the C unit.
- `stop` sends EOS, waits up to five seconds for EOS/error, releases the active
  pipeline, and resets codec selection. `destroy` also releases the renderer.
- `pipeline_plan` returns the source's MP4 path, launch string, and caps for
  inspection and deterministic characterization.
- The generated C ABI crate exposes the four bridgeable process-global methods.
  Install a Rust-owned instance with `install_global` before calling those
  wrappers. The pointer-based C `init` and packet callbacks remain represented
  by the safe instance API rather than raw-pointer FFI.

## Evidence

`record_shims/mux_renderer_oracle.ps1` compiles the unchanged C unit against the
installed GStreamer runtime and records five lifecycle scenarios plus source-pad
timestamp traces. The checked-in 45-row fixture compares logger output, pipeline
descriptions, file names, codec changes, packet PTS/DTS, and leading silence.

```powershell
$env:PATH = "$env:LOCALAPPDATA\Programs\MSYS2\ucrt64\bin;$env:PATH"
cargo +stable-x86_64-pc-windows-gnu fmt --check
cargo +stable-x86_64-pc-windows-gnu test --all-targets
cargo +stable-x86_64-pc-windows-gnu clippy --all-targets -- -D warnings
.\record_shims\mux_renderer_oracle.ps1
```

The C oracle defaults to comparison; pass `-Record` only when intentionally
refreshing fixtures from the unchanged source. Native builds and runtime still
require GStreamer plus the `mp4mux`, `filesink`, `h264parse`, `h265parse`, and
`aacparse` plugins. No media-client or cross-OS renderer acceptance is claimed.

## Current limits

The Rust app constructs and owns this renderer through `MediaOutputRuntime` when
MP4 output is enabled, and sends decoded audio/video into it from the live media
callbacks. App integration tests cover audio-only and video-only finalization,
including EOS and MP4 container atoms. A source-valid AAC-ELD fixture is still
missing, so mixed A/V finalization is not claimed.

The generic `rust-anything record`/`gate` recorder cannot characterize these
static C globals because its C harness attempts to compile the whole UxPlay tree,
which pulls in optional DNS-SD/libplist and renderer dependencies. The isolated
C oracle and Rust tests provide focused evidence; live Apple-client playback,
cross-platform renderer acceptance, and final generic unit-gate status remain
open.
