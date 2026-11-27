# Windows user configuration

On Windows, the first normal launch creates `%APPDATA%\UxPlay\config.toml`. Set
`UXPLAY_CONFIG` to use a different file, which is useful for portable deployments
and diagnostics. The tracked template is
[`config.example.toml`](../../app/assets/config.example.toml).

Settings are applied in this order:

1. Built-in defaults
2. Existing legacy `uxplayrc` selected by `UXPLAYRC`, `-rc`, or the standard
   home-directory lookup
3. `config.toml`
4. Command-line arguments

The later source wins when it sets the same option. This keeps old rc files
working while allowing command-line values to override saved preferences.

## Supported settings

| Section / keys | Values and bounds | Effect |
|---|---|---|
| `server.name`, `server.append_hostname` | Non-empty name; boolean | Advertised AirPlay name and whether to add the computer hostname |
| `video.renderer` | `auto`, `d3d11`, `vulkan` | Select renderer; unavailable backends may fall back |
| `video.resolution`, `video.max_fps` | `WIDTHxHEIGHT[@FPS]`, dimensions 1–9999, FPS 1–255 | Set requested size and frame-rate caps |
| `video.h265`, `low_latency`, `vsync`, `fullscreen`, `overscan` | Booleans | Codec negotiation, queue latency, clock sync, fullscreen and crop behavior |
| `video.sync_delay_ms` | -999.999..999.999 ms; requires `vsync=true` | Video clock offset |
| `video.flip`, `video.rotation` | `none`/`horizontal`/`vertical`/`invert`; `none`/`left`/`right` | Flip and rotate the displayed image |
| `audio.enabled`, `audio.sync`, `audio.volume_taper` | Booleans | Enable audio, synchronize it, and taper volume |
| `audio.sink` | GStreamer element and optional properties | Choose the audio output sink |
| `audio.sync_delay_ms`, `audio.latency_seconds`, `audio.volume`, `audio.db_range` | Delay -999.999..999.999 ms; latency 0..10 sec; volume 0..1; dB `[low,high]` with low < 0 and high > low | Audio synchronization, buffering, initial volume and gain range |
| `connection.close_window_on_client_stop`, `takeover`, `nofreeze` | Booleans | Control video-window reset, client takeover and freeze recovery |
| `connection.reset_after_seconds` | Integer 0..86400; 0 disables | Wait before resetting an idle stream |
| `connection.tcp_ports`, `udp_ports` | Three distinct integers per list, each 1024..65535 | Set the receiver's TCP/UDP port triplets |
| `access.mode` | `open`, `first_time`, `every_time`, `password` | No challenge, paired 4-digit PIN, generated 4-digit code for each new connection, or password |
| `access.pin`, `password`, `pairing_file` | PIN `0001`..`9999`; password at least 4 chars; file path | Fixed first-pairing PIN, custom password, and paired-device registry file |
| `access.allow_ids`, `block_ids`, `restrict` | Arrays of non-empty device IDs; boolean | Allow/block clients and optionally reject IDs not in allow list |
| `startup.enabled`, `idle_restart_minutes` | Boolean; integer 0..10080 (0 disables; default 360) | Start hidden at Windows sign-in; restart receiver after that many minutes with no clients |
| `gstreamer.*` | Element names: video parser/decoder/converter/sink and audio sink | Override GStreamer elements; availability depends on installed plugins |
| `hls.enabled`, `player_version`, `languages`, `subtitles`, `custom_profile` | Boolean; player 2 or 3; colon-separated language tags; UxPlay custom profile syntax | Configure HLS playback for compatible AirPlay apps, mainly YouTube |
| `recording.*` | Booleans, optional filenames and positive packet limits | MP4, cover art, metadata, audio/video packet dumps, and FPS output |
| `identity.mac_address`, `random_mac`, `key_file` | MAC format `xx:xx:xx:xx:xx:xx`; booleans; path | Change receiver identity or pairing key. Fixed and random MAC cannot both be set |
| `advanced.video_rtp_pipeline`, `audio_rtp_pipeline`, `dacp_file` | Non-empty GStreamer pipeline strings / path | RTP output pipelines and DACP service file; advanced use only |
| `ble.enabled`, `ble.handoff_path` | Boolean / path | Start or stop the Windows BLE companion and select its handoff file |
| `logging.level`, `logging.file`, `notifications` | `normal`/`debug`/`debug_data`; path; boolean | Choose log verbosity/file and tray PIN notifications |

In `first_time` mode the registration list is written to `access.pairing_file`;
the default is `paired-devices.txt` beside the config. The tray menu's Reset
Paired Devices action removes this file and restarts the receiver.
In `every_time` mode UxPlay uses its random-password challenge, which displays a
new 4-digit code for each new connection. The code is written to the log in both
cases. `notifications = false` suppresses the tray notification but does not hide
the PIN from the log terminal or log file. Since the notification includes the
PIN, anyone who can see Windows notifications can see that code too.

Only listed keys are accepted. Wrong TOML types, unknown keys, invalid renderer
names, empty receiver names, and out-of-range dimensions/FPS stop startup with
the offending key and reason. The application does not silently ignore typos.

The file is plain TOML and can be edited in any text editor. Save it and use the
tray's Restart action to apply changes. Settings that name GStreamer elements
and pipelines are passed to the installed GStreamer runtime, so this parser can
validate their type but cannot guarantee a plugin is installed on a specific PC.

## Network and firewall

AirPlay discovery uses mDNS on UDP 5353 and advertises the service ports that
UxPlay actually bound. Windows may assign those TCP/UDP ports dynamically, so a
fixed-port firewall exception can miss the active connections. When Windows
Firewall prompts on first launch, allow `uxplay.exe` for the network where the
iPhone/iPad is connected. On a trusted LAN, prefer the Private profile or an
application rule restricted to `LocalSubnet`; avoid opening the receiver to
all remote addresses on a Public network. If the receiver appears on the phone
but no stream starts, check the active Windows network profile and inbound
UxPlay rules, then inspect the configured log for AirPlay connection and video
packet diagnostics.

For a temporary diagnostic when an explicit block rule exists for the running
receiver, open PowerShell as Administrator. First inspect the profile and the
exact rules that match the running executable:

```powershell
Get-NetConnectionProfile | Format-Table InterfaceAlias, NetworkCategory
$uxplayPath = (Get-Process -Name uxplay | Select-Object -First 1 -ExpandProperty Path)
$profile = 'Public' # Set this to the profile used by the iPhone's network.
$blockedRules = @(Get-NetFirewallRule -PolicyStore ActiveStore -Direction Inbound -Enabled True -Action Block |
    Where-Object { ($_.Profile -eq 'Any' -or $_.Profile -match $profile) -and
        (Get-NetFirewallApplicationFilter -AssociatedNetFirewallRule $_).Program -ieq $uxplayPath })
$blockedRules | Select-Object Name, DisplayName, Profile
```

Only if that output lists the intended UxPlay executable's block rules, disable
those exact rules and add a temporary application rule scoped to the local
subnet. The receiver binds dynamic ports, so the application rule does not pin
an obsolete port:

```powershell
if ($blockedRules.Count -eq 0) { throw 'No matching UxPlay block rules were found; stop and inspect the firewall first.' }
Disable-NetFirewallRule -Name $blockedRules.Name
$testAllow = New-NetFirewallRule -DisplayName 'UxPlay temporary LocalSubnet test' -Direction Inbound `
    -Program $uxplayPath -Action Allow -Profile $profile -RemoteAddress LocalSubnet
```

After the test, remove the temporary allow and restore the rules that were
disabled:

```powershell
Disable-NetFirewallRule -Name $testAllow.Name
Enable-NetFirewallRule -Name $blockedRules.Name
```

Keep this as a temporary diagnostic. Do not disable the firewall globally or
create an unrestricted inbound rule for a Public network.

When BLE is enabled, UxPlay writes the negotiated AirPlay port and its process
identity to the handoff file, then starts the Windows Runtime BLE companion next
to `uxplay.exe`. If the companion binary or an adapter is unavailable, UxPlay
logs the failure and keeps AirPlay running. Shutdown removes the handoff file so
the companion can stop its advertisement before UxPlay exits.
