//! The TLS seam of the mobile listener (docs/23 section 2, TM-01).
//!
//! The listener side is [`TlsAcceptor`]: it takes a connected byte stream,
//! runs the server handshake and hands back a [`TlsConnection`] that reads
//! and writes plaintext and reports what was negotiated. The platform
//! implementation ([`ServerConfig`], `mobile-lan` only) is SChannel on
//! Windows and the system OpenSSL 3 on Linux; a test double can implement
//! the same traits without the feature.
//!
//! Trust comes from the QR's SPKI pin, not from a CA: the phone compares
//! the SHA-256 of the server certificate's SubjectPublicKeyInfo with the
//! pin before it sends anything ([`PinnedClient`] is the desktop double of
//! that check). The negotiated version and suite are checked again after
//! every handshake ([`SessionInfo::check`]), so an OS default that slips
//! past the configured restrictions fails closed.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

/// The only application protocol the listener speaks (ALPN). A session
/// without it is refused.
pub const ALPN_HTTP11: &[u8] = b"http/1.1";

/// docs/23 section 2: the handshake gets 10 seconds.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Handshake bytes accepted before the session is up. M2 has no client
/// certificates, so a ClientHello and Finished stay far below this.
pub const MAX_HANDSHAKE_BYTES: usize = 32 * 1024;

/// TLS 1.3 cipher suites (RFC 8446 B.4): all of them are AEAD with an
/// ephemeral key exchange.
pub const TLS13_SUITES: [u16; 3] = [0x1301, 0x1302, 0x1303];

/// The TLS 1.2 suites a listener may fall back to where the OS has no TLS
/// 1.3 server (Windows 10): ECDHE with the listener's ECDSA key and an AEAD
/// cipher only (`ECDHE_ECDSA_WITH_AES_128_GCM_SHA256`,
/// `..._AES_256_GCM_SHA384`, `..._CHACHA20_POLY1305_SHA256`). No CBC, no
/// static RSA or finite-field DH.
pub const TLS12_AEAD_SUITES: [u16; 3] = [0xC02B, 0xC02C, 0xCCA9];

/// A negotiated protocol version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TlsVersion {
    /// TLS 1.2 (only under [`VersionPolicy::Tls12Aead`]).
    Tls12,
    /// TLS 1.3.
    Tls13,
}

impl TlsVersion {
    /// The name kept in the pairing record and shown in the PC UI.
    pub fn as_str(self) -> &'static str {
        match self {
            TlsVersion::Tls12 => "TLS1.2",
            TlsVersion::Tls13 => "TLS1.3",
        }
    }
}

impl fmt::Display for TlsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which versions and suites a listener accepts (docs/23 section 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VersionPolicy {
    /// TLS 1.3 only. The default wherever the OS has a TLS 1.3 server:
    /// Windows 11 / Server 2022 and later, Linux with OpenSSL 3.
    Tls13Only,
    /// TLS 1.3 when the OS has it, otherwise TLS 1.2 limited to
    /// [`TLS12_AEAD_SUITES`]. Windows 10 servers have no TLS 1.3.
    Tls12Aead,
}

impl VersionPolicy {
    /// Whether a session of `version` with IANA suite `suite` passes.
    pub fn allows(self, version: TlsVersion, suite: u16) -> bool {
        match (self, version) {
            (_, TlsVersion::Tls13) => TLS13_SUITES.contains(&suite),
            (VersionPolicy::Tls12Aead, TlsVersion::Tls12) => TLS12_AEAD_SUITES.contains(&suite),
            (VersionPolicy::Tls13Only, TlsVersion::Tls12) => false,
        }
    }

    /// The policy for this machine: [`VersionPolicy::Tls13Only`] when the OS
    /// has a TLS 1.3 server, else [`VersionPolicy::Tls12Aead`].
    #[cfg(feature = "mobile-lan")]
    pub fn platform_default() -> VersionPolicy {
        if crate::sys::tls::has_tls13_server() {
            VersionPolicy::Tls13Only
        } else {
            VersionPolicy::Tls12Aead
        }
    }
}

/// What a finished handshake negotiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Protocol version.
    pub version: TlsVersion,
    /// IANA cipher suite number.
    pub cipher_suite: u16,
    /// The platform's name for the suite (diagnostics only).
    pub cipher_name: String,
    /// The ALPN protocol, if one was agreed.
    pub alpn: Option<Vec<u8>>,
}

impl SessionInfo {
    /// The post-handshake gate: version and suite allowed by `policy` and
    /// ALPN `http/1.1`. Both ends call it before any application data.
    pub fn check(&self, policy: VersionPolicy) -> Result<(), TlsError> {
        if !policy.allows(self.version, self.cipher_suite) {
            return Err(TlsError::Policy(format!(
                "{} with suite 0x{:04x} ({}) is outside {policy:?}",
                self.version, self.cipher_suite, self.cipher_name
            )));
        }
        if self.alpn.as_deref() != Some(ALPN_HTTP11) {
            return Err(TlsError::Policy("ALPN http/1.1 was not negotiated".into()));
        }
        Ok(())
    }
}

/// SHA-256 of a certificate's DER SubjectPublicKeyInfo: the QR pin.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpkiSha256(pub [u8; 32]);

impl SpkiSha256 {
    /// Constant-time comparison (docs/23 section 2: the client compares the
    /// pin and the presented key in constant time).
    pub fn ct_eq(&self, other: &SpkiSha256) -> bool {
        ct_eq(&self.0, &other.0)
    }

    /// Lowercase hex.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for SpkiSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpkiSha256({})", self.to_hex())
    }
}

impl fmt::Display for SpkiSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Byte comparison whose time depends only on the lengths.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// Why a TLS operation failed. Messages carry no secret or peer data.
#[derive(Debug)]
pub enum TlsError {
    /// The transport failed or closed during the handshake.
    Io(io::Error),
    /// The handshake ran past its deadline.
    Timeout,
    /// The handshake input exceeded [`MAX_HANDSHAKE_BYTES`].
    TooLarge,
    /// The platform TLS stack refused (`code` is the OS status).
    Os {
        /// The call that failed.
        what: &'static str,
        /// SSPI status (Windows) or OpenSSL error code (Linux).
        code: i64,
    },
    /// The session negotiated something outside the policy.
    Policy(String),
    /// The server key does not match the pin.
    PinMismatch,
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Io(e) => write!(f, "TLS transport: {e}"),
            TlsError::Timeout => f.write_str("TLS handshake timed out"),
            TlsError::TooLarge => f.write_str("TLS handshake input too large"),
            TlsError::Os { what, code } => write!(f, "{what} failed (0x{:08x})", *code as u32),
            TlsError::Policy(why) => write!(f, "TLS policy: {why}"),
            TlsError::PinMismatch => f.write_str("server key does not match the pinned SPKI"),
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TlsError {
    fn from(e: io::Error) -> Self {
        if matches!(
            e.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            TlsError::Timeout
        } else {
            TlsError::Io(e)
        }
    }
}

/// An established TLS session: plaintext `Read`/`Write` plus what was
/// negotiated.
pub trait TlsConnection: Read + Write {
    /// Version, suite and ALPN of this session.
    fn session(&self) -> &SessionInfo;
    /// DER of the peer's leaf certificate. `None` on the listener: M2 asks
    /// no client certificate (devices prove their key by signing nonces).
    fn peer_certificate(&self) -> Option<&[u8]>;
    /// The SPKI pin of the peer's certificate, when there is one.
    fn peer_spki_sha256(&self) -> Option<SpkiSha256>;
    /// Sends close_notify. The transport stays open for the caller to drop.
    fn shutdown(&mut self) -> io::Result<()>;
}

/// The listener side: one handshake per accepted transport.
///
/// `accept` blocks on the transport. The caller bounds each read and write
/// (for a `TcpStream`, socket timeouts); the implementation also stops at
/// its handshake deadline between reads.
pub trait TlsAcceptor<S: Read + Write> {
    /// The established session.
    type Stream: TlsConnection;
    /// Runs the server handshake on `io` and checks the session policy.
    fn accept(&self, io: S) -> Result<Self::Stream, TlsError>;
    /// The pin of the certificate this acceptor presents.
    fn spki_sha256(&self) -> SpkiSha256;
}

#[cfg(feature = "mobile-lan")]
pub(crate) mod platform;
#[cfg(feature = "mobile-lan")]
pub use platform::{ClientCiphers, ClientOffer, PinnedClient, ServerConfig, TlsStream};

#[cfg(test)]
mod tests {
    use super::*;

    fn info(version: TlsVersion, suite: u16, alpn: Option<&[u8]>) -> SessionInfo {
        SessionInfo {
            version,
            cipher_suite: suite,
            cipher_name: "x".into(),
            alpn: alpn.map(<[u8]>::to_vec),
        }
    }

    #[test]
    fn tls13_only_refuses_every_tls12_suite() {
        for s in TLS13_SUITES {
            assert!(VersionPolicy::Tls13Only.allows(TlsVersion::Tls13, s));
        }
        for s in TLS12_AEAD_SUITES {
            assert!(!VersionPolicy::Tls13Only.allows(TlsVersion::Tls12, s));
        }
    }

    #[test]
    fn tls12_fallback_is_ecdhe_ecdsa_aead_only() {
        for s in TLS12_AEAD_SUITES {
            assert!(VersionPolicy::Tls12Aead.allows(TlsVersion::Tls12, s));
        }
        // ECDHE_ECDSA AES-CBC (SHA, SHA256), ECDHE_RSA GCM, static RSA GCM,
        // DHE_RSA GCM, and a TLS 1.3 suite number on a 1.2 session.
        for s in [0xC009, 0xC023, 0xC02F, 0x009C, 0x009E, 0x1301] {
            assert!(
                !VersionPolicy::Tls12Aead.allows(TlsVersion::Tls12, s),
                "0x{s:04x}"
            );
        }
        // A 1.2 suite number on a 1.3 session.
        assert!(!VersionPolicy::Tls12Aead.allows(TlsVersion::Tls13, 0xC02B));
    }

    #[test]
    fn session_check_needs_the_policy_and_http11() {
        let ok = info(TlsVersion::Tls13, 0x1301, Some(ALPN_HTTP11));
        assert!(ok.check(VersionPolicy::Tls13Only).is_ok());
        for bad in [
            info(TlsVersion::Tls13, 0x1301, None),
            info(TlsVersion::Tls13, 0x1301, Some(b"h2")),
            info(TlsVersion::Tls12, 0xC02B, Some(ALPN_HTTP11)),
        ] {
            assert!(
                matches!(
                    bad.check(VersionPolicy::Tls13Only),
                    Err(TlsError::Policy(_))
                ),
                "{bad:?}"
            );
        }
        assert!(
            info(TlsVersion::Tls12, 0xC02B, Some(ALPN_HTTP11))
                .check(VersionPolicy::Tls12Aead)
                .is_ok()
        );
    }

    #[test]
    fn pins_compare_by_value() {
        let a = SpkiSha256([7; 32]);
        let mut b = a;
        assert!(a.ct_eq(&b));
        b.0[31] ^= 1;
        assert!(!a.ct_eq(&b));
        assert!(!ct_eq(&[1, 2], &[1, 2, 3]));
        assert_eq!(SpkiSha256([0xab; 32]).to_hex(), "ab".repeat(32));
    }

    #[test]
    fn io_timeouts_are_handshake_timeouts() {
        assert!(matches!(
            TlsError::from(io::Error::from(io::ErrorKind::TimedOut)),
            TlsError::Timeout
        ));
        assert!(matches!(
            TlsError::from(io::Error::from(io::ErrorKind::WouldBlock)),
            TlsError::Timeout
        ));
        assert!(matches!(
            TlsError::from(io::Error::from(io::ErrorKind::UnexpectedEof)),
            TlsError::Io(_)
        ));
    }
}
