# Current handoff

- Updated 2026-11-26. Continue the Rust migration without changing the C reference checkout at `D:\Save\Uxplay\UxPlay`. The user's priority is essential cutover work first while preserving the agreed full feature/platform scope.
- The Rust app no longer links to or launches the C executable. `original.rs` was removed; `fallback_log.rs` only initializes the migration harness's zero-fallback report.
- Windows GNU acceptance after that change: `cargo test --all-targets` passes 526/526; `--features external-dns-sd` passes 528/528. Clippy `-D warnings` and rustfmt pass both modes. A clean target build passes, `--help` runs, and focused `/server-info` replay passes 1/1 at 0 fallback.
- Generic 8-case HTTP replay is 5/8 at 0 fallback because its reader sees resets on persistent responses. Raw-byte C/Rust HTTP diff remains 8/8 with only `Date` normalized. Preserve source connection behavior.
- Earlier recorded Windows evidence covers all 118 CLI cases in Rust, including three `-async` missing-plugin startup failures. Invalid UTF-8 `uxplayrc` options/comments match C on 3/3. Native Unix argv/runtime replay remains.
- `rust-anything app_lift` did not lift: its generated unit wiring metadata does not recognize this hand-integrated app and its subprocess chose MSVC without `link.exe`. The actionable request to remove the original launcher has been completed; GNU clean build/test evidence is valid.
- The source replacement was pushed to `AtlasSolutionsMC/UxPlayRusty:master`; removal of the C launcher is commit `ea365fb`, and this checkpoint is synced after it. Generated targets, baseline binaries and oracle executables are excluded.
- The real-iPhone mirror decrypt/frame failure still needs full stderr, raw packets and a same-device C comparison. Keep Vulkan and native iPhone Mirroring out of this AirPlay cutover; see `rust/research/iphone-mirroring-vulkan-feasibility.md`.
- Windows GNU toolchain: prepend `%LOCALAPPDATA%\Programs\MSYS2\ucrt64\bin` to `PATH`, set `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`, and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to that folder's `gcc.exe`.
- Read `rust/MAP.md`, `rust/DECISIONS.md` and `Checkpoints.md` before resuming; do not repeat completed setup/research.

## Historical acceptance notes
- External acceptance still needed: live Apple pairing/FairPlay and audio/video playback, BLE radios, native Linux/macOS/BSD plus Bonjour/D-Bus paths, real A/V timing, and a valid AAC-ELD fixture for mixed MP4.
- Upstream source was fetched read-only into Git metadata and is ahead by two commits; the C worktree remains unchanged. Missing-DeviceID admission and client-name CR/LF sanitation are now ported with regressions; PIN-mode auth behavior was already equivalent. Research report: `rust/research/iphone-mirroring-vulkan-feasibility.md`.
- Native Windows toolchain: prepend `%LOCALAPPDATA%\Programs\MSYS2\ucrt64\bin` to `PATH`, set `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`, and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to that MSYS2 `gcc.exe`.
- Decisions and remaining acceptance are in `rust/DECISIONS.md` and `rust/MAP.md`; chronological evidence remains in this file below.
