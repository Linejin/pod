//! Platform backends (`mobile-lan`). Each exposes the same three modules:
//! `tls` (SChannel / OpenSSL handshake and record layer), `crypto`
//! (randomness, SHA-256, P-256 verification) and `identity` (listener key
//! store, self-signed certificate, certificate decoding).

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{crypto, identity, tls};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{crypto, identity, tls};
