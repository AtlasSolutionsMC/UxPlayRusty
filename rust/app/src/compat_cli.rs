//! Source-compatible CLI paths implemented in Rust ahead of server startup.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = include_str!("../assets/help-short.txt");
const CUSTOM_HELP: &str = include_str!("../assets/custom-profile-help.txt");
const RPI_HELP: &str = include_str!("../assets/rpi-model-help.txt");
const VERSION: &str = "1.74";

/// Handles `-rc` paths that run before the C parser validates argv encodings.
pub fn run_explicit_rc_compat(argv: &[OsString]) -> Option<ExitCode> {
    let binary = argv
        .first()
        .map_or_else(|| OsStr::new("uxplay"), OsString::as_os_str);
    match explicit_rc(argv) {
        RcSelection::Error { text, code } => {
            write_stdout(&text);
            Some(ExitCode::from(code))
        }
        RcSelection::NotFound(path) => {
            write_stdout_bytes(&rc_not_found_message(&path));
            Some(ExitCode::SUCCESS)
        }
        RcSelection::File(path) => action_from_rc(binary, &path),
        RcSelection::Default => None,
    }
}

/// Handles stable early-exit paths. `None` leaves the complete invocation to the original.
pub fn run_compat(argv: &[OsString]) -> Option<ExitCode> {
    let binary = argv
        .first()
        .map_or_else(|| OsStr::new("uxplay"), OsString::as_os_str);
    let args: Vec<String> = argv
        .iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    if let Some(code) = run_explicit_rc_compat(argv) {
        return Some(code);
    }
    if !args.iter().any(|arg| arg == "-rc")
        && let Some(path) = default_rc()
    {
        if let Some(action) = action_from_rc(binary, &path) {
            return Some(action);
        }
        return None;
    }

    if args.len() == 1 {
        match args[0].as_str() {
            "-h" | "--help" | "-?" | "-help" => {
                let path = binary.to_string_lossy();
                let text = HELP.replace("@@UXPLAY_BIN@@", &path);
                write_stdout(&text);
                return Some(ExitCode::SUCCESS);
            }
            "-v" => {
                write_stdout(&format!(
                    "UxPlay version {VERSION}; for help, use option \"-h\"\n"
                ));
                return Some(ExitCode::SUCCESS);
            }
            "-rc" => {
                write_stdout("*** ERROR: option -rc requires a filename  (-rc <filename>)\n");
                return Some(ExitCode::from(1));
            }
            "-custom" => {
                write_stderr(CUSTOM_HELP);
                return Some(ExitCode::SUCCESS);
            }
            "-rpi" => {
                write_stderr(RPI_HELP);
                return Some(ExitCode::SUCCESS);
            }
            "-p2p" if !apple_p2p_available() => {
                write_stderr(
                    "invalid: the \"-p2p\" option is only available on macOS hosts with UxPlay compiled to use Bonjour services\n",
                );
                return Some(ExitCode::from(1));
            }
            arg if arg.starts_with('-') && !known_flag(arg) => {
                write_stderr(&format!(
                    "unknown option {arg}, stopping (for help use option \"-h\")\n"
                ));
                return Some(ExitCode::from(1));
            }
            _ => {}
        }
    }

    if args.len() == 2 {
        match (args[0].as_str(), args[1].as_str()) {
            ("-s", value) if !valid_resolution(value) => {
                write_stderr(&format!(
                    "invalid \"-s {value}\"; -s wxh : max w,h=9999; -s wxh@r : max r=255\n"
                ));
                return Some(ExitCode::from(1));
            }
            ("-fps", value) if !valid_unsigned(value, 255) => {
                write_stderr(&format!(
                    "invalid \"-fps {value}\"; -fps n : max n=255, default n=30\n"
                ));
                return Some(ExitCode::from(1));
            }
            ("-p2p", _) if !apple_p2p_available() => {
                write_stderr(
                    "invalid: the \"-p2p\" option is only available on macOS hosts with UxPlay compiled to use Bonjour services\n",
                );
                return Some(ExitCode::from(1));
            }
            _ => {}
        }
    }

    None
}

enum RcSelection {
    Default,
    File(PathBuf),
    NotFound(PathBuf),
    Error { text: String, code: u8 },
}

fn explicit_rc(argv: &[OsString]) -> RcSelection {
    for (index, arg) in argv.iter().enumerate().skip(1) {
        if arg != OsStr::new("-rc") {
            continue;
        }
        let Some(filename) = argv.get(index + 1) else {
            return RcSelection::Error {
                text: "*** ERROR: option -rc requires a filename  (-rc <filename>)\n".into(),
                code: 1,
            };
        };
        let path = PathBuf::from(filename);
        if !path.exists() {
            return RcSelection::NotFound(path);
        }
        return RcSelection::File(path);
    }
    RcSelection::Default
}

fn rc_not_found_message(path: &Path) -> Vec<u8> {
    let mut message = b"*** ERROR: startup file ".to_vec();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        message.extend_from_slice(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    message.extend_from_slice(&crate::compat::c_runtime_output(&path.to_string_lossy()));
    #[cfg(not(any(unix, windows)))]
    message.extend_from_slice(path.to_string_lossy().as_bytes());
    #[cfg(windows)]
    message.extend_from_slice(b" specified by option -rc was not found\r\n");
    #[cfg(not(windows))]
    message.extend_from_slice(b" specified by option -rc was not found\n");
    message
}

fn default_rc() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("UXPLAYRC").map(PathBuf::from)
        && path.exists()
    {
        return Some(path);
    }
    let home = std::env::var_os("XDG_CONFIG_HOMEDIR")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let legacy = home.join(".uxplayrc");
    if legacy.exists() {
        return Some(legacy);
    }
    let modern = home.join(".config").join("uxplayrc");
    modern.exists().then_some(modern)
}

fn action_from_rc(binary: &OsStr, path: &Path) -> Option<ExitCode> {
    let text = std::fs::read_to_string(path).ok()?;
    let options = rc_options(&text);
    let mut stdout = format!("UxPlay: reading configuration from  {}\n", path.display());
    for option in &options {
        if option == "-v" {
            stdout.push_str(&format!(
                "UxPlay version {VERSION}; for help, use option \"-h\"\n"
            ));
            write_stdout(&stdout);
            return Some(ExitCode::SUCCESS);
        }
        if matches!(option.as_str(), "-h" | "--help" | "-?" | "-help") {
            let help = HELP.replace("@@UXPLAY_BIN@@", &binary.to_string_lossy());
            stdout.push_str(&help);
            write_stdout(&stdout);
            return Some(ExitCode::SUCCESS);
        }
        if option.starts_with('-') && !known_flag(option) {
            write_stdout(&stdout);
            write_stderr(&format!(
                "unknown option {option}, stopping (for help use option \"-h\")\n"
            ));
            return Some(ExitCode::from(1));
        }
    }
    None
}

fn rc_options(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let first = fields.next()?;
            let mut options = vec![format!("-{first}")];
            options.extend(fields.map(str::to_owned));
            Some(options)
        })
        .flatten()
        .collect()
}

fn known_flag(arg: &str) -> bool {
    matches!(
        arg,
        "-allow"
            | "-block"
            | "-restrict"
            | "-n"
            | "-nh"
            | "-h265"
            | "-mp4"
            | "-hls"
            | "-lang"
            | "-slang"
            | "-custom"
            | "-rpi"
            | "-scrsv"
            | "-pin"
            | "-p2p"
            | "-reg"
            | "-pw"
            | "-s"
            | "-fps"
            | "-o"
            | "-f"
            | "-r"
            | "-p"
            | "-m"
            | "-a"
            | "-d"
            | "-vp"
            | "-vd"
            | "-vc"
            | "-vs"
            | "-as"
            | "-t"
            | "-nc"
            | "-avdec"
            | "-v4l2"
            | "-fs"
            | "-FPSdata"
            | "-reset"
            | "-vrtp"
            | "-artp"
            | "-vdmp"
            | "-admp"
            | "-bt709"
            | "-srgb"
            | "-nohold"
            | "-al"
            | "-key"
            | "-dacp"
            | "-taper"
            | "-db"
            | "-vol"
            | "-async"
            | "-vsync"
            | "-nofreeze"
            | "-ble"
            | "-rc"
            | "-h"
            | "--help"
            | "-?"
            | "-help"
            | "-v"
    )
}

fn valid_unsigned(value: &str, max: u32) -> bool {
    c_unsigned(value, 10, max, true)
}

fn valid_resolution(value: &str) -> bool {
    let (resolution, refresh) = value
        .split_once('@')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let Some((width, height)) = resolution.split_once('x') else {
        return false;
    };
    if !c_unsigned(width, 4, 9999, true) || !c_unsigned(height, 4, 9999, true) {
        return false;
    }
    refresh.is_none_or(|rate| c_unsigned(rate, 3, 255, true))
}

fn c_unsigned(value: &str, max_len: usize, max: u32, positive: bool) -> bool {
    if value.is_empty() || value.len() > max_len || value.starts_with('-') {
        return false;
    }
    let value = value.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
    let value = value.strip_prefix('+').unwrap_or(value);
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    value
        .parse::<u32>()
        .is_ok_and(|number| number <= max && (!positive || number > 0))
}

fn apple_p2p_available() -> bool {
    cfg!(all(target_os = "macos", feature = "apple-p2p"))
}

fn write_stdout(text: &str) {
    write_stream(io::stdout(), text);
}

fn write_stdout_bytes(bytes: &[u8]) {
    let _ = io::stdout().write_all(bytes);
}

fn write_stderr(text: &str) {
    write_stream(io::stderr(), text);
}

fn write_stream(mut stream: impl Write, text: &str) {
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");
    #[cfg(not(windows))]
    let text = text.to_owned();
    let _ = stream.write_all(&crate::compat::c_runtime_output(&text));
}

#[cfg(test)]
mod tests {
    use super::{
        RcSelection, c_unsigned, explicit_rc, rc_not_found_message, rc_options, valid_resolution,
        valid_unsigned,
    };
    use std::ffi::OsString;

    #[test]
    fn display_settings_follow_the_source_shape_and_ranges() {
        for input in ["1920x1080", "1x1", " 1x 2", "1920x1080@60", "+1x2@255"] {
            assert!(valid_resolution(input), "expected valid display {input:?}");
        }
        for input in ["bad", "1920X1080", "0x1", "10000x1", "1x1@0", "1x1@256"] {
            assert!(
                !valid_resolution(input),
                "expected invalid display {input:?}"
            );
        }
    }

    #[test]
    fn unsigned_options_require_positive_decimal_values_in_range() {
        assert!(valid_unsigned("30", 255));
        assert!(valid_unsigned("+30", 255));
        assert!(!valid_unsigned("0", 255));
        assert!(!valid_unsigned("256", 255));
        assert!(!valid_unsigned("30x", 255));
        assert!(!c_unsigned("-1", 255, 255, true));
    }

    #[test]
    fn rc_file_first_token_gets_its_option_prefix() {
        assert_eq!(
            rc_options("# comment\nv\nn Screen\n"),
            ["-v", "-n", "Screen"]
        );
    }

    #[test]
    fn explicit_rc_checks_the_native_path_and_formats_missing_path() {
        let directory = std::env::temp_dir().join(format!(
            "uxplay rc {} {}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("startup config.rc");
        std::fs::write(&path, b"-n receiver\n").unwrap();
        let argv = [
            OsString::from("uxplay"),
            OsString::from("-rc"),
            path.as_os_str().to_owned(),
        ];
        assert!(matches!(explicit_rc(&argv), RcSelection::File(found) if found == path));

        let missing = directory.join("missing config.rc");
        let missing_argv = [
            OsString::from("uxplay"),
            OsString::from("-rc"),
            missing.as_os_str().to_owned(),
        ];
        assert!(
            matches!(explicit_rc(&missing_argv), RcSelection::NotFound(found) if found == missing)
        );
        let message = rc_not_found_message(&missing);
        assert!(String::from_utf8_lossy(&message).contains("missing config.rc"));
        #[cfg(windows)]
        assert!(message.ends_with(b"specified by option -rc was not found\r\n"));
        #[cfg(not(windows))]
        assert!(message.ends_with(b"specified by option -rc was not found\n"));

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn explicit_rc_keeps_unix_filename_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let directory = std::env::temp_dir().join(format!(
            "uxplay-rc-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let filename = OsString::from_vec("config-λ.rc".as_bytes().to_vec());
        let path = directory.join(&filename);
        std::fs::write(&path, b"-n receiver\n").unwrap();

        let argv = [
            OsString::from("uxplay"),
            OsString::from("-rc"),
            path.as_os_str().to_owned(),
        ];
        assert!(matches!(explicit_rc(&argv), RcSelection::File(found) if found == path));

        let missing_name = "missing-雪.rc";
        let missing = directory.join(OsString::from_vec(missing_name.as_bytes().to_vec()));
        assert!(matches!(
            explicit_rc(&[
                OsString::from("uxplay"),
                OsString::from("-rc"),
                missing.as_os_str().to_owned(),
            ]),
            RcSelection::NotFound(found) if found == missing
        ));
        let message = rc_not_found_message(&missing);
        assert!(
            message
                .windows(missing_name.len())
                .any(|window| window == missing_name.as_bytes())
        );
        assert!(message.ends_with(b" specified by option -rc was not found\n"));

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
