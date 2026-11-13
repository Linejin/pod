//! Pod mobile LAN endpoint pieces (docs/23-mobile-m2.md).
//!
//! This crate holds what the daemon needs to talk to a phone on the LAN and
//! nothing that reaches the daemon itself (tools/check-boundaries.ps1):
//!
//! - [`tls`]: the TLS seam of the listener side. The policy types, the
//!   [`tls::TlsAcceptor`] / [`tls::TlsConnection`] traits and the SPKI pin
//!   are always built, so callers and test doubles can use them without
//!   the feature; the platform implementation needs `mobile-lan`.
//! - `crypto` and `identity` (`mobile-lan` only): platform crypto for the
//!   rest of M2 (256-bit randomness, SHA-256, P-256 ECDSA verification) and
//!   the listener's self-signed certificate and key.
//!
//! TLS and crypto come from the platform (user decision 2026-11-13,
//! docs/mobile-m2/dependencies.md): SChannel and CNG on Windows, the
//! system OpenSSL 3 (dynamically linked) on Linux. The crate implements no
//! cryptography and no ASN.1 itself.

#[cfg(all(feature = "mobile-lan", not(any(windows, target_os = "linux"))))]
compile_error!("mobile-lan is built on Windows (SChannel + CNG) and Linux (OpenSSL 3) only");

pub mod tls;

#[cfg(feature = "mobile-lan")]
pub mod crypto;
#[cfg(feature = "mobile-lan")]
pub mod identity;
#[cfg(feature = "mobile-lan")]
mod sys;

/// Whether this build carries the mobile TLS stack (`mobile-lan`). Without
/// it there is no listener, no TLS and no third-party dependency.
pub const TLS_BUILT: bool = cfg!(feature = "mobile-lan");
