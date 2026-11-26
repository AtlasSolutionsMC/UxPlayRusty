//! Port of `wsa_strerror` from `lib/compat.c`.
//!
//! Copyright (c) 2024 F. Duncanh. Derived from the UxPlay LGPL-2.1-or-later
//! source; preserve the license terms when redistributing this crate.

use std::sync::Mutex;
use windows_sys::Win32::System::Diagnostics::Debug::{
    FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageA,
};

const MESSAGE_CAPACITY: usize = 256;
const FORMAT_FLAGS: u32 = FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;

static MESSAGE: Mutex<[u8; MESSAGE_CAPACITY]> = Mutex::new([0; MESSAGE_CAPACITY]);

/// Return the Windows socket error text as bytes, matching the source's ANSI
/// `FormatMessage` buffer and single-line truncation. The shared buffer keeps
/// the original's retained contents when Windows cannot resolve an error ID.
pub fn wsa_strerror(errnum: i32) -> Vec<u8> {
    let mut message = MESSAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: `message` points to a writable 256-byte buffer and the API is
    // given that exact capacity. Null source/arguments select system text and
    // FORMAT_MESSAGE_IGNORE_INSERTS means no variadic argument buffer is used.
    unsafe {
        FormatMessageA(
            FORMAT_FLAGS,
            std::ptr::null(),
            errnum as u32,
            0,
            message.as_mut_ptr(),
            MESSAGE_CAPACITY as u32,
            std::ptr::null(),
        );
    }

    let nul = message
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(message.len());
    if let Some(newline) = message[..nul].iter().position(|byte| *byte == b'\n') {
        message[newline] = 0;
    }
    let end = message
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(message.len());
    message[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_checked_in_c_oracle() {
        for row in include_str!("../../../../tools/record-shims/compat_oracle.expected.txt").lines()
        {
            let (code, expected_hex) = row.split_once('\t').unwrap();
            let (pairs, remainder) = expected_hex.as_bytes().as_chunks::<2>();
            assert!(remainder.is_empty());
            let expected = pairs
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                wsa_strerror(code.parse().unwrap()),
                expected,
                "error {code}"
            );
        }
    }
}
