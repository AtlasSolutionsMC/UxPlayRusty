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
| 3 | Low-latency mode and D3D11 preset | Opt-in behavior preserves defaults; bounded pre-decode queue and leaky post-decode queue; correct H.264/H.265 decoder selection; logs show actual pipeline; repeatable A/B and real-device evidence. | NOT STARTED |
| 4 | Vulkan renderer | Config selects `auto`/`d3d11`/`vulkan`; Windows plugin/driver/window/caps lifecycle is verified; fallback states the reason and actual renderer; compare with D3D11. | NOT STARTED |
| 5 | User-editable config | First-run template, documented supported values, validation errors, stable default/config/CLI precedence, and compatibility with existing CLI/rc. | NOT STARTED |
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
- Milestone 2 is ready for a scoped commit/push. Next: begin milestone 3 by inspecting renderer pipeline/options and source comparison evidence before editing latency behavior.
