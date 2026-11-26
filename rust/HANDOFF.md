# Current handoff

- Latest acceptance: Windows GNU empty-target release build and `external-dns-sd` release build pass; `-h`, 429/431 app plus 10/98/7 auxiliary tests, both Clippy modes and fmt pass. Raw-byte C/Rust HTTP differential is 8/8 (only Date normalized); aggregate app replay is 5/8 at zero fallback because three grouped reads reset, and each passes individually 1/1.
- Core source tree (811 tracked files) is synchronized to `AtlasSolutionsMC/UxPlayRusty` `master`; code commit `9d9e15b` and the following checkpoint/map update are pushed, remote SHA verified and clone clean. The C reference checkout at `D:\Save\Uxplay\UxPlay` remains unchanged.
- Latest local fixes: unmatched HTTP fallback/log, full FCUP error-log assertion, and per-playlist FCUP ID lifetime (reused UUID retains counter; new slot resets). Default/feature tests and both lint modes pass.
- The two upstream `UxPlay` commits were fetched and inspected; password callback sentinel, `-restrict` behavior for absent DeviceID and registration-name CR/LF sanitation are represented in Rust tests/implementation.
- `rust-anything app_lift` refused the hand-integrated app because no units are wired in its generated metadata and its subprocess selected unavailable MSVC `link.exe`. The same application builds cleanly with the installed Windows GNU toolchain, including from an empty target.
- Remaining acceptance: reproduce the iPhone invalid-frame issue with full stderr/raw packets and same-device C comparison; native Unix argv/config, DNS-SD, X11 and media runtime; real A/V and packaging. Do not change validated crypto/parser framing without captured evidence.
- Before resuming, read `TestingBeforeResume.md`, `rust/MAP.md`, `rust/DECISIONS.md` and the latest Checkpoints state. The `Historical acceptance notes` below retain prior session details.

## Historical acceptance notes
- External acceptance still needed: live Apple pairing/FairPlay and audio/video playback, BLE radios, native Linux/macOS/BSD plus Bonjour/D-Bus paths, real A/V timing, and a valid AAC-ELD fixture for mixed MP4.
- Upstream source was fetched read-only into Git metadata and is ahead by two commits; the C worktree remains unchanged. Missing-DeviceID admission and client-name CR/LF sanitation are now ported with regressions; PIN-mode auth behavior was already equivalent. Research report: `rust/research/iphone-mirroring-vulkan-feasibility.md`.
- Native Windows toolchain: prepend `%LOCALAPPDATA%\Programs\MSYS2\ucrt64\bin` to `PATH`, set `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`, and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to that MSYS2 `gcc.exe`.
- Decisions and remaining acceptance are in `rust/DECISIONS.md` and `rust/MAP.md`; chronological evidence remains in this file below.
