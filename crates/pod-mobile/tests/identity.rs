//! The listener identity (docs/23 section 2): certificate shape,
//! generation/rotation in the platform key store, reload, and the store's
//! name confinement. Windows keys go to the current user's software KSP
//! under a test-only prefix and are deleted when each test ends.

#![cfg(feature = "mobile-lan")]

mod common;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::TestStore;
use pod_mobile::crypto::{self, CryptoError};
use pod_mobile::identity::{
    CERT_BACKDATE, CERT_VALIDITY, CertificateInfo, IdentityParams, ServerIdentity,
};

const LAN_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 7, 23));

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn generated_certificate_has_the_docs23_shape() {
    let t = TestStore::new("shape");
    let params = IdentityParams::new(LAN_IP);
    let id = ServerIdentity::generate(&t.store, &params).unwrap();
    let info = id.info();
    assert_eq!(info, &CertificateInfo::parse(id.cert_der()).unwrap());
    assert_eq!(info.san_ips, vec![LAN_IP]);
    assert_eq!(info.san_other, 0, "an IP SAN only, never a DNS name");
    #[cfg(windows)]
    assert_eq!(info.ca, Some(false));
    assert!(
        info.server_auth && info.digital_signature && info.self_signed,
        "{info:?}"
    );
    let issued = unix(params.issued_at);
    assert_eq!(info.not_before, issued - CERT_BACKDATE.as_secs() as i64);
    assert_eq!(info.not_after, issued + CERT_VALIDITY.as_secs() as i64);
    assert_eq!(CERT_VALIDITY, Duration::from_secs(90 * 86_400));
    // An uncompressed P-256 SubjectPublicKeyInfo; the pin is its SHA-256.
    assert_eq!(info.spki_der.len(), 91);
    assert_eq!(
        &info.spki_der[..26],
        &common::unhex("3059301306072a8648ce3d020106082a8648ce3d030107034200")[..]
    );
    assert_eq!(
        id.spki_sha256(),
        crypto::spki_sha256(&info.spki_der).unwrap()
    );
    assert_eq!(id.spki_sha256(), info.spki_sha256().unwrap());
    assert!(id.key_name().starts_with("PodMobileTest-shape-"));
    // Random serials: at least 64 bits and different per certificate.
    let other = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    assert!(info.serial.len() >= 8, "serial {:?}", info.serial);
    assert_ne!(info.serial, other.info().serial);
    assert_ne!(id.spki_sha256(), other.spki_sha256());
}

#[test]
fn ipv6_listener_address_is_the_one_san() {
    let t = TestStore::new("v6");
    let ula = IpAddr::V6(Ipv6Addr::new(0xfd12, 0x3456, 0x789a, 1, 0, 0, 0, 0x23));
    let id = ServerIdentity::generate(&t.store, &IdentityParams::new(ula)).unwrap();
    assert_eq!(id.info().san_ips, vec![ula]);
}

#[test]
fn stored_identity_reloads_with_the_same_pin() {
    let t = TestStore::new("reload");
    let id = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    let (name, der, pin) = (
        id.key_name().to_string(),
        id.cert_der().to_vec(),
        id.spki_sha256(),
    );
    drop(id);
    let again = ServerIdentity::load(&t.store, &name, &der).unwrap();
    assert_eq!(again.spki_sha256(), pin);
    assert_eq!(t.store.keys().unwrap(), vec![name]);
}

#[test]
fn rotation_makes_a_new_key_and_retires_the_old_one() {
    let t = TestStore::new("rotate");
    let old = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    let new = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    assert_ne!(old.spki_sha256(), new.spki_sha256());
    let mut both = vec![old.key_name().to_string(), new.key_name().to_string()];
    both.sort();
    assert_eq!(t.store.keys().unwrap(), both);
    // The caller stored the new generation; now the old key goes.
    assert_eq!(t.store.delete(old.key_name()), Ok(true));
    assert_eq!(t.store.delete(old.key_name()), Ok(false));
    assert_eq!(
        ServerIdentity::load(&t.store, old.key_name(), old.cert_der()).unwrap_err(),
        CryptoError::NotFound
    );
    assert!(ServerIdentity::load(&t.store, new.key_name(), new.cert_der()).is_ok());
    assert_eq!(t.store.keys().unwrap(), vec![new.key_name().to_string()]);
}

#[test]
fn load_refuses_a_certificate_of_another_key() {
    let t = TestStore::new("mismatch");
    let a = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    let b = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    assert_eq!(
        ServerIdentity::load(&t.store, a.key_name(), b.cert_der()).unwrap_err(),
        CryptoError::Mismatch("key and certificate")
    );
    assert_eq!(
        ServerIdentity::load(&t.store, a.key_name(), b"not a certificate").unwrap_err(),
        CryptoError::BadInput("certificate")
    );
}

#[test]
fn stores_only_touch_names_with_their_prefix() {
    let t = TestStore::new("names");
    let id = ServerIdentity::generate(&t.store, &IdentityParams::new(LAN_IP)).unwrap();
    for bad in ["Other-0011", "../x", "", "PodMobileTest-names-..\\x"] {
        assert_eq!(
            t.store.delete(bad),
            Err(CryptoError::BadInput("key name")),
            "{bad:?}"
        );
        assert!(
            ServerIdentity::load(&t.store, bad, id.cert_der()).is_err(),
            "{bad:?}"
        );
    }
    #[cfg(windows)]
    for bad in ["", "a/b", "with space", &"x".repeat(41)] {
        let r = pod_mobile::identity::KeyStore::current_user(bad);
        assert!(matches!(r, Err(CryptoError::BadInput(_))), "{bad:?}");
    }
    // Another store's prefix never lists or deletes this key.
    let other = TestStore::new("names2");
    assert!(other.store.keys().unwrap().is_empty());
    assert_eq!(
        other.store.delete(id.key_name()),
        Err(CryptoError::BadInput("key name"))
    );
}

#[test]
fn bad_validity_windows_are_refused() {
    let t = TestStore::new("window");
    let mut zero = IdentityParams::new(LAN_IP);
    zero.validity = Duration::ZERO;
    assert_eq!(
        ServerIdentity::generate(&t.store, &zero).unwrap_err(),
        CryptoError::BadInput("validity")
    );
    let mut early = IdentityParams::new(LAN_IP);
    early.issued_at = UNIX_EPOCH - Duration::from_secs(1);
    assert!(ServerIdentity::generate(&t.store, &early).is_err());
    assert!(t.store.keys().unwrap().is_empty(), "no key left behind");
}
