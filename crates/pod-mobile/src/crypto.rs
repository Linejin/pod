//! Platform crypto for M2 (`mobile-lan`): CNG (BCrypt/NCrypt) on Windows,
//! the system OpenSSL 3 on Linux. Nothing here implements a primitive or
//! parses ASN.1 by hand; DER is read and written by the platform.

use std::fmt;

use crate::sys::crypto as imp;
use crate::tls::SpkiSha256;

/// Why a crypto operation failed. Messages carry no key or input bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// The platform call failed (`code`: NTSTATUS/HRESULT/Win32 error on
    /// Windows, the OpenSSL error code on Linux).
    Os {
        /// The call that failed.
        what: &'static str,
        /// The platform's code.
        code: i64,
    },
    /// The public key is not a well-formed P-256 SubjectPublicKeyInfo.
    BadKey,
    /// The signature is not a strict DER ECDSA-Sig-Value for P-256.
    BadSignatureEncoding,
    /// The signature does not verify.
    BadSignature,
    /// An input outside the accepted shape (named).
    BadInput(&'static str),
    /// A stored key and certificate (or similar pair) do not belong
    /// together.
    Mismatch(&'static str),
    /// The named key does not exist in the store.
    NotFound,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::Os { what, code } => write!(f, "{what} failed (0x{:08x})", *code as u32),
            CryptoError::BadKey => f.write_str("not a P-256 public key"),
            CryptoError::BadSignatureEncoding => f.write_str("not a DER P-256 ECDSA signature"),
            CryptoError::BadSignature => f.write_str("signature does not verify"),
            CryptoError::BadInput(what) => write!(f, "invalid {what}"),
            CryptoError::Mismatch(what) => write!(f, "{what} do not match"),
            CryptoError::NotFound => f.write_str("key not found"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Fills `buf` from the OS CSPRNG (`BCryptGenRandom` with the system
/// preferred RNG; OpenSSL `RAND_bytes` on Linux).
pub fn random_bytes(buf: &mut [u8]) -> Result<(), CryptoError> {
    imp::random(buf)
}

/// 256 random bits: pairing secrets, nonces, bearer tokens (docs/23 3).
pub fn random_256() -> Result<[u8; 32], CryptoError> {
    let mut out = [0u8; 32];
    random_bytes(&mut out)?;
    Ok(out)
}

/// SHA-256 (`BCryptHash`; OpenSSL on Linux).
pub fn sha256(data: &[u8]) -> Result<[u8; 32], CryptoError> {
    imp::sha256(data)
}

/// The QR pin of a DER SubjectPublicKeyInfo.
pub fn spki_sha256(spki_der: &[u8]) -> Result<SpkiSha256, CryptoError> {
    sha256(spki_der).map(SpkiSha256)
}

/// Verifies an ECDSA P-256 / SHA-256 signature over `message` (docs/23 3:
/// device keys sign the nonce challenges). `spki_der` is the device's DER
/// SubjectPublicKeyInfo (what Android's `PublicKey.getEncoded()` returns);
/// `signature_der` is the DER `ECDSA-Sig-Value` (what
/// `Signature("SHA256withECDSA")` returns).
///
/// Strict: the key must be an uncompressed point on P-256 with the named
/// curve parameter, and both inputs must be canonical DER without trailing
/// bytes. `Ok(())` means the signature verifies;
/// [`CryptoError::BadSignature`] that it does not.
pub fn verify_p256_sha256(
    spki_der: &[u8],
    message: &[u8],
    signature_der: &[u8],
) -> Result<(), CryptoError> {
    imp::verify_p256_sha256(spki_der, message, signature_der)
}

/// A P-256 signing key in process memory, never stored. For tests and the
/// desktop double of a device; real devices keep their key in Android
/// Keystore.
pub struct SoftwareSigner(imp::SoftSigner);

impl SoftwareSigner {
    /// A fresh key from the platform.
    pub fn generate() -> Result<SoftwareSigner, CryptoError> {
        imp::SoftSigner::generate().map(SoftwareSigner)
    }

    /// DER SubjectPublicKeyInfo of the key.
    pub fn spki_der(&self) -> &[u8] {
        self.0.spki_der()
    }

    /// DER ECDSA-Sig-Value over SHA-256 of `message`.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.0.sign(message)
    }
}
