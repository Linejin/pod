//! The platform TLS behind the seam (`mobile-lan`): SChannel on Windows,
//! the system OpenSSL 3 on Linux (`crate::sys`).

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use super::{
    HANDSHAKE_TIMEOUT, SessionInfo, SpkiSha256, TlsAcceptor, TlsConnection, TlsError, VersionPolicy,
};
use crate::identity::ServerIdentity;
use crate::sys::tls as imp;

/// Cipher families the [`PinnedClient`] offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientCiphers {
    /// AEAD only, as the listener accepts (the normal client).
    Aead,
    /// CBC only, never accepted: proves the listener refuses CBC suites.
    CbcOnly,
}

/// What a [`PinnedClient`] offers in its ClientHello.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientOffer {
    /// Offer TLS 1.2.
    pub tls12: bool,
    /// Offer TLS 1.3.
    pub tls13: bool,
    /// Cipher families.
    pub ciphers: ClientCiphers,
}

/// Versions and ciphers one end enables before the handshake. The listener
/// always runs AEAD only; `cbc_only` exists for the refusing client.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Offer {
    pub(crate) tls12: bool,
    pub(crate) tls13: bool,
    pub(crate) cbc_only: bool,
}

/// The listener's TLS configuration: the identity it presents and the
/// version policy. Implements [`TlsAcceptor`].
pub struct ServerConfig {
    inner: imp::Server,
    policy: VersionPolicy,
    spki: SpkiSha256,
    handshake_timeout: Duration,
}

impl ServerConfig {
    /// A listener presenting `identity` under `policy`. Under
    /// [`VersionPolicy::Tls13Only`] on an OS without a TLS 1.3 server every
    /// handshake fails (fail closed); pick
    /// [`VersionPolicy::platform_default`] for the product.
    pub fn new(identity: &ServerIdentity, policy: VersionPolicy) -> Result<ServerConfig, TlsError> {
        let offer = Offer {
            tls12: policy == VersionPolicy::Tls12Aead,
            tls13: policy == VersionPolicy::Tls13Only || imp::has_tls13_server(),
            cbc_only: false,
        };
        Ok(ServerConfig {
            inner: imp::Server::new(identity.credential(), offer)?,
            policy,
            spki: identity.spki_sha256(),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        })
    }

    /// Replaces the [`HANDSHAKE_TIMEOUT`] deadline.
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> ServerConfig {
        self.handshake_timeout = timeout;
        self
    }

    /// The version policy every session is checked against.
    pub fn policy(&self) -> VersionPolicy {
        self.policy
    }
}

impl<S: Read + Write> TlsAcceptor<S> for ServerConfig {
    type Stream = TlsStream<S>;

    fn accept(&self, io: S) -> Result<TlsStream<S>, TlsError> {
        let deadline = Instant::now() + self.handshake_timeout;
        let mut stream = TlsStream(self.inner.accept(io, deadline)?);
        if let Err(e) = stream.session().check(self.policy) {
            let _ = stream.shutdown();
            return Err(e);
        }
        Ok(stream)
    }

    fn spki_sha256(&self) -> SpkiSha256 {
        self.spki
    }
}

/// A client that trusts exactly one server key: the QR pin. It is the
/// desktop double of the phone's check (docs/23 section 2): no CA, no
/// hostname, and nothing is written before the presented key matches the
/// pin and the session passes the policy. The platform still verifies the
/// handshake signature (CertificateVerify) with the presented key, so a
/// matching pin also proves possession of the private key.
pub struct PinnedClient {
    inner: imp::Client,
    pin: SpkiSha256,
    policy: VersionPolicy,
    handshake_timeout: Duration,
}

impl PinnedClient {
    /// A client offering what `policy` allows (AEAD only).
    pub fn new(pin: SpkiSha256, policy: VersionPolicy) -> Result<PinnedClient, TlsError> {
        let offer = ClientOffer {
            tls12: policy == VersionPolicy::Tls12Aead,
            tls13: true,
            ciphers: ClientCiphers::Aead,
        };
        PinnedClient::with_offer(pin, policy, offer)
    }

    /// A client with an explicit offer, to prove the listener refuses what
    /// its policy does not allow.
    pub fn with_offer(
        pin: SpkiSha256,
        policy: VersionPolicy,
        offer: ClientOffer,
    ) -> Result<PinnedClient, TlsError> {
        let offer = Offer {
            tls12: offer.tls12,
            tls13: offer.tls13,
            cbc_only: offer.ciphers == ClientCiphers::CbcOnly,
        };
        Ok(PinnedClient {
            inner: imp::Client::new(offer)?,
            pin,
            policy,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        })
    }

    /// Runs the client handshake, then checks the pin and the policy. On
    /// [`TlsError::PinMismatch`] or a policy error the session is dropped
    /// without application data.
    pub fn connect<S: Read + Write>(&self, io: S) -> Result<TlsStream<S>, TlsError> {
        let deadline = Instant::now() + self.handshake_timeout;
        let stream = TlsStream(self.inner.connect(io, deadline)?);
        match stream.peer_spki_sha256() {
            Some(presented) if presented.ct_eq(&self.pin) => {}
            _ => return Err(TlsError::PinMismatch),
        }
        stream.session().check(self.policy)?;
        Ok(stream)
    }
}

/// An established platform TLS session over `S`.
pub struct TlsStream<S>(imp::Stream<S>);

impl<S> TlsStream<S> {
    /// Sets an absolute read deadline, checked before each transport read.
    /// The caller must also bound the socket timeout by the remaining deadline.
    pub fn set_read_deadline(&mut self, deadline: Option<Instant>) {
        self.0.set_read_deadline(deadline);
    }

    /// The transport.
    pub fn get_ref(&self) -> &S {
        self.0.get_ref()
    }

    /// The transport, mutably (for socket options; never read or write it
    /// directly).
    pub fn get_mut(&mut self) -> &mut S {
        self.0.get_mut()
    }
}

impl<S: Read + Write> Read for TlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<S: Read + Write> Write for TlsStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<S: Read + Write> TlsConnection for TlsStream<S> {
    fn session(&self) -> &SessionInfo {
        self.0.session()
    }

    fn peer_certificate(&self) -> Option<&[u8]> {
        self.0.peer_certificate()
    }

    fn peer_spki_sha256(&self) -> Option<SpkiSha256> {
        self.0.peer_spki_sha256()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.0.shutdown()
    }
}
