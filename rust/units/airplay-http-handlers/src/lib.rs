//! Typed, isolated ports for the independent HTTP handlers in `lib/http_handlers.h`.
//!
//! Callback and HLS-store work is returned as explicit effects so an app layer can
//! carry it out without fabricating renderer or connection state.

pub mod port;

pub use port::*;
