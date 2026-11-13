//! OpenSSL: randomness, SHA-256, P-256 ECDSA verification and the
//! in-memory test signer.

use openssl::ec::{EcGroup, EcKey};
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private, Public};
use openssl::sign::{Signer, Verifier};

use super::ossl;
use crate::crypto::CryptoError;

/// Length of the DER SubjectPublicKeyInfo of an uncompressed P-256 point.
const P256_SPKI_LEN: usize = 91;

pub(crate) fn random(buf: &mut [u8]) -> Result<(), CryptoError> {
    openssl::rand::rand_bytes(buf).map_err(ossl("RAND_bytes"))
}

pub(crate) fn sha256(data: &[u8]) -> Result<[u8; 32], CryptoError> {
    Ok(openssl::sha::sha256(data))
}

/// A strict P-256 SubjectPublicKeyInfo: named curve prime256v1, a point on
/// the curve, uncompressed, canonical DER.
fn import_p256(spki_der: &[u8]) -> Result<PKey<Public>, CryptoError> {
    let key = PKey::public_key_from_der(spki_der).map_err(|_| CryptoError::BadKey)?;
    let ec = key.ec_key().map_err(|_| CryptoError::BadKey)?;
    if ec.group().curve_name() != Some(Nid::X9_62_PRIME256V1) {
        return Err(CryptoError::BadKey);
    }
    ec.check_key().map_err(|_| CryptoError::BadKey)?;
    if spki_der.len() != P256_SPKI_LEN
        || key.public_key_to_der().map_err(|_| CryptoError::BadKey)? != spki_der
    {
        return Err(CryptoError::BadKey);
    }
    Ok(key)
}

pub(crate) fn verify_p256_sha256(
    spki_der: &[u8],
    message: &[u8],
    signature_der: &[u8],
) -> Result<(), CryptoError> {
    let key = import_p256(spki_der)?;
    let sig = EcdsaSig::from_der(signature_der).map_err(|_| CryptoError::BadSignatureEncoding)?;
    if sig
        .to_der()
        .map_err(|_| CryptoError::BadSignatureEncoding)?
        != signature_der
    {
        return Err(CryptoError::BadSignatureEncoding);
    }
    let mut v =
        Verifier::new(MessageDigest::sha256(), &key).map_err(ossl("EVP_DigestVerifyInit"))?;
    v.update(message).map_err(ossl("EVP_DigestVerifyUpdate"))?;
    match v.verify(signature_der) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(CryptoError::BadSignature),
    }
}

pub(crate) fn generate_p256() -> Result<PKey<Private>, CryptoError> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)
        .map_err(ossl("EC_GROUP_new_by_curve_name"))?;
    let ec = EcKey::generate(&group).map_err(ossl("EC_KEY_generate_key"))?;
    PKey::from_ec_key(ec).map_err(ossl("EVP_PKEY_assign"))
}

/// An in-memory P-256 key.
pub(crate) struct SoftSigner {
    key: PKey<Private>,
    spki: Vec<u8>,
}

impl SoftSigner {
    pub(crate) fn generate() -> Result<SoftSigner, CryptoError> {
        let key = generate_p256()?;
        let spki = key.public_key_to_der().map_err(ossl("i2d_PUBKEY"))?;
        Ok(SoftSigner { key, spki })
    }

    pub(crate) fn spki_der(&self) -> &[u8] {
        &self.spki
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut s =
            Signer::new(MessageDigest::sha256(), &self.key).map_err(ossl("EVP_DigestSignInit"))?;
        s.update(message).map_err(ossl("EVP_DigestSignUpdate"))?;
        s.sign_to_vec().map_err(ossl("EVP_DigestSignFinal"))
    }
}
