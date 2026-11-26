# AgentsTwo — Agent Two progress journal and handoff

Agent: **Agent Two** (temporary development agent, not the Main Agent)
Created: 2026-11-23
Scope: continue the UxPlay C/C++/Python → Rust migration, prioritizing unfinished
implementation and source compatibility over general debugging, optimization or release work.

Reference project (READ ONLY): `D:\Save\Uxplay\UxPlay`
Rust project: `D:\Save\Uxplay\UxPlayRusty`

This file is Agent Two's own journal. Main Agent history in `docs/development/Checkpoints.md` is preserved
unchanged; detailed Agent Two work lives here.

---

## 1. Records read before any code change

| Record | State found |
|---|---|
| `docs/development/MAP.md` | Present, 74,718 bytes, last write 2026-11-23 05:47. Read Persistent Goal Execution Policy, Critical-path priorities, Current proof, Analyzer units ledger, Manual units, Feature acceptance matrix. |
| `docs/development/Checkpoints.md` | Present, 253,383 bytes, last write 2026-11-23 05:47. Read `Latest Resume State` (lines 6–17) and the recent 2026-11-23 entries. |
| `docs/history/TestingBeforeResume.md` | Present, 150 lines. Earlier real-iPhone test session; documents `*** ERROR decryption of video packet failed` and a truncated `GStreamer error (` line. |
| `docs/history/AgentsTwo.md` | **Did not exist.** Created by this session before any implementation change. |
| `docs/development/PORT_MAP.md` | Present but reports 0/12. MAP line 78 documents this as a known source-association false negative. **Not used as a completion metric.** |
| `docs/development/HANDOFF.md` | Present, older (2026-11-22 13:06). Superseded by MAP + Checkpoints. |

No Git metadata exists in `D:\Save\Uxplay\UxPlayRusty`. All change tracking in this file is an
explicit filesystem ledger. No commit or clean-tree claim is made.

### Baseline project state (from MAP, not re-derived)

- Analyzer snapshot (MAP line 80): **7/45 source units complete, 27 partial, 10 pending, 1 claimed.**
- Normal startup still falls back to `baseline/original-build/uxplay.exe`; the Rust-owned server is
  opt-in through `UXPLAY_RUST_SERVER=1` (`rust/app/src/main.rs` lines 177 and 223).
- Main Agent's last recorded app gate: 524/524 default all-target tests, 526/526 with
  `external-dns-sd`; Clippy `-D warnings` both modes, rustfmt, optimized release builds pass;
  raw C/Rust HTTP differential 8/8 normalizing only `Date`.
- Main Agent's stated next batch (Checkpoints line 15): unchanged-C vs Rust audit of AirPlay video
  cache/playlist state and cancellation/reconnect/error lifecycle (`lib/airplay_video.c` ↔
  `rust/app/src/airplay_video.rs`).

### Toolchain confirmed on this host

```
cargo 1.100.0 (3acc79c0c 2026-11-05)
rustc 1.100.0 (0ef1bc386 2026-11-09)
MSYS2 UCRT64 gcc present at %LOCALAPPDATA%\Programs\MSYS2\ucrt64\bin\gcc.exe
```

### Build staleness observed before starting

The acceptance binary used by the earlier iPhone test is **older than current source**:

```
rust/app/target/acceptance-release/release/uxplay.exe   built 2026-11-23 03:18:47
rust/app/src/protocol_connection.rs                     modified 2026-11-23 05:31:40
rust/app/src/airplay_video.rs                           modified 2026-11-23 05:50:27
rust/units/airplay-http-handlers/src/port.rs            modified 2026-11-23 05:50:27
rust/app/src/raop_media_runtime.rs                      modified 2026-11-23 05:17:19
rust/app/src/http_transport.rs                          modified 2026-11-23 05:28:05
rust/app/src/server_runtime.rs                          modified 2026-11-23 05:17:08
rust/units/raop-rtp/src/lib.rs                          modified 2026-11-23 04:18:15
rust/app/src/raop_ntp_runtime.rs                        modified 2026-11-23 04:18:02
rust/beacon/src/backend/bluez.rs                        modified 2026-11-23 05:44:57
```

Consequence: the earlier iPhone log **cannot** be attributed to current source. Any reproduction
must use a fresh build. This is recorded, not assumed away.

---

## 2. Baseline verification (in progress at creation time)

Started before any edit:

```powershell
$msys = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msys\bin;$env:Path"
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $msys 'bin\gcc.exe'
$env:GST_PLUGIN_PATH_1_0 = "$msys\lib\gstreamer-1.0"
$env:GST_PLUGIN_SCANNER_1_0 = "$msys\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
$env:LANG = 'C.UTF-8'
cargo test --manifest-path rust/units/raop-rtp-mirror/Cargo.toml --all-targets
```

Actual baseline result (completed before any edit):

```text
running 10 tests
...
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

The shell command's PowerShell exit code 1 came from the `Select-Object -Last 25` pipeline /
native-command stderr redirection behavior, not from `cargo test`: the Cargo result line is `ok`.
Recorded as a pass because the actual test result line was `ok. 10 passed; 0 failed`.

---

## 3. Batch 1 selection

### Selected subsystem

**RAOP encrypted video mirror: live AES re-key on repeated SETUP**
Source unit `lib/raop_rtp_mirror.c` (MAP wave 3, status `partial`), with
`lib/mirror_buffer.c` (wave 3, `partial`) and `lib/raop_handlers.h` (wave 2, `partial`).

### Why this batch

MAP Critical-path priority **1** explicitly lists "encrypted audio/video session handling and media
decryption" and "session setup, reconnect, timeout, reset and teardown". This is unfinished
**source behavior**, not a speculative runtime bug: the C reference performs an operation that the
Rust port does not perform at all. It is locally verifiable with deterministic tests and needs no
Apple hardware, so it is unblocked.

It is chosen over the Main Agent's suggested `airplay_video.c` audit because priority 1 (protocol
cryptography / session setup) outranks priority 2/3 work, and because a concrete, already-located
source divergence is available now.

### Original C/C++ files examined

| File | Lines read | Behavior |
|---|---|---|
| `lib/raop_handlers.h` | 930–1029 | SETUP creates `raop_rtp_mirror` with `aeskey`; type 110 reads `streamConnectionID`, calls `raop_rtp_mirror_init_aes()` then `raop_rtp_mirror_start()`. |
| `lib/raop_rtp_mirror.c` | 133–219, 900–954 | `raop_rtp_mirror_init_aes` → `mirror_buffer_init_aes(raop_rtp_mirror->buffer, …)`; worker loop reads the shared `buffer`; `start` early-returns when `running \|\| !joined`. |
| `lib/mirror_buffer.c` | 1–143 (whole file) | `mirror_buffer_init_aes` destroys the old `aes_ctx`, builds a new one, resets `nextDecryptCount` and `og`. The buffer is a single shared object owned by `raop_rtp_mirror_t`. |
| `renderers/video_renderer.c` | 620–664, 937 | `if (data[0])` → `*** ERROR decryption of video packet failed `; else first-packet `Begin streaming to GStreamer video pipeline`; bus error text `GStreamer error (video): %s %s`. |

### Rust files inspected

| File | Lines | Observation |
|---|---|---|
| `rust/units/mirror-buffer/src/lib.rs` | 1–115 | `init_aes` and `decrypt` are a faithful line-by-line port of `mirror_buffer.c` (pending keystream, `encryptlen`, `restlen`, re-key reset). **Not the defect.** |
| `rust/units/raop-rtp-mirror/src/runtime.rs` | 41–204, 270–324 | `init_aes()` only assigns `self.stream_connection_id`. `start()` early-returns when `self.worker.is_some()`. The worker builds its **own** `MirrorPacketProcessor` from `audio_key.clone()` and the captured `stream_id` at spawn time. |
| `rust/units/raop-rtp-mirror/src/lib.rs` | 145–146 | `MirrorPacketProcessor::init_aes` forwards to `self.cipher.init_aes`. |
| `rust/units/raop-session/src/lib.rs` | 375–464 | SETUP pushes `InitializeMirror{config}` (only in the ekey/eiv block), then for stream type 110 pushes `InitializeMirrorAes{stream_connection_id}` and `StartMirror`. |
| `rust/app/src/raop_media_runtime.rs` | 855–934 | `InitializeMirror` stops any previous receiver and creates a new one; `InitializeMirrorAes` calls `mirror.init_aes(id)`; `StartMirror` calls `mirror.start(port, …)`. |
| `rust/render/video-renderer/src/port.rs` | 545–614, 1115 | `if *first != 0` → same error string as C. **Renderer parity is correct**; the message is a symptom of upstream decryption, not a renderer defect. |

### Concrete missing behavior (source divergence)

C re-keys the **live, shared** mirror buffer, so a repeated SETUP changes the AES key/IV used by the
already-running worker immediately:

```
raop_handlers.h:1006  raop_rtp_mirror_init_aes(..., &stream_connection_id)
raop_rtp_mirror.c:167 → mirror_buffer_init_aes(raop_rtp_mirror->buffer, ...)
mirror_buffer.c:69-77 → nextDecryptCount = 0; memset(og); destroy old aes_ctx; aes_ctr_init(new)
```

Rust stores the new ID on the receiver only. When a worker is already running, `start()` returns
early and the worker keeps decrypting with the **previous** stream's key/IV and CTR state:

```
runtime.rs:99-101   init_aes() → self.stream_connection_id = Some(id)   // never reaches the worker
runtime.rs:108-110  start()    → if self.worker.is_some() { return Ok(port) }
runtime.rs:128-134  worker owns a private MirrorPacketProcessor built at spawn
```

Observable consequence: after a second SETUP carrying a different `streamConnectionID` (or a
different FairPlay-derived key) while the mirror worker is still alive, every decrypted payload is
garbage, its first byte is non-zero, and the renderer logs
`*** ERROR decryption of video packet failed ` repeatedly — exactly the shape reported in
`docs/history/TestingBeforeResume.md`.

**Status of the iPhone hypothesis:** this code divergence is verified by reading both sources. It is
**not yet** a verified root cause of the user's iPhone failure, because (a) the tested binary predates
current source and (b) no current-build reproduction or C-reference comparison has been run. Recorded
as a compatibility defect to fix on source-parity grounds, independent of that hypothesis.

### Acceptance criteria for this batch

1. A repeated `init_aes` while the mirror worker is running must change the key/IV/CTR state actually
   used by that worker, matching `mirror_buffer_init_aes` semantics (new CTR stream, pending
   keystream dropped, `nextDecryptCount` reset).
2. A deterministic Rust test must prove it: start a listener, stream an encrypted payload under
   stream ID A, re-key to stream ID B without stopping the worker, then stream a payload encrypted
   under B and observe correct plaintext (and observe that A-encrypted data no longer decrypts).
3. Existing mirror unit tests, app integration transcripts and the two-mode app gates must not
   regress.
4. No public API break for existing callers unless justified; no silent C fallback; no weakened
   assertions.

---

## 4. Implementation performed

Only `rust/units/raop-rtp-mirror/src/runtime.rs` was changed. No public API signature changed.

1. Added `pending_stream_id: Arc<Mutex<Option<u64>>>` to `MirrorReceiver`.
2. `MirrorReceiver::init_aes` records the ID for a future start **and publishes it to the running
   worker**. This is the missing C behavior: `raop_rtp_mirror_init_aes` re-keys the live shared
   `mirror_buffer_t`.
3. `start()` clears the slot before spawning a worker, because the spawn-time `init_aes` already
   applies the captured ID; otherwise the first packet would be decrypted with a restarted CTR
   stream twice.
4. Bundled the worker's private processor and the shared re-key slot into `LiveProcessor`, with
   `apply_pending_rekey()` calling `MirrorPacketProcessor::init_aes` (which derives the new key/IV,
   resets the CTR stream and drops pending keystream, as C does). The source's codec,
   parameter-set and first-packet locals are deliberately preserved, matching C.
5. Applied pending re-keys in the accept loop and in `serve_client` both at the top of each read
   loop and immediately before decryption. The latter covers a SETUP published while the current
   packet header/payload was being read.
6. Grouped the worker parameters into `LiveProcessor` so Clippy `-D warnings`
   (`too_many_arguments`) passes without suppressing the lint.
7. Added `repeated_setup_rekeys_the_running_mirror_worker`: encrypt frame A under stream A,
   re-key without stopping the worker, decrypt frame B under stream B successfully, then verify a
   stale stream-A payload no longer decrypts (source-shaped leading `0x01` marker / invalid
   payload).

Negative control: temporarily removing the publish in `init_aes` made the new test fail with
`VideoFrame { data: [1, 0, 0, 0, 0, 0] }` — the exact failed-decryption marker shape seen in the
renderer error. The temporary edit was restored; the final file contains no negative-control code.

An initial test failure was an incorrect test expectation (`[0,0,0,2,...]` instead of the
Annex-B start code `[0,0,0,1,...]` actually produced); the assertion was corrected to the real
source-compatible bytes, not weakened.

---

## 5. Verification actually executed

Environment: Windows GNU (`stable-x86_64-pc-windows-gnu`), MSYS2 UCRT64 `gcc.exe` linker.

| Command | Actual result |
|---|---|
| `cargo test --manifest-path rust/units/raop-rtp-mirror/Cargo.toml --all-targets` (baseline, before edit) | `ok. 10 passed; 0 failed` |
| Same command after edit | `ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `cargo fmt --manifest-path rust/units/raop-rtp-mirror/Cargo.toml -- --check` | exit 0 |
| `cargo clippy --manifest-path rust/units/raop-rtp-mirror/Cargo.toml --all-targets -- -D warnings` | exit 0 |
| Focused negative control (`--lib repeated_setup_rekeys`) | FAILED as intended when publish removed; restored afterwards |
| `cargo test --manifest-path rust/app/Cargo.toml --all-targets` | exit 0; `410 + 10 + 98 + 7 = 525` tests pass |
| Same with `--features external-dns-sd` | exit 0; `412 + 10 + 98 + 7 = 527` tests pass |
| `cargo clippy --manifest-path rust/app/Cargo.toml --all-targets -- -D warnings` | exit 0 |
| Same with `--features external-dns-sd` | exit 0 |
| `cargo fmt --manifest-path rust/app/Cargo.toml -- --check` | exit 0 |
| `cargo build --manifest-path rust/app/Cargo.toml --release` | exit 0 |
| Same with `--features external-dns-sd` | exit 0 |

The 525/527 counts are from the current source tree. Earlier records cite 524/526; the difference
comes from source edits already present in the tree before this batch, not from a weakened or
skipped check. No test is reported as passed unless its actual result line was `ok`.

Commands **not run** in this session: mirror C oracle, HTTP differentials, current-build iPhone
reproduction, and C-reference device comparison. They are not reported as passed.

---

## 6. File-change ledger

| Path | Change | Status |
|---|---|---|
| `docs/history/AgentsTwo.md` | created and updated (this file) | documentation only |
| `rust/units/raop-rtp-mirror/src/runtime.rs` | live re-key delivery + `LiveProcessor` + regression test | implementation, all gates passed |
| `docs/development/MAP.md` | appended verified Agent Two evidence to the `mirror_buffer.c`, `raop_rtp_mirror.c` and encrypted-mirroring matrix entries | record update, no status promotion |
| `docs/development/Checkpoints.md` | added an attributed Agent Two checkpoint section | record update, Main Agent history preserved |

No other implementation file changed. Reference project `D:\Save\Uxplay\UxPlay`: **untouched**.
Verified timestamps of `lib/raop_rtp_mirror.c`, `lib/mirror_buffer.c`, `lib/raop_handlers.h`
remain 2026-11-21 19:16:19 after the work.

`docs/development/MAP.md` and `docs/development/Checkpoints.md` were updated only with verified facts for this batch. Main Agent
retains ownership of completion and cutover decisions; no source unit was promoted to `complete`.

---

## 7. Deferred items recorded

| ID | Class | Item |
|---|---|---|
| D1 | P2 (record, do not chase) | Real-iPhone `decryption of video packet failed` + truncated `GStreamer error (`. Needs a current-build reproduction with full stderr captured, and a same-device C-reference comparison. The earlier tested binary predates current source. The source divergence fixed here is a plausible contributor but **not** a confirmed root cause of that device log. |
| D2 | P2 | `docs/history/TestingBeforeResume.md` §6 still states PID 22432 is running. That process was killed and verified gone (no `uxplay` process, port 57788 free). The document is stale on that point; left unchanged because it belongs to the earlier session's record. |
| D3 | P3 | `rust/units/sap-hash` generated duplicate crate noted in MAP line 64; already implemented, no action. |
| D4 | P1 for Main Agent | App two-mode tests, Clippy, rustfmt and both release builds now pass in this batch (525/527). The mirror C oracle and real-device reproduction of this fix have not been run. Do not promote `lib/raop_rtp_mirror.c` or `lib/mirror_buffer.c` to `complete` without live-client evidence. |

---

## 8. Remaining work (outside this batch)

1. Rebuild the acceptance binary from the current source, capture full stderr, reproduce with the
   same iPhone, and compare against `baseline/original-build/uxplay.exe` before making any claim
   about the user-visible error.
2. Run the mirror C oracle / broader C differential if Main Agent requires additional source-gate
   evidence for these two units.
3. Continue with the Main Agent's own next migration batch; Agent Two did not start another batch.

---

### Final Handoff to Main Agent

**Agent Two status:** Completed for this batch (returned to Main Agent; no further batch started)

**Initial baseline:** 7/45 source units complete / 27 partial / 10 pending / 1 claimed;
Rust-owned server opt-in; reference project read-only; mirror unit baseline 10/10 tests.

**Completed migration batches:** RAOP encrypted video mirror — repeated SETUP now re-keys the live
worker's AES-CTR state, matching `raop_rtp_mirror_init_aes` → `mirror_buffer_init_aes` in the C
reference. One deterministic regression proves both successful new-stream decryption and rejection
of stale-stream data.

**Modified files:** `rust/units/raop-rtp-mirror/src/runtime.rs`, `docs/history/AgentsTwo.md` (created),
`docs/development/MAP.md`, `docs/development/Checkpoints.md`.

**Verification evidence:** mirror crate tests 11/11 pass (baseline 10/10); crate fmt/Clippy exit 0;
negative control reproduced the failed-decryption marker when the fix was removed; app all-target
tests 525/525 default and 527/527 with `external-dns-sd`; app Clippy both modes, app fmt and both
optimized release builds exit 0.

**Known regressions:** none observed in the mirror crate or app gates.

**Deferred bugs:** D1–D4 above, especially the unverified real-iPhone root cause and the stale binary.

**Unfinished work:** current-build device reproduction, C-reference comparison, and optional mirror
C oracle evidence. No new migration batch was started.

**Current build and runtime state:** no server process running (PID 22432 and child already killed,
port 57788 free); the acceptance binary remains stale (built 03:18:47, before current source edits);
this batch changed only source, not that binary.

**Recommended next action:** rebuild from current source, reproduce the iPhone case with full stderr,
then compare with the C reference before deciding whether D1 is resolved. Keep the reference project
untouched.

**Important decisions:** chose source-parity fix over broad debugging; preserved public APIs; used a
shared re-key slot rather than a second CTR stream on startup; did not weaken assertions; did not
modify Main Agent's MAP/Checkpoints or `docs/history/TestingBeforeResume.md`; no Git commit or clean-tree claim
(no Git metadata exists).
