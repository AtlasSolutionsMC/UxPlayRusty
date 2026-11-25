# TestingBeforeResume — สรุปก่อนเริ่มงานต่อ

วันที่: 2026-11-23  
ผู้เขียน: session นี้  
สถานะ: **ยังไม่ได้แก้โค้ด** มีเพียงรัน binary ที่ build ค้างไว้ และเขียนไฟล์สรุปนี้เท่านั้น

---

## 1. สิ่งที่ทำใน session นี้

### 1.1 ตรวจสถานะพอร์ตก่อนรัน
- อ่าน `rust/MAP.md`, `Checkpoints.md` → `Latest Resume State`, `rust/HANDOFF.md`, `README.md`
- สรุปได้ว่าโปรเจกต์อยู่ใน阶段 **ยังไม่ cutover**:
  - 45 C units: complete 7, partial 27, pending 10, claimed 1
  - startup ปกติยัง fallback ไป `baseline/original-build/uxplay.exe`
  - ต้องตั้ง `UXPLAY_RUST_SERVER=1` ถึงจะให้ Rust ถือ listener เอง

### 1.2 ทดสอบว่าเปิด/run ได้
- `-h` ของ binary Rust และ C baseline ทำงาน exit 0 และข้อความตรงกัน
- รัน Rust-owned server แบบชั่วคราว:

```powershell
$msys = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'
$env:Path = "$msys\bin;$env:Path"
$env:GST_PLUGIN_PATH_1_0 = "$msys\lib\gstreamer-1.0"
$env:GST_PLUGIN_SCANNER_1_0 = "$msys\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
$env:LANG = 'C.UTF-8'
$env:UXPLAY_RUST_SERVER = '1'

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

## 4. สิ่งที่ยังไม่ควรสรุป

- ยังไม่ควรสรุปว่า crypto port ผิดทั้งหมด:Pair Setup / FairPlay / audio transcript ผ่าน local oracle มาก่อนแล้ว
- ยังไม่ควรสรุปว่า GStreamer ผิด: error message ถูกตัด และอาจเป็นผลจาก decrypt fail
- ยังไม่ควรเปลี่ยน fixture/recorded case เพื่อให้ผ่าน; ถ้าจะแก้ต้องแก้ port ตามหลัก rust-anything workflow
- ยังไม่ควรตัด fallback หรือทำ cutover

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
- `D:\Save\Uxplay\UxPlayRusty\rust\app\src\main.rs` — จุดตัดสินใจ fallback vs `UXPLAY_RUST_SERVER`
- `D:\Save\Uxplay\UxPlayRusty\rust\app-cases\http\raw-byte-diff.ps1` — วิธี start server และรัน HTTP differential
- `D:\Save\Uxplay\UxPlayRusty\rust\units\raop-rtp-mirror\`
- `D:\Save\Uxplay\UxPlayRusty\rust\units\mirror-buffer\`
- `D:\Save\Uxplay\UxPlayRusty\baseline\original-build\uxplay.exe` — C reference สำหรับเทียบ
