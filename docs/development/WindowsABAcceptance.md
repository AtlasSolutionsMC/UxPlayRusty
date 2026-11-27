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

Use the same packaged `uxplay.exe` for every run. The standard per-user install
path and launch example are:

```powershell
$exe = Join-Path $env:LOCALAPPDATA 'Programs\UxPlay\uxplay.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "UxPlay is not installed at $exe" }
Get-Content (Join-Path (Split-Path $exe) 'BUILD-INFO.txt')
Get-FileHash -LiteralPath $exe -Algorithm SHA256

$env:UXPLAY_CONFIG = Join-Path $testRoot 'default.toml'
Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe)
```

Exit that run from the tray before changing profile. Set `UXPLAY_CONFIG` to
`low-latency.toml` and run the same command for the next trial. Do not start a
second instance. Keep `renderer`, `h265`, resolution, FPS, and `vsync` identical
in both files; only `video.low_latency` and the per-run log filename should
differ. If testing a specific renderer, set the same value (for example
`d3d11`) in both configs. Wait for each process to exit before reading its log
or starting the next profile.

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
4. Save the UxPlay log for each run. Record the `AirPlay mirror codec
   configured` line (codec, source/frame dimensions and renderer selection), the
   `GStreamer video pipeline` line matching that codec (decoder, queues and
   sink), first AirPlay video packet acceptance, and any first GStreamer flow
   error. The pipeline is logged at Info, so the normal log level is sufficient.

After the low-latency comparison, run a separate synchronization comparison if
you want to check `video.vsync`. Keep `video.low_latency`, renderer, codec,
resolution, FPS, sender, and network fixed; change only `video.vsync` between
`true` and `false`. Alternate the order and use the same scene and measurement
method. This setting controls media timestamp scheduling; it does not change
the monitor refresh rate. Record audio/video alignment and smoothness as well
as delay. The sink's `max-lateness` setting is not evidence of lower latency
when synchronization is disabled.

For the chosen settings, keep a continuous stream running for at least 20
minutes and record whether delay grows, frames freeze/drop, audio drifts, or
the window changes size unexpectedly. Capture CPU/GPU load if readily
available. Report observations rather than applying an unsupported latency
pass threshold.

If no sender connection or first-video-packet diagnostic appears, classify the
run as **ingress/network not established** and do not score it as a renderer or
latency result. If packets are accepted but the window stays blank, preserve the
log and classify it as a renderer/display failure.

## Results sheet

Record one row per run:

| Trial | Profile | Renderer/decoder | Codec and source/frame size | Median delay | Worst delay | Drops/freezes | First packet accepted | Log file |
|---:|---|---|---|---:|---:|---|---|---|
| 1 | default | | | | | | | |
| 2 | low-latency | | | | | | | |
| 3 | low-latency | | | | | | | |
| 4 | default | | | | | | | |

Separate synchronization runs (keep the low-latency setting fixed):

| Trial | `video.vsync` | Renderer/decoder | Codec and source/frame size | Median delay | Worst delay | Audio/video alignment and smoothness | Log file |
|---:|---|---|---|---:|---:|---|---|
| 1 | true | | | | | | |
| 2 | false | | | | | | |

Keep the raw camera recording and logs with the worksheet. Report the measured
difference and any smoothness/quality change; do not call either mode better
from queue configuration or synthetic pipeline tests alone.
