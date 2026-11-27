//! Optional Unix/X11 window lookup and EWMH fullscreen handling from
//! `renderers/x_display_fix.h`.

#[cfg(unix)]
use x11rb::connection::Connection;
#[cfg(unix)]
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode,
    Window,
};
#[cfg(unix)]
use x11rb::rust_connection::RustConnection;
#[cfg(unix)]
use x11rb::wrapper::ConnectionExt as _;

#[cfg(unix)]
const MAX_WINDOW_NAME_BYTES: u32 = 1024;

#[cfg(unix)]
pub(crate) struct X11WindowFix {
    connection: RustConnection,
    root: Window,
    server_name: Vec<u8>,
    state_atom: Atom,
    fullscreen_atom: Atom,
    net_wm_name_atom: Atom,
    utf8_string_atom: Atom,
    window: Option<Window>,
    fullscreen: bool,
    alt_pressed: bool,
    search_attempts: u8,
}

#[cfg(unix)]
impl X11WindowFix {
    pub(crate) fn open(
        server_name: &str,
        initially_fullscreen: bool,
        apply_old_gstreamer_zoom_fix: bool,
    ) -> Option<Self> {
        let (connection, screen) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        let state_atom = intern_atom(&connection, b"_NET_WM_STATE", true)?;
        let fullscreen_atom = intern_atom(&connection, b"_NET_WM_STATE_FULLSCREEN", true)?;
        let net_wm_name_atom = if apply_old_gstreamer_zoom_fix {
            intern_atom(&connection, b"_NET_WM_NAME", false)?
        } else {
            0
        };
        let utf8_string_atom = if apply_old_gstreamer_zoom_fix {
            intern_atom(&connection, b"UTF8_STRING", false)?
        } else {
            0
        };
        let server_name = server_name
            .split('\0')
            .next()
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        Some(Self {
            connection,
            root,
            server_name,
            state_atom,
            fullscreen_atom,
            net_wm_name_atom,
            utf8_string_atom,
            window: None,
            fullscreen: initially_fullscreen,
            alt_pressed: false,
            search_attempts: 0,
        })
    }

    pub(crate) fn waiting_for_window(&mut self, use_x11: bool) -> bool {
        if !use_x11 {
            return false;
        }
        if self.window.is_none() {
            self.search_window();
        }
        self.window.is_none()
    }

    /// Search once after a mirror frame, as the C renderer does.
    pub(crate) fn search_after_frame(&mut self, use_x11: bool) -> SearchResult {
        if !use_x11 || self.window.is_some() {
            return SearchResult::NotSearching;
        }
        self.search_attempts = self.search_attempts.saturating_add(1);
        if self.search_window() {
            SearchResult::Found(self.search_attempts)
        } else {
            SearchResult::Waiting(self.search_attempts)
        }
    }

    pub(crate) fn handle_key(&mut self, key: &str, pressed: bool) {
        if self.window.is_none() {
            return;
        }
        if pressed {
            if key == "F11" || (self.alt_pressed && key == "Return") {
                self.fullscreen = !self.fullscreen;
                self.set_fullscreen();
            } else if key == "Alt_L" {
                self.alt_pressed = true;
            }
        } else if key == "Alt_L" {
            self.alt_pressed = false;
        }
    }

    fn search_window(&mut self) -> bool {
        let Some(window) = find_window(&self.connection, self.root, &self.server_name) else {
            return false;
        };
        self.window = Some(window);
        if self.net_wm_name_atom != 0 && self.utf8_string_atom != 0 {
            let _ = self.connection.change_property8(
                PropMode::REPLACE,
                window,
                self.net_wm_name_atom,
                self.utf8_string_atom,
                &self.server_name,
            );
            let _ = self.connection.sync();
        }
        if self.fullscreen {
            self.set_fullscreen();
        }
        true
    }

    fn set_fullscreen(&self) {
        let Some(window) = self.window else {
            return;
        };
        if self.state_atom == 0 || self.fullscreen_atom == 0 {
            return;
        }
        let message = ClientMessageEvent::new(
            32,
            window,
            self.state_atom,
            ClientMessageData::from([
                u32::from(self.fullscreen),
                self.fullscreen_atom,
                AtomEnum::NONE.into(),
                0,
                1,
            ]),
        );
        if let Ok(cookie) = self.connection.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            message,
        ) {
            let _ = cookie.check();
            let _ = self.connection.flush();
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchResult {
    NotSearching,
    Waiting(u8),
    Found(u8),
}

#[cfg(unix)]
fn intern_atom(connection: &RustConnection, name: &[u8], only_if_exists: bool) -> Option<Atom> {
    connection
        .intern_atom(only_if_exists, name)
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
}

#[cfg(unix)]
fn find_window(connection: &RustConnection, parent: Window, title: &[u8]) -> Option<Window> {
    let name = connection
        .get_property(
            false,
            parent,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            0,
            MAX_WINDOW_NAME_BYTES,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|property| property.value);
    if name.as_deref().is_some_and(|name| c_prefix(name) == title) {
        return Some(parent);
    }
    let Ok(cookie) = connection.query_tree(parent) else {
        return None;
    };
    let Ok(tree) = cookie.reply() else {
        return None;
    };
    for window in tree.children {
        if let Some(found) = find_window(connection, window, title) {
            return Some(found);
        }
    }
    None
}

#[cfg(unix)]
fn c_prefix(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())]
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::c_prefix;

    #[test]
    fn c_window_name_comparison_stops_at_nul() {
        assert_eq!(c_prefix(b"UxPlay\0ignored"), b"UxPlay");
        assert_eq!(c_prefix(b"UxPlay"), b"UxPlay");
    }
}

#[cfg(not(unix))]
pub(crate) struct X11WindowFix;

#[cfg(not(unix))]
impl X11WindowFix {
    pub(crate) fn open(_: &str, _: bool, _: bool) -> Option<Self> {
        None
    }

    pub(crate) fn waiting_for_window(&mut self, _: bool) -> bool {
        false
    }

    pub(crate) fn search_after_frame(&mut self, _: bool) -> SearchResult {
        SearchResult::NotSearching
    }

    pub(crate) fn handle_key(&mut self, _: &str, _: bool) {}
}

#[cfg(not(unix))]
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchResult {
    NotSearching,
    Waiting(u8),
    Found(u8),
}
