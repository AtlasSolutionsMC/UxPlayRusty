//! User-editable Windows settings layered between the legacy rc file and CLI arguments.

use std::path::PathBuf;

const DEFAULT_CONFIG: &str = include_str!("../assets/config.example.toml");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub startup_enabled: Option<bool>,
    pub log_file: Option<PathBuf>,
}

pub fn parse(text: &str) -> Result<(Vec<String>, UserConfig), String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|error| format!("invalid TOML syntax: {error}"))?;
    let root = value
        .as_table()
        .ok_or_else(|| "configuration root must be a TOML table".to_owned())?;
    reject_unknown(root, &["server", "video", "ble", "startup", "logging"], "")?;

    let mut args = Vec::new();
    if let Some(server) = table(root, "server")? {
        reject_unknown(server, &["name"], "server")?;
        if let Some(name) = string(server, "name", "server.name")? {
            if name.trim().is_empty() {
                return Err("`server.name` must not be empty".to_owned());
            }
            args.extend(["-n".to_owned(), name.to_owned()]);
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
    }

    let startup_enabled = if let Some(startup) = table(root, "startup")? {
        reject_unknown(startup, &["enabled"], "startup")?;
        boolean(startup, "enabled", "startup.enabled")?
    } else {
        None
    };

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
    let log_path = config.log_file?;
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
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read config file {}: {error}", path.display()))?;
    let (mut args, config) =
        parse(&text).map_err(|error| format!("invalid config file {}: {error}", path.display()))?;
    resolve_relative_ble_path(&mut args, &path);
    Ok((path, args, config))
}

fn resolve_relative_ble_path(args: &mut [String], config_path: &std::path::Path) {
    let Some(parent) = config_path.parent() else {
        return;
    };
    for index in 0..args.len().saturating_sub(1) {
        if args[index] == "-ble" && args[index + 1] != "off" {
            let handoff = PathBuf::from(&args[index + 1]);
            if !handoff.is_absolute() {
                args[index + 1] = parent.join(handoff).to_string_lossy().into_owned();
            }
        }
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
