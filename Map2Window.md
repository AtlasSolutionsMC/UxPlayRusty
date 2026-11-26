# UxPlayRusty Windows Delivery Map

Updated: 2026-11-27 (Asia/Bangkok)

## Goal

Deliver UxPlayRusty as a Windows AirPlay receiver that is straightforward to install and operate, with configurable low-latency playback, renderer selection, a documented user config, system-tray controls, startup support, BLE lifecycle management, and a tested `Setup.exe`. A successful compile or unit-test run alone does not count as acceptance.

Detailed requirements: `DEVELOPMENT_PLAN_WINDOWS.md` supplied by the user and the attached Windows delivery goal. The latest user goal takes precedence over older conflicting repository notes.

## Scope rules

- Implement in `D:\Save\Uxplay\UxPlayRusty` first. Keep `UxPlayRusty(Final)` for the final repository presentation and cleanup after the main work passes acceptance.
- Windows GNU/MSYS2 UCRT64 is the target development environment. Keep Linux/macOS CI enabled; record platform-specific failures without allowing them to block Windows work.
- Do not include native iPhone Mirroring protocol implementation or phone mouse/keyboard control.
- Preserve existing local edits and test/CI coverage. Do not reset or overwrite unrelated work.
- Keep `app` and `beacon` as separate programs. Merge library crates only when dependency review shows a concrete maintenance benefit; keep that separate from latency changes.
- Push only after the active development checkout and target branch are positively identified. Do not publish build output or local-only paths/secrets.

## Milestones and current status

Status values: `DONE`, `IN PROGRESS`, `NOT STARTED`, `BLOCKED`, `NOT VERIFIED`.

| # | Milestone | Acceptance evidence | Status |
|---|---|---|---|
| 1 | Baseline, Git target, and repeatable Windows toolchain | Confirm source directory, repository, branch, remote, clean/changed files, GNU toolchain and native dependencies; record revision, commands, and baseline results. | DONE — branch and baseline are established; `d5afffc` published the map/checkpoint. |
| 2 | Workspace and documentation organization | Move only after mapping all links, scripts, and CI references; keep unit tests with crates; ignore generated outputs; verify build/test paths. Crate merges require a separate dependency-based decision. | DONE — moved docs/fixtures/oracles, updated callers and script defaults, and passed post-move Windows format/test gates. |
| 3 | Low-latency mode and D3D11 preset | Opt-in behavior preserves defaults; bounded pre-decode queue and leaky post-decode queue; correct H.264/H.265 decoder selection; logs show actual pipeline; repeatable A/B and real-device evidence. | IN PROGRESS — `-lowlatency` and `-d3d11` are implemented with focused pipeline/CLI tests; hardware/device A/B evidence remains open. |
| 4 | Vulkan renderer | Config selects `auto`/`d3d11`/`vulkan`; Windows plugin/driver/window/caps lifecycle is verified; fallback states the reason and actual renderer; compare with D3D11. | IN PROGRESS — renderer selection and Vulkan upload/sink path are implemented; a Windows one-frame Vulkan pipeline passed; app/window lifecycle and persistent config remain. |
| 5 | User-editable config | First-run template, documented supported values, validation errors, stable default/config/CLI precedence, and compatibility with existing CLI/rc. | IN PROGRESS — Windows TOML creation/loading, strict validation, sample settings, and default < rc < config < CLI precedence are implemented; tray Restart/startup integration remains in M6. |
| 6 | Tray and startup lifecycle | Windows app opens to tray with Config/Restart/Disconnect/Exit; startup is user-selectable; duplicate instances and restart/shutdown leaks are handled. | NOT STARTED |
| 7 | Main app manages BLE companion | Starts after server identity/ports are known; failure does not stop AirPlay; restart/exit leaves no orphan companion. | NOT STARTED |
| 8 | Windows acceptance before release build | Relevant format/tests/Clippy/regression, renderer/config/tray/startup/BLE checks; device and network results are distinguished from fixture results. | NOT STARTED |
| 9 | Release binaries and installer | Build `uxplay.exe`, `uxplay-beacon.exe`, runtime bundle and `Setup.exe` from verified revision; test install through uninstall on a clean Windows machine. | NOT STARTED |
| 10 | Final repository and handoff | After main acceptance, clean `UxPlayRusty(Final)`, retain user-facing README, remove internal reports/local-machine references as requested, then push to confirmed remote/branch and record artifacts/evidence. | NOT STARTED |

## Current blockers and evidence gaps

- The ordinary development directory is on local/remote branch `windows-delivery`, based on `origin/Workspace` at `53fa163404402d88c746b01f174febe79ff24d5a`; initial map/checkpoint commit `d5afffc` is pushed to GitHub. `master` remains the repo's default branch; keep Windows development on `windows-delivery`.
- The folder's original unrelated workflow and unit-checkpoint edits were preserved and remain outside the milestone 2 commit. The workspace moves and their path changes are committed separately.
- Windows GNU stable is installed and used by setting `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`; rustc/cargo are 1.100.0. The shell defaults to MSVC, where rustfmt is missing, but GNU `cargo fmt --check` passed for app and beacon.
- MSYS2 UCRT64: GCC 16.2.0, GStreamer 1.28.9, GLib 2.90.1, OpenSSL 4.1.0. `d3d11h264dec`, `d3d11videosink`, `vulkanupload`, and `vulkansink` factories are installed. This confirms plugin presence only, not driver/hardware Vulkan runtime support.
- Baseline on Windows GNU: app all-targets 550 tests passed (default: 434+10+99+7); external DNS-SD 552 passed (436+10+99+7); BLE companion 73 passed (68+5). App Clippy with warnings denied passed both DNS-SD modes; beacon Clippy passed; app/beacon formatting checks passed.
- Release builds passed for app default, app `external-dns-sd`, and BLE companion. Smoke `uxplay -h`, `uxplay -v`, and `uxplay-beacon --help` passed when MSYS2 UCRT64 `bin` is on `PATH`; app help/version fail with Windows status `-1073741515` when that runtime path is absent, confirming runtime packaging is required.
- Post-move verification on Windows GNU: app `cargo test --all-targets` passed (434 app + 10 controls + 99 HTTP + 7 RAOP info = 550); `external-dns-sd` passed 552; beacon passed 68 library + 5 binary; compat crate passed 1. App, beacon, compat rustfmt checks and `git diff --check` passed. All local Markdown links resolve. No full-tree release rebuild was required by the organization-only change.
- Real iPhone media, Vulkan on a supported Windows GPU, BLE radio behavior, and clean-machine installer acceptance need their respective devices/environments; never mark these complete from unit tests alone.
- Milestone 3 implementation: `-lowlatency` is opt-in and adds a bounded 16-buffer pre-decode queue plus a two-buffer downstream-leaky post-decode queue for mirror H.264/H.265 only. JPEG, HLS, RTP output, and defaults keep their prior pipeline shape. `-d3d11` selects `d3d11h264dec`/`d3d11h265dec` and `d3d11videosink`; missing factories produce a warning and component-level automatic fallback. `-vsync` remains independent. Current Windows GStreamer reports all three D3D11 factories present; this is plugin discovery, not driver/device rendering acceptance.
- Renderer pipeline creation already emits the exact pipeline under debug logging. Synthetic Windows D3D11 H.264/H.265 decode and D3D11 sink tests now pass, but real-device low-latency A/B (same phone/content/network), sustained A/V quality, actual AirPlay resolution/FPS, and client playback remain unrecorded; do not mark milestone 3 complete without them.
- Milestone 4 implementation: `-renderer auto|d3d11|vulkan` now reaches the renderer. Vulkan mirror video builds `... ! videoconvert ! videoscale ! vulkanupload ! vulkansink`; H.265 remains codec-specific. HLS falls back to the configured sink with a warning; JPEG cover art keeps its configured sink; RTP output remains unchanged. Vulkan startup errors report the cause and retry the configured sink. `vulkansink` supports `GstVideoOverlay` and accepts VulkanImage; `vulkanupload` bridges system-memory video to Vulkan memory.
- Windows runtime evidence: `gst-launch-1.0 -q videotestsrc num-buffers=1 ! video/x-raw,width=320,height=240 ! videoconvert ! videoscale ! vulkanupload ! vulkansink sync=false` exited 0. A synthetic Vulkan frame passed through the installed plugin/driver/sink. `vulkansink` exposes `GstVideoOverlay`; this still does not prove the app's resize/fullscreen/teardown behavior or AirPlay/iPhone playback.
- The Windows renderer constructor test selected `VideoRendererKind::Vulkan` and reached READY without fallback; a separate GStreamer one-frame run exercised buffer flow through the same uploader/sink. This verifies local Vulkan initialization and a synthetic frame, not the UxPlay AirPlay receive path.
- D3D11 runtime evidence: the installed D3D11 sink displayed a synthetic frame with exit 0. `d3d11h264dec` decoded the H.264 renderer fixture; `d3d11h265dec` decoded a generated 8-bit 4:2:0 Main-profile stream from `x265enc`; both pipelines exited 0. Each emitted a nonfatal `0x80004002, No such interface supported` D3D11 device warning. The checked-in HEVC fixture is Main 4:4:4 12-bit, outside the decoder's advertised Main/Main-10 caps, so it correctly did not negotiate with D3D11.
- Milestone 5 implementation: first Windows startup creates `%APPDATA%\UxPlay\config.toml` from the documented template; `UXPLAY_CONFIG` overrides its location. Strict TOML validation covers receiver name, renderer, resolution, max FPS, H.265, low latency, vsync, and the future startup preference. Values layer as built-in defaults < legacy rc < TOML config < CLI. Boolean false values explicitly clear rc options. Unknown keys and invalid types/ranges name the offending key and stop startup. `startup.enabled` is saved and validated now; the tray milestone will apply it to Windows startup registration.
- Milestone 5 Windows GNU verification: app tests pass 441 default (441+10+99+7 = 557) and 443 external-DNS-SD (443+10+99+7 = 559); config conversion, invalid-key/type/range, precedence, and first-run template creation tests pass. App formatting and Clippy with `-D warnings` pass in both DNS-SD modes.

## Checkpoint log

### 2026-11-27 — Goal started and workspace organized (in progress)

- Read the attached Windows delivery goal and identified the user-specified scope and ten milestones above.
- Confirmed GitHub repository `AtlasSolutionsMC/UxPlayRusty` is public and default branch `master`; it also has development branch `Workspace`.
- Initialized Git metadata in the ordinary folder without replacing files; created `windows-delivery` from `origin/Workspace`. Commit `d5afffc` added this map and checkpoint and was pushed to `origin/windows-delivery`.
- Verified Windows GNU toolchain and native versions/factories; recorded default and external app tests, beacon tests, Clippy and format results above.
- Moved top-level/design/development/research/history docs into `docs/architecture`, `docs/development`, `docs/research`, and `docs/history`.
- Moved the app-case tree (383 files before moving three replay reports to `docs/history`; 380 cases/fixtures remain) into `tests/app-cases`, and 54 oracle scripts/fixtures into `tools/record-shims`. Updated all 13 Rust compile-time fixture references, docs links, `.gitattributes`, `.gitignore`, and active oracle-script defaults to derive the reference checkout beside the repository and MSYS2 under `%LOCALAPPDATA%`.
- Verified all local Markdown links resolve and audited source/CI for old `rust/app-cases` and `rust/record-shims` paths. Windows GNU post-move gates passed: app 550 default/552 external DNS-SD tests; beacon 73 tests; compat 1 test; formatting checks and `git diff --check` passed.
- One standalone C compat-oracle invocation could not compile on this host (GCC exited 1 without diagnostic text); the crate's checked-in C-oracle fixture test passed. This host limitation is recorded separately and does not block path/layout validation.
- Milestone 2 is complete and pushed as `cfa68cc` to `origin/windows-delivery`.
- Began milestone 3: added opt-in `-lowlatency` and `-d3d11`, codec-specific decoder selection, bounded queues limited to H.264/H.265 mirror pipelines, warning plus auto fallback when selected D3D11 factories are missing, help text, CLI tests, and updated help snapshots. Default, JPEG, HLS, and RTP pipeline behavior remains unchanged.
- Focused Windows GNU verification: renderer tests pass 15/15; app tests pass 551 default (435+10+99+7) and 553 with external DNS-SD (437+10+99+7); app and renderer Clippy with `-D warnings` and formatting checks pass all modes. Help smoke exits 0 and the four help fixtures contain the added options. Fallback behavior is unit-tested by component. MSYS2 reports `d3d11h264dec`, `d3d11h265dec`, and `d3d11videosink` installed.
- After renderer-mode integration, renderer tests pass 18/18; app tests pass 553 default (437+10+99+7). App/renderer formatting and Clippy gates pass. Help smoke exits 0; the help snapshots now include `-renderer <auto|d3d11|vulkan>`. The app test suite with external DNS-SD previously passed 555 before the final renderer enum consistency test was added; that test is renderer-only.
- Still required for milestone 3: actual D3D11 playback on a Windows GPU and repeated real-iPhone A/B playback/latency/quality evidence. Those are not implied by these local pipeline tests or plugin inventory.
- Still required for milestone 4: drive the new renderer through the UxPlay app with real mirror caps, resize/fullscreen, client disconnect/reconnect and teardown; compare its latency/output with D3D11; and persist/select renderer through the user config in milestone 5.
