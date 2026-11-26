# Windows tray lifecycle

The no-argument Windows launch starts UxPlay in the notification area. The
receiver stays available while no device is connected; the video window remains
owned by the renderer and appears when media arrives. Command-line launches
continue to use the console path, which is useful for diagnostics.

The tray menu provides:

- **Config** opens `%APPDATA%\UxPlay\config.toml` with the file's registered
  editor.
- **Restart** validates current settings before stopping the active server,
  cleans up renderer, output, discovery and worker resources, then starts the
  server again with the new values.
- **Disconnect** closes classified client control connections and leaves the
  listener available for a new connection.
- **Exit** requests graceful server shutdown, waits for cleanup, removes the
  tray icon and exits.

The `startup.enabled` setting registers or removes UxPlay under the current
user's Windows `Run` key. The value is synchronized at launch and whenever
Restart applies the config. A named mutex prevents multiple receiver instances.
If Explorer restarts, the application registers its tray icon again.

Normal GUI logs are appended to `%APPDATA%\UxPlay\logs\uxplay.log`, unless
`logging.file` selects another path. Invoking
the executable with command-line arguments attaches to the caller's console or
opens one for diagnostics. The release executable uses the Windows GUI
subsystem for the no-argument launch.

Automated Windows checks verify compilation, config parsing, shutdown signaling
and closing client sockets. The tray menu, startup sign-in behavior and live
AirPlay connection lifecycle still need an interactive Windows/device check.
