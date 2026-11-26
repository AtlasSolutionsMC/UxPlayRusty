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
| `startup.enabled` | Boolean | `false` | Saved startup preference; Windows registration is implemented with the tray/startup milestone |

Only listed keys are accepted. Wrong TOML types, unknown keys, invalid renderer
names, empty receiver names, and out-of-range dimensions/FPS stop startup with
the offending key and reason. The application does not silently ignore typos.

The file is plain TOML and can be edited in any text editor. Save it and use the
tray's Restart action to apply changes once the tray milestone is available.
