//! Platform error text compatibility for `lib/compat.c` and `lib/sockets.h`.

use std::io;

#[cfg(windows)]
fn c_runtime_argument_bytes(argument: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CP_ACP, WideCharToMultiByte};

    let wide: Vec<u16> = argument.encode_wide().collect();
    if wide.is_empty() {
        return Vec::new();
    }
    let wide_len = i32::try_from(wide.len()).expect("Windows argv length fits i32");
    // SAFETY: `wide` is valid UTF-16 and the zero-length call requests output size only.
    let byte_len = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide_len,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    assert!(byte_len > 0, "Windows ACP conversion must size valid argv");
    let mut bytes = vec![0u8; byte_len as usize];
    // SAFETY: pointers reference buffers with the exact lengths supplied.
    let written = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide_len,
            bytes.as_mut_ptr(),
            byte_len,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    assert!(written > 0, "Windows ACP conversion must encode valid argv");
    bytes.truncate(written as usize);
    bytes
}

#[cfg(unix)]
fn c_runtime_argument_bytes(argument: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    argument.as_bytes().to_vec()
}

#[cfg(not(any(windows, unix)))]
fn c_runtime_argument_bytes(argument: &std::ffi::OsStr) -> Vec<u8> {
    argument.to_string_lossy().as_bytes().to_vec()
}

/// Find the first argv element rejected by UxPlay's C `is_utf8` pre-parse check.
pub fn c_runtime_argv_utf8_error(args: &[std::ffi::OsString]) -> Option<Vec<u8>> {
    let suffix = b"\"while parsing input arguments";
    for argument in args.iter().skip(1) {
        let bytes = c_runtime_argument_bytes(argument);
        if std::str::from_utf8(&bytes).is_err() {
            let mut error = b"Error: detected a non-ascii or non-UTF-8 string \"".to_vec();
            error.extend_from_slice(&bytes);
            error.extend_from_slice(suffix);
            return Some(error);
        }
    }
    None
}

/// Convert native process arguments to the narrow strings seen by the C runtime.
/// Windows C `main` receives argv encoded using the active ANSI code page.
pub fn c_runtime_arguments(args: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};

        args.iter()
            .map(|argument| {
                let bytes = c_runtime_argument_bytes(argument);
                if bytes.is_empty() {
                    return std::ffi::OsString::new();
                }
                let byte_len = i32::try_from(bytes.len()).expect("Windows argv length fits i32");
                // SAFETY: `bytes` contains the active code-page representation produced above;
                // the zero-length call only asks Windows for the decoded UTF-16 length.
                let wide_len = unsafe {
                    MultiByteToWideChar(
                        CP_ACP,
                        0,
                        bytes.as_ptr(),
                        byte_len,
                        std::ptr::null_mut(),
                        0,
                    )
                };
                assert!(wide_len > 0, "Windows ACP output must decode to UTF-16");
                let mut decoded = vec![0u16; wide_len as usize];
                // SAFETY: pointers reference buffers with the exact lengths supplied.
                let written = unsafe {
                    MultiByteToWideChar(
                        CP_ACP,
                        0,
                        bytes.as_ptr(),
                        byte_len,
                        decoded.as_mut_ptr(),
                        wide_len,
                    )
                };
                assert!(written > 0, "Windows ACP output must decode fully");
                decoded.truncate(written as usize);
                std::ffi::OsString::from_wide(&decoded)
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        args.to_vec()
    }
}

/// Encode Rust-generated startup text the same way narrow C stdio writes its bytes.
pub fn c_runtime_output(text: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return Vec::new();
        }
        let wide_len = i32::try_from(wide.len()).expect("console text length fits i32");
        // SAFETY: `wide` is valid UTF-16 and the zero-length call requests output size only.
        let byte_len = unsafe {
            WideCharToMultiByte(
                CP_ACP,
                0,
                wide.as_ptr(),
                wide_len,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        assert!(byte_len > 0, "Windows ACP conversion must size valid text");
        let mut bytes = vec![0u8; byte_len as usize];
        // SAFETY: pointers reference buffers with the exact lengths supplied.
        let written = unsafe {
            WideCharToMultiByte(
                CP_ACP,
                0,
                wide.as_ptr(),
                wide_len,
                bytes.as_mut_ptr(),
                byte_len,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        assert!(written > 0, "Windows ACP conversion must encode valid text");
        bytes.truncate(written as usize);
        bytes
    }
    #[cfg(not(windows))]
    {
        text.as_bytes().to_vec()
    }
}

/// Return the system socket error text used by the C `SOCKET_ERROR_STRING` macro.
pub fn socket_error_message(error_code: i32) -> String {
    #[cfg(windows)]
    {
        String::from_utf8_lossy(&compat_rs::port::wsa_strerror(error_code)).into_owned()
    }
    #[cfg(not(windows))]
    {
        io::Error::from_raw_os_error(error_code).to_string()
    }
}

/// Format an OS error as the source's numeric code followed by its socket text.
pub fn socket_error_description(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{code}: {}", socket_error_message(code)),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_socket_message_matches_the_source_buffer_contract() {
        let message = socket_error_message(10061);
        assert!(!message.is_empty());
        assert!(message.len() <= 255);
        assert!(!message.contains('\n'));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_socket_message_uses_the_system_errno_text() {
        assert_eq!(
            socket_error_message(2),
            io::Error::from_raw_os_error(2).to_string()
        );
    }

    #[test]
    fn errors_without_an_os_code_keep_their_display_text() {
        let error = io::Error::other("renderer failed");
        assert_eq!(socket_error_description(&error), "renderer failed");
    }
}
