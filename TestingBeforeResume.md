# TestingBeforeResume — สรุปก่อนเริ่มงานต่อ

วันที่: 2026-11-23  
ผู้เขียน: session นี้  
สถานะ: บันทึกย้อนหลัง ณ 2026-11-23; ดู addenda ด้านล่างสำหรับการทำงานต่อวันที่ 2026-11-26

---

## Current authoritative state — 2026-11-26

This file begins with a historical snapshot from 2026-11-23; use the later addenda and current `README.md`/`rust/MAP.md` for present behavior. The Rust executable is now the only application path: the C launcher/fallback and `UXPLAY_REFERENCE` / `UXPLAY_RUST_SERVER` selectors are absent. `fallback_log` only reports the migration harness counter. Do not follow the old fallback instructions in the historical snapshot below.

Windows GNU tests/builds and the unchanged-C HTTP differential are recorded in README and the latest checkpoint. Linux run `45813361718` passes fmt, all-target tests, Clippy (`-D warnings`), embedded/external DNS-SD release builds, clean-HOME CLI smoke, and loopback `/server-info` 200/plist/SIGINT endpoint smoke with video disabled. Earlier attempts identified and fixed smoke setup issues: `-vs 0` skips renderer initialization; `-hls` is needed for no-CSeq HTTP; Python `urllib` lowercases `X-Apple-Session-ID`, so the smoke sends raw HTTP to preserve the C-compatible exact-case lookup. A focused source audit found a production mismatch in new-protocol SETUP: unchanged `lib/crypto.c::sha_init()` uses SHA-512, but Rust `raop-session` used SHA-256 for `clear FairPlay key || Pair Verify ECDH secret`. Rust now uses SHA-512 and truncates to the first 16 bytes, matching C. The raop-session focused test passes 1/1, the full in-process PIN/Pair Verify/FairPlay/SETUP/encrypted audio+video delivery app test passes 1/1, and rustfmt passes. Linux CI and full Windows app gates after this key fix are pending. The real iPhone video issue remains unresolved without full stderr, raw packet capture and same-device C comparison. The two current upstream UxPlay commits are in a separate detached worktree; the original C checkout is unchanged.

## 1. สิ่งที่ทำใน session นี้

### 1.1 ตรวจสถานะพอร์ตก่อนรัน (snapshot วันที่ 2026-11-23)
- อ่าน `rust/MAP.md`, `Checkpoints.md` → `Latest Resume State`, `rust/HANDOFF.md`, `README.md`
- สรุปได้ว่าโปรเจกต์อยู่ใน阶段 **ยังไม่ cutover**:
  - 45 C units: complete 7, partial 27, pending 10, claimed 1
  - (historical; superseded) startup ปกติยัง fallback ไป `baseline/original-build/uxplay.exe`
  - (historical; superseded) ต้องตั้ง `UXPLAY_RUST_SERVER=1` ถึงจะให้ Rust ถือ listener เอง

### 1.2 ทดสอบว่าเปิด/run ได้
- `-h` ของ binary Rust และ C baseline ทำงาน exit 0 และข้อความตรงกัน
- รัน Rust-owned server แบบชั่วคราว:

```powershell
$msys = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msys\bin;$env:Path"
$env:GST_PLUGIN_PATH_1_0 = "$msys\lib\gstreamer-1.0"
$env:GST_PLUGIN_SCANNER_1_0 = "$msys\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
$env:LANG = 'C.UTF-8'
# Historical command from 2026-11-23. The current app ignores this variable.

& 'D:\Save\Uxplay\UxPlayRusty\rust\app\target\acceptance-release\release\uxplay.exe' `
  -n opencode -p tcp <p1>,57788,<p3> -hls
```

- ผล:
  - **PID 22432** ยังรันอยู่, dual-stack listener `0.0.0.0` + `::` บน TCP `57788`
  - `GET /server-info` พร้อม `X-Apple-Session-ID: session` → **HTTP 200, 574 bytes**
  - stderr จาก smoke launch ก่อนหน้าเป็นไฟล์ว่างทั้งหมด; Event Log ไม่มี error ของ uxplay

> หมายเหตุ: รอบที่เปิดไว้ให้ผู้ใช้ทดสอบ **ไม่ได้ redirect stderr ลงไฟล์** จึงอ่านข้อความ error จากหน้าต่าง console ไม่ได้โดยตรง

---

## 2. log ที่ผู้ใช้เห็นจริงจาก iPhone

บรรทัดแรกของ log ที่ผู้ใช้ก๊อปมาถูกตัด เหลือ `y 1.74: An Open-Source ...` (น่าจะเป็น `UxPlay 1.74:`)

```text
UxPlay Rust: connection request from Window’s Ten (iPhone12,1) with deviceID = 7A:EE:11:E2:3F:8C
raop_rtp_mirror starting mirroring
*** ERROR: *** ERROR decryption of video packet failed
... (ซ้ำจำนวนมากในรอบแรก)
raop_rtp_mirror->running is no longer true

UxPlay Rust: connection request from Window’s Ten (iPhone12,1) with deviceID = 7A:EE:11:E2:3F:8C
raop_rtp_mirror starting mirroring
Begin streaming to GStreamer video pipeline
raop_rtp_mirror->running is no longer true

UxPlay Rust: connection request from Window’s Ten (iPhone12,1) with deviceID = 7A:EE:11:E2:3F:8C
raop_rtp_mirror starting mirroring
Begin streaming to GStreamer video pipeline
GStreamer error (
```

ผู้ใช้รายงานว่า **ตอนแรกยังไม่ติด แต่รอนต่อมาเริ่ม stream ได้** ("Begin streaming to GStreamer video pipeline") แต่ยังมี `running is no longer true` และข้อความ `GStreamer error (` ถูกตัด ยังไม่เห็นรายละเอียด

---

## 3. แปลผลเบื้องต้น

### สิ่งที่ทำงานแล้ว
- iPhone เห็น service และส่ง connection request จริง ( device `iPhone12,1`, deviceID `7A:EE:11:E2:3F:8C`)
- RAOP mirror listener ถูก start (`raop_rtp_mirror starting mirroring`)
- รอบหลัง ๆ เข้าถึง `Begin streaming to GStreamer video pipeline` แสดงว่า pipeline ถูกเลือก/เริ่มแล้ว ไม่ได้ fail ตั้งแต่ handshake

### สิ่งที่ยังผิด
1. **`decryption of video packet failed` ซ้ำจำนวนมากในรอบแรก**
   - หมายถึง AES mirror decryption ใน path ของ Rust ไม่ตรงกับสิ่งที่ iPhone ส่ง
   - การ decrypt ใช้ข้อมูลจาก SETUP/FairPlay/Pair Verify ได้แก่ media key, EIV, `streamConnectionID` และ mirror AES-CTR state
   - นี่คือ release blocker ตาม `rust/MAP.md` → Critical-path priority #1 (cryptography / encrypted video) และ feature matrix แถว **Encrypted screen mirroring and video**

2. **mirror worker หยุด (`running is no longer true`) หลังเริ่ม stream**
   - อาจเป็นผลต่อเนื่องจาก decrypt fail หรือเป็น GStreamer pipeline error อีกชั้นหนึ่ง

3. **ข้อความ `GStreamer error (` ถูกตัด**
   - ต้องได้ full message ก่อนวินิจฉัยว่าเป็น codec caps / plugin / bus error หรือ shutdown หลัง decrypt fail

### บริบทสำคัญ
- 本地ทดสอบใน `Checkpoints.md` เคยผ่านด้วย fixture สynthetic:
  - SPS/PPS/IDR จาก fixture → `h264parse ! avdec_h264 ! fakesink`
  -encrypted RTP → decrypt → `.h264` dump ตรงตาม fixture
- อย่างไรก็ตาม MAP ระบุชัดว่า local simulation **ไม่ได้พิสูจน์ Apple interoperability**
- log นี้คือหลักฐานจาก Apple client จริงครั้งแรกใน session นี้ และแสดงว่า **ยังมีช่องว่างระหว่าง fixture กับ client จริง**

---

## 4. สิ่งที่ยังไม่ควรสรุป (คำแนะนำใน snapshot เก่า)

สถานะเรื่อง fallback ด้านล่างถูก supersede แล้วเมื่อ 2026-11-26: Rust เป็น executable path เดียวและไม่มี runtime fallback ส่วนหลักฐาน/ข้อจำกัดเรื่อง Apple interoperability ยังมีผลอยู่

- ยังไม่ควรสรุปว่า crypto port ผิดทั้งหมด:Pair Setup / FairPlay / audio transcript ผ่าน local oracle มาก่อนแล้ว
- ยังไม่ควรสรุปว่า GStreamer ผิด: error message ถูกตัด และอาจเป็นผลจาก decrypt fail
- ยังไม่ควรเปลี่ยน fixture/recorded case เพื่อให้ผ่าน; ถ้าจะแก้ต้องแก้ port ตามหลัก rust-anything workflow
- (superseded) ยังไม่ควรตัด fallback หรือทำ cutover

---

## 5. ก่อนเริ่มเขียนโค้ด ควรทำตามลำดับนี้

1. **เก็บ full stderr**  
   รันใหม่แบบ redirect stderr ลงไฟล์ แล้วก๊อปข้อความ `GStreamer error (...)` ให้ครบ

2. **เทียบกับ C reference ด้วย iPhone เครื่องเดิม**  
   รัน `baseline/original-build/uxplay.exe` แล้วดูว่ามี `decryption of video packet failed` เหมือนกันหรือไม่  
   - ถ้า C ก็ fail → อาจเป็นสภาพแวดล้อม/รุ่น iOS หรือวิธีทดสอบ
   - ถ้า C ผ่านแต่ Rust fail → แยกปัญหาเป็น Rust mirror crypto แน่นอน

3. **ตรวจ key/nonce lifecycle ใน Rust mirror path**  
   - `rust/units/raop-rtp-mirror/src/runtime.rs`
   - `rust/units/mirror-buffer/`
   - `rust/app/src/protocol_connection.rs` (SETUP stream 110, FairPlay key, EIV)
   - เทียบกับ `lib/raop_rtp_mirror.c`, `lib/mirror_buffer.c`, `lib/raop_handlers.h`

---

## 6. Addendum หลังตรวจ source ต่อ (2026-11-26)

- ข้อสรุปในหัวข้อ 3.1 ที่เขียนว่า `decryption of video packet failed` หมายถึง AES decrypt ผิดโดยตรงนั้นแรงเกินหลักฐาน: C ปล่อยข้อความนี้จาก `video_renderer.c` เมื่อ byte แรกของข้อมูลจาก mirror parser เป็น marker error; parser ตั้ง marker เมื่อ NAL length/forbidden-bit ไม่ถูกต้องหลังถอดรหัส ข้อความนี้จึงบอกว่า renderer input ใช้ไม่ได้ แต่ยังแยกไม่ได้ว่า key, encrypted bytes หรือ framing เป็นสาเหตุ ต้องเก็บ packet bytes และเทียบกับ C บน iPhone เครื่องเดิมก่อนแก้ crypto/parser
- Rust แก้ pending-keystream continuation กรณี packet สั้นกว่า AES block เพื่อไม่อ่านเกิน input; `cargo check -p uxplay-mirror-buffer` และ format ผ่าน แต่ยังไม่มีหลักฐานว่าการแก้นี้แก้ real-device failure
- พบช่องว่างแยกต่างหากใน RAOP link-local IPv6: C ส่ง `zone_id` จาก accepted socket ไปยัง NTP destination; Rust เคยทิ้ง scope ID. ตอนนี้ Rust คง `%<scope>` ไว้ใน remote address, logger ใช้ formatter ตาม C, และ address-length/RTP parsing รองรับรูปแบบนี้แล้ว; Windows GNU app `cargo check` กับ rustfmt ผ่าน แต่ real interface acceptance ยังไม่มี
- สถานะจริงของ real-device issue จึงยังเปิด: ต้องมี full stderr, raw packet capture และ same-device C comparison. อย่าอ้างว่าปัญหา iPhone mirroring จบจาก synthetic fixture หรือจาก IPv6 scope fix นี้

4. **ตรวจ reconnect state**  
   log มี connection request ซ้ำ 3 ครั้ง ต้องดูว่า key/CTR state ถูก reset ต่อ connection ใหม่หรือยังใช้ของเก่า

5. **แยก crypto error กับ GStreamer error**  
   ถ้า decrypt สำเร็จแล้วแต่ video ยังไม่ขึ้น ค่อยไปดู renderer/caps

---

## 6. สถานะ process ตอนเขียนไฟล์นี้

- Rust-owned server ยังรันอยู่: **PID 22432**, HTTP port **57788**
- ข้อมูลข้างต้นเป็นสถานะขณะบันทึกไฟล์ (2026-11-23) เท่านั้น; ตรวจซ้ำวันที่ 2026-11-26 แล้ว **PID 22432 ไม่ได้รันและไม่มี listener บน port 57788**
- คำสั่ง `taskkill` ด้านล่างจึงไม่จำเป็นกับสถานะปัจจุบัน:

```powershell
taskkill /PID 22432 /T /F
```

---

## 7. ไฟล์ที่เกี่ยวข้อง

- `D:\Save\Uxplay\UxPlayRusty\rust\MAP.md` — ledger หลัก / Critical-path priorities / Feature acceptance matrix
- `D:\Save\Uxplay\UxPlayRusty\Checkpoints.md` → `Latest Resume State`
- `D:\Save\Uxplay\UxPlayRusty\rust\app\src\main.rs` — Rust-only startup; no reference-launch or environment selector remains
- `D:\Save\Uxplay\UxPlayRusty\rust\app-cases\http\raw-byte-diff.ps1` — วิธี start server และรัน HTTP differential
- `D:\Save\Uxplay\UxPlayRusty\rust\units\raop-rtp-mirror\`
- `D:\Save\Uxplay\UxPlayRusty\rust\units\mirror-buffer\`
- `D:\Save\Uxplay\UxPlayRusty\baseline\original-build\uxplay.exe` — C reference สำหรับเทียบ

## 8. Addendum: X11 renderer integration (2026-11-26)

- This file was read before resuming work. The earlier statement that the session made no code changes is historical and ends at the timestamp in the prior addendum.
- The optional Unix `x_display_fix.h` behavior is now implemented in Rust and connected to the renderer: window title traversal, EWMH fullscreen, the old-GStreamer title zoom fix, F11/left-Alt+Enter navigation, mirror-frame lookup, and HLS window polling.
- Windows GNU all-target tests, both feature modes, app Clippy (`-D warnings`) and formatting pass. A standalone `x86_64-unknown-linux-gnu` API harness check and Clippy pass. Native Unix GStreamer/X11, a real X server/window manager, and on-device video remain unverified.
- Continue with remaining locally verifiable media/protocol gaps. Do not run the final clean release build until implementation batches are complete. Keep the target GitHub synchronization and final clean build as the end-of-migration gates, and keep the original C checkout unchanged.

## 9. Addendum: RAOP header limits and CSeq (2026-11-26)

- Before editing, read this file, `rust/MAP.md` and the latest checkpoint. Compared `lib/raop.c`, `utils.c::parse_int`, and `http_request.c::http_request_header_get_size` with the Rust request adapter and response builder.
- Ported the 20-field / 64-byte name / 1024-byte value limits before connection classification, returning the source 431 response; invalid CSeq now logs the source message and yields no reply. Valid decimal CSeq is normalized for regular replies and the single-RAOP-client 409 response.
- Windows GNU default/`external-dns-sd` app gates pass (426/428 app tests plus 10/98/7 auxiliary targets), both Clippy modes with warnings denied pass, and formatting is clean. Focused request/response tests cover accepted and rejected field-count/name/value boundaries, drop, normalization, and strtol range behavior.
- The C reference checkout remains unchanged. Continue local handler/runtime work; final clean build and target Git sync/push remain at the end. The real-device mirroring issue and native Unix runtime are still unresolved.

## 10. Addendum: process termination hooks (2026-11-26)

- Compared `uxplay.cpp`'s Unix SIGINT/SIGTERM/SIGHUP and Windows Ctrl+C/close/shutdown handling against the app runtime and installed `ctrlc` crate. Rust had only Ctrl+C semantics enabled.
- Enabled `ctrlc`'s `termination` feature. Its Unix implementation registers SIGTERM and SIGHUP alongside SIGINT; its Windows handler accepts console control events. All events call the existing shared shutdown callback and normal server cleanup.
- `cargo tree -e features -i ctrlc` confirms activation; a minimal harness with `termination` also checks for Linux, macOS and FreeBSD. Windows GNU check, default and `external-dns-sd` all-target tests/Clippy, and fmt pass. This Windows host could not inject native Unix signals or Windows close/shutdown events.

## 11. Addendum: unmatched route fallback (2026-11-26)

- Read this checkpoint file in full before resuming. Compared `lib/raop.c` request initialization, route selection and `handler == NULL` logging with `protocol_routes.rs`, `protocol_dispatch.rs` and `ProtocolConnection::request`.
- C initializes 200 OK before route selection. Unknown HTTP/1.1 method/URL combinations leave that response unchanged and log `Unhandled Client Request: %s %s %s`; unknown RTSP methods set 501 and also log because no handler is selected. Rust incorrectly returned HTTP 404 and emitted no matching INFO log.
- Removed the HTTP-only 404 decision, retained default 200, and added byte-preserving source INFO logging for every route decision without a selected handler. Focused Windows GNU tests pass 3/3 for dispatch bytes, live HTTP response/log and RTSP 501/log. Default/feature all-target suites pass 428/430 app tests plus auxiliary 10/98/7 targets; both Clippy modes (`-D warnings`) and fmt check pass.
- The app-level unsupported-HLS precondition can drop requests before route dispatch; the regression enables HLS support so it exercises the unmatched-route behavior itself. Full default/feature suites, Clippy and final clean build remain later gates. Original C files remain unchanged.

- Follow-up audit: `lib/fcup_request.h` explicitly logs socket error code, platform message and trailing newline after a failed single `send()`. Tightened the existing HLS route integration regression from prefix-only to the complete C-shaped error on Windows (`WSAENOTSOCK`) and Unix (`EBADF`); focused Windows GNU test passes. This closes the MAP's FCUP logger acceptance item; wire bytes, single-send and nonnegative partial-send behavior were already covered.

## 12. Addendum: HLS FCUP request ID lifetime (2026-11-26)

- Compared `airplay_video.c::get_next_FCUP_RequestID` and `airplay_video_init` with the HLS store. C increments one `FCUP_RequestID` stored on the long-lived server video object. Rust kept a counter on each playlist and reset it for each new HLS playback.
- Moved the counter to `AirPlayVideoStore`, incrementing it only when generating an FCUP Fetch action. Cache clearing does not reset it, matching the source object's lifetime. Regression checks master/media IDs 1/2, the next playback 3, and post-cache-clear playback 4.
- Focused Windows GNU test passes. Default/feature all-target app tests pass 429/431 plus auxiliary 10/98/7 targets; both Clippy modes (`-D warnings`) and fmt check pass. C reference remains unchanged.

## 13. Correction: HLS FCUP request ID belongs to each slot (2026-11-26)

- The conclusion above that C keeps one server-wide counter was incorrect. Re-read the complete `airplay_video_t` structure, `airplay_video_init()`, new-play allocation, reuse path, and `get_next_FCUP_RequestID()` caller: every cached slot is its own allocated `airplay_video_t`, and initialization sets its `FCUP_RequestID` to zero. Reusing the stored UUID keeps that slot/counter; allocating or recreating a slot resets it.
- Reverted the incorrect store-global counter. Rust keeps `next_request_id` in each `AirPlayVideo`, incrementing for each FCUP Fetch on that playlist. The revised test proves master/media IDs 1/2/3, UUID reuse continues at 4, a new slot starts at 1, and clearing slots means the next one starts at 1.
- Focused Windows GNU regression passes. Default/feature all-target suites pass 429/431 app tests plus auxiliary 10/98/7 targets; both Clippy modes (`-D warnings`) and fmt check pass. This addendum supersedes the conclusion in Addendum 12; the original C tree remains unchanged.

## 14. Addendum: final build and replay boundary (2026-11-26)

- Built the default Windows GNU release binary from the previously absent/empty `rust/app/target/acceptance-release`, then built the `external-dns-sd` release configuration. Both pass; `-h` exits 0.
- Final all-target suites pass: default app 429 plus 10/98/7 auxiliary tests; feature app 431 plus 10/98/7. Clippy with `-D warnings` and fmt check pass in both configurations.
- Rust Anything server replay reports 5/8 at 0 fallback; the three failures are close/reset reads (`hls-master-missing-current-video`, `play-invalid-body-with-session`, `server-info`), each of which passes its own 1/1 replay. Raw C/Rust HTTP byte comparison passes 8/8 after normalizing only `Date`.
- Upstream source refresh fetched exactly two commits, both touching `uxplay.cpp`: PIN-mode password callback sentinel, `-restrict` admission for missing DeviceID, and registration-name CR/LF sanitation. The behaviors are present in the Rust port. No merge/update was applied to the original C checkout to keep it unchanged.
- Target clone was current with remote `master` before synchronization. Mirrored the workspace source/docs while excluding `.git`, `target`, `baseline` and generated artifacts. Compared exact filtered path sets: 811/811 tracked paths, with no source-only or destination-only files. `git diff --cached --check` passed. Pushed code at `9d9e15b9d32b45230a25fd8dd6fffcdabafb39d7`; fetch confirmed the remote `master` SHA. The C reference checkout remains unchanged.

## 15. Addendum: generic app-lift refusal (2026-11-26)

- Ran `rust-anything app_lift` as the final workflow check. It refused because its generated unit metadata does not recognize the hand-integrated app (`no unit is wired yet`) and its subprocess used the default MSVC toolchain without `link.exe`.
- This is not a product build failure: the same app passed clean default/feature Windows GNU release builds and all test/Clippy/fmt gates using the installed `stable-x86_64-pc-windows-gnu` toolchain plus MSYS2 UCRT64 linker. No code change was made for the generic tool's mismatch.
- Reconciled the Feature acceptance frontier: all 118 Windows CLI cases are Rust-only; fallback is removed; clean manual build/replay is complete. Native Unix/platform, live-device, and packaging gates remain open.

## 16. Addendum: standalone path and NTP receive audit (2026-11-26)

- Re-read `main.rs`, `compat_cli.rs`, and the app's command creation sites. The old guidance about removing unrepresentable-argument forwarding/rollback was stale: no reference executable launch or rollback path remains in the Rust app; `fallback_log` only reports zero to the replay harness. Native Unix argument/runtime acceptance remains open.
- Re-read `lib/raop_ntp.c` socket setup and receive loop alongside `raop_ntp_runtime.rs`. C sets `SO_RCVTIMEO`, and any negative `kernel_timestamp_session_recv()` result is logged as the ordinary timeout; Rust maps receive errors to the same timeout path. NTP does not use `select()` here. Keep OS error injection/hardware timestamp acceptance as unverified evidence, but do not treat an unimplemented NTP select branch as a product gap.
- No code change was warranted by this audit. C reference sources remain unchanged.

## 17. Addendum: Linux CI acceptance path (2026-11-26)

- Confirmed `wsl --status` reports WSL is not installed; this host has Rust Linux/FreeBSD targets but no `gcc`/`clang` and no Linux OpenSSL/libplist/GStreamer sysroot. Do not repeat the known failing native cross-check with unchanged prerequisites.
- Added `.github/workflows/rust-linux.yml` for an actual Ubuntu 24.04 build/test environment. It installs Ubuntu's native OpenSSL, libplist, GStreamer development/runtime and Avahi DNS-SD packages, then runs app fmt, tests, Clippy and release builds with both embedded mDNS and `external-dns-sd`.
- GitHub rejected both `git push` and the connected GitHub Contents API write for `.github/workflows/rust-linux.yml` (Git credential lacks OAuth `workflow` scope; Contents API returned 403 `Resource not accessible by integration`). No remote workflow or Actions run exists. The latest checkpoint/MAP update is pushed to target `master` as `1fcb65afa1fa899c068720786b71651cf39f58fa`. Keep Linux acceptance open; publish with an authorized credential/integration, then inspect and fix any actual Linux failures.
- Follow-up confirms the active `gh` credential has `repo` scope only; the GitHub Actions API currently reports zero runs on `master`. User authorization to refresh/add the `workflow` scope is pending.
- The latest documentation-only checkpoint commit is `35d26bd96dff7aed869d7bdccc36d58c002b8c3f`; remote `master` and local HEAD matched after fetch. The only unsynced path is the drafted workflow, which the current credential cannot publish.

## 18. Addendum: isolated upstream refresh (2026-11-26)

- To honor the request to pull the two upstream UxPlay commits without changing the C reference checkout, created detached worktree `D:\Save\Uxplay\UxPlay-upstream-20261126` at upstream `fbd040e70e754e01fa7e6df810fecd08b6484ada`. The original `D:\Save\Uxplay\UxPlay` stays on its prior `master`; its untracked probe artifacts predate this task.
- Both commits only change `uxplay.cpp`: PIN password callback sentinel `-2`, missing-DeviceID admission under `-restrict`, and CR/LF sanitization of pairing-register client names. Rust already implements these source behaviors. No merge to the reference or Rust code change was needed.
- Resume by reading this section and `Checkpoints.md` first. Target Rusty `master` last verified at `dc0f914c204f8628fb5023be5854cb10fb1178eb`; the current local workflow is still untracked in the target review clone because Git writes lack the `workflow` scope. GitHub reports no Actions run yet.
- Static placeholder scan of integrated app/renderer/beacon source found no `todo!` or `unimplemented!`; the only FIXME is the intentional C-compatible `playlistInsert` no-op. This does not close native OS, live Apple-client, Linux CI, or release packaging acceptance.

## 19. Addendum: current startup documentation reconciliation (2026-11-26)

- Re-read the current main path and searched the app for `UXPLAY_REFERENCE` / `UXPLAY_RUST_SERVER`; neither selector is present. Corrected ADR-007/009 and the historical fallback instructions above, and changed README's “Rust front door” build heading to “standalone Rust application.” This is documentation cleanup only; no app source changed.
- Target remote `master` is verified at `6557d4df0d20492353a95fc285259a0e4d77d415`; the target review clone is clean except for the local, untracked `.github/workflows/rust-linux.yml`. GitHub reports zero Actions runs. Keep the authorization and real iPhone capture gaps open.

## 20. Addendum: resumed acceptance blocker audit (2026-11-26)

- Re-read this file and `Checkpoints.md` before continuing. Target `master` is now verified at `e7834a62a3c23ab8fff6834bc52bbbcbe9d6462a`; local HEAD matches remote. `.github/workflows/rust-linux.yml` is the sole untracked path. GitHub CLI scope remains `repo` only (no `workflow`); the Actions API returns `total_count: 0`.
- Rechecked for Docker, Podman, WSL, Linux GCC/Clang, Zig and cross-build wrappers; none are installed. This host cannot provide the native Linux app build/runtime gate.
- Targeted static review found no `todo!`/`unimplemented!` in integrated app/renderer/beacon code and no obvious unhandled protocol session effect; the only FIXME is the source-compatible `playlistInsert` no-op. Avoid speculative parser/crypto changes without a source discrepancy.
- Remaining concrete blockers: authorization to publish `.github/workflows/rust-linux.yml` and inspect/fix the resulting Linux Actions run; full iPhone stderr/raw packets and same-device C comparison for the reported mirror frame failure; native supported-OS/device/release acceptance. The migration is not complete.

## 21. Addendum: first native Linux CI failure and fix (2026-11-26)

- User authorized GitHub CLI `workflow` scope. Pushed `.github/workflows/rust-linux.yml`; first Ubuntu 24.04 run is `45798130202` on workflow commit `dd73925021fe61c016b240402d3d6155efdfe413`.
- Native dependencies installed and fmt passed. Test compilation failed in Unix NTP ancillary timestamp parsing: `libc::CMSG_LEN(...) as _` left the comparison target ambiguous on Linux (`E0282`, `raop_ntp_runtime.rs:1255`). `size_of`/`size_of_val` imports also warned unused on Unix.
- Fixed by comparing CMSG length in explicit `usize` and gating the Windows-only imports with `#[cfg(windows)]`. `cargo +stable-x86_64-pc-windows-gnu fmt --manifest-path rust/app/Cargo.toml -- --check` passes. Push this source fix, then inspect the next complete Linux run; no Linux pass is claimed yet.

## 22. Addendum: Linux test portability fixes (2026-11-26)

- The second Ubuntu run compiled and executed 426 tests; 424 passed and two failed for platform-specific reasons. `fcup_request.expected.bin` had been treated as text by Git, converting the unchanged C HTTP CRLF fixture to LF on Linux even though `http_response.c` writes CRLF bytes explicitly. Marked `rust/record-shims/*.bin` as `-text` so the fixture stays byte-exact on every host.
- Fixed the Unix non-UTF-8 `-rc` test to pass the absolute temp-file path it created; the previous test wrote under a temp directory but invoked `-rc` with only the basename.
- Next: sync these changes, push a checkpoint, and inspect the full Ubuntu rerun through tests, Clippy and both release configurations. Linux acceptance is still open.
- Linux Clippy fixes are in `rust/app/src/host_identity.rs`, `pairing_registry.rs`, and `server_runtime.rs`; rerun native CI for both DNS-SD modes.
- External DNS-SD Linux Clippy follow-up: Windows Bonjour DLL export helpers are now compiled only on Windows or tests; narrow Unix external-DNS-SD dead-code allowance added for the error variants needed by shared API typing.

### 2026-11-26 — Linux CI green
- Ubuntu 24.04 workflow run `45801697290` succeeded on commit `5b8fec0e8572e6ed9c114128085743d9e7096f04`: format, all-target tests, Clippy (`-D warnings`) and optimized release builds all passed for embedded mDNS and `external-dns-sd`.
- Linux CI acceptance for those two configurations is closed. Native Linux device/runtime validation and macOS/BSD acceptance remain open. Resume by reading this file and `Checkpoints.md`; the published target `master` is at the above commit.

### 2026-11-26 — Linux release CLI smoke
- Run `45803017972` passed all Linux gates on code commit `a78c7191c814eb2c0dcfab0c21fcebe10f1b4663`, including external-DNS-SD Clippy after converting the narrow Unix suppression to `#[expect(dead_code)]` for non-test binaries only.
- Added a workflow step to invoke the embedded-mDNS release binary with `-h` and `-v` under an empty temporary home, confirming standalone Linux CLI startup without user config. This workflow-only addition is pending its next CI run.
- Latest target `master` at this checkpoint: `a78c7191c814eb2c0dcfab0c21fcebe10f1b4663`.
- HLS follow-up: `45805967580` passed the test with logger capture in both DNS-SD modes, followed by all Clippy/release steps and the CLI smoke. The previous single timeout did not recur; no runtime change was made.
- 2026-11-26 header audit: MAP rows for `global.h`, `sockets.h`, and `threads.h` now record their replacement at the Rust runtime boundary. C constants/macros have no independent ABI; exact OS thread-create failure parity remains open.

### 2026-11-26 — generated llhttp replacement mapped
- Reviewed `http_request.c` callback/getter use with `http_request.rs`. The 10,007 LOC generated parser and its API/HTTP support files are replaced by the Rust `HttpRequest` boundary (`httparse` plus source-specific state/error rules), not copied line-for-line. Reclassified the three analyzer rows as replaced and kept residual malformed/incremental parser edges on the `http_request.c` row. Current Linux CI run `45806443997` already passed both modes; this is documentation-only.
- 2026-11-26 `dnssdint.h` review: active TXT constants and fields are runtime-covered by the Rust TXT builder and its C oracle; commented alternatives are inactive. No source change required.
