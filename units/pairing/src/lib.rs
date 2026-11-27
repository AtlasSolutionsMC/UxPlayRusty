//! Rust port of UxPlay's pairing and SRP session behavior.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod port;

pub use port::{Pairing, PairingError, PairingSession, PairingStatus, SrpOffer};
