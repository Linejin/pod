//! CNG: randomness, SHA-256, P-256 ECDSA verification and the in-memory
//! test signer.

use std::ptr;

use windows_sys::Win32::Foundation::STATUS_INVALID_SIGNATURE;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_KEY_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptDestroyKey,
    BCryptGenRandom, BCryptHash, BCryptVerifySignature, CERT_ECC_SIGNATURE, CERT_PUBLIC_KEY_INFO,
    CryptImportPublicKeyInfoEx2, NCRYPT_ECDSA_P256_ALGORITHM, NCRYPT_SILENT_FLAG,
    NCryptCreatePersistedKey, NCryptFinalizeKey, NCryptSignHash, X509_ASN_ENCODING,
    X509_ECC_SIGNATURE, X509_PUBLIC_KEY_INFO,
};

use super::identity::{Key, Provider, key_spki_der};
use super::{blob, blob_bytes, decode, encode, oid_str};
use crate::crypto::CryptoError;

/// id-ecPublicKey.
const OID_EC_PUBLIC_KEY: &[u8] = b"1.2.840.10045.2.1";
/// The DER parameter naming prime256v1 (OID 1.2.840.10045.3.1.7).
const P256_PARAMETER: [u8; 10] = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

pub(crate) fn random(buf: &mut [u8]) -> Result<(), CryptoError> {
    for chunk in buf.chunks_mut(1 << 30) {
        // SAFETY: `chunk` is writable for its length; null algorithm with the
        // system-preferred RNG flag is the documented CSPRNG call.
        let st = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if st != 0 {
            return Err(CryptoError::Os {
                what: "BCryptGenRandom",
                code: i64::from(st),
            });
        }
    }
    Ok(())
}

pub(crate) fn sha256(data: &[u8]) -> Result<[u8; 32], CryptoError> {
    let len = u32::try_from(data.len()).map_err(|_| CryptoError::BadInput("hash input length"))?;
    let mut out = [0u8; 32];
    // SAFETY: input and output buffers are valid for the given lengths; the
    // SHA-256 pseudo-handle needs no open/close.
    let st = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            ptr::null(),
            0,
            data.as_ptr(),
            len,
            out.as_mut_ptr(),
            32,
        )
    };
    if st != 0 {
        return Err(CryptoError::Os {
            what: "BCryptHash",
            code: i64::from(st),
        });
    }
    Ok(out)
}

/// A BCrypt public key handle.
struct PublicKey(BCRYPT_KEY_HANDLE);

impl Drop for PublicKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from CryptImportPublicKeyInfoEx2.
        unsafe { BCryptDestroyKey(self.0) };
    }
}

/// Imports a strict DER P-256 SubjectPublicKeyInfo.
fn import_p256(spki_der: &[u8]) -> Result<PublicKey, CryptoError> {
    let decoded = decode(X509_PUBLIC_KEY_INFO, spki_der).map_err(|_| CryptoError::BadKey)?;
    // SAFETY: X509_PUBLIC_KEY_INFO decodes to CERT_PUBLIC_KEY_INFO.
    let info: &CERT_PUBLIC_KEY_INFO = unsafe { decoded.get() };
    // SAFETY: the decoded structure's pointers point into `decoded`.
    let (oid, param) = unsafe {
        (
            oid_str(info.Algorithm.pszObjId),
            blob_bytes(&info.Algorithm.Parameters),
        )
    };
    let point_len = info.PublicKey.cbData;
    // SAFETY: as above.
    let first = if point_len > 0 {
        unsafe { *info.PublicKey.pbData }
    } else {
        0
    };
    if oid != OID_EC_PUBLIC_KEY
        || param != P256_PARAMETER
        || point_len != 65
        || info.PublicKey.cUnusedBits != 0
        || first != 4
    {
        return Err(CryptoError::BadKey);
    }
    // Canonical DER only: the platform re-encoding must give the input back.
    // SAFETY: `info` is a valid CERT_PUBLIC_KEY_INFO.
    if unsafe { encode(X509_PUBLIC_KEY_INFO, ptr::from_ref(info).cast()) }? != spki_der {
        return Err(CryptoError::BadKey);
    }
    let mut key: BCRYPT_KEY_HANDLE = ptr::null_mut();
    // SAFETY: `info` is valid; `key` receives a handle we destroy.
    let ok =
        unsafe { CryptImportPublicKeyInfoEx2(X509_ASN_ENCODING, info, 0, ptr::null(), &mut key) };
    if ok == 0 || key.is_null() {
        return Err(CryptoError::BadKey);
    }
    Ok(PublicKey(key))
}

/// Strict DER ECDSA-Sig-Value -> the 64-byte big-endian r || s BCrypt uses.
fn raw_signature(der: &[u8]) -> Result<[u8; 64], CryptoError> {
    let decoded = decode(X509_ECC_SIGNATURE, der).map_err(|_| CryptoError::BadSignatureEncoding)?;
    // SAFETY: X509_ECC_SIGNATURE decodes to CERT_ECC_SIGNATURE.
    let sig: &CERT_ECC_SIGNATURE = unsafe { decoded.get() };
    // SAFETY: `sig` is valid; canonical DER only.
    if unsafe { encode(X509_ECC_SIGNATURE, ptr::from_ref(sig).cast()) }? != der {
        return Err(CryptoError::BadSignatureEncoding);
    }
    let mut out = [0u8; 64];
    // SAFETY: the blobs point into `decoded`.
    let (r, s) = unsafe { (blob_bytes(&sig.r), blob_bytes(&sig.s)) };
    let (r_out, s_out) = out.split_at_mut(32);
    for (le, half) in [(r, r_out), (s, s_out)] {
        // Little-endian unsigned; drop high zero bytes, then right-align.
        let mut le = le;
        while le.len() > 32 && le.last() == Some(&0) {
            le = &le[..le.len() - 1];
        }
        if le.len() > 32 {
            return Err(CryptoError::BadSignatureEncoding);
        }
        for (i, b) in le.iter().enumerate() {
            half[31 - i] = *b;
        }
    }
    Ok(out)
}

pub(crate) fn verify_p256_sha256(
    spki_der: &[u8],
    message: &[u8],
    signature_der: &[u8],
) -> Result<(), CryptoError> {
    let key = import_p256(spki_der)?;
    let sig = raw_signature(signature_der)?;
    let hash = sha256(message)?;
    // SAFETY: the key handle is live; hash and signature buffers are valid.
    let st = unsafe {
        BCryptVerifySignature(key.0, ptr::null(), hash.as_ptr(), 32, sig.as_ptr(), 64, 0)
    };
    match st {
        0 => Ok(()),
        STATUS_INVALID_SIGNATURE => Err(CryptoError::BadSignature),
        other => Err(CryptoError::Os {
            what: "BCryptVerifySignature",
            code: i64::from(other),
        }),
    }
}

/// An ephemeral (never persisted) CNG P-256 key.
pub(crate) struct SoftSigner {
    key: Key,
    spki: Vec<u8>,
}

impl SoftSigner {
    pub(crate) fn generate() -> Result<SoftSigner, CryptoError> {
        let prov = Provider::open()?;
        let mut handle = 0usize;
        // SAFETY: a null key name makes an ephemeral key; `handle` receives it.
        let hr = unsafe {
            NCryptCreatePersistedKey(
                prov.0,
                &mut handle,
                NCRYPT_ECDSA_P256_ALGORITHM,
                ptr::null(),
                0,
                0,
            )
        };
        if hr != 0 {
            return Err(CryptoError::Os {
                what: "NCryptCreatePersistedKey",
                code: i64::from(hr),
            });
        }
        let key = Key(handle);
        // SAFETY: a key fresh from NCryptCreatePersistedKey.
        let hr = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
        if hr != 0 {
            return Err(CryptoError::Os {
                what: "NCryptFinalizeKey",
                code: i64::from(hr),
            });
        }
        let spki = key_spki_der(&key)?;
        Ok(SoftSigner { key, spki })
    }

    pub(crate) fn spki_der(&self) -> &[u8] {
        &self.spki
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let hash = sha256(message)?;
        let mut raw = [0u8; 64];
        let mut len = 0u32;
        // SAFETY: the key is live; `raw` holds a P-256 signature (64 bytes).
        let hr = unsafe {
            NCryptSignHash(
                self.key.0,
                ptr::null(),
                hash.as_ptr(),
                32,
                raw.as_mut_ptr(),
                64,
                &mut len,
                NCRYPT_SILENT_FLAG,
            )
        };
        if hr != 0 || len != 64 {
            return Err(CryptoError::Os {
                what: "NCryptSignHash",
                code: i64::from(hr),
            });
        }
        // CryptoAPI integers are little-endian.
        let mut r: Vec<u8> = raw[..32].iter().rev().copied().collect();
        let mut s: Vec<u8> = raw[32..].iter().rev().copied().collect();
        let sig = CERT_ECC_SIGNATURE {
            r: blob(&mut r),
            s: blob(&mut s),
        };
        // SAFETY: `sig` points at `r` and `s`, alive for the call.
        unsafe { encode(X509_ECC_SIGNATURE, ptr::from_ref(&sig).cast()) }
    }
}
