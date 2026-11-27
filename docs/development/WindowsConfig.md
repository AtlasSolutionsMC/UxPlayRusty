# Windows user configuration

On Windows, the first normal launch creates `%APPDATA%\UxPlay\config.toml`. Set
`UXPLAY_CONFIG` to use a different file, which is useful for portable deployments
and diagnostics. The tracked template is
[`config.example.toml`](../../rust/app/assets/config.example.toml).

Settings are applied in this order:

1. Built-in defaults
2. Existing legacy `uxplayrc` selected by `UXPLAYRC`, `-rc`, or the standard
   home-directory lookup
3. `config.toml`
4. Command-line arguments

The later source wins when it sets the same option. This keeps old rc files
working while allowing command-line values to override saved preferences.

## Supported settings

| Key | Type and allowed values | Default | Effect |
|---|---|---|---|
| `server.name` | Non-empty string | `"UxPlay"` | AirPlay receiver name |
| `video.renderer` | `"auto"`, `"d3d11"`, `"vulkan"` | `"auto"` | Video renderer; unavailable backends report a warning and fall back |
| `video.resolution` | String `WIDTHxHEIGHT` or `WIDTHxHEIGHT@FPS`; dimensions 1–9999, inline FPS 1–255 | unset | Maximum requested mirror dimensions/rate |
| `video.max_fps` | Integer 1–255 | unset | Maximum requested frame rate |
| `video.h265` | Boolean | `false` | Enable H.265 mirror negotiation |
| `video.low_latency` | Boolean | `false` | Bound mirror queues and drop stale decoded frames |
| `video.vsync` | Boolean | `true` | Synchronize the video sink to its clock |
| `ble.enabled` | Boolean | `false` | Start/stop the Windows `uxplay-beacon.exe` companion |
| `ble.handoff_path` | String path, absolute or relative to this config file | `%APPDATA%\UxPlay\uxplay.ble` | Handoff file shared with the BLE companion |
| `startup.enabled` | Boolean | `false` | Register or remove UxPlay in the current user's Windows sign-in startup list |
| `logging.level` | `"normal"`, `"debug"`, `"debug_data"` | `"normal"` | Select routine, diagnostic, or packet-level logging |
| `logging.file` | String path, absolute or relative to this config file | `%APPDATA%\UxPlay\logs\uxplay.log` | GUI log destination |

Only listed keys are accepted. Wrong TOML types, unknown keys, invalid renderer
names, empty receiver names, and out-of-range dimensions/FPS stop startup with
the offending key and reason. The application does not silently ignore typos.

The file is plain TOML and can be edited in any text editor. Save it and use the
tray's Restart action to apply changes.

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
