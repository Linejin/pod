//! OpenSSL 3 TLS: one context per configuration with the version range,
//! TLS 1.3 suites and TLS 1.2 AEAD-only cipher list, no tickets, no
//! compression, no renegotiation, no session cache; ALPN `http/1.1`.

use std::io::{self, Read, Write};
use std::time::Instant;

use openssl::error::ErrorStack;
use openssl::ssl::{
    AlpnError, HandshakeError, ShutdownState, Ssl, SslContext, SslContextBuilder, SslMethod,
    SslOptions, SslSessionCacheMode, SslStream, SslVerifyMode, SslVersion, select_next_proto,
};

use super::identity::Credential;
use super::ossl_code;
use crate::tls::platform::Offer;
use crate::tls::{MAX_HANDSHAKE_BYTES, SessionInfo, SpkiSha256, TlsError, TlsVersion};

const TLS13_SUITES: &str =
    "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256";
/// `crate::tls::TLS12_AEAD_SUITES` in OpenSSL names.
const TLS12_AEAD: &str =
    "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305";
/// The refusing client's offer.
const TLS12_CBC: &str = "ECDHE-ECDSA-AES128-SHA256:ECDHE-ECDSA-AES128-SHA:ECDHE-ECDSA-AES256-SHA";
/// ALPN wire format of `http/1.1`.
const ALPN_WIRE: &[u8] = b"\x08http/1.1";
const OPENSSL_3: i64 = 0x3000_0000;

fn err(what: &'static str) -> impl Fn(ErrorStack) -> TlsError {
    move |e| TlsError::Os {
        what,
        code: ossl_code(&e),
    }
}

/// OpenSSL 3 always has a TLS 1.3 server; older libraries are refused.
pub(crate) fn has_tls13_server() -> bool {
    openssl::version::number() >= OPENSSL_3
}

fn context(server: bool, offer: Offer) -> Result<SslContextBuilder, TlsError> {
    if openssl::version::number() < OPENSSL_3 {
        return Err(TlsError::Policy(
            "the system OpenSSL is older than 3.0".into(),
        ));
    }
    if !offer.tls12 && !offer.tls13 {
        return Err(TlsError::Policy("no TLS version enabled".into()));
    }
    let e = err("SSL_CTX setup");
    let method = if server {
        SslMethod::tls_server()
    } else {
        SslMethod::tls_client()
    };
    let mut b = SslContextBuilder::new(method).map_err(&e)?;
    let min = if offer.tls12 {
        SslVersion::TLS1_2
    } else {
        SslVersion::TLS1_3
    };
    let max = if offer.tls13 {
        SslVersion::TLS1_3
    } else {
        SslVersion::TLS1_2
    };
    b.set_min_proto_version(Some(min)).map_err(&e)?;
    b.set_max_proto_version(Some(max)).map_err(&e)?;
    b.set_ciphersuites(TLS13_SUITES).map_err(&e)?;
    b.set_cipher_list(if offer.cbc_only {
        TLS12_CBC
    } else {
        TLS12_AEAD
    })
    .map_err(&e)?;
    b.set_options(
        SslOptions::NO_COMPRESSION
            | SslOptions::NO_TICKET
            | SslOptions::NO_RENEGOTIATION
            | SslOptions::CIPHER_SERVER_PREFERENCE,
    );
    b.set_session_cache_mode(SslSessionCacheMode::OFF);
    Ok(b)
}

/// Bounds the handshake: a deadline checked before each read and a byte
/// budget; both are lifted once the session is up.
pub(crate) struct Guard<S> {
    inner: S,
    deadline: Option<Instant>,
    budget: Option<usize>,
}

impl<S> Guard<S> {
    fn new(inner: S, deadline: Instant) -> Guard<S> {
        Guard {
            inner,
            deadline: Some(deadline),
            budget: Some(MAX_HANDSHAKE_BYTES),
        }
    }
}

const TOO_LARGE: &str = "TLS handshake input too large";

impl<S: Read> Read for Guard<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = self.inner.read(buf)?;
        if let Some(b) = &mut self.budget {
            *b = b
                .checked_sub(n)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, TOO_LARGE))?;
        }
        Ok(n)
    }
}

impl<S: Write> Write for Guard<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn handshake_error<S>(e: HandshakeError<Guard<S>>) -> TlsError {
    match e {
        HandshakeError::SetupFailure(stack) => err("SSL setup")(stack),
        HandshakeError::WouldBlock(_) => TlsError::Timeout,
        HandshakeError::Failure(mid) => {
            if let Some(io_err) = mid.error().io_error() {
                if io_err.kind() == io::ErrorKind::InvalidData && io_err.to_string() == TOO_LARGE {
                    return TlsError::TooLarge;
                }
                if matches!(
                    io_err.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    return TlsError::Timeout;
                }
            }
            let code = mid
                .error()
                .ssl_error()
                .map_or(i64::from(mid.error().code().as_raw()), ossl_code);
            TlsError::Os {
                what: "SSL handshake",
                code,
            }
        }
    }
}

pub(crate) struct Server {
    ctx: SslContext,
}

impl Server {
    pub(crate) fn new(credential: &Credential, offer: Offer) -> Result<Server, TlsError> {
        let mut b = context(true, offer)?;
        let e = err("SSL_CTX server setup");
        b.set_certificate(&credential.cert).map_err(&e)?;
        b.set_private_key(&credential.key).map_err(&e)?;
        b.check_private_key().map_err(&e)?;
        b.set_num_tickets(0).map_err(&e)?;
        b.set_max_early_data(0).map_err(&e)?;
        b.set_alpn_select_callback(|_, client| {
            select_next_proto(ALPN_WIRE, client).ok_or(AlpnError::ALERT_FATAL)
        });
        Ok(Server { ctx: b.build() })
    }

    pub(crate) fn accept<S: Read + Write>(
        &self,
        io: S,
        deadline: Instant,
    ) -> Result<Stream<S>, TlsError> {
        let ssl = Ssl::new(&self.ctx).map_err(err("SSL_new"))?;
        let s = ssl
            .accept(Guard::new(io, deadline))
            .map_err(handshake_error)?;
        Stream::new(s, true)
    }
}

pub(crate) struct Client {
    ctx: SslContext,
}

impl Client {
    pub(crate) fn new(offer: Offer) -> Result<Client, TlsError> {
        let mut b = context(false, offer)?;
        // No chain or name validation: the caller compares the SPKI pin.
        // OpenSSL still checks the handshake signature with the presented key.
        b.set_verify(SslVerifyMode::NONE);
        b.set_alpn_protos(ALPN_WIRE)
            .map_err(err("SSL_CTX_set_alpn_protos"))?;
        Ok(Client { ctx: b.build() })
    }

    pub(crate) fn connect<S: Read + Write>(
        &self,
        io: S,
        deadline: Instant,
    ) -> Result<Stream<S>, TlsError> {
        let ssl = Ssl::new(&self.ctx).map_err(err("SSL_new"))?;
        let s = ssl
            .connect(Guard::new(io, deadline))
            .map_err(handshake_error)?;
        Stream::new(s, false)
    }
}

pub(crate) struct Stream<S> {
    s: SslStream<Guard<S>>,
    info: SessionInfo,
    peer_cert: Option<Vec<u8>>,
    peer_spki: Option<SpkiSha256>,
}

impl<S: Read + Write> Stream<S> {
    fn new(mut s: SslStream<Guard<S>>, server: bool) -> Result<Stream<S>, TlsError> {
        s.get_mut().deadline = None;
        s.get_mut().budget = None;
        let ssl = s.ssl();
        let version = match ssl.version2() {
            Some(v) if v == SslVersion::TLS1_3 => TlsVersion::Tls13,
            Some(v) if v == SslVersion::TLS1_2 => TlsVersion::Tls12,
            _ => return Err(TlsError::Policy("unexpected protocol version".into())),
        };
        let cipher = ssl
            .current_cipher()
            .ok_or_else(|| TlsError::Policy("no cipher".into()))?;
        let info = SessionInfo {
            version,
            cipher_suite: u16::from_be_bytes(cipher.protocol_id()),
            cipher_name: cipher.standard_name().unwrap_or(cipher.name()).to_string(),
            alpn: ssl.selected_alpn_protocol().map(<[u8]>::to_vec),
        };
        let (peer_cert, peer_spki) = if server {
            (None, None)
        } else {
            let cert = ssl.peer_certificate().ok_or(TlsError::PinMismatch)?;
            let e = err("peer certificate");
            let spki = cert
                .public_key()
                .map_err(&e)?
                .public_key_to_der()
                .map_err(&e)?;
            (
                Some(cert.to_der().map_err(&e)?),
                Some(SpkiSha256(openssl::sha::sha256(&spki))),
            )
        };
        Ok(Stream {
            s,
            info,
            peer_cert,
            peer_spki,
        })
    }

    pub(crate) fn shutdown(&mut self) -> io::Result<()> {
        self.s
            .shutdown()
            .map(|_| ())
            .map_err(|e| e.into_io_error().unwrap_or_else(io::Error::other))
    }
}

impl<S> Stream<S> {
    pub(crate) fn set_read_deadline(&mut self, deadline: Option<Instant>) {
        self.s.get_mut().deadline = deadline;
    }

    pub(crate) fn get_ref(&self) -> &S {
        &self.s.get_ref().inner
    }

    pub(crate) fn get_mut(&mut self) -> &mut S {
        &mut self.s.get_mut().inner
    }

    pub(crate) fn session(&self) -> &SessionInfo {
        &self.info
    }

    pub(crate) fn peer_certificate(&self) -> Option<&[u8]> {
        self.peer_cert.as_deref()
    }

    pub(crate) fn peer_spki_sha256(&self) -> Option<SpkiSha256> {
        self.peer_spki
    }
}

impl<S: Read + Write> Read for Stream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.s.read(buf)?;
        if n == 0 && !buf.is_empty() && !self.s.get_shutdown().contains(ShutdownState::RECEIVED) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(n)
    }
}

impl<S: Read + Write> Write for Stream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.s.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.s.flush()
    }
}
