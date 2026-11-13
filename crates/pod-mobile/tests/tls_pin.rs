//! TM-01: no application bytes escape a failed certificate/endpoint gate.
#![cfg(feature = "mobile-lan")]

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, SystemTime};

use common::TestStore;
use pod_mobile::identity::{IdentityParams, ServerIdentity};
use pod_mobile::tls::{PinnedClient, ServerConfig, SpkiSha256, TlsAcceptor, VersionPolicy};

fn attempt(
    id: &ServerIdentity,
    pin: SpkiSha256,
    now: SystemTime,
    wrong_port: bool,
    mapped: bool,
) -> (bool, usize) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let dial = if mapped {
        SocketAddr::new(Ipv4Addr::LOCALHOST.to_ipv6_mapped().into(), address.port())
    } else {
        address
    };
    let client_io = match TcpStream::connect(dial) {
        Ok(io) => io,
        Err(error) if mapped && error.kind() == std::io::ErrorKind::AddrNotAvailable => {
            // Windows may refuse mapped loopback at the TCP layer itself.
            // Also exercise the TLS endpoint gate with a connected IPv4 socket.
            let io = TcpStream::connect(address).unwrap();
            let client = PinnedClient::new(pin, VersionPolicy::platform_default()).unwrap();
            assert!(
                client
                    .connect_endpoint_with_clock(io, dial, || now)
                    .is_err()
            );
            return (false, 0);
        }
        Err(error) => panic!("loopback connect: {error}"),
    };
    let (server_io, _) = listener.accept().unwrap();
    for io in [&client_io, &server_io] {
        io.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        io.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    }
    let policy = VersionPolicy::platform_default();
    let server = ServerConfig::new(id, policy).unwrap();
    let peer = std::thread::spawn(move || {
        let mut received = 0;
        if let Ok(mut stream) = server.accept(server_io) {
            let mut buf = [0; 64];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                received += n;
            }
        }
        received
    });
    let mut expected = dial;
    if wrong_port {
        expected.set_port(if dial.port() == 65535 {
            65534
        } else {
            dial.port() + 1
        });
    }
    let result = PinnedClient::new(pin, policy)
        .unwrap()
        .connect_endpoint_with_clock(client_io, expected, || now);
    let accepted = result.is_ok();
    if let Ok(mut stream) = result {
        stream.write_all(b"secret").unwrap();
    }
    (accepted, peer.join().unwrap())
}

#[test]
fn certificate_and_endpoint_matrix_before_secret() {
    let t = TestStore::new("pin-matrix");
    let now = SystemTime::now();
    let mut params = IdentityParams::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
    params.issued_at = now;
    let id = ServerIdentity::generate(&t.store, &params).unwrap();
    assert_eq!(attempt(&id, id.spki_sha256(), now, false, false), (true, 6));
    for (clock, port, mapped) in [
        (now + Duration::from_secs(91 * 86400), false, false),
        (now - Duration::from_secs(600), false, false),
        (now, true, false),
        (now, false, true),
    ] {
        assert_eq!(
            attempt(&id, id.spki_sha256(), clock, port, mapped),
            (false, 0)
        );
    }
    let other = ServerIdentity::generate(&t.store, &params).unwrap();
    assert_eq!(
        attempt(&other, id.spki_sha256(), now, false, false),
        (false, 0)
    );
    for issued in [
        now - Duration::from_secs(91 * 86400),
        now + Duration::from_secs(600),
    ] {
        let mut outside = params.clone();
        outside.issued_at = issued;
        let outside = ServerIdentity::generate(&t.store, &outside).unwrap();
        assert_eq!(
            attempt(&outside, outside.spki_sha256(), now, false, false),
            (false, 0)
        );
    }
    params.ip = IpAddr::from([127, 0, 0, 2]);
    let other_san = ServerIdentity::generate(&t.store, &params).unwrap();
    assert_eq!(
        attempt(&other_san, other_san.spki_sha256(), now, false, false),
        (false, 0)
    );
    // A matching key/pin does not excuse a forged certificate signature.
    let mut forged = id.cert_der().to_vec();
    *forged.last_mut().unwrap() ^= 1;
    let forged = ServerIdentity::load(&t.store, id.key_name(), &forged).unwrap();
    assert_eq!(
        attempt(&forged, id.spki_sha256(), now, false, false),
        (false, 0)
    );
}
