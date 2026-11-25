//! Safe HTTP response byte construction.

#[derive(Debug, Default)]
pub struct HttpResponse {
    complete: bool,
    disconnect: bool,
    data: Vec<u8>,
}

impl HttpResponse {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reserve_initial(&mut self) -> Result<(), std::collections::TryReserveError> {
        self.data.try_reserve_exact(1024)
    }

    fn add_data(&mut self, data: &[u8]) {
        assert!(!data.is_empty());
        self.data.extend_from_slice(data);
    }

    pub fn init(&mut self, protocol: &[u8], code: i32, message: &[u8]) {
        self.data.clear();
        assert!((100..1000).contains(&code));
        self.add_data(protocol);
        self.add_data(b" ");
        self.add_data(code.to_string().as_bytes());
        self.add_data(b" ");
        self.add_data(message);
        self.add_data(b"\r\n");
    }

    pub fn reverse_request_init(&mut self, method: &[u8], url: &[u8], protocol: &[u8]) {
        self.data.clear();
        self.add_data(method);
        self.add_data(b" ");
        self.add_data(url);
        self.add_data(b" ");
        self.add_data(protocol);
        self.add_data(b"\r\n");
    }

    pub fn add_header(&mut self, name: &[u8], value: &[u8]) {
        self.add_data(name);
        self.add_data(b": ");
        self.add_data(value);
        self.add_data(b"\r\n");
    }

    pub fn finish(&mut self, data: Option<&[u8]>, datalen: i32) {
        assert!(datalen == 0 || (data.is_some() && datalen > 0));
        if let Some(body) = data
            && datalen > 0
        {
            let length = datalen.to_string();
            self.add_data(b"Content-Length");
            self.add_data(b": ");
            self.add_data(length.as_bytes());
            self.add_data(b"\r\n\r\n");
            assert!(body.len() >= datalen as usize);
            self.add_data(&body[..datalen as usize]);
        } else {
            const CONTENT_TYPE: &[u8] = b"Content-Type";
            let end = self.data.len() as isize - CONTENT_TYPE.len() as isize;
            for index in 0..end.max(0) as usize {
                if self.data[index..index + CONTENT_TYPE.len()] == *CONTENT_TYPE {
                    self.add_data(b"Content-Length: 0");
                    self.add_data(b"\r\n");
                    break;
                }
            }
            self.add_data(b"\r\n");
        }
        self.complete = true;
    }

    pub fn set_disconnect(&mut self, value: i32) {
        self.disconnect = value != 0;
    }

    pub fn disconnect(&self) -> i32 {
        i32::from(self.disconnect)
    }

    pub fn completed_data(&self) -> &[u8] {
        assert!(self.complete);
        &self.data
    }

    pub(crate) fn data_ptr_and_len(&self) -> (*const u8, usize) {
        assert!(self.complete);
        (self.data.as_ptr(), self.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::HttpResponse;

    #[test]
    fn status_line_and_body_are_exact_bytes() {
        let mut response = HttpResponse::new();
        response.init(b"HTTP/1.1", 200, b"OK");
        response.add_header(b"Content-Type", b"application/octet-stream");
        response.finish(Some(b"a\0b"), 3);
        assert_eq!(
            response.completed_data(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\n\r\na\0b"
        );
    }

    #[test]
    fn empty_body_inserts_content_length_only_when_content_type_occurs() {
        let mut typed = HttpResponse::new();
        typed.init(b"HTTP/1.1", 204, b"No Content");
        typed.add_header(b"Content-Type", b"text/plain");
        typed.finish(None, 0);
        assert_eq!(
            typed.completed_data(),
            b"HTTP/1.1 204 No Content\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n"
        );

        let mut untyped = HttpResponse::new();
        untyped.init(b"HTTP/1.1", 204, b"No Content");
        untyped.add_header(b"X-Test", b"yes");
        untyped.finish(None, 0);
        assert_eq!(
            untyped.completed_data(),
            b"HTTP/1.1 204 No Content\r\nX-Test: yes\r\n\r\n"
        );
    }

    #[test]
    fn reverse_request_and_reinitialization_match_source_state() {
        let mut response = HttpResponse::new();
        response.set_disconnect(-9);
        response.init(b"HTTP/1.1", 201, b"Created");
        response.finish(None, 0);
        response.reverse_request_init(b"POST", b"/pair-setup", b"PTTH/1.0");
        assert_eq!(response.disconnect(), 1);
        // Source leaves `complete` set through reinitialization.
        assert_eq!(response.completed_data(), b"POST /pair-setup PTTH/1.0\r\n");
    }

    #[test]
    fn long_header_and_body_grow_the_initial_buffer() {
        let mut response = HttpResponse::new();
        response.init(b"HTTP/1.1", 200, b"OK");
        response.add_header(b"X-Long", &vec![b'v'; 1800]);
        response.finish(Some(&vec![b'z'; 3000]), 3000);
        assert_eq!(
            response.completed_data().len(),
            8 + 1 + 3 + 1 + 2 + 2 + 6 + 2 + 1800 + 2 + 14 + 2 + 4 + 4 + 3000
        );
    }
}
