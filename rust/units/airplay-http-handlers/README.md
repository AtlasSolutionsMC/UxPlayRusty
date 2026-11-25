# AirPlay HTTP handler port

This crate isolates deterministic behavior from `lib/http_handlers.h` without
changing the target app's dispatcher. It ports property handling, the FairPlay
2 rejection response, reverse-channel upgrade decision, action/play request
planning, and HLS request/response planning. Playlist storage, FCUP transmission,
renderer callbacks, connection mutation, and condensed-URI expansion remain
typed plans for the Rust app to execute once its state layer is ready.

## Evidence and limitations

- `fixtures/c-oracles.tsv` records response and decision oracles derived from
  the unchanged C source with source line references. The C functions are static
  handlers inside `http_handlers.h`, depend on `raop.c` state, libplist, and
  callbacks, and have no direct test hooks; this fixture set is not a live C/Rust
  differential run.
- `rust-anything analyze D:\Save\Uxplay\UxPlay --lang c` finds
  `lib/http_handlers.h` in an import cycle with `raop.c`, `airplay_video.c`, and
  RTP units, and reports no C tests. The generated C-ABI bridge/record/gate flow
  cannot characterize these static callback-driven handlers independently.
- Run `cargo test --manifest-path rust/units/airplay-http-handlers/Cargo.toml`
  for the Rust contract tests. A `rust-anything gate` result is intentionally not
  claimed because no executable original-handler oracle is wired to this crate.
- `UxPlay` was used read-only.

## Source behavior notes

- `fp-setup2` adds a Content-Type header and then calls `http_response_init`,
  which resets response data length. The final response is therefore 421 with
  no Content-Type header.
- On `/reverse`, C marks the connection as EVENT before counting EVENT
  connections. A count other than one leaves the initialized response alone.
- `/action` requires exact session ID equality, a case-sensitive
  `apple-binary-plist` substring in the complete header text, and a binary plist.
  The playlist update and follow-up FCUP/play callback are emitted as typed plans.
- `/play` prioritizes a reusable UUID before requiring Content-Location. Direct
  HTTP(S) URLs take priority over the HLS `/master.m3u8` suffix path.
- The source reads `FCUP_Response_Data[-1]` for an empty data field. The Rust
  parser returns `BadRequest` for this undefined C case rather than reproducing
  an out-of-bounds read.
