use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const FALLBACK_LOG: &str = "RUST_ANYTHING_FALLBACK_LOG";

fn reference_program() -> std::path::PathBuf {
    Path::new(ROOT)
        .join("baseline")
        .join("original-build")
        .join(format!("uxplay{}", std::env::consts::EXE_SUFFIX))
}

fn log() -> Option<std::fs::File> {
    let path = std::env::var_os(FALLBACK_LOG)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

#[cfg(windows)]
fn reference_runtime_path() -> Option<std::ffi::OsString> {
    let cache = Path::new(ROOT)
        .join("baseline")
        .join("original-build")
        .join("CMakeCache.txt");
    let contents = std::fs::read_to_string(cache).ok()?;
    let runtime_bin = contents.lines().find_map(|line| {
        let (_, value) = line.split_once('=')?;
        let import_library = Path::new(value);
        let file_name = import_library.file_name()?.to_str()?;
        if !file_name.starts_with("lib") || !file_name.ends_with(".dll.a") {
            return None;
        }
        let prefix = import_library.parent()?.parent()?;
        let bin = prefix.join("bin");
        bin.is_dir().then_some(bin)
    })?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(runtime_bin).chain(std::env::split_paths(&inherited));
    std::env::join_paths(paths).ok()
}

/// Tells `rust-anything app replay` this build counts its fallbacks, even when it makes none.
pub fn report_fallbacks() {
    let _ = log();
}

/// Runs the original with the same arguments, stdin and environment, and passes its output and
/// exit code through. Each call is one fallback.
pub fn run(args: &[OsString]) -> ExitCode {
    run_with(args, None)
}

/// For glue that read stdin and then declines the case: runs the original with `input` as its
/// stdin. Each call is one fallback.
#[allow(dead_code)]
pub fn run_with_stdin(args: &[OsString], input: &[u8]) -> ExitCode {
    run_with(args, Some(input))
}

fn run_with(args: &[OsString], input: Option<&[u8]>) -> ExitCode {
    if let Some(mut file) = log() {
        let line: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let _ = writeln!(file, "{}", line.join(" "));
    }
    let program = reference_program();
    let mut command = Command::new(&program);
    command.args(args);
    #[cfg(windows)]
    if let Some(path) = reference_runtime_path() {
        command.env("PATH", path);
    }
    let status = match input {
        None => command.status(),
        Some(input) => command
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut pipe) = child.stdin.take() {
                    let _ = pipe.write_all(input);
                }
                child.wait()
            }),
    };
    match status {
        Ok(status) => ExitCode::from(exit_code(status)),
        Err(e) => {
            eprintln!(
                "front door: cannot run the original `{}`: {e}",
                program.display()
            );
            ExitCode::from(127)
        }
    }
}

fn exit_code(status: ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return (code & 0xff) as u8;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return (128 + signal) as u8;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::reference_program;
    use std::path::Path;

    #[test]
    fn reference_executable_path_uses_the_current_platform_suffix() {
        let path = reference_program();

        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(format!("uxplay{}", std::env::consts::EXE_SUFFIX).as_str())
        );
        assert!(path.parent().is_some_and(|parent| {
            parent.ends_with(Path::new("baseline").join("original-build"))
        }));
    }
}
