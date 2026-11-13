//! The listener's identity (`mobile-lan`): a P-256 key and a self-signed
//! leaf certificate (docs/23 section 2).
//!
//! - Windows: the key is a persisted, non-exportable CNG key of the current
//!   user in the Microsoft Software Key Storage Provider, protected by the
//!   user's DPAPI. SChannel signs with it by name. No certificate store is
//!   touched: the certificate DER is the caller's to keep.
//! - Linux: the key is a PKCS#8 file (0600) in a directory the caller
//!   chooses (0700), written by OpenSSL. Runtime unverified in M2.
//!
//! The generation record (certificate DER, pin, key name, epoch) and its
//! atomic storage belong to the caller (card P1). Rotation is: generate a new
//! identity, store the new record, then [`KeyStore::delete`] the old key.

use std::fmt;
use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use crate::crypto::{self, CryptoError};
use crate::sys::identity as imp;
use crate::tls::SpkiSha256;

/// docs/23 section 2: certificates are valid for 90 days ...
pub const CERT_VALIDITY: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// ... starting 5 minutes before issuance (clock skew).
pub const CERT_BACKDATE: Duration = Duration::from_secs(5 * 60);

/// What a new certificate says.
#[derive(Clone, Debug)]
pub struct IdentityParams {
    /// The one listener address: the only SAN entry (an IP, never a name).
    pub ip: IpAddr,
    /// Issuance time; `notBefore` is this minus `backdate`.
    pub issued_at: SystemTime,
    /// `notAfter` is `issued_at` plus this.
    pub validity: Duration,
    /// How far `notBefore` precedes `issued_at`.
    pub backdate: Duration,
}

impl IdentityParams {
    /// The docs/23 defaults for `ip`, issued now.
    pub fn new(ip: IpAddr) -> IdentityParams {
        IdentityParams {
            ip,
            issued_at: SystemTime::now(),
            validity: CERT_VALIDITY,
            backdate: CERT_BACKDATE,
        }
    }

    pub(crate) fn window(&self) -> Result<(i64, i64), CryptoError> {
        let secs = |t: SystemTime| -> Result<i64, CryptoError> {
            let d = t
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|_| CryptoError::BadInput("issue time"))?;
            i64::try_from(d.as_secs()).map_err(|_| CryptoError::BadInput("issue time"))
        };
        let issued = secs(self.issued_at)?;
        let back = i64::try_from(self.backdate.as_secs())
            .map_err(|_| CryptoError::BadInput("backdate"))?;
        let valid = i64::try_from(self.validity.as_secs())
            .map_err(|_| CryptoError::BadInput("validity"))?;
        if valid == 0 {
            return Err(CryptoError::BadInput("validity"));
        }
        let not_before = issued
            .checked_sub(back)
            .ok_or(CryptoError::BadInput("backdate"))?;
        let not_after = issued
            .checked_add(valid)
            .ok_or(CryptoError::BadInput("validity"))?;
        Ok((not_before, not_after))
    }
}

/// Where listener keys live. Each store names its keys `<prefix><32 hex>`
/// and only lists and deletes names with its prefix.
pub struct KeyStore {
    pub(crate) inner: imp::Store,
    pub(crate) prefix: String,
}

impl KeyStore {
    /// The current user's Microsoft Software Key Storage Provider (per-user
    /// keys, DPAPI-protected). `prefix`: 1-40 of `A-Z a-z 0-9 - _`.
    #[cfg(windows)]
    pub fn current_user(prefix: &str) -> Result<KeyStore, CryptoError> {
        check_prefix(prefix)?;
        Ok(KeyStore {
            inner: imp::Store::current_user()?,
            prefix: prefix.to_string(),
        })
    }

    /// PKCS#8 files in `dir` (created 0700 if missing; key files 0600).
    #[cfg(target_os = "linux")]
    pub fn directory(dir: &std::path::Path, prefix: &str) -> Result<KeyStore, CryptoError> {
        check_prefix(prefix)?;
        Ok(KeyStore {
            inner: imp::Store::directory(dir)?,
            prefix: prefix.to_string(),
        })
    }

    /// Names of this store's keys (orphans after a crash show up here).
    pub fn keys(&self) -> Result<Vec<String>, CryptoError> {
        let mut names: Vec<String> = self
            .inner
            .list()?
            .into_iter()
            .filter(|n| n.starts_with(&self.prefix) && valid_name(n))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Deletes a key of this store; `Ok(false)` if it did not exist.
    pub fn delete(&self, key_name: &str) -> Result<bool, CryptoError> {
        self.check_name(key_name)?;
        self.inner.delete(key_name)
    }

    fn check_name(&self, key_name: &str) -> Result<(), CryptoError> {
        if key_name.starts_with(&self.prefix) && valid_name(key_name) {
            Ok(())
        } else {
            Err(CryptoError::BadInput("key name"))
        }
    }
}

fn check_prefix(prefix: &str) -> Result<(), CryptoError> {
    if (1..=40).contains(&prefix.len()) && valid_name(prefix) {
        Ok(())
    } else {
        Err(CryptoError::BadInput("key store prefix"))
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The listener's key and certificate, ready for
/// [`crate::tls::ServerConfig::new`].
pub struct ServerIdentity {
    key_name: String,
    cert_der: Vec<u8>,
    spki: SpkiSha256,
    info: CertificateInfo,
    credential: imp::Credential,
}

impl ServerIdentity {
    /// Creates a fresh key in `store` and a self-signed certificate for
    /// `params`: SAN = the one IP, EKU serverAuth, CA=false (critical),
    /// key usage digitalSignature (critical), ECDSA-SHA256, random serial.
    /// If the certificate step fails the new key is deleted again.
    pub fn generate(
        store: &KeyStore,
        params: &IdentityParams,
    ) -> Result<ServerIdentity, CryptoError> {
        let (not_before, not_after) = params.window()?;
        let mut id = [0u8; 16];
        crypto::random_bytes(&mut id)?;
        let key_name = format!("{}{}", store.prefix, hex(&id));
        let key = store.inner.create(&key_name)?;
        match imp::self_signed(&key, params.ip, not_before, not_after) {
            Ok(der) => ServerIdentity::bind(&store.inner, key_name, key, der),
            Err(e) => {
                drop(key);
                let _ = store.inner.delete(&key_name);
                Err(e)
            }
        }
    }

    /// Reopens a stored identity: the key named `key_name` must exist in
    /// `store` and match the certificate's public key.
    pub fn load(
        store: &KeyStore,
        key_name: &str,
        cert_der: &[u8],
    ) -> Result<ServerIdentity, CryptoError> {
        store.check_name(key_name)?;
        let key = store.inner.open(key_name)?.ok_or(CryptoError::NotFound)?;
        ServerIdentity::bind(&store.inner, key_name.to_string(), key, cert_der.to_vec())
    }

    fn bind(
        store: &imp::Store,
        key_name: String,
        key: imp::Key,
        cert_der: Vec<u8>,
    ) -> Result<ServerIdentity, CryptoError> {
        let info = CertificateInfo::parse(&cert_der)?;
        if imp::key_spki_der(&key)? != info.spki_der {
            return Err(CryptoError::Mismatch("key and certificate"));
        }
        let spki = crypto::spki_sha256(&info.spki_der)?;
        let credential = imp::Credential::new(store, &key_name, key, &cert_der)?;
        Ok(ServerIdentity {
            key_name,
            cert_der,
            spki,
            info,
            credential,
        })
    }

    /// The key's name in its store.
    pub fn key_name(&self) -> &str {
        &self.key_name
    }

    /// The certificate (DER).
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// The QR pin.
    pub fn spki_sha256(&self) -> SpkiSha256 {
        self.spki
    }

    /// The parsed certificate.
    pub fn info(&self) -> &CertificateInfo {
        &self.info
    }

    pub(crate) fn credential(&self) -> &imp::Credential {
        &self.credential
    }
}

impl fmt::Debug for ServerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerIdentity")
            .field("key_name", &self.key_name)
            .field("spki", &self.spki)
            .finish()
    }
}

/// What a certificate says, read by the platform decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    /// Serial number, big-endian without sign padding.
    pub serial: Vec<u8>,
    /// `notBefore`, Unix seconds.
    pub not_before: i64,
    /// `notAfter`, Unix seconds.
    pub not_after: i64,
    /// IP addresses in the SubjectAltName.
    pub san_ips: Vec<IpAddr>,
    /// SubjectAltName entries that are not IP addresses.
    pub san_other: usize,
    /// BasicConstraints cA. `None` where the platform does not expose it
    /// (Linux: the openssl crate has no accessor).
    pub ca: Option<bool>,
    /// The ExtendedKeyUsage lists serverAuth.
    pub server_auth: bool,
    /// The KeyUsage has digitalSignature.
    pub digital_signature: bool,
    /// The signature verifies under the certificate's own key and the
    /// subject equals the issuer.
    pub self_signed: bool,
    /// DER SubjectPublicKeyInfo.
    pub spki_der: Vec<u8>,
}

impl CertificateInfo {
    /// Decodes a DER certificate with the platform decoder.
    pub fn parse(der: &[u8]) -> Result<CertificateInfo, CryptoError> {
        imp::parse_certificate(der)
    }

    /// The QR pin of this certificate.
    pub fn spki_sha256(&self) -> Result<SpkiSha256, CryptoError> {
        crypto::spki_sha256(&self.spki_der)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
