# Audio renderer port evidence

## Reference unit

- Source: `D:\Save\Uxplay\UxPlay\renderers\audio_renderer.c` and `audio_renderer.h`.
- rust-anything `analyze`: unit `renderers/audio_renderer.c`, status `caution`, 412 LOC, 58 pointer risks, 29 null-reference risks, 12 integer-semantics risks, 2 division-semantics risks.
- rust-anything `bridge`: generated the C ABI exports `gstreamer_init`, `audio_renderer_stop`, `audio_renderer_set_volume`, `audio_renderer_flush`, and `audio_renderer_destroy`. It left `audio_renderer_init`, `audio_renderer_start`, `audio_renderer_render_buffer`, and `audio_renderer_listen` for an ownership design because their signatures carry pointers.
- Original C tree was not modified. `reference/audio_renderer_oracle.c` is a small caller harness that includes the original header and links the original source file.

## Recorded C behavior

The harness was compiled against the original `audio_renderer.c`, using the MSYS2 UCRT64 compiler and GStreamer 1.28.9, with `fakesink` as the sink. It returned:

```text
gstreamer_init=1
L7:Audio format 1: AAC-ELD 44100/2
L7:GStreamer audio pipeline 1: "appsrc name=audio_source ! queue ! avdec_aac ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=false"
L7:Audio format 2: ALAC 44100/16/2
L7:GStreamer audio pipeline 2: "appsrc name=audio_source ! queue ! avdec_alac ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=false"
watch_ids=1,2
L6:start audio connection, format AAC-ELD 44100/2
L3:*** ERROR invalid  audio frame (compression_type 8) skipped 
L3:***       first byte of invalid frame was  0xff 
L6:changed audio connection, format ALAC 44100/16/2
```

The Rust tests cover the matching pipeline descriptions, the AAC-ELD/ALAC frame headers, format switching, volume clamp, PTS subtraction, and RTP pipeline grammar. The native lifecycle test builds both actual pipelines and starts/stops them through `fakesink`; it does not exercise an audio device.

## Behavior retained

- `NFORMATS` is 2. Although caps for AAC-LC (`ct = 4`) and PCM (`ct = 1`) exist in the C file, those renderers are not constructed. Starts for those types therefore follow the unknown-type path. The Rust port preserves that enabled-format set.
- AAC-ELD accepts first bytes `8c`, `8d`, `8e`, `80`, `81`, and `82`; ALAC accepts `20`. `seqnum` is unused. DTS and duration remain unset. PTS is assigned only in sync mode and is `ntp_time - appsrc_base_time`; an earlier NTP value is logged and dropped.
- Local output keeps `level` and the supplied sink text. AAC-ELD uses `video_sync`, ALAC uses `audio_sync`. RTP output converts to 44.1 kHz stereo S16BE and appends the user pipeline after `rtpL16pay` without retokenizing it. The source's static sync values start false in RTP mode, so the new instance keeps them false there.
- Start, switch, stop, EOS-before-NULL ordering, bus error handling, missing plugin checks, and volume levels from 0 to 10 are represented. The C source's empty `flush` callback remains empty.

## Known boundaries

- A zero-length C buffer reaches `data[0]` after the source mistakenly tests the length pointer instead of `*data_len`; that is undefined behavior. Safe Rust drops an empty slice as `RenderOutcome::EmptyPacket`.
- A GStreamer parse failure calls `g_error` in C and terminates the process. The Rust API returns an initialization `Err` so its caller can report the same startup failure through the Rust app's error path.
- The C callback API uses a raw `logger_t*`, input pointers, and a caller-owned `GMainLoop*`. The Rust crate exposes per-instance and process-global Rust methods with a logger callback and owned bus-watch guard. The generated C ABI has only five pointer-free symbols, so it is not a full C drop-in; app call sites must use the Rust methods.
- `rust-anything record` could not produce `cases.jsonl`. Its MCP attempt had no `cc` on the server PATH. Running the CLI with MSYS2 `cc` then failed because the automatic whole-tree C build did not pass the GStreamer include flags, lacked `dns_sd.h`, and hit an unrelated `plist_get_data_val` compile error in `lib/http_handlers.h`. A focused source adapter compiled but rust-anything did not accept the GStreamer linker flags, so that trial was removed. The C harness above was compiled manually with `pkg-config` flags instead.
- Rust GStreamer bindings require native GStreamer at least 1.14. The local build used the matching Windows GNU toolchain and MSYS2 UCRT64 GStreamer 1.28.9. Linux/BSD/macOS builds and audio-device playback remain unverified.

## Native dependencies

Build requires Rust plus GStreamer development libraries discoverable by `pkg-config`: GStreamer core, GLib, `gstreamer-app`, and their headers/import libraries. At runtime the C-compatible startup check requires plugin modules `app`, `libav`, `playback`, `autodetect`, and `videoparsersbad`; this audio renderer also uses the `avdec_aac` and `avdec_alac` factories when installed. Local output needs `audioconvert`, `audioresample`, `volume`, `level`, and the configured sink. RTP output additionally needs `rtpL16pay` and the elements named by the caller's pipeline. Codec, plugin, and OS audio server packages stay native dependencies.

## rust-anything gate status

The source was analyzed and bridged. The build and test portions pass locally, but the rust-anything diff/gate cannot be reported as passed while the original-library recorder is blocked by the C build issues listed above. Do not mark this unit gated until the source recorder/replay can run against a compatible reference build and the app-side call sites use this Rust API.
