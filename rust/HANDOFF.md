# Current handoff

- Latest acceptance: Linux run `45826231365` for code commit `f6aa6d6` passes format, all-target tests, Clippy (`-D warnings`), both release configurations, CLI/raw argv/native Unix `-rc` smokes and `/server-info` HTTP/plist/SIGINT smoke. Its logs confirm the H.265 MP4 integration test passes in both DNS-SD modes.
- Latest target code is pushed to `AtlasSolutionsMC/UxPlayRusty` `master` as `f6aa6d6`; workflow `45826231365` passed. Source workspace `D:\Save\Uxplay\UxPlayRusty` is not itself a Git checkout, so publish through `C:\Users\Alphacomputer\AppData\Local\Temp\UxPlayRusty-target-review-20261126`. The C reference checkout at `D:\Save\Uxplay\UxPlay` remains unchanged.
- Latest source-derived fixes: paired SETUP AES key uses first 16 bytes of SHA-512(clear FairPlay key || Pair Verify ECDH secret); legacy User-Agent selection preserves both C checks (`strstr("AirMyPC/2.0;xxx", agent)` and `strstr(agent, "AirMyPC")`). Linux CI covers SETUP timing, stream, FairPlay-result, User-Agent and actual bound timing-port diagnostics. Linux raw Unix `-rc` smoke had one fixture-only failure (`-v` vs rc token `v`) in run `45818167471`; corrected fixture passed in `45819281259`.
- The two upstream `UxPlay` commits were fetched and inspected; password callback sentinel, `-restrict` behavior for absent DeviceID and registration-name CR/LF sanitation are represented in Rust tests/implementation.
- `rust-anything app_lift` refused the hand-integrated app because no units are wired in its generated metadata and its subprocess selected unavailable MSVC `link.exe`. The same application builds cleanly with the installed Windows GNU toolchain, including from an empty target.
- Latest local source-unit differential: `lib/stream.h` Windows x64 C/Rust layout oracle matches both structs; crate tests pass 1/1 and Clippy passes. Other native ABI targets remain unverified.
- Latest local renderer slice: generated HEVC decodes through appsrc/h265parse/avdec_h265 to fakesink, and the mirror callback finalizes an H.265 MP4 with `ftyp`/`moov`/`mdat`. Windows GNU renderer tests pass 13/13; app tests pass 431 plus 10/98/7 auxiliary suites; Clippy (`-D warnings`) and fmt pass. Linux CI also passes the app H.265 mux integration in both DNS-SD modes.
- Remaining acceptance: retest iPhone video after the SHA-512 correction with full stderr/raw packets and same-device C comparison; complete full Unix option/runtime, DNS-SD, X11, native macOS/BSD and media/device runtime; validate real A/V; packaging and final clean build/tree sync. Vulkan remains a post-interoperability experiment. Do not change validated crypto/parser framing without captured evidence.
- Before resuming, read `TestingBeforeResume.md`, `rust/MAP.md`, `rust/DECISIONS.md` and the latest Checkpoints state. The `Historical acceptance notes` below retain prior session details.

## Historical acceptance notes
- External acceptance still needed: live Apple pairing/FairPlay and audio/video playback, BLE radios, native Linux/macOS/BSD plus Bonjour/D-Bus paths, real A/V timing, and a valid AAC-ELD fixture for mixed MP4.
- Upstream source was fetched read-only into Git metadata and is ahead by two commits; the C worktree remains unchanged. Missing-DeviceID admission and client-name CR/LF sanitation are now ported with regressions; PIN-mode auth behavior was already equivalent. Research report: `rust/research/iphone-mirroring-vulkan-feasibility.md`.
- Native Windows toolchain: prepend `%LOCALAPPDATA%\Programs\MSYS2\ucrt64\bin` to `PATH`, set `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`, and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to that MSYS2 `gcc.exe`.
- Decisions and remaining acceptance are in `rust/DECISIONS.md` and `rust/MAP.md`; chronological evidence remains in this file below.

## Resume next

- Read `TestingBeforeResume.md`, `rust/MAP.md`, this handoff, `rust/DECISIONS.md` and the latest `Checkpoints.md` state.
- Continue with one source-led, locally verifiable protocol/runtime gap. Start with the remaining RAOP/HTTP edge rows in `rust/MAP.md`; do not spend time on Vulkan or native iPhone Mirroring before AirPlay media interoperability works.
- The last device report is `rust/research/iphone-mirroring-vulkan-feasibility.md`; real iPhone validation still needs full log, encrypted packet capture and C-vs-Rust comparison.
- Before completion, perform the requested empty-target clean build, verify a whole-tree mirror to the target Git repo (deletions included), then push and verify remote HEAD.