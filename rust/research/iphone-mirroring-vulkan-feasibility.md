# iPhone mirroring and Vulkan feasibility

Assessment date: 2026-11-26. This is a research note, not a change to the UxPlay protocol or renderer.

## Recommendation

Finish the UxPlay Rust cutover first. Treat a Vulkan sink as a later optional renderer experiment inside the existing GStreamer path. Treat Apple-style iPhone Mirroring as a separate product/binary and protocol stack; do not fold it into the AirPlay receiver while the current real-device video-decryption failure remains unresolved.

The requested projects demonstrate useful pieces, but neither proves that Apple's native Mac-to-iPhone Mirroring protocol has been reimplemented for Windows/Linux or that it works while the phone is locked.

## What the two projects implement

### `iphone-mirror-rs`

The project describes a Linux Rust viewer using CoreDevice/RemoteXPC-side pairing and display services, FFmpeg decoding, and wgpu presentation. Its README reports a live Wi-Fi test on one iPhone 15 / iOS 27 / Arch Linux configuration; it says USB exists but has not been live-tested. The user must have an unlocked, trusted phone, Developer Mode, a mounted developer image, and a saved CoreDevice pairing record. It supports mouse/keyboard/Home and orientation handling, but lists audio, clipboard, and Unicode/IME input as unimplemented. Those prerequisites and the one-device test boundary do not establish locked-screen support or full native iPhone Mirroring compatibility.

Source: [Jakeelamb/iphone-mirror-rs](https://github.com/Jakeelamb/iphone-mirror-rs).

### `iphone-mirror-windows` / iMirror

This is a Windows Rust/Win32 engineering preview using Media Foundation decoding and D3D11 presentation. Its documented paths are USB QuickTime-compatible capture and a wireless AirPlay receiver. Reverse input uses a separate BLE mouse/keyboard connection with iPhone AssistiveTouch. Optional WDA clicks need a signed/trusted runner, Developer Mode, and a compatible developer image. The README says audio is disabled; wireless video and selected WDA behavior were tested on limited hardware, while clean-machine installation and wider compatibility remain open.

This is a practical Windows control companion, not evidence that the native Apple iPhone Mirroring protocol is available outside macOS. AirPlay video does not carry the mouse commands; the control paths are separate. The README instructs the user to unlock/trust the iPhone for USB setup and documents no locked-screen guarantee.

Source: [phh235/iphone-mirror-windows](https://github.com/phh235/iphone-mirror-windows), [release validation and limitations](https://github.com/phh235/iphone-mirror-windows/releases).

### Apple's native feature

Apple documents iPhone Mirroring as a Mac feature requiring macOS Sequoia 15 or later on Apple silicon or T2 hardware, iOS 18 or later, the same Apple Account with two-factor authentication, and Wi-Fi/Bluetooth. Apple's user instructions say the iPhone stays locked during use. These are requirements for Apple's feature and do not show that third-party Windows/Linux implementations use its native protocol.

Source: [Apple Support: iPhone Mirroring requirements and behavior](https://support.apple.com/en-ca/120421).

## Fit with UxPlayRusty

- **Protocol:** UxPlayRusty implements AirPlay/RAOP over DNS-SD, HTTP/RTSP, FairPlay and media RTP. CoreDevice/RemoteXPC/DisplayService and the associated input services are separate protocols and device setup flows. There is no direct route or handler to reuse for receiving the native Apple feature.
- **Reusable work:** Rust ownership, logging, app lifecycle and some display/window utilities could be reused after a separate design review. UxPlay pairing identities and Apple Developer pairing records are not interchangeable by assumption. Do not copy foreign implementation code into the main port; review GPL-3.0 and bundled third-party terms first.
- **Vulkan today:** The UxPlay Rust video renderer is GStreamer-based and this workspace has no wgpu/Vulkan dependency. The local MSYS2 GStreamer 1.28.9 installation does contain the Vulkan plugin and `vulkansink`, but its sink accepts Vulkan-image memory and has no factory rank. GStreamer documents `vulkanupload` as the system-memory to Vulkan-memory bridge. The current renderer pipeline does not insert that bridge, so the plugin's presence does not mean UxPlay currently renders through Vulkan.
- **Smallest Vulkan experiment:** Add an opt-in, Linux-first sink path after the core cutover. Keep the default sink unchanged; create a caps-compatible `... ! vulkanupload ! vulkansink` pipeline, confirm surface/window overlay integration, and measure decode-to-present latency, dropped frames, CPU/GPU use and resource teardown against the current sink. Test only on a Vulkan-capable host with the required GStreamer Bad Plug-ins. Do not promise zero-copy: this depends on decoder memory and device interoperability.
- **The real-device blocker:** `TestingBeforeResume.md` records repeated AES mirror-packet failures on an iPhone before later intermittent pipeline startup. A Vulkan sink cannot repair a wrong AirPlay key/IV/CTR sequence. First collect complete stderr and compare the same phone/session with the unchanged C reference; do not spend the cutover budget on a renderer rewrite before that diagnosis.

## Feasibility by goal

| Goal | Assessment now | Work boundary |
|---|---|---|
| Complete the current AirPlay Rust port | High for code already within this workspace; acceptance is still incomplete | Continue upstream parity, real-client crypto diagnosis, source-unit gates, native platform validation, final clean build and Git delivery |
| Optional Vulkan presentation for UxPlay | Medium | A contained GStreamer renderer feature; needs native Vulkan/GStreamer host and performance/teardown proof |
| Windows/Linux AirPlay mirror plus convenient mouse control | Medium | Existing AirPlay video and a separately validated BLE AssistiveTouch/WDA control channel; not native iPhone Mirroring |
| Native iPhone Mirroring on Windows/Linux, including locked-phone operation | Low/unknown | New protocol reverse engineering, pairing/security, DisplayService, input, audio and OS/GPU support. Current public-project evidence does not close those gaps |

## Decision for this migration

No iPhone-Mirroring protocol integration or Vulkan production change is included in the current fast UxPlay cutover. Keep both as post-cutover projects. Continue first with the actual upstream UxPlay compatibility fixes, mirror decryption interoperability, full CLI/runtime parity and clean Git delivery. Revisit the Vulkan spike when the current media path is accepted; start native iPhone Mirroring only as a separately scoped Rust binary with its own pairing and device acceptance plan.
