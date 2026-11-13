//! The TLS seam over loopback TCP (docs/23 section 2, TM-01): the listener
//! presents its identity, the pinned client checks the SPKI before any
//! application data, the version/suite policy holds at both ends, and the
//! listener refuses what the policy does not allow. Only 127.0.0.1.

#![cfg(feature = "mobile-lan")]

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use common::{TestStore, tcp_pair};
use pod_mobile::identity::{IdentityParams, ServerIdentity};
use pod_mobile::tls::{
    ALPN_HTTP11, ClientCiphers, ClientOffer, PinnedClient, ServerConfig, SpkiSha256,
    TLS12_AEAD_SUITES, TLS13_SUITES, TlsAcceptor, TlsConnection, TlsError, TlsStream, TlsVersion,
    VersionPolicy,
};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn identity(t: &TestStore) -> ServerIdentity {
    ServerIdentity::generate(&t.store, &IdentityParams::new(LOOPBACK)).unwrap()
}

/// Runs `accept` on a thread and `connect` here; returns both results.
fn handshake(
    server: ServerConfig,
    client: &PinnedClient,
) -> (
    Result<TlsStream<TcpStream>, TlsError>,
    Result<TlsStream<TcpStream>, TlsError>,
) {
    let (s, c) = tcp_pair();
    let srv = thread::spawn(move || server.accept(s));
    let cli = client.connect(c);
    // A refused client drops its socket, which ends the server's handshake.
    (srv.join().unwrap(), cli)
}

#[test]
fn pinned_handshake_negotiates_the_policy_and_moves_data() {
    let t = TestStore::new("tls-ok");
    let id = identity(&t);
    let policy = VersionPolicy::platform_default();
    let server = ServerConfig::new(&id, policy).unwrap();
    assert_eq!(
        TlsAcceptor::<TcpStream>::spki_sha256(&server),
        id.spki_sha256()
    );
    let client = PinnedClient::new(id.spki_sha256(), policy).unwrap();
    let (s, c) = handshake(server, &client);
    let (mut s, mut c) = (s.unwrap(), c.unwrap());

    for session in [s.session(), c.session()] {
        assert_eq!(session.alpn.as_deref(), Some(ALPN_HTTP11));
        match session.version {
            TlsVersion::Tls13 => {
                assert!(TLS13_SUITES.contains(&session.cipher_suite), "{session:?}")
            }
            TlsVersion::Tls12 => assert!(
                TLS12_AEAD_SUITES.contains(&session.cipher_suite),
                "{session:?}"
            ),
        }
    }
    assert_eq!(s.session(), c.session(), "both ends agree");
    if policy == VersionPolicy::Tls13Only {
        assert_eq!(c.session().version, TlsVersion::Tls13);
    }
    // The client saw the listener certificate; the listener asks for none.
    assert_eq!(c.peer_certificate(), Some(id.cert_der()));
    assert_eq!(c.peer_spki_sha256(), Some(id.spki_sha256()));
    assert_eq!(s.peer_certificate(), None);
    assert_eq!(s.peer_spki_sha256(), None);

    // Small request, then a response spanning several records.
    const REQUEST: &[u8] = b"GET /mobile/v1/auth/challenge HTTP/1.1\r\n\r\n";
    c.write_all(REQUEST).unwrap();
    let mut req = vec![0u8; REQUEST.len()];
    s.read_exact(&mut req).unwrap();
    assert_eq!(req, REQUEST);
    let body: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let writer = {
        let body = body.clone();
        thread::spawn(move || {
            s.write_all(&body).unwrap();
            s.flush().unwrap();
            s
        })
    };
    let mut got = vec![0u8; body.len()];
    c.read_exact(&mut got).unwrap();
    assert_eq!(got, body);
    let mut s = writer.join().unwrap();

    // close_notify: the listener reads a clean end of stream.
    c.shutdown().unwrap();
    let mut rest = Vec::new();
    assert_eq!(s.read_to_end(&mut rest).unwrap(), 0);
}

#[test]
fn wrong_pin_fails_before_any_application_data() {
    let t = TestStore::new("tls-pin");
    let id = identity(&t);
    let policy = VersionPolicy::platform_default();
    let mut wrong = id.spki_sha256();
    wrong.0[0] ^= 0x80;
    let client = PinnedClient::new(wrong, policy).unwrap();
    let (s, c) = handshake(ServerConfig::new(&id, policy).unwrap(), &client);
    assert!(matches!(c, Err(TlsError::PinMismatch)), "{:?}", c.err());
    // The listener finished its side but never receives a byte of data.
    let mut s = s.unwrap();
    let mut buf = [0u8; 64];
    match s.read(&mut buf) {
        Ok(0) | Err(_) => {}
        Ok(n) => panic!("listener received {n} bytes after a pin mismatch"),
    }
}

#[test]
fn a_rotated_key_does_not_satisfy_the_old_pin() {
    let t = TestStore::new("tls-rot");
    let old = identity(&t);
    let new = identity(&t);
    let policy = VersionPolicy::platform_default();
    let old_pin = old.spki_sha256();
    t.store.delete(old.key_name()).unwrap();
    let client = PinnedClient::new(old_pin, policy).unwrap();
    let (_, c) = handshake(ServerConfig::new(&new, policy).unwrap(), &client);
    assert!(matches!(c, Err(TlsError::PinMismatch)));
    let client = PinnedClient::new(new.spki_sha256(), policy).unwrap();
    let (s, c) = handshake(ServerConfig::new(&new, policy).unwrap(), &client);
    assert!(s.is_ok() && c.is_ok());
}

#[test]
fn tls12_fallback_is_ecdhe_ecdsa_aead_only() {
    let t = TestStore::new("tls12");
    let id = identity(&t);
    let only12 = ClientOffer {
        tls12: true,
        tls13: false,
        ciphers: ClientCiphers::Aead,
    };
    let client =
        PinnedClient::with_offer(id.spki_sha256(), VersionPolicy::Tls12Aead, only12).unwrap();
    let (s, c) = handshake(
        ServerConfig::new(&id, VersionPolicy::Tls12Aead).unwrap(),
        &client,
    );
    let (s, c) = (s.unwrap(), c.unwrap());
    assert_eq!(c.session().version, TlsVersion::Tls12);
    assert!(
        TLS12_AEAD_SUITES.contains(&c.session().cipher_suite),
        "{:?}",
        c.session()
    );
    assert_eq!(s.session(), c.session());

    // A client that offers only CBC suites gets no session at all.
    let cbc = ClientOffer {
        tls12: true,
        tls13: false,
        ciphers: ClientCiphers::CbcOnly,
    };
    let client = PinnedClient::with_offer(id.spki_sha256(), VersionPolicy::Tls12Aead, cbc).unwrap();
    let (s, c) = handshake(
        ServerConfig::new(&id, VersionPolicy::Tls12Aead).unwrap(),
        &client,
    );
    assert!(s.is_err() && c.is_err(), "CBC must not negotiate");
}

#[test]
fn tls13_only_listener_refuses_a_tls12_client() {
    let t = TestStore::new("tls13");
    let id = identity(&t);
    if VersionPolicy::platform_default() != VersionPolicy::Tls13Only {
        eprintln!("skipped: this OS has no TLS 1.3 server (Windows 10)");
        return;
    }
    let only12 = ClientOffer {
        tls12: true,
        tls13: false,
        ciphers: ClientCiphers::Aead,
    };
    let client =
        PinnedClient::with_offer(id.spki_sha256(), VersionPolicy::Tls12Aead, only12).unwrap();
    let (s, c) = handshake(
        ServerConfig::new(&id, VersionPolicy::Tls13Only).unwrap(),
        &client,
    );
    assert!(s.is_err() && c.is_err());
}

#[test]
fn plain_http_is_not_a_handshake() {
    let t = TestStore::new("tls-http");
    let id = identity(&t);
    let server = ServerConfig::new(&id, VersionPolicy::platform_default()).unwrap();
    let (s, mut c) = tcp_pair();
    let srv = thread::spawn(move || server.accept(s));
    c.write_all(b"GET /mobile/v1/call HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let r = srv.join().unwrap();
    assert!(
        matches!(r, Err(TlsError::Os { .. } | TlsError::Io(_))),
        "{:?}",
        r.err()
    );
}

#[test]
fn a_silent_client_hits_the_handshake_deadline() {
    let t = TestStore::new("tls-slow");
    let id = identity(&t);
    let server = ServerConfig::new(&id, VersionPolicy::platform_default())
        .unwrap()
        .with_handshake_timeout(Duration::from_millis(300));
    let (s, c) = tcp_pair();
    s.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let start = Instant::now();
    let r = server.accept(s);
    assert!(matches!(r, Err(TlsError::Timeout)), "{:?}", r.err());
    assert!(start.elapsed() < Duration::from_secs(5));
    drop(c);
}

#[test]
fn pins_are_independent_of_the_transport() {
    // The pin is the SHA-256 of the SPKI, not of the certificate: a second
    // certificate for the same key would keep it (rotation makes a new key).
    let t = TestStore::new("tls-spki");
    let id = identity(&t);
    let cert_hash = pod_mobile::crypto::sha256(id.cert_der()).unwrap();
    assert_ne!(SpkiSha256(cert_hash), id.spki_sha256());
}
