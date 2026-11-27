# Windows real-device A/B acceptance

Use this procedure after AirPlay can connect through the host firewall. It
compares the default mirror queues with the opt-in low-latency queues while
keeping the renderer, network, device, and stream settings constant. It does not
replace the clean-machine installer check or Vulkan hardware acceptance.

## Prepare isolated configs

Start from the checked-in template and use separate temporary files so the
existing user configuration is not changed:

```powershell
$testRoot = Join-Path $env:TEMP ("uxplay-ab-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$template = Get-Content .\rust\app\assets\config.example.toml -Raw
$defaultToml = $template.Replace('# file = "logs/uxplay.log"', 'file = "' + (Join-Path $testRoot 'default.log').Replace('\', '/') + '"')
$lowLatencyToml = $template.Replace('low_latency = false', 'low_latency = true').Replace('# file = "logs/uxplay.log"', 'file = "' + (Join-Path $testRoot 'low-latency.log').Replace('\', '/') + '"')
[IO.File]::WriteAllText((Join-Path $testRoot 'default.toml'), $defaultToml, [Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText((Join-Path $testRoot 'low-latency.toml'), $lowLatencyToml, [Text.UTF8Encoding]::new($false))
```

Use the same packaged `uxplay.exe` for every run. Set `UXPLAY_CONFIG` to the
selected file before starting it, then launch the executable from the same
PowerShell session so it inherits that setting. Exit the previous receiver
through its tray menu before changing profiles; do not start a duplicate instance. Leave
`renderer`, `h265`, resolution, FPS, and `vsync` identical in both files. If
testing a specific renderer, set the same value (for example `d3d11`) in both.

## Run and measure

1. Record the source revision from the package's `BUILD-INFO.txt`, package hash,
   Windows build, GPU/driver, iPhone model/iOS, Wi-Fi AP/band, and the test date.
2. Alternate the profiles (default, low-latency, low-latency, default) to reduce
   ordering and warm-up bias. For each run, wait 15 seconds after mirroring
   begins, then record 60 seconds of the same moving image or counter.
3. If possible, film the iPhone and receiver display together at 120/240 fps.
   Compare at least five matching on-screen timestamps or frame changes per run;
   record median and worst observed delay, plus visible freezes/drops. Use the
   same camera, angle, brightness, and display refresh settings for both modes.
4. Save the UxPlay log for each run. Record whether it reports the first AirPlay
   video packet accepted, the selected renderer/decoder, negotiated codec and
   dimensions, and any first GStreamer flow error.

If no sender connection or first-video-packet diagnostic appears, classify the
run as **ingress/network not established** and do not score it as a renderer or
latency result. If packets are accepted but the window stays blank, preserve the
log and classify it as a renderer/display failure.

## Results sheet

Record one row per run:

| Profile | Renderer/decoder | Codec and size | Median delay | Worst delay | Drops/freezes | First packet accepted | Log file |
|---|---|---|---:|---:|---|---|---|
| default | | | | | | | |
| low-latency | | | | | | | |

Keep the raw camera recording and logs with the worksheet. Report the measured
difference and any smoothness/quality change; do not call either mode better
from queue configuration or synthetic pipeline tests alone.
