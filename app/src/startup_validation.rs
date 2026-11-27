//! Startup checks whose failures belong to the command-line boundary.

use std::fs::{self, OpenOptions};
use std::path::Path;

/// Match UxPlay's writable-path probe for an option as it is parsed.
pub fn path_has_write_access(path: &str) -> bool {
    let path = Path::new(path);

    #[cfg(unix)]
    if let Some(writable) = existing_unix_path_write_access(path) {
        return writable;
    }

    #[cfg(windows)]
    if path.exists() {
        return OpenOptions::new().write(true).open(path).is_ok();
    }

    let Ok(file) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return false;
    };
    drop(file);
    let _ = fs::remove_file(path);
    true
}

#[cfg(unix)]
fn existing_unix_path_write_access(path: &Path) -> Option<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid, NUL-terminated path and remains alive for both calls.
    let exists = unsafe { libc::access(path.as_ptr(), libc::F_OK) } == 0;
    if !exists {
        return None;
    }
    // SAFETY: same valid path; W_OK is the source's access mode for existing files.
    Some(unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "uxplay-startup-validation-{}-{label}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn missing_path_probe_creates_then_removes_the_file() {
        let path = temporary_path("missing");
        assert!(!path.exists());

        assert!(path_has_write_access(path.to_str().unwrap()));
        assert!(!path.exists());
    }

    #[test]
    fn existing_path_probe_preserves_contents() {
        let path = temporary_path("existing");
        fs::write(&path, b"keep these bytes").unwrap();

        assert!(path_has_write_access(path.to_str().unwrap()));
        assert_eq!(fs::read(&path).unwrap(), b"keep these bytes");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_parent_probe_fails_without_creating_a_file() {
        let path = temporary_path("absent-parent").join("output.bin");
        assert!(!path_has_write_access(&path.to_string_lossy()));
        assert!(!path.exists());
    }
}
