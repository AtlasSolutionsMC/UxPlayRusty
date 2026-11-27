# Windows tray lifecycle

Launching UxPlay without arguments opens a terminal log window and adds the
receiver to the notification area. The receiver stays available while no device
is connected; the video window remains owned by the renderer and appears when
media arrives. Command-line launches continue to use the caller's console.

The tray menu provides:

- **Config** opens `%APPDATA%\UxPlay\config.toml` in Notepad.
- **Restart** validates current settings before stopping the active server,
  cleans up renderer, output, discovery and worker resources, then starts the
  server again with the new values.
- **Disconnect** closes classified client control connections and leaves the
  listener available for a new connection.
- **Hide Terminal / Show Terminal** closes or reopens the log window while
  keeping the receiver in the notification area. Closing the log window with
  **X** also leaves the receiver running.
- **Reset Paired Devices** asks for confirmation, removes the configured pairing
  registry, and restarts the receiver so saved device keys are forgotten.
- **Exit** requests graceful server shutdown, waits for cleanup, removes the
  tray icon and exits.

The `startup.enabled` setting registers UxPlay under the current user's
Windows `Run` key with a hidden-start argument. It starts directly in the
notification area without opening the log window. The value is synchronized at
launch and whenever Restart applies the config. A named mutex prevents multiple
receiver instances.
If Explorer restarts, the application registers its tray icon again.

Each normal GUI launch writes to a new timestamped log file under
`%APPDATA%\UxPlay\logs`, for example
`uxplay-2026-11-28_14-05-09.log`. `logging.file` selects the base path and
filename; UxPlay adds the launch timestamp before the extension. Receiver
restarts within the same app launch continue using that session's log. The log
window tails the current session file. Invoking the executable with
command-line arguments attaches to the caller's console or opens one for
diagnostics. The release executable uses the Windows GUI subsystem.

`startup.idle_restart_minutes` controls the automatic restart interval while no
client is connected. It defaults to six hours; `0` disables it. The idle timer
starts over whenever a client connects, so an active session is never cut short.

Automated Windows checks verify compilation, config parsing, shutdown signaling
and closing client sockets. The tray menu, startup sign-in behavior and live
AirPlay connection lifecycle still need an interactive Windows/device check.
