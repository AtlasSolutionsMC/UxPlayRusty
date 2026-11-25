//! Client-key registration list used by PIN pairing, from `uxplay.cpp` callbacks.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
struct RegistryState {
    public_keys: HashSet<String>,
}

pub struct PairingRegistry {
    registration_list: bool,
    require_registered_clients: bool,
    path: Option<PathBuf>,
    state: Mutex<RegistryState>,
}

impl PairingRegistry {
    pub fn load(
        registration_list: bool,
        require_registered_clients: bool,
        path: Option<PathBuf>,
    ) -> Self {
        let mut state = RegistryState::default();
        if registration_list
            && require_registered_clients
            && let Some(path) = path.as_deref()
            && let Ok(file) = fs::File::open(path)
        {
            for line in io::BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                if line.len() >= 44
                    && let Ok(key) = std::str::from_utf8(&line.as_bytes()[..44])
                {
                    state.public_keys.insert(key.to_owned());
                }
            }
        }
        Self {
            registration_list,
            require_registered_clients: registration_list && require_registered_clients,
            path,
            state: Mutex::new(state),
        }
    }

    pub fn contains(&self, public_key_base64: &str) -> bool {
        if !self.require_registered_clients {
            return true;
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .public_keys
            .contains(public_key_base64)
    }

    pub fn register(&self, public_key_base64: &str, device_id: &str, client_name: &str) {
        if !self.registration_list {
            return;
        }
        let public_key_base64 = c_string_prefix(public_key_base64);
        let device_id = c_string_prefix(device_id);
        let client_name: String = c_string_prefix(client_name)
            .chars()
            .filter(|character| !matches!(character, '\r' | '\n'))
            .collect();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .public_keys
            .insert(public_key_base64.to_owned());
        if let Some(path) = self.path.as_deref() {
            append_registration(path, public_key_base64, device_id, &client_name);
        }
    }
}

fn append_registration(path: &Path, public_key: &str, device_id: &str, client_name: &str) {
    if let Ok(mut file) = open_registration_append(path) {
        let newline = if cfg!(windows) { "\r\n" } else { "\n" };
        let _ = write!(
            file,
            "{public_key},{},{device_id},{client_name}{newline}",
            local_timestamp()
        );
    }
}

#[cfg(windows)]
fn open_registration_append(path: &Path) -> io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorA, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, OPEN_ALWAYS,
    };

    // Match the source's protected DACL: stop inheriting permissions and grant
    // full access only to the file owner when the registration file is created.
    let descriptor_text = b"D:P(A;;GA;;;OW)\0";
    let mut descriptor = null_mut();
    // SAFETY: descriptor_text is NUL-terminated and descriptor points to writable storage.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorA(
            descriptor_text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut path_wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    path_wide.push(0);
    // SAFETY: path_wide and security remain valid for the call. OPEN_ALWAYS
    // applies the protected descriptor only when creating the file, as in C.
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            FILE_APPEND_DATA,
            0,
            &security,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    // SAFETY: the descriptor came from the Windows LocalAlloc family API.
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: the successful CreateFileW handle is transferred to File exactly once.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(unix)]
fn open_registration_append(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.create(true).append(true).mode(0o600);
    options.open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_registration_append(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;

    OpenOptions::new().create(true).append(true).open(path)
}

fn c_string_prefix(value: &str) -> &str {
    value.split_once('\0').map_or(value, |(prefix, _)| prefix)
}

#[cfg(unix)]
fn local_timestamp() -> String {
    // SAFETY: a null pointer requests the current epoch seconds without writing through it.
    let mut epoch = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `epoch` and the writable `tm` storage are valid for `localtime_r`.
    if unsafe { libc::localtime_r(&mut epoch, local.as_mut_ptr()) }.is_null() {
        return String::new();
    }
    // SAFETY: localtime_r returned a non-null pointer and initialized the tm value.
    let local = unsafe { local.assume_init() };
    let format = b"%Y-%m-%d %H:%M:%S\0";
    let mut output = [0_i8; 32];
    // SAFETY: the format is NUL-terminated and the output buffer is writable.
    let length = unsafe {
        libc::strftime(
            output.as_mut_ptr(),
            output.len(),
            format.as_ptr().cast(),
            &local,
        )
    };
    // SAFETY: strftime wrote `length` initialized bytes to the fixed output buffer.
    let bytes = unsafe { std::slice::from_raw_parts(output.as_ptr().cast::<u8>(), length) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
fn local_timestamp() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut local = SYSTEMTIME::default();
    // SAFETY: GetLocalTime initializes the writable SYSTEMTIME structure.
    unsafe { GetLocalTime(&mut local) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

#[cfg(not(any(unix, windows)))]
fn local_timestamp() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "uxplay-pair-registry-{}-{nonce}.txt",
            std::process::id()
        ))
    }

    #[test]
    fn disabled_registry_allows_clients_without_storing_keys() {
        let registry = PairingRegistry::load(false, false, None);
        assert!(registry.contains("unknown"));
        registry.register("key", "device", "name");
        assert!(registry.contains("another-unknown"));
    }

    #[test]
    fn registration_list_persists_clients_without_pin_mode_enforcement() {
        let path = temp_file();
        let registry = PairingRegistry::load(true, false, Some(path.clone()));
        let key = "Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
        registry.register(key, "device-id", "Apple TV");

        assert!(registry.contains("unregistered-client"));
        let contents = fs::read(&path).unwrap();
        assert!(contents.starts_with(format!("{key},").as_bytes()));
        let newline: &[u8] = if cfg!(windows) { b"\r\n" } else { b"\n" };
        assert!(contents.ends_with(newline));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn loads_the_first_44_characters_of_each_registration_line() {
        let path = temp_file();
        let key = "Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
        assert_eq!(key.len(), 44);
        fs::write(
            &path,
            format!("{key},2026-11-22 12:00:00,device,name\nshort\n"),
        )
        .unwrap();
        let registry = PairingRegistry::load(true, true, Some(path.clone()));
        assert!(registry.contains(key));
        assert!(!registry.contains("missing"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn newly_registered_clients_are_admitted_and_appended_in_source_format() {
        let path = temp_file();
        let registry = PairingRegistry::load(true, true, Some(path.clone()));
        let key = "Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
        registry.register(key, "device-id", "Apple TV");
        assert!(registry.contains(key));
        let contents = fs::read_to_string(&path).unwrap();
        let line = contents.trim_end();
        assert!(line.starts_with(&format!("{key},")));
        assert!(line.ends_with(",device-id,Apple TV"));
        assert_eq!(line.matches(',').count(), 3);
        let reloaded = PairingRegistry::load(true, true, Some(path.clone()));
        assert!(reloaded.contains(key));
        assert!(!reloaded.contains("unregistered"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn registration_client_name_cannot_inject_extra_lines() {
        let path = temp_file();
        let registry = PairingRegistry::load(true, false, Some(path.clone()));
        let key = "Abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
        registry.register(key, "device-id", "Phone\r\nforged,entry");

        let contents = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = contents.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with(",device-id,Phoneforged,entry"));
        fs::remove_file(path).unwrap();
    }
}
