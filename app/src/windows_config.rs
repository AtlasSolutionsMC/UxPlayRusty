//! User-editable Windows settings layered between the legacy rc file and CLI arguments.

use std::path::PathBuf;

const DEFAULT_CONFIG: &str = include_str!("../assets/config.example.toml");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub startup_enabled: Option<bool>,
    pub notifications_enabled: Option<bool>,
    pub idle_restart_minutes: Option<u32>,
    pub pairing_file: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
}

pub fn parse(text: &str) -> Result<(Vec<String>, UserConfig), String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|error| format!("invalid TOML syntax: {error}"))?;
    let root = value
        .as_table()
        .ok_or_else(|| "configuration root must be a TOML table".to_owned())?;
    reject_unknown(
        root,
        &[
            "server",
            "video",
            "audio",
            "connection",
            "access",
            "gstreamer",
            "hls",
            "recording",
            "identity",
            "advanced",
            "ble",
            "startup",
            "logging",
            "notifications",
        ],
        "",
    )?;

    let mut args = Vec::new();
    let mut pairing_file = None;
    if let Some(server) = table(root, "server")? {
        reject_unknown(server, &["name", "append_hostname"], "server")?;
        if let Some(name) = string(server, "name", "server.name")? {
            if name.trim().is_empty() {
                return Err("`server.name` must not be empty".to_owned());
            }
            args.extend(["-n".to_owned(), name.to_owned()]);
        }
        if let Some(enabled) = boolean(server, "append_hostname", "server.append_hostname")? {
            args.push(if enabled { "-append-hostname" } else { "-nh" }.to_owned());
        }
    }

    if let Some(video) = table(root, "video")? {
        reject_unknown(
            video,
            &[
                "renderer",
                "resolution",
                "max_fps",
                "h265",
                "low_latency",
                "vsync",
                "sync_delay_ms",
                "fullscreen",
                "flip",
                "rotation",
                "overscan",
            ],
            "video",
        )?;
        if let Some(renderer) = string(video, "renderer", "video.renderer")? {
            if !["auto", "d3d11", "vulkan"]
                .iter()
                .any(|value| renderer.eq_ignore_ascii_case(value))
            {
                return Err("`video.renderer` must be `auto`, `d3d11`, or `vulkan`".to_owned());
            }
            args.extend(["-renderer".to_owned(), renderer.to_owned()]);
        }
        if let Some(resolution) = string(video, "resolution", "video.resolution")? {
            if !valid_resolution(resolution) {
                return Err("`video.resolution` must be WIDTHxHEIGHT or WIDTHxHEIGHT@FPS (1..9999 pixels, FPS 1..255)".to_owned());
            }
            args.extend(["-s".to_owned(), resolution.to_owned()]);
        }
        if let Some(fps) = integer(video, "max_fps", "video.max_fps")? {
            if !(1..=255).contains(&fps) {
                return Err("`video.max_fps` must be an integer from 1 to 255".to_owned());
            }
            args.extend(["-fps".to_owned(), fps.to_string()]);
        }
        if let Some(enabled) = boolean(video, "h265", "video.h265")? {
            args.push(if enabled { "-h265" } else { "-no-h265" }.to_owned());
        }
        if let Some(enabled) = boolean(video, "low_latency", "video.low_latency")? {
            args.push(
                if enabled {
                    "-lowlatency"
                } else {
                    "-nolowlatency"
                }
                .to_owned(),
            );
        }
        if let Some(enabled) = boolean(video, "vsync", "video.vsync")? {
            // `-vsync` enables timestamp synchronization by itself; its optional
            // argument is a numeric delay, while the only disabling value is `no`.
            // Emitting `yes` here is parsed as an invalid delay by the legacy CLI.
            args.push("-vsync".to_owned());
            if !enabled {
                args.push("no".to_owned());
            }
        }
        if let Some(delay) = number(video, "sync_delay_ms", "video.sync_delay_ms")? {
            if !(-999.999..=999.999).contains(&delay) {
                return Err(
                    "`video.sync_delay_ms` must be between -999.999 and 999.999 milliseconds"
                        .to_owned(),
                );
            }
            if boolean(video, "vsync", "video.vsync")? == Some(false) {
                return Err("`video.sync_delay_ms` requires `video.vsync = true`".to_owned());
            }
            args.extend(["-vsync".to_owned(), number_text(delay)]);
        }
        if let Some(enabled) = boolean(video, "fullscreen", "video.fullscreen")? {
            args.push(if enabled { "-fs" } else { "-no-fs" }.to_owned());
        }
        if let Some(flip) = string(video, "flip", "video.flip")? {
            let value = match flip {
                "none" => "N",
                "horizontal" => "H",
                "vertical" => "V",
                "invert" => "I",
                _ => {
                    return Err(
                        "`video.flip` must be `none`, `horizontal`, `vertical`, or `invert`"
                            .to_owned(),
                    );
                }
            };
            args.extend(["-f".to_owned(), value.to_owned()]);
        }
        if let Some(rotation) = string(video, "rotation", "video.rotation")? {
            let value = match rotation {
                "none" => "N",
                "right" => "R",
                "left" => "L",
                _ => return Err("`video.rotation` must be `none`, `right`, or `left`".to_owned()),
            };
            args.extend(["-r".to_owned(), value.to_owned()]);
        }
        if let Some(enabled) = boolean(video, "overscan", "video.overscan")? {
            args.push(if enabled { "-o" } else { "-no-o" }.to_owned());
        }
    }

    if let Some(audio) = table(root, "audio")? {
        reject_unknown(
            audio,
            &[
                "enabled",
                "sink",
                "sync",
                "sync_delay_ms",
                "latency_seconds",
                "volume",
                "volume_taper",
                "db_range",
            ],
            "audio",
        )?;
        if let Some(enabled) = boolean(audio, "enabled", "audio.enabled")? {
            args.push(if enabled { "-audio" } else { "-a" }.to_owned());
        }
        if let Some(sink) = string(audio, "sink", "audio.sink")? {
            if sink.trim().is_empty() {
                return Err("`audio.sink` must not be empty".to_owned());
            }
            args.extend(["-as".to_owned(), sink.to_owned()]);
        }
        let sync = boolean(audio, "sync", "audio.sync")?;
        let sync_delay = number(audio, "sync_delay_ms", "audio.sync_delay_ms")?;
        if let Some(delay) = sync_delay {
            if !(-999.999..=999.999).contains(&delay) {
                return Err(
                    "`audio.sync_delay_ms` must be between -999.999 and 999.999 milliseconds"
                        .to_owned(),
                );
            }
            if sync == Some(false) {
                return Err("`audio.sync_delay_ms` requires `audio.sync = true`".to_owned());
            }
            args.extend(["-async".to_owned(), number_text(delay)]);
        } else if let Some(enabled) = sync {
            args.push("-async".to_owned());
            if !enabled {
                args.push("no".to_owned());
            }
        }
        if let Some(latency) = number(audio, "latency_seconds", "audio.latency_seconds")? {
            if !(0.0..=10.0).contains(&latency) {
                return Err("`audio.latency_seconds` must be between 0 and 10 seconds".to_owned());
            }
            args.extend(["-al".to_owned(), number_text(latency)]);
        }
        if let Some(volume) = number(audio, "volume", "audio.volume")? {
            if !(0.0..=1.0).contains(&volume) {
                return Err(
                    "`audio.volume` must be between 0.0 (mute) and 1.0 (full volume)".to_owned(),
                );
            }
            args.extend(["-vol".to_owned(), number_text(volume)]);
        }
        if let Some(enabled) = boolean(audio, "volume_taper", "audio.volume_taper")? {
            args.push(if enabled { "-taper" } else { "-no-taper" }.to_owned());
        }
        if let Some(range) = numbers(audio, "db_range", "audio.db_range")? {
            if range.len() != 2 || range[0] >= 0.0 || range[1] <= range[0] {
                return Err(
                    "`audio.db_range` must be [low, high] with low < 0 and high > low".to_owned(),
                );
            }
            args.extend([
                "-db".to_owned(),
                format!("{}:{}", number_text(range[0]), number_text(range[1])),
            ]);
        }
    }

    if let Some(connection) = table(root, "connection")? {
        reject_unknown(
            connection,
            &[
                "close_window_on_client_stop",
                "reset_after_seconds",
                "takeover",
                "tcp_ports",
                "udp_ports",
                "nofreeze",
            ],
            "connection",
        )?;
        if let Some(close) = boolean(
            connection,
            "close_window_on_client_stop",
            "connection.close_window_on_client_stop",
        )? {
            args.extend(if close {
                ["-nc".to_owned(), "no".to_owned()]
            } else {
                ["-nc".to_owned(), String::new()]
            });
            if !close {
                args.pop();
            }
        }
        if let Some(seconds) = integer(
            connection,
            "reset_after_seconds",
            "connection.reset_after_seconds",
        )? {
            if !(0..=86_400).contains(&seconds) {
                return Err("`connection.reset_after_seconds` must be between 0 and 86400; 0 disables reset".to_owned());
            }
            args.extend(["-reset".to_owned(), seconds.to_string()]);
        }
        if let Some(takeover) = boolean(connection, "takeover", "connection.takeover")? {
            args.push(if takeover { "-nohold" } else { "-hold" }.to_owned());
        }
        for (key, label) in [("tcp_ports", "TCP"), ("udp_ports", "UDP")] {
            if let Some(ports) = integers(connection, key, &format!("connection.{key}"))? {
                if ports.len() != 3
                    || ports.iter().any(|port| !(1024..=65535).contains(port))
                    || ports.iter().collect::<std::collections::HashSet<_>>().len() != 3
                {
                    return Err(format!(
                        "`connection.{key}` must contain three distinct ports from 1024 to 65535"
                    ));
                }
                args.extend([
                    "-p".to_owned(),
                    label.to_ascii_lowercase(),
                    ports
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ]);
            }
        }
        if let Some(enabled) = boolean(connection, "nofreeze", "connection.nofreeze")? {
            args.push(if enabled { "-nofreeze" } else { "-freeze" }.to_owned());
        }
    }

    if let Some(gstreamer) = table(root, "gstreamer")? {
        reject_unknown(
            gstreamer,
            &[
                "video_parser",
                "video_decoder",
                "video_converter",
                "video_sink",
                "audio_sink",
            ],
            "gstreamer",
        )?;
        for (key, option) in [
            ("video_parser", "-vp"),
            ("video_decoder", "-vd"),
            ("video_converter", "-vc"),
            ("video_sink", "-vs"),
            ("audio_sink", "-as"),
        ] {
            if let Some(value) = string(gstreamer, key, &format!("gstreamer.{key}"))? {
                if value.trim().is_empty() {
                    return Err(format!("`gstreamer.{key}` must not be empty"));
                }
                args.extend([option.to_owned(), value.to_owned()]);
            }
        }
    }

    if let Some(hls) = table(root, "hls")? {
        reject_unknown(
            hls,
            &[
                "enabled",
                "player_version",
                "languages",
                "subtitles",
                "custom_profile",
            ],
            "hls",
        )?;
        let enabled = boolean(hls, "enabled", "hls.enabled")?;
        let player_version = integer(hls, "player_version", "hls.player_version")?;
        if let Some(version) = player_version {
            if !(2..=3).contains(&version) {
                return Err("`hls.player_version` must be 2 or 3".to_owned());
            }
        }
        let languages = string(hls, "languages", "hls.languages")?;
        let subtitles = string(hls, "subtitles", "hls.subtitles")?;
        let profile = string(hls, "custom_profile", "hls.custom_profile")?;
        if enabled == Some(true) {
            args.push("-hls".to_owned());
            args.push(player_version.unwrap_or(3).to_string());
            if let Some(value) = languages.filter(|value| !value.trim().is_empty()) {
                args.extend(["-lang".to_owned(), value.to_owned()]);
            }
            if let Some(value) = subtitles.filter(|value| !value.trim().is_empty()) {
                args.extend(["-slang".to_owned(), value.to_owned()]);
            }
            if let Some(value) = profile.filter(|value| !value.trim().is_empty()) {
                args.extend(["-custom".to_owned(), value.to_owned()]);
            }
        } else if enabled == Some(false) {
            args.push("-no-hls".to_owned());
        } else if languages.is_some_and(|value| !value.trim().is_empty())
            || subtitles.is_some_and(|value| !value.trim().is_empty())
            || profile.is_some_and(|value| !value.trim().is_empty())
        {
            return Err(
                "`hls.enabled` must be true to apply HLS language or profile options".to_owned(),
            );
        }
    }

    if let Some(recording) = table(root, "recording")? {
        reject_unknown(
            recording,
            &[
                "mp4_enabled",
                "mp4_file",
                "cover_art",
                "cover_art_file",
                "metadata_file",
                "video_dump_enabled",
                "video_dump_limit",
                "video_dump_file",
                "audio_dump_enabled",
                "audio_dump_limit",
                "audio_dump_file",
                "show_fps",
            ],
            "recording",
        )?;
        if let Some(enabled) = boolean(recording, "mp4_enabled", "recording.mp4_enabled")? {
            if enabled {
                args.push("-mp4".to_owned());
                if let Some(path) = string(recording, "mp4_file", "recording.mp4_file")? {
                    args.push(path.to_owned());
                }
            } else {
                args.push("-no-mp4".to_owned());
            }
        }
        let cover_art = boolean(recording, "cover_art", "recording.cover_art")?;
        let cover_art_file = string(recording, "cover_art_file", "recording.cover_art_file")?;
        if cover_art == Some(true) && cover_art_file.is_some() {
            return Err(
                "use only one of `recording.cover_art` and `recording.cover_art_file`".to_owned(),
            );
        }
        if cover_art_file.is_some() {
            args.extend(["-ca".to_owned(), cover_art_file.unwrap().to_owned()]);
        } else if let Some(enabled) = cover_art {
            args.push(if enabled { "-ca" } else { "-no-ca" }.to_owned());
        }
        if let Some(path) = string(recording, "metadata_file", "recording.metadata_file")? {
            args.extend(["-md".to_owned(), path.to_owned()]);
        }
        for (enabled_key, limit_key, file_key, flag, label) in [
            (
                "video_dump_enabled",
                "video_dump_limit",
                "video_dump_file",
                "-vdmp",
                "video",
            ),
            (
                "audio_dump_enabled",
                "audio_dump_limit",
                "audio_dump_file",
                "-admp",
                "audio",
            ),
        ] {
            let enabled = boolean(recording, enabled_key, &format!("recording.{enabled_key}"))?;
            let limit = integer(recording, limit_key, &format!("recording.{limit_key}"))?;
            let file = string(recording, file_key, &format!("recording.{file_key}"))?;
            if enabled == Some(false) && (limit.is_some() || file.is_some()) {
                return Err(format!(
                    "`recording.{limit_key}` and `recording.{file_key}` require `{enabled_key} = true`"
                ));
            }
            if enabled == Some(true) {
                args.push(flag.to_owned());
                if let Some(limit) = limit {
                    if limit <= 0 {
                        return Err(format!("`recording.{limit_key}` must be positive"));
                    }
                    args.push(limit.to_string());
                }
                if let Some(file) = file {
                    args.push(file.to_owned());
                }
            } else if enabled == Some(false) {
                args.push(
                    if label == "video" {
                        "-no-vdmp"
                    } else {
                        "-no-admp"
                    }
                    .to_owned(),
                );
            }
        }
        if let Some(enabled) = boolean(recording, "show_fps", "recording.show_fps")? {
            args.push(if enabled { "-FPSdata" } else { "-no-FPSdata" }.to_owned());
        }
    }

    if let Some(identity) = table(root, "identity")? {
        reject_unknown(
            identity,
            &["mac_address", "random_mac", "key_file"],
            "identity",
        )?;
        let mac = string(identity, "mac_address", "identity.mac_address")?;
        let random = boolean(identity, "random_mac", "identity.random_mac")?.unwrap_or(false);
        if mac.is_some() && random {
            return Err(
                "`identity.mac_address` and `identity.random_mac = true` cannot be used together"
                    .to_owned(),
            );
        }
        if let Some(mac) = mac {
            args.extend(["-m".to_owned(), mac.to_owned()]);
        } else if random {
            args.push("-m".to_owned());
        } else {
            args.extend(["-m".to_owned(), "default".to_owned()]);
        }
        if let Some(path) = string(identity, "key_file", "identity.key_file")? {
            args.extend(["-key".to_owned(), path.to_owned()]);
        } else {
            args.extend(["-key".to_owned(), "0".to_owned()]);
        }
    }

    if let Some(advanced) = table(root, "advanced")? {
        reject_unknown(
            advanced,
            &["video_rtp_pipeline", "audio_rtp_pipeline", "dacp_file"],
            "advanced",
        )?;
        for (key, flag) in [
            ("video_rtp_pipeline", "-vrtp"),
            ("audio_rtp_pipeline", "-artp"),
            ("dacp_file", "-dacp"),
        ] {
            if let Some(value) = string(advanced, key, &format!("advanced.{key}"))? {
                if value.trim().is_empty() {
                    return Err(format!("`advanced.{key}` must not be empty"));
                }
                args.extend([flag.to_owned(), value.to_owned()]);
            }
        }
    }

    if let Some(access) = table(root, "access")? {
        reject_unknown(
            access,
            &[
                "mode",
                "pin",
                "password",
                "pairing_file",
                "allow_ids",
                "block_ids",
                "restrict",
            ],
            "access",
        )?;
        let mode = string(access, "mode", "access.mode")?.unwrap_or("open");
        let pin = string(access, "pin", "access.pin")?;
        let password = string(access, "password", "access.password")?;
        match mode {
            "open" => {
                if pin.is_some() || password.is_some() {
                    return Err("`access.pin` and `access.password` are only valid with their matching access mode".to_owned());
                }
                args.push("-no-pin".to_owned());
            }
            "first_time" => {
                if password.is_some() {
                    return Err(
                        "`access.password` is only valid when `access.mode = \"password\"`"
                            .to_owned(),
                    );
                }
                if let Some(pin) = pin {
                    if pin.len() != 4
                        || !pin.bytes().all(|byte| byte.is_ascii_digit())
                        || pin == "0000"
                    {
                        return Err(
                            "`access.pin` must be a four-digit PIN from `0001` to `9999`"
                                .to_owned(),
                        );
                    }
                }
                args.push("-pin".to_owned());
                if let Some(pin) = pin {
                    args.push(pin.to_owned());
                }
                let path = string(access, "pairing_file", "access.pairing_file")?
                    .unwrap_or("paired-devices.txt");
                pairing_file = Some(PathBuf::from(path));
                args.extend(["-reg".to_owned(), path.to_owned()]);
            }
            "every_time" => {
                if pin.is_some() || password.is_some() {
                    return Err("`access.pin` and `access.password` cannot be set when `access.mode = \"every_time\"`".to_owned());
                }
                // UxPlay's random-password mode displays a fresh four-digit
                // challenge for each successful new client connection.
                args.push("-pw".to_owned());
            }
            "password" => {
                if pin.is_some() {
                    return Err(
                        "`access.pin` is only valid when `access.mode = \"first_time\"`".to_owned(),
                    );
                }
                let password = password.ok_or_else(|| {
                    "`access.password` is required when `access.mode = \"password\"`".to_owned()
                })?;
                if password.len() < 4 {
                    return Err("`access.password` must contain at least 4 characters".to_owned());
                }
                if password.starts_with('-') || password.chars().any(char::is_control) {
                    return Err(
                        "`access.password` cannot start with `-` or contain control characters"
                            .to_owned(),
                    );
                }
                args.extend(["-pw".to_owned(), password.to_owned()]);
            }
            _ => {
                return Err(
                    "`access.mode` must be `open`, `first_time`, `every_time`, or `password`"
                        .to_owned(),
                );
            }
        }
        args.push("-clear-clients".to_owned());
        if let Some(restrict) = boolean(access, "restrict", "access.restrict")? {
            args.push("-restrict".to_owned());
            if !restrict {
                args.push("no".to_owned());
            }
        }
        for (key, flag) in [("allow_ids", "-allow"), ("block_ids", "-block")] {
            if let Some(ids) = strings(access, key, &format!("access.{key}"))? {
                for id in ids {
                    if id.trim().is_empty() {
                        return Err(format!("`access.{key}` entries must not be empty"));
                    }
                    args.extend([flag.to_owned(), id]);
                }
            }
        }
    }

    let startup_enabled = if let Some(startup) = table(root, "startup")? {
        reject_unknown(startup, &["enabled", "idle_restart_minutes"], "startup")?;
        boolean(startup, "enabled", "startup.enabled")?
    } else {
        None
    };
    let idle_restart_minutes = table(root, "startup")?.map(|startup| integer(startup, "idle_restart_minutes", "startup.idle_restart_minutes")).transpose()?.flatten().map(|value| {
        if !(0..=10080).contains(&value) { Err("`startup.idle_restart_minutes` must be from 0 to 10080; 0 disables automatic restart".to_owned()) } else { Ok(value as u32) }
    }).transpose()?;

    let notifications_enabled = boolean(root, "notifications", "notifications")?;

    if let Some(ble) = table(root, "ble")? {
        reject_unknown(ble, &["enabled", "handoff_path"], "ble")?;
        let enabled = boolean(ble, "enabled", "ble.enabled")?;
        let handoff_path = string(ble, "handoff_path", "ble.handoff_path")?;
        if handoff_path.is_some_and(|path| path.trim().is_empty()) {
            return Err("`ble.handoff_path` must not be empty".to_owned());
        }
        if let Some(enabled) = enabled {
            if enabled {
                let path = match handoff_path {
                    Some(path) => PathBuf::from(path),
                    None => default_ble_handoff_path()?,
                };
                args.extend(["-ble".to_owned(), path.to_string_lossy().into_owned()]);
            } else {
                args.extend(["-ble".to_owned(), "off".to_owned()]);
            }
        }
    }

    let log_file = if let Some(logging) = table(root, "logging")? {
        reject_unknown(logging, &["level", "file"], "logging")?;
        if let Some(level) = string(logging, "level", "logging.level")? {
            let option = match level.to_ascii_lowercase().as_str() {
                "normal" => "-nodebug",
                "debug" => "-d",
                "debug_data" => "-debug-data",
                _ => {
                    return Err(
                        "`logging.level` must be `normal`, `debug`, or `debug_data`".to_owned()
                    );
                }
            };
            args.push(option.to_owned());
        }
        string(logging, "file", "logging.file")?
            .map(|path| {
                if path.trim().is_empty() {
                    Err("`logging.file` must not be empty".to_owned())
                } else {
                    Ok(PathBuf::from(path))
                }
            })
            .transpose()?
    } else {
        None
    };

    Ok((
        args,
        UserConfig {
            startup_enabled,
            notifications_enabled,
            idle_restart_minutes,
            pairing_file,
            log_file,
        },
    ))
}

pub fn config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("UXPLAY_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| {
        "Windows APPDATA is not available; set UXPLAY_CONFIG to a config path".to_owned()
    })?;
    Ok(PathBuf::from(appdata).join("UxPlay").join("config.toml"))
}

pub fn user_log_path() -> Option<PathBuf> {
    let (config_path, _, config) = load_or_create().ok()?;
    let log_path = match config.log_file {
        Some(path) => path,
        None => PathBuf::from(std::env::var_os("APPDATA")?)
            .join("UxPlay")
            .join("logs")
            .join("uxplay.log"),
    };
    if log_path.is_absolute() {
        Some(log_path)
    } else {
        config_path.parent().map(|parent| parent.join(log_path))
    }
}

fn default_ble_handoff_path() -> Result<PathBuf, String> {
    Ok(config_path()?
        .parent()
        .ok_or_else(|| "config path has no parent directory".to_owned())?
        .join("uxplay.ble"))
}

pub fn load_or_create() -> Result<(PathBuf, Vec<String>, UserConfig), String> {
    load_or_create_at(config_path()?)
}

fn load_or_create_at(path: PathBuf) -> Result<(PathBuf, Vec<String>, UserConfig), String> {
    if !path.exists() {
        let parent = path
            .parent()
            .ok_or_else(|| "config path has no parent directory".to_owned())?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create config directory {}: {error}",
                parent.display()
            )
        })?;
        std::fs::write(&path, DEFAULT_CONFIG)
            .map_err(|error| format!("could not create config file {}: {error}", path.display()))?;
    }
    let original_text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read config file {}: {error}", path.display()))?;
    let text = add_access_and_notification_defaults(&original_text);
    let (mut args, mut config) =
        parse(&text).map_err(|error| format!("invalid config file {}: {error}", path.display()))?;
    if text != original_text {
        std::fs::write(&path, &text)
            .map_err(|error| format!("could not update config file {}: {error}", path.display()))?;
    }
    resolve_relative_config_paths(&mut args, &path);
    if let (Some(parent), Some(pairing_file)) = (path.parent(), config.pairing_file.as_mut()) {
        if !pairing_file.is_absolute() {
            *pairing_file = parent.join(&*pairing_file);
        }
    }
    Ok((path, args, config))
}

fn add_access_and_notification_defaults(text: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut updated = text.to_owned();

    if !has_root_key(&updated, "notifications") {
        let mut insertion = 0;
        for line in updated.split_inclusive('\n') {
            if line.trim_start().starts_with('[') {
                break;
            }
            insertion += line.len();
        }
        let setting = format!(
            "# Show a tray notification with a requested AirPlay PIN while the log terminal is hidden.{newline}# Clicking the notification opens the log terminal. PINs are visible in Windows notifications.{newline}notifications = true{newline}{newline}"
        );
        updated.insert_str(insertion, &setting);
    }

    if !has_table(&updated, "access") {
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push_str(newline);
        }
        updated.push_str(newline);
        updated.push_str(&format!(
            "[access]{newline}# Client access: open | first_time | every_time | password.{newline}# first_time asks for a 4-digit pairing PIN and remembers paired devices.{newline}# every_time generates a random 4-digit code for each new connection.{newline}mode = \"open\"{newline}# Optional fixed PIN for first_time only: exactly four digits, 0001..9999.{newline}# pin = \"1234\"{newline}# For password mode, set at least 4 characters (letters, numbers, or symbols).{newline}# A password cannot start with '-' or contain control characters.{newline}# The password is stored in plain text in this file; keep it private.{newline}# password = \"ChangeThisPassword\"{newline}"
        ));
    }

    if has_table(&updated, "startup") && !has_table_key(&updated, "startup", "idle_restart_minutes")
    {
        if let Some(insertion) = table_end(&updated, "startup") {
            let prefix = if insertion > 0 && !updated[..insertion].ends_with('\n') {
                newline
            } else {
                ""
            };
            updated.insert_str(insertion, &format!("{prefix}# Restart after idle minutes with no connected clients; 0 disables. Default 360 (6 hours).{newline}idle_restart_minutes = 360{newline}"));
        }
    }

    updated
}

fn has_root_key(text: &str, key: &str) -> bool {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            return false;
        }
        if line
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
        {
            return true;
        }
    }
    false
}

fn has_table(text: &str, table_name: &str) -> bool {
    text.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default().trim();
        line.strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
            .is_some_and(|name| name.split('.').next() == Some(table_name))
    })
}

fn has_table_key(text: &str, table_name: &str, key: &str) -> bool {
    let mut in_table = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_table = line
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .is_some_and(|name| name == table_name);
        } else if in_table
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            return true;
        }
    }
    false
}

fn table_end(text: &str, table_name: &str) -> Option<usize> {
    let mut in_table = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.split('#').next().unwrap_or_default().trim();
        if trimmed.starts_with('[') {
            if in_table {
                return Some(offset);
            }
            in_table = trimmed
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .is_some_and(|name| name == table_name);
        }
        offset += line.len();
    }
    in_table.then_some(text.len())
}

fn resolve_relative_config_paths(args: &mut [String], config_path: &std::path::Path) {
    let Some(parent) = config_path.parent() else {
        return;
    };
    let path_options = [
        "-ble", "-reg", "-key", "-mp4", "-ca", "-md", "-dacp", "-vdmp", "-admp",
    ];
    let mut index = 0;
    while index + 1 < args.len() {
        if path_options.contains(&args[index].as_str()) && !args[index + 1].starts_with('-') {
            if args[index] == "-ble" && args[index + 1] == "off" {
                index += 2;
                continue;
            }
            let arg_index = index + 1;
            // Dump options may place a packet limit before their optional filename.
            let path_index = if matches!(args[index].as_str(), "-vdmp" | "-admp")
                && args[arg_index].parse::<u32>().is_ok()
                && arg_index + 1 < args.len()
            {
                arg_index + 1
            } else {
                arg_index
            };
            let configured_path = PathBuf::from(&args[path_index]);
            if !configured_path.is_absolute() {
                args[path_index] = parent.join(configured_path).to_string_lossy().into_owned();
            }
        }
        index += 1;
    }
}

fn table<'a>(
    parent: &'a toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<Option<&'a toml::map::Map<String, toml::Value>>, String> {
    parent
        .get(key)
        .map(|value| {
            value
                .as_table()
                .ok_or_else(|| format!("`{key}` must be a table"))
        })
        .transpose()
}

fn reject_unknown(
    values: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    prefix: &str,
) -> Result<(), String> {
    if let Some(key) = values.keys().find(|key| !allowed.contains(&key.as_str())) {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        return Err(format!("unknown config key `{full_key}`"));
    }
    Ok(())
}

fn string<'a>(
    values: &'a toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<&'a str>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("`{full_key}` must be a string"))
        })
        .transpose()
}

fn boolean(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<bool>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("`{full_key}` must be a boolean"))
        })
        .transpose()
}

fn integer(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<i64>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_integer()
                .ok_or_else(|| format!("`{full_key}` must be an integer"))
        })
        .transpose()
}

fn number(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<f64>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|n| n as f64))
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("`{full_key}` must be a finite number"))
        })
        .transpose()
}

fn number_text(value: f64) -> String {
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn numbers(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<Vec<f64>>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("`{full_key}` must be an array"))
                .and_then(|array| {
                    array
                        .iter()
                        .map(|v| {
                            v.as_float()
                                .or_else(|| v.as_integer().map(|n| n as f64))
                                .filter(|n| n.is_finite())
                                .ok_or_else(|| {
                                    format!("all `{full_key}` entries must be finite numbers")
                                })
                        })
                        .collect()
                })
        })
        .transpose()
}

fn integers(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<Vec<i64>>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("`{full_key}` must be an array"))
                .and_then(|array| {
                    array
                        .iter()
                        .map(|v| {
                            v.as_integer()
                                .ok_or_else(|| format!("all `{full_key}` entries must be integers"))
                        })
                        .collect()
                })
        })
        .transpose()
}

fn strings(
    values: &toml::map::Map<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<Option<Vec<String>>, String> {
    values
        .get(key)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("`{full_key}` must be an array"))
                .and_then(|array| {
                    array
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| format!("all `{full_key}` entries must be strings"))
                        })
                        .collect()
                })
        })
        .transpose()
}

fn valid_resolution(value: &str) -> bool {
    let Some((width, height_rate)) = value.split_once('x') else {
        return false;
    };
    let (height, rate) = height_rate
        .split_once('@')
        .map_or((height_rate, None), |(height, rate)| (height, Some(rate)));
    let valid_dimension = |value: &str| {
        value
            .parse::<u32>()
            .is_ok_and(|number| (1..=9999).contains(&number))
    };
    valid_dimension(width)
        && valid_dimension(height)
        && rate.is_none_or(|value| {
            value
                .parse::<u16>()
                .is_ok_and(|number| (1..=255).contains(&number))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_into_existing_cli_options() {
        let (args, config) = parse(
            r#"
                [server]
                name = "Living Room TV"
                [video]
                renderer = "Vulkan"
                resolution = "3840x2160@60"
                max_fps = 60
                h265 = false
                low_latency = true
                vsync = false
                [startup]
                enabled = true
                [ble]
                enabled = true
                handoff_path = "uxplay.ble"
                [logging]
                level = "debug_data"
                file = "logs/uxplay.log"
            "#,
        )
        .unwrap();

        assert_eq!(
            args,
            [
                "-n",
                "Living Room TV",
                "-renderer",
                "Vulkan",
                "-s",
                "3840x2160@60",
                "-fps",
                "60",
                "-no-h265",
                "-lowlatency",
                "-vsync",
                "no",
                "-ble",
                "uxplay.ble",
                "-debug-data",
            ]
        );
        assert_eq!(config.startup_enabled, Some(true));
        assert_eq!(config.log_file, Some(PathBuf::from("logs/uxplay.log")));
        assert!(args.windows(2).any(|pair| pair == ["-ble", "uxplay.ble"]));
        assert_eq!(args.last().map(String::as_str), Some("-debug-data"));
    }

    #[test]
    fn reports_invalid_keys_types_and_ranges() {
        assert!(
            parse("[video]\nrenderer = 'metal'")
                .unwrap_err()
                .contains("video.renderer")
        );
        assert!(
            parse("[video]\nmax_fps = 300")
                .unwrap_err()
                .contains("video.max_fps")
        );
        assert!(
            parse("[video]\nlow_latency = 'true'")
                .unwrap_err()
                .contains("video.low_latency")
        );
        assert!(
            parse("[server]\nnaem = 'typo'")
                .unwrap_err()
                .contains("server.naem")
        );
        assert!(
            parse("[logging]\nlevel = 'verbose'")
                .unwrap_err()
                .contains("logging.level")
        );
        assert!(
            parse("[ble]\nenabled = 'yes'")
                .unwrap_err()
                .contains("ble.enabled")
        );
    }

    #[test]
    fn true_vsync_uses_the_legacy_flag_without_a_yes_argument() {
        let (args, _) = parse("[video]\nvsync = true").unwrap();
        assert_eq!(args, ["-vsync"]);
    }

    #[test]
    fn first_run_writes_a_parseable_config_template() {
        let path = std::env::temp_dir().join(format!(
            "uxplay-config-test-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        let (loaded_path, args, config) = load_or_create_at(path.clone()).unwrap();
        assert_eq!(loaded_path, path);
        assert!(path.is_file());
        assert!(args.contains(&"-renderer".to_owned()));
        assert!(!args.contains(&"-lowlatency".to_owned()));
        assert!(args.contains(&"-ble".to_owned()));
        assert!(args.contains(&"off".to_owned()));
        assert!(args.contains(&"-nodebug".to_owned()));
        assert_eq!(config.startup_enabled, Some(false));

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn relative_ble_handoff_path_uses_the_config_directory() {
        let root = std::env::temp_dir().join(format!(
            "uxplay-config-relative-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(
            &path,
            "[ble]\nenabled = true\nhandoff_path = 'runtime/uxplay.ble'\n",
        )
        .unwrap();

        let (_, args, _) = load_or_create_at(path).unwrap();
        let handoff_index = args.iter().position(|argument| argument == "-ble").unwrap() + 1;
        assert_eq!(
            PathBuf::from(&args[handoff_index]),
            root.join("runtime").join("uxplay.ble")
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
