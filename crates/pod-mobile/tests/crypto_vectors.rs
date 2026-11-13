//! Platform crypto against fixed vectors (docs/23 sections 2-3): SHA-256
//! (FIPS 180-2 examples), P-256 ECDSA verification (RFC 6979 A.2.5, the
//! SHA-256 cases, cross-checked with .NET's ECDsa), strict DER for keys and
//! signatures, and the in-memory signer round trip.

#![cfg(feature = "mobile-lan")]

mod common;

use common::unhex;
use pod_mobile::crypto::{self, CryptoError, SoftwareSigner};

/// RFC 6979 A.2.5 public key, as a DER SubjectPublicKeyInfo.
const SPKI_PREFIX: &str = "3059301306072a8648ce3d020106082a8648ce3d030107034200";
const UX: &str = "60FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6";
const UY: &str = "7903FE1008B8BC99A41AE9E95628BC64F2F1B20C2D7E9F5177A3C294D4462299";
/// (message, r, s) with SHA-256.
const VECTORS: [(&str, &str, &str); 2] = [
    (
        "sample",
        "EFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716",
        "F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8",
    ),
    (
        "test",
        "F1ABB023518351CD71D881567B1EA663ED3EFCF6C5132B354F28D3B0B7D38367",
        "019F4113742A2B14BD25926B49C649155F267E60D3814B4C0CC84250E46F0083",
    ),
];

fn spki(x: &str, y: &str) -> Vec<u8> {
    unhex(&format!("{SPKI_PREFIX}04{x}{y}"))
}

/// DER INTEGER of an unsigned big-endian value (minimal), for building
/// the vectors' signatures.
fn der_int(v: &[u8]) -> Vec<u8> {
    let mut v = v;
    while v.len() > 1 && v[0] == 0 {
        v = &v[1..];
    }
    let mut out = vec![0x02];
    let pad = v[0] & 0x80 != 0;
    out.push((v.len() + usize::from(pad)) as u8);
    if pad {
        out.push(0);
    }
    out.extend_from_slice(v);
    out
}

fn der_sig(r: &[u8], s: &[u8]) -> Vec<u8> {
    let (r, s) = (der_int(r), der_int(s));
    let mut out = vec![0x30, (r.len() + s.len()) as u8];
    out.extend(r);
    out.extend(s);
    out
}

#[test]
fn sha256_matches_the_fips_examples() {
    let hex = |d: [u8; 32]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert_eq!(
        hex(crypto::sha256(b"abc").unwrap()),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(crypto::sha256(b"").unwrap()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(crypto::sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").unwrap()),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn random_256_draws_fresh_bits() {
    let a = crypto::random_256().unwrap();
    let b = crypto::random_256().unwrap();
    assert_ne!(a, b);
    assert_ne!(a, [0u8; 32]);
}

#[test]
fn rfc6979_p256_sha256_signatures_verify() {
    let key = spki(UX, UY);
    for (msg, r, s) in VECTORS {
        let sig = der_sig(&unhex(r), &unhex(s));
        assert_eq!(
            crypto::verify_p256_sha256(&key, msg.as_bytes(), &sig),
            Ok(()),
            "{msg}"
        );
    }
}

#[test]
fn wrong_message_key_or_signature_does_not_verify() {
    let key = spki(UX, UY);
    let (msg, r, s) = VECTORS[0];
    let (r, s) = (unhex(r), unhex(s));
    let sig = der_sig(&r, &s);
    // Another message, the other vector's signature, r and s swapped, r+1.
    assert_eq!(
        crypto::verify_p256_sha256(&key, b"samplf", &sig),
        Err(CryptoError::BadSignature)
    );
    let other = der_sig(&unhex(VECTORS[1].1), &unhex(VECTORS[1].2));
    assert_eq!(
        crypto::verify_p256_sha256(&key, msg.as_bytes(), &other),
        Err(CryptoError::BadSignature)
    );
    assert_eq!(
        crypto::verify_p256_sha256(&key, msg.as_bytes(), &der_sig(&s, &r)),
        Err(CryptoError::BadSignature)
    );
    let mut r1 = r.clone();
    r1[31] ^= 1;
    assert_eq!(
        crypto::verify_p256_sha256(&key, msg.as_bytes(), &der_sig(&r1, &s)),
        Err(CryptoError::BadSignature)
    );
    // A different (valid) key.
    let signer = SoftwareSigner::generate().unwrap();
    assert_eq!(
        crypto::verify_p256_sha256(signer.spki_der(), msg.as_bytes(), &sig),
        Err(CryptoError::BadSignature)
    );
    // r = 0 never verifies, whichever layer refuses it.
    assert!(crypto::verify_p256_sha256(&key, msg.as_bytes(), &der_sig(&[0], &s)).is_err());
}

#[test]
fn signatures_must_be_strict_der() {
    let key = spki(UX, UY);
    let (msg, r, s) = VECTORS[0];
    let (r, s) = (unhex(r), unhex(s));
    let good = der_sig(&r, &s);
    let mut trailing = good.clone();
    trailing.push(0);
    let mut long_len = vec![0x30, 0x81, good[1]];
    long_len.extend_from_slice(&good[2..]);
    // r with a redundant leading zero byte.
    let mut padded_r = vec![0x02, 0x22, 0x00, 0x00];
    padded_r.extend_from_slice(&r);
    let s_int = der_int(&s);
    let mut redundant = vec![0x30, (padded_r.len() + s_int.len()) as u8];
    redundant.extend(padded_r);
    redundant.extend(s_int);
    let raw: Vec<u8> = r.iter().chain(&s).copied().collect();
    for (what, sig) in [
        ("trailing byte", trailing),
        ("long-form length", long_len),
        ("non-minimal integer", redundant),
        ("raw r||s", raw),
        ("empty", vec![]),
    ] {
        assert_eq!(
            crypto::verify_p256_sha256(&key, msg.as_bytes(), &sig),
            Err(CryptoError::BadSignatureEncoding),
            "{what}"
        );
    }
}

#[test]
fn keys_must_be_uncompressed_p256_spki() {
    let (msg, r, s) = VECTORS[0];
    let sig = der_sig(&unhex(r), &unhex(s));
    let good = spki(UX, UY);
    let mut trailing = good.clone();
    trailing.push(0);
    // Compressed point (Uy is odd -> 03).
    let compressed = unhex(&format!(
        "3039301306072a8648ce3d020106082a8648ce3d030107032200 03{UX}"
    ));
    // The same point labelled as P-384 (secp384r1, 1.3.132.0.34).
    let p384 = unhex(&format!(
        "3056301006072a8648ce3d020106052b81040022034200 04{UX}{UY}"
    ));
    for (what, key) in [
        ("trailing byte", trailing),
        ("compressed point", compressed),
        ("P-384 label", p384),
        ("garbage", vec![1, 2, 3]),
        ("empty", vec![]),
    ] {
        assert_eq!(
            crypto::verify_p256_sha256(&key, msg.as_bytes(), &sig),
            Err(CryptoError::BadKey),
            "{what}"
        );
    }
    // A point off the curve (Uy + 1) is refused at import or at verify.
    let mut y = unhex(UY);
    y[31] ^= 1;
    let off: String = y.iter().map(|b| format!("{b:02x}")).collect();
    assert!(crypto::verify_p256_sha256(&spki(UX, &off), msg.as_bytes(), &sig).is_err());
}

#[test]
fn software_signer_round_trips_through_the_verifier() {
    let signer = SoftwareSigner::generate().unwrap();
    let spki = signer.spki_der();
    assert_eq!(spki.len(), 91);
    assert_eq!(&spki[..26], &unhex(SPKI_PREFIX)[..]);
    let a = signer.sign(b"pod-mobile nonce").unwrap();
    let b = signer.sign(b"pod-mobile nonce").unwrap();
    assert_ne!(a, b, "ECDSA draws a fresh k per signature");
    for sig in [&a, &b] {
        assert_eq!(
            crypto::verify_p256_sha256(spki, b"pod-mobile nonce", sig),
            Ok(())
        );
        assert_eq!(
            crypto::verify_p256_sha256(spki, b"pod-mobile nonc3", sig),
            Err(CryptoError::BadSignature)
        );
    }
}
