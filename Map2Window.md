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
| 1 | Baseline, Git target, and repeatable Windows toolchain | Confirm source directory, repository, branch, remote, clean/changed files, GNU toolchain and native dependencies; record revision, commands, and baseline results. | DONE for local baseline; existing doc/workflow diffs are preserved and reviewable. Development branch has not yet been pushed. |
| 2 | Workspace and documentation organization | Move only after mapping all links, scripts, and CI references; keep unit tests with crates; ignore generated outputs; verify build/test paths. Crate merges require a separate dependency-based decision. | NOT STARTED |
| 3 | Low-latency mode and D3D11 preset | Opt-in behavior preserves defaults; bounded pre-decode queue and leaky post-decode queue; correct H.264/H.265 decoder selection; logs show actual pipeline; repeatable A/B and real-device evidence. | NOT STARTED |
| 4 | Vulkan renderer | Config selects `auto`/`d3d11`/`vulkan`; Windows plugin/driver/window/caps lifecycle is verified; fallback states the reason and actual renderer; compare with D3D11. | NOT STARTED |
| 5 | User-editable config | First-run template, documented supported values, validation errors, stable default/config/CLI precedence, and compatibility with existing CLI/rc. | NOT STARTED |
| 6 | Tray and startup lifecycle | Windows app opens to tray with Config/Restart/Disconnect/Exit; startup is user-selectable; duplicate instances and restart/shutdown leaks are handled. | NOT STARTED |
| 7 | Main app manages BLE companion | Starts after server identity/ports are known; failure does not stop AirPlay; restart/exit leaves no orphan companion. | NOT STARTED |
| 8 | Windows acceptance before release build | Relevant format/tests/Clippy/regression, renderer/config/tray/startup/BLE checks; device and network results are distinguished from fixture results. | NOT STARTED |
| 9 | Release binaries and installer | Build `uxplay.exe`, `uxplay-beacon.exe`, runtime bundle and `Setup.exe` from verified revision; test install through uninstall on a clean Windows machine. | NOT STARTED |
| 10 | Final repository and handoff | After main acceptance, clean `UxPlayRusty(Final)`, retain user-facing README, remove internal reports/local-machine references as requested, then push to confirmed remote/branch and record artifacts/evidence. | NOT STARTED |

## Current blockers and evidence gaps

- The ordinary development directory is now a Git checkout on local branch `windows-delivery`, based on and tracking `origin/Workspace` at `53fa163404402d88c746b01f174febe79ff24d5a`. The branch has not been pushed yet. GitHub's default branch is `master`; keep this work on the development branch.
- Before Git was attached, a read-only tree comparison found five content-different tracked files versus `Workspace`: `.github/workflows/rust-linux.yml`, `Checkpoints.md`, `README.md`, `TestingBeforeResume.md`, and `rust/MAP.md`. These pre-existing edits remain unstaged; preserve them and review before any commit. `Map2Window.md` is new.
- Windows GNU stable is installed and used by setting `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`; rustc/cargo are 1.100.0. The shell defaults to MSVC, where rustfmt is missing, but GNU `cargo fmt --check` passed for app and beacon.
- MSYS2 UCRT64: GCC 16.2.0, GStreamer 1.28.9, GLib 2.90.1, OpenSSL 4.1.0. `d3d11h264dec`, `d3d11videosink`, `vulkanupload`, and `vulkansink` factories are installed. This confirms plugin presence only, not driver/hardware Vulkan runtime support.
- Baseline on Windows GNU: app all-targets 550 tests passed (default: 434+10+99+7); external DNS-SD 552 passed (436+10+99+7); BLE companion 73 passed (68+5). App Clippy with warnings denied passed both DNS-SD modes; beacon Clippy passed; app/beacon formatting checks passed.
- Release builds passed for app default, app `external-dns-sd`, and BLE companion. Smoke `uxplay -h`, `uxplay -v`, and `uxplay-beacon --help` passed when MSYS2 UCRT64 `bin` is on `PATH`; app help/version fail with Windows status `-1073741515` when that runtime path is absent, confirming runtime packaging is required.
- Current diff is intentionally not clean: five pre-existing local content changes are `.github/workflows/rust-linux.yml`, `Checkpoints.md`, `README.md`, `TestingBeforeResume.md`, and `rust/MAP.md`; this goal also adds `Map2Window.md`. Review and preserve those changes before any commit.
- Real iPhone media, Vulkan on a supported Windows GPU, BLE radio behavior, and clean-machine installer acceptance need their respective devices/environments; never mark these complete from unit tests alone.

## Checkpoint log

### 2026-11-27 — Goal started

- Read the attached Windows delivery goal and identified the user-specified scope and ten milestones above.
- Confirmed GitHub repository `AtlasSolutionsMC/UxPlayRusty` is public and default branch `master`; it also has development branch `Workspace`.
- Initialized Git metadata in the ordinary folder without replacing files; created local `windows-delivery` from `origin/Workspace` and set its upstream to `origin/Workspace`. The existing five-file diff was preserved unstaged.
- Verified Windows GNU toolchain and native versions/factories; recorded default and external app tests, beacon tests, Clippy and format results above.
- Next: publish the map and this goal's checkpoint on `windows-delivery` without absorbing unrelated existing doc/workflow changes, then begin the workspace/documentation audit for milestone 2.
