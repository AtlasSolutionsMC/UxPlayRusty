//! Narrow C ABI adapters. All data transformations live in safe `port` functions.

use std::ffi::{CStr, OsString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::ptr;

// SAFETY: `malloc` follows the C ABI and allocates memory that C callers release with `free`.
unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

#[cfg(windows)]
// SAFETY: this declaration matches the Windows system ABI for the documented conversion API.
unsafe extern "system" {
    // SAFETY: this is the Windows API used by the UCRT to decode narrow `fopen` paths.
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        input: *const c_char,
        input_len: c_int,
        output: *mut u16,
        output_len: c_int,
    ) -> c_int;
}

macro_rules! export_pair {
    ($name:ident, $shadow_name:ident, ($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty, $implementation:ident) => {
        #[cfg(not(feature = "shadow"))]
        // SAFETY: exports the replacement under the expected C symbol.
        #[unsafe(no_mangle)]
        // SAFETY: the function preserves the C signature; each implementation validates pointers
        // and lengths before dereferencing, and requires readable/writable regions as documented.
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            // SAFETY: the caller supplies the pointer arguments according to this C ABI.
            unsafe { $implementation($($arg),*) }
        }

        // SAFETY: exports a unique shadow alias with the same validated pointer contract.
        #[unsafe(no_mangle)]
        // SAFETY: this is the shadow-only alias for the same C ABI and has the same pointer contract.
        pub unsafe extern "C" fn $shadow_name($($arg: $ty),*) -> $ret {
            // SAFETY: the caller supplies the pointer arguments according to this C ABI.
            unsafe { $implementation($($arg),*) }
        }
    };
}

fn allocate(bytes: &[u8], terminate: bool) -> *mut c_char {
    let Some(required) = bytes.len().checked_add(usize::from(terminate)) else {
        return ptr::null_mut();
    };
    let required = required.max(1);
    // SAFETY: `malloc` accepts any size; its result is checked before use.
    let output = unsafe { malloc(required) }.cast::<u8>();
    if output.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `output` points to `required` writable bytes, `bytes` is valid for its length, and
    // the C allocation cannot overlap this Rust slice.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
        if terminate {
            output.add(bytes.len()).write(0);
        }
    }
    output.cast()
}

fn c_string_copy(input: *const c_char) -> Option<Vec<u8>> {
    if input.is_null() {
        return None;
    }
    // SAFETY: callers of this helper must pass a valid NUL-terminated C string.
    Some(unsafe { CStr::from_ptr(input) }.to_bytes().to_vec())
}

#[cfg(unix)]
fn path_from_c_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(windows)]
fn path_from_c_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    if bytes.is_empty() {
        return Some(PathBuf::new());
    }
    let input_len = c_int::try_from(bytes.len()).ok()?;
    // SAFETY: valid input; a null output with length zero queries the required UTF-16 length.
    let wide_len =
        // SAFETY: valid input; a null output with length zero queries the required UTF-16 length.
        unsafe { MultiByteToWideChar(0, 0, bytes.as_ptr().cast(), input_len, ptr::null_mut(), 0) };
    if wide_len <= 0 {
        return None;
    }
    let mut wide = vec![0_u16; wide_len as usize];
    // SAFETY: `wide` has the requested capacity and the input remains valid for this call.
    let written = unsafe {
        MultiByteToWideChar(
            0,
            0,
            bytes.as_ptr().cast(),
            input_len,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    (written == wide_len).then(|| PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(not(any(unix, windows)))]
fn path_from_c_bytes(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(std::str::from_utf8(bytes).ok()?))
}

// SAFETY: caller must provide valid C strings and a writable pointer-to-pointer as required by the ABI.
unsafe fn strsep_impl(stringp: *mut *mut c_char, delimiter: *const c_char) -> *mut c_char {
    if stringp.is_null() || delimiter.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `stringp` is non-null and points to the caller's writable char pointer.
    let current = unsafe { *stringp };
    if current.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both values are NUL-terminated strings supplied by the caller; copy delimiter bytes
    // before creating a mutable view so even overlapping input/delimiter pointers remain sound.
    // SAFETY: `current` is a caller-provided NUL-terminated string.
    let input_len = unsafe { CStr::from_ptr(current) }.to_bytes().len();
    // SAFETY: `delimiter` is a caller-provided NUL-terminated string.
    let delimiter = unsafe { CStr::from_ptr(delimiter) }.to_bytes().to_vec();
    // SAFETY: `current` is a writable NUL-terminated buffer of at least input_len + 1 bytes.
    let buffer = unsafe { std::slice::from_raw_parts_mut(current.cast::<u8>(), input_len) };
    let mut cursor = Some(0);
    let Some(range) = crate::port::strsep_in_place(buffer, &mut cursor, &delimiter) else {
        return ptr::null_mut();
    };
    let next = cursor.map_or(ptr::null_mut(), |offset| {
        // SAFETY: offset is at most input_len, pointing into the current C string or its NUL.
        unsafe { current.add(offset) }
    });
    // SAFETY: stringp was validated as a non-null writable pointer above.
    unsafe { *stringp = next };
    // SAFETY: range.start is at most input_len and therefore points into the C string or its NUL.
    unsafe { current.add(range.start) }
}

// SAFETY: caller must provide a valid C filename and writable output pointer.
unsafe fn read_file_impl(dst: *mut *mut c_char, filename: *const c_char) -> c_int {
    if dst.is_null() {
        return -1;
    }
    let Some(filename) = c_string_copy(filename) else {
        return -1;
    };
    let Some(path) = path_from_c_bytes(&filename) else {
        return -1;
    };
    let bytes = match crate::port::read_file(&path) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    let output = allocate(&bytes, true);
    if output.is_null() {
        return -2;
    }
    // SAFETY: `dst` was checked non-null and the caller provides a writable pointer-to-pointer.
    unsafe { *dst = output };
    c_int::try_from(bytes.len()).unwrap_or(-2)
}

// SAFETY: caller must provide buffers of the declared lengths.
unsafe fn hwaddr_raop_impl(
    output: *mut c_char,
    output_len: c_int,
    address: *const c_char,
    address_len: c_int,
) -> c_int {
    if output.is_null()
        || output_len <= 0
        || address_len < 0
        || (address_len > 0 && address.is_null())
    {
        return -1;
    }
    // SAFETY: output is writable for output_len bytes; address is readable for address_len bytes.
    let output =
        // SAFETY: output is writable for output_len bytes.
        unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), output_len as usize) };
    // SAFETY: address_len is non-negative and a null address was rejected unless length is zero.
    let address = if address_len == 0 {
        &[]
    } else {
        // SAFETY: address_len is non-negative and a null address was rejected unless length is zero.
        unsafe { std::slice::from_raw_parts(address.cast::<u8>(), address_len as usize) }
    };
    crate::port::hwaddr_raop_into(output, address)
}

// SAFETY: caller must provide buffers of the declared lengths.
unsafe fn hwaddr_airplay_impl(
    output: *mut c_char,
    output_len: c_int,
    address: *const c_char,
    address_len: c_int,
) -> c_int {
    if output.is_null()
        || output_len <= 0
        || address_len < 0
        || (address_len > 0 && address.is_null())
    {
        return -1;
    }
    // SAFETY: output is writable for output_len bytes; address is readable for address_len bytes.
    let output =
        // SAFETY: output is writable for output_len bytes.
        unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), output_len as usize) };
    // SAFETY: address_len is non-negative and a null address was rejected unless length is zero.
    let address = if address_len == 0 {
        &[]
    } else {
        // SAFETY: address_len is non-negative and a null address was rejected unless length is zero.
        unsafe { std::slice::from_raw_parts(address.cast::<u8>(), address_len as usize) }
    };
    crate::port::hwaddr_airplay_into(output, address)
}

// SAFETY: caller must provide an input buffer and writable length pointer per declared lengths.
unsafe fn parse_hex_impl(
    input: *const c_char,
    input_len: c_int,
    output_len: *mut c_int,
) -> *mut c_char {
    if input_len < 0 || output_len.is_null() || (input_len > 0 && input.is_null()) {
        return ptr::null_mut();
    }
    // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
    let input = if input_len == 0 {
        &[]
    } else {
        // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
        unsafe { std::slice::from_raw_parts(input.cast::<u8>(), input_len as usize) }
    };
    let Some(bytes) = crate::port::parse_hex(input) else {
        return ptr::null_mut();
    };
    let output = allocate(&bytes, false);
    if output.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: output_len was checked non-null and the caller provides a writable int pointer.
    unsafe { *output_len = bytes.len() as c_int };
    output
}

// SAFETY: caller must provide a readable input buffer of `input_len` bytes.
unsafe fn hex_to_string_impl(input: *const u8, input_len: c_int) -> *mut c_char {
    if input_len < 0 || (input_len > 0 && input.is_null()) {
        return ptr::null_mut();
    }
    // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
    let input = if input_len == 0 {
        &[]
    } else {
        // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
        unsafe { std::slice::from_raw_parts(input, input_len as usize) }
    };
    allocate(crate::port::hex_to_string(input).as_bytes(), true)
}

// SAFETY: caller must provide a readable input buffer of `input_len` bytes.
unsafe fn data_to_string_impl(
    input: *const u8,
    input_len: c_int,
    chars_per_line: c_int,
) -> *mut c_char {
    if input_len < 0 || chars_per_line <= 0 || (input_len > 0 && input.is_null()) {
        return ptr::null_mut();
    }
    // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
    let input = if input_len == 0 {
        &[]
    } else {
        // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
        unsafe { std::slice::from_raw_parts(input, input_len as usize) }
    };
    let Some(output) = crate::port::data_to_string(input, chars_per_line as usize) else {
        return ptr::null_mut();
    };
    allocate(output.as_bytes(), true)
}

// SAFETY: caller must provide a readable input buffer of `input_len` bytes.
unsafe fn data_to_text_impl(input: *const c_char, input_len: c_int) -> *mut c_char {
    if input_len < 0 || (input_len > 0 && input.is_null()) {
        return ptr::null_mut();
    }
    // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
    let input = if input_len == 0 {
        &[]
    } else {
        // SAFETY: input_len is non-negative and the null input was rejected unless it is empty.
        unsafe { std::slice::from_raw_parts(input.cast::<u8>(), input_len as usize) }
    };
    allocate(&crate::port::data_to_text(input), true)
}

// SAFETY: caller must provide a writable output buffer of `size` bytes.
unsafe fn ntp_timestamp_to_time_impl(timestamp: u64, output: *mut c_char, size: usize) {
    if output.is_null() || size <= 29 {
        return;
    }
    let Some(formatted) = crate::port::ntp_timestamp_to_time(timestamp) else {
        return;
    };
    // SAFETY: the caller supplies `size` writable bytes, and size > 29 covers this fixed 30-byte string.
    let output = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), size) };
    write_c_string(output, formatted.as_bytes());
}

// SAFETY: caller must provide a writable output buffer of `size` bytes.
unsafe fn ntp_timestamp_to_seconds_impl(timestamp: u64, output: *mut c_char, size: usize) {
    if output.is_null() || size <= 12 {
        return;
    }
    let Some(formatted) = crate::port::ntp_timestamp_to_seconds(timestamp) else {
        return;
    };
    // SAFETY: the caller supplies `size` writable bytes, and size > 12 covers this fixed 13-byte string.
    let output = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), size) };
    write_c_string(output, formatted.as_bytes());
}

// SAFETY: caller must provide buffers of the declared lengths.
unsafe fn ipaddress_to_string_impl(
    address_len: c_int,
    address: *const u8,
    zone_id: u32,
    output: *mut c_char,
    output_len: c_int,
) -> c_int {
    if output.is_null() || output_len <= 0 {
        return 0;
    }
    if address_len < 0 || (address_len > 0 && address.is_null()) {
        // SAFETY: output is writable for output_len bytes.
        unsafe { *output = 0 };
        return 0;
    }
    // SAFETY: output is writable for output_len bytes; address is copied before output is mutably viewed.
    let address = if address_len == 0 {
        Vec::new()
    } else {
        // SAFETY: address_len is positive and the null pointer case was rejected above.
        unsafe { std::slice::from_raw_parts(address, address_len as usize) }.to_vec()
    };
    // SAFETY: output is writable for output_len bytes.
    let output =
        // SAFETY: output is writable for output_len bytes.
        unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), output_len as usize) };
    crate::port::ipaddress_to_string_into(&address, zone_id, output)
}

// SAFETY: caller must provide a writable NUL-terminated XML string.
unsafe fn strip_data_from_plist_xml_impl(input: *mut c_char) -> *mut c_char {
    let Some(input) = c_string_copy(input) else {
        return ptr::null_mut();
    };
    match crate::port::strip_data_from_plist_xml(&input) {
        Ok(Some(output)) => allocate(&output, true),
        Ok(None) | Err(_) => ptr::null_mut(),
    }
}

export_pair!(utils_strsep, rust_anything_utils_strsep, (stringp: *mut *mut c_char, delimiter: *const c_char) -> *mut c_char, strsep_impl);
export_pair!(utils_read_file, rust_anything_utils_read_file, (dst: *mut *mut c_char, filename: *const c_char) -> c_int, read_file_impl);
export_pair!(utils_hwaddr_raop, rust_anything_utils_hwaddr_raop, (output: *mut c_char, output_len: c_int, address: *const c_char, address_len: c_int) -> c_int, hwaddr_raop_impl);
export_pair!(utils_hwaddr_airplay, rust_anything_utils_hwaddr_airplay, (output: *mut c_char, output_len: c_int, address: *const c_char, address_len: c_int) -> c_int, hwaddr_airplay_impl);
export_pair!(utils_parse_hex, rust_anything_utils_parse_hex, (input: *const c_char, input_len: c_int, output_len: *mut c_int) -> *mut c_char, parse_hex_impl);
export_pair!(utils_hex_to_string, rust_anything_utils_hex_to_string, (input: *const u8, input_len: c_int) -> *mut c_char, hex_to_string_impl);
export_pair!(utils_data_to_string, rust_anything_utils_data_to_string, (input: *const u8, input_len: c_int, chars_per_line: c_int) -> *mut c_char, data_to_string_impl);
export_pair!(utils_data_to_text, rust_anything_utils_data_to_text, (input: *const c_char, input_len: c_int) -> *mut c_char, data_to_text_impl);
export_pair!(ntp_timestamp_to_time, rust_anything_ntp_timestamp_to_time, (timestamp: u64, output: *mut c_char, size: usize) -> (), ntp_timestamp_to_time_impl);
export_pair!(ntp_timestamp_to_seconds, rust_anything_ntp_timestamp_to_seconds, (timestamp: u64, output: *mut c_char, size: usize) -> (), ntp_timestamp_to_seconds_impl);
export_pair!(utils_ipaddress_to_string, rust_anything_utils_ipaddress_to_string, (address_len: c_int, address: *const u8, zone_id: u32, output: *mut c_char, output_len: c_int) -> c_int, ipaddress_to_string_impl);
export_pair!(utils_strip_data_from_plist_xml, rust_anything_utils_strip_data_from_plist_xml, (input: *mut c_char) -> *mut c_char, strip_data_from_plist_xml_impl);

// SAFETY: exports the original C symbol with a stable thread-local result pointer.
#[cfg(not(feature = "shadow"))]
// SAFETY: exports the original C symbol with a stable thread-local result pointer.
#[unsafe(no_mangle)]
pub extern "C" fn gmt_time_string() -> *const c_char {
    gmt_time_string_impl()
}

// SAFETY: exports a unique shadow alias with the same stable result pointer.
#[unsafe(no_mangle)]
pub extern "C" fn rust_anything_gmt_time_string() -> *const c_char {
    gmt_time_string_impl()
}

thread_local! {
    static GMT_TIME_BUFFER: std::cell::RefCell<[c_char; 64]> = const { std::cell::RefCell::new([0; 64]) };
}

fn gmt_time_string_impl() -> *const c_char {
    let formatted = crate::port::gmt_time_string_now();
    GMT_TIME_BUFFER.with(|buffer| {
        let Ok(mut buffer) = buffer.try_borrow_mut() else {
            return c"".as_ptr();
        };
        let bytes = formatted.as_bytes();
        let written = bytes.len().min(buffer.len() - 1);
        for (dst, src) in buffer[..written].iter_mut().zip(&bytes[..written]) {
            *dst = *src as c_char;
        }
        buffer[written] = 0;
        buffer.as_ptr().cast()
    })
}

fn write_c_string(output: &mut [u8], input: &[u8]) {
    if output.is_empty() {
        return;
    }
    let copied = input.len().min(output.len() - 1);
    output[..copied].copy_from_slice(&input[..copied]);
    output[copied] = 0;
}
