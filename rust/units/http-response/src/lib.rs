//! Rust implementation of the opaque C `http_response_t` API.

pub mod port;

mod ffi {
    use crate::port::HttpResponse;
    use std::alloc::{Layout, alloc, dealloc};
    use std::ffi::{CStr, c_char, c_int};
    use std::ptr;

    unsafe fn response_mut<'a>(response: *mut HttpResponse) -> &'a mut HttpResponse {
        assert!(!response.is_null());
        // SAFETY: each exported caller must pass a live handle from `http_response_create`.
        unsafe { &mut *response }
    }

    unsafe fn c_bytes<'a>(value: *const c_char) -> &'a [u8] {
        assert!(!value.is_null());
        // SAFETY: each exported caller must pass a readable NUL-terminated C string.
        unsafe { CStr::from_ptr(value) }.to_bytes()
    }

    /// Creates an empty response with a 1024-byte initial data reservation.
    #[unsafe(no_mangle)]
    pub extern "C" fn http_response_create() -> *mut HttpResponse {
        let mut response = HttpResponse::new();
        if response.reserve_initial().is_err() {
            return ptr::null_mut();
        }
        let layout = Layout::new::<HttpResponse>();
        // SAFETY: `layout` is non-zero and matches the value written below.
        let raw = unsafe { alloc(layout).cast::<HttpResponse>() };
        if raw.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `raw` points to aligned storage for one `HttpResponse`.
        unsafe { raw.write(response) };
        raw
    }

    /// Starts an HTTP response line, resetting the byte length but retaining state as in C.
    ///
    /// # Safety
    /// `response` must be a live handle and `protocol` / `message` must be NUL-terminated strings.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_init(
        response: *mut HttpResponse,
        protocol: *const c_char,
        code: c_int,
        message: *const c_char,
    ) {
        // SAFETY: required by this exported function's contract.
        let response = unsafe { response_mut(response) };
        // SAFETY: required by this exported function's contract.
        let protocol = unsafe { c_bytes(protocol) };
        // SAFETY: required by this exported function's contract.
        let message = unsafe { c_bytes(message) };
        response.init(protocol, code, message);
    }

    /// Starts a reverse-HTTP request line.
    ///
    /// # Safety
    /// `request` must be a live handle; all string arguments must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_reverse_request_init(
        request: *mut HttpResponse,
        method: *const c_char,
        url: *const c_char,
        protocol: *const c_char,
    ) {
        // SAFETY: required by this exported function's contract.
        let request = unsafe { response_mut(request) };
        // SAFETY: required by this exported function's contract.
        let method = unsafe { c_bytes(method) };
        // SAFETY: required by this exported function's contract.
        let url = unsafe { c_bytes(url) };
        // SAFETY: required by this exported function's contract.
        let protocol = unsafe { c_bytes(protocol) };
        request.reverse_request_init(method, url, protocol);
    }

    /// Appends a response header followed by CRLF.
    ///
    /// # Safety
    /// `response` must be a live handle; `name` and `value` must be NUL-terminated strings.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_add_header(
        response: *mut HttpResponse,
        name: *const c_char,
        value: *const c_char,
    ) {
        // SAFETY: required by this exported function's contract.
        let response = unsafe { response_mut(response) };
        // SAFETY: required by this exported function's contract.
        let name = unsafe { c_bytes(name) };
        // SAFETY: required by this exported function's contract.
        let value = unsafe { c_bytes(value) };
        response.add_header(name, value);
    }

    /// Appends an optional body and terminates the response headers.
    ///
    /// # Safety
    /// `response` must be a live handle. If `data` is non-null and `datalen > 0`, it must point to
    /// at least `datalen` readable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_finish(
        response: *mut HttpResponse,
        data: *const c_char,
        datalen: c_int,
    ) {
        // SAFETY: required by this exported function's contract.
        let response = unsafe { response_mut(response) };
        let bytes = if data.is_null() || datalen <= 0 {
            None
        } else {
            // SAFETY: guaranteed by the exported function's contract for positive lengths.
            Some(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), datalen as usize) })
        };
        response.finish(bytes, datalen);
    }

    /// Sets the disconnect flag using C's logical-bool conversion.
    ///
    /// # Safety
    /// `response` must be a live handle.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_set_disconnect(
        response: *mut HttpResponse,
        disconnect: c_int,
    ) {
        // SAFETY: required by this exported function's contract.
        unsafe { response_mut(response) }.set_disconnect(disconnect);
    }

    /// Returns the disconnect flag.
    ///
    /// # Safety
    /// `response` must be a live handle.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_get_disconnect(response: *mut HttpResponse) -> c_int {
        // SAFETY: required by this exported function's contract.
        unsafe { response_mut(response) }.disconnect()
    }

    /// Returns a pointer to the completed response bytes and writes their length.
    ///
    /// # Safety
    /// `response` must be a live completed handle and `datalen` must be writable.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_get_data(
        response: *mut HttpResponse,
        datalen: *mut c_int,
    ) -> *const c_char {
        assert!(!datalen.is_null());
        // SAFETY: required by this exported function's contract.
        let response = unsafe { response_mut(response) };
        // SAFETY: required by this exported function's contract.
        let (data, len) = response.data_ptr_and_len();
        // SAFETY: required by this exported function's contract.
        unsafe { datalen.write(len as c_int) };
        data.cast::<c_char>()
    }

    /// Destroys a response; a null pointer is accepted.
    ///
    /// # Safety
    /// `response` must be null or a live handle from `http_response_create` not already destroyed.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn http_response_destroy(response: *mut HttpResponse) {
        if response.is_null() {
            return;
        }
        // SAFETY: the exported contract requires this pointer to reference a live created handle.
        unsafe { ptr::drop_in_place(response) };
        // SAFETY: allocated with `alloc(Layout::new::<HttpResponse>())` in create.
        unsafe { dealloc(response.cast::<u8>(), Layout::new::<HttpResponse>()) };
    }
}
