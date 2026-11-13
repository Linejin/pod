//! Versioned, atomic TLS generation storage. All errors require mobile off.
use crate::{
    crypto::{self, CryptoError},
    identity::{IdentityParams, KeyStore, ServerIdentity},
    sys::identity::storage,
    tls::{SpkiSha256, VersionPolicy, ct_eq},
};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The sole authoritative file; temporary files are never load candidates.
pub const RECORD_FILE: &str = "generation.bin";
const MAX_RECORD: usize = 32768;

/// Every failure requires the listener to stay off, without fallback.
#[derive(Debug)]
pub enum GenerationError {
    /// Missing, corrupt, insecure, or mismatched record/key.
    NoValidGeneration,
    /// OS storage failure.
    Storage(io::Error),
    /// Platform crypto failure.
    Crypto(CryptoError),
    /// New record committed but retiring the old key failed. No rollback.
    RetirementFailed,
}
impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TLS generation failure (mobile must stay off): {self:?}")
    }
}
impl std::error::Error for GenerationError {}
impl From<io::Error> for GenerationError {
    fn from(e: io::Error) -> Self {
        Self::Storage(e)
    }
}
impl From<CryptoError> for GenerationError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}
type Result<T> = std::result::Result<T, GenerationError>;

/// Public metadata bound to the certificate and named platform key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// Self-signed leaf DER.
    pub certificate_der: Vec<u8>,
    /// SHA-256 of the certificate and stored key's SPKI.
    pub spki_sha256: SpkiSha256,
    /// CNG key name or OpenSSL file stem within the caller's KeyStore.
    pub key_name: String,
    /// Starts at one; checked increment on every rotation.
    pub key_epoch: u64,
    /// Actual bound endpoint; no zero port or scoped/mapped IPv6.
    pub endpoint: SocketAddr,
    /// Unix seconds from the certificate.
    pub not_before: i64,
    /// Unix seconds from the certificate.
    pub not_after: i64,
    /// Permitted negotiated versions/ciphers.
    pub version_policy: VersionPolicy,
}
impl Generation {
    fn new(
        id: &ServerIdentity,
        key_epoch: u64,
        endpoint: SocketAddr,
        version_policy: VersionPolicy,
    ) -> Self {
        Self {
            certificate_der: id.cert_der().to_vec(),
            spki_sha256: id.spki_sha256(),
            key_name: id.key_name().into(),
            key_epoch,
            endpoint,
            not_before: id.info().not_before,
            not_after: id.info().not_after,
            version_policy,
        }
    }
    /// Warn from 14 days before expiry, including after expiry. Invalid clocks warn too.
    pub fn expiry_warning(&self, now: SystemTime) -> bool {
        now.duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .is_none_or(|t| t >= self.not_after.saturating_sub(14 * 86400))
    }
    /// Reopen the named key and prove all certificate/record bindings.
    pub fn identity(&self, keys: &KeyStore) -> Result<ServerIdentity> {
        let id = ServerIdentity::load(keys, &self.key_name, &self.certificate_der)
            .map_err(|_| GenerationError::NoValidGeneration)?;
        let i = id.info();
        if self.key_epoch == 0
            || !valid_endpoint(self.endpoint)
            || !id.spki_sha256().ct_eq(&self.spki_sha256)
            || i.san_ips != [self.endpoint.ip()]
            || i.san_other != 0
            || i.not_before != self.not_before
            || i.not_after != self.not_after
            || i.not_before >= i.not_after
            || !i.self_signed
            || !i.server_auth
            || !i.digital_signature
            || i.ca == Some(true)
        {
            return Err(GenerationError::NoValidGeneration);
        }
        Ok(id)
    }
    /// V1: PodGen01, eight ordered TLVs (u8 tag, u32 BE length, bytes),
    /// SHA-256 of all preceding bytes. No optional/extension fields.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = b"PodGen01".to_vec();
        let mut endpoint = match self.endpoint.ip() {
            IpAddr::V4(ip) => {
                let mut b = vec![4];
                b.extend(ip.octets());
                b
            }
            IpAddr::V6(ip) => {
                let mut b = vec![6];
                b.extend(ip.octets());
                b
            }
        };
        endpoint.extend(self.endpoint.port().to_be_bytes());
        let policy = [match self.version_policy {
            VersionPolicy::Tls13Only => 13,
            VersionPolicy::Tls12Aead => 12,
        }];
        for (tag, b) in [
            (1, self.certificate_der.as_slice()),
            (2, &self.spki_sha256.0),
            (3, self.key_name.as_bytes()),
            (4, &self.key_epoch.to_be_bytes()),
            (5, endpoint.as_slice()),
            (6, &self.not_before.to_be_bytes()),
            (7, &self.not_after.to_be_bytes()),
            (8, &policy),
        ] {
            if b.len() > MAX_RECORD {
                return Err(GenerationError::NoValidGeneration);
            }
            out.push(tag);
            out.extend((b.len() as u32).to_be_bytes());
            out.extend(b);
        }
        if out.len() + 32 > MAX_RECORD {
            return Err(GenerationError::NoValidGeneration);
        }
        out.extend(crypto::sha256(&out)?);
        Ok(out)
    }
    /// Refuse unknown/duplicate/missing fields, lengths, trailing data, bad
    /// checksum, certificate, pin, or stored key. No dependency on serde.
    pub fn decode(bytes: &[u8], keys: &KeyStore) -> Result<Self> {
        let bad = || GenerationError::NoValidGeneration;
        if bytes.len() < 40 || bytes.len() > MAX_RECORD || &bytes[..8] != b"PodGen01" {
            return Err(bad());
        }
        let end = bytes.len() - 32;
        if !ct_eq(&crypto::sha256(&bytes[..end])?, &bytes[end..]) {
            return Err(bad());
        }
        let mut input = &bytes[8..end];
        let mut fields = Vec::new();
        for tag in 1..=8 {
            if input.len() < 5 || input[0] != tag {
                return Err(bad());
            }
            let size = u32::from_be_bytes(input[1..5].try_into().map_err(|_| bad())?) as usize;
            input = &input[5..];
            fields.push(input.get(..size).ok_or_else(bad)?);
            input = &input[size..];
        }
        if !input.is_empty()
            || fields[0].is_empty()
            || fields[0].len() > 16384
            || fields[2].len() > 80
        {
            return Err(bad());
        }
        let endpoint = match fields[4] {
            [4, a, b, c, d, hi, lo] => SocketAddr::new(
                Ipv4Addr::new(*a, *b, *c, *d).into(),
                u16::from_be_bytes([*hi, *lo]),
            ),
            b if b.len() == 19 && b[0] == 6 => SocketAddr::new(
                Ipv6Addr::from(<[u8; 16]>::try_from(&b[1..17]).map_err(|_| bad())?).into(),
                u16::from_be_bytes([b[17], b[18]]),
            ),
            _ => return Err(bad()),
        };
        let g = Self {
            certificate_der: fields[0].to_vec(),
            spki_sha256: SpkiSha256(fields[1].try_into().map_err(|_| bad())?),
            key_name: std::str::from_utf8(fields[2]).map_err(|_| bad())?.into(),
            key_epoch: u64::from_be_bytes(fields[3].try_into().map_err(|_| bad())?),
            endpoint,
            not_before: i64::from_be_bytes(fields[5].try_into().map_err(|_| bad())?),
            not_after: i64::from_be_bytes(fields[6].try_into().map_err(|_| bad())?),
            version_policy: match fields[7] {
                [13] => VersionPolicy::Tls13Only,
                [12] => VersionPolicy::Tls12Aead,
                _ => return Err(bad()),
            },
        };
        g.identity(keys)?;
        Ok(g)
    }
}
fn valid_endpoint(e: SocketAddr) -> bool {
    e.port() != 0
        && match e {
            SocketAddr::V4(_) => true,
            SocketAddr::V6(v) => {
                v.scope_id() == 0 && v.flowinfo() == 0 && v.ip().to_ipv4_mapped().is_none()
            }
        }
}
/// Load only the authoritative file. Missing/torn/insecure records mean off.
pub fn load(dir: &Path, keys: &KeyStore) -> Result<Generation> {
    let _handle = storage::directory(dir, false).map_err(|_| GenerationError::NoValidGeneration)?;
    read_record(dir, keys)
}
fn read_record(dir: &Path, keys: &KeyStore) -> Result<Generation> {
    let f =
        storage::read(&dir.join(RECORD_FILE)).map_err(|_| GenerationError::NoValidGeneration)?;
    let mut bytes = Vec::new();
    f.take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| GenerationError::NoValidGeneration)?;
    Generation::decode(&bytes, keys)
}
struct Temporary {
    path: PathBuf,
    file: Option<File>,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}
/// Deterministic storage failure checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationStage {
    /// Temp bytes written, before flush.
    AfterTempWrite,
    /// Temp file flushed, before replacement.
    Replace,
    /// Complete new record committed, before retiring old key.
    OldKeyDelete,
}
/// Stop the listener first. Fresh key, checked epoch increment, durable
/// commit, then old-key deletion. IP/port changes also require this rotation.
pub fn rotate(
    dir: &Path,
    keys: &KeyStore,
    endpoint: SocketAddr,
    policy: VersionPolicy,
    now: SystemTime,
) -> Result<Generation> {
    rotate_with_hook(dir, keys, endpoint, policy, now, |_| Ok(()))
}
/// Inject clock/storage failures. Post-commit retirement failure keeps the
/// new record and reports off; it never rolls back to an old key.
pub fn rotate_with_hook(
    dir: &Path,
    keys: &KeyStore,
    endpoint: SocketAddr,
    policy: VersionPolicy,
    now: SystemTime,
    mut hook: impl FnMut(RotationStage) -> io::Result<()>,
) -> Result<Generation> {
    if !valid_endpoint(endpoint) {
        return Err(GenerationError::NoValidGeneration);
    }
    let directory = storage::directory(dir, true)?;
    // Cross-process serialization. A crashed writer's lock requires explicit
    // operator recovery; never automatically break another writer's lock.
    let path = dir.join("generation.lock");
    let _lock = Temporary {
        file: Some(storage::create(&path)?),
        path,
    };
    let old = match fs::symlink_metadata(dir.join(RECORD_FILE)) {
        Ok(_) => Some(read_record(dir, keys)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let epoch = old
        .as_ref()
        .map_or(Some(1), |g| g.key_epoch.checked_add(1))
        .ok_or(GenerationError::NoValidGeneration)?;
    let mut params = IdentityParams::new(endpoint.ip());
    params.issued_at = now;
    let identity = ServerIdentity::generate(keys, &params)?;
    let new = Generation::new(&identity, epoch, endpoint, policy);
    drop(identity);
    let mut replacement_attempted = false;
    let mut write = || -> Result<()> {
        new.identity(keys)?;
        let bytes = new.encode()?;
        let path = dir.join(format!(
            "generation-{}.tmp",
            crate::identity::hex(&crypto::random_256()?[..16])
        ));
        let mut temp = Temporary {
            file: Some(storage::create(&path)?),
            path,
        };
        let file = temp.file.as_mut().expect("new temporary file");
        file.write_all(&bytes)?;
        hook(RotationStage::AfterTempWrite)?;
        file.sync_all()?; // FlushFileBuffers / fsync
        temp.file.take();
        hook(RotationStage::Replace)?;
        replacement_attempted = true;
        storage::replace(&temp.path, &dir.join(RECORD_FILE), &directory)?;
        Ok(())
    };
    if let Err(error) = write() {
        // Directory fsync/write-through may fail after rename. Never delete
        // the key referenced by the visible new record.
        let definitely_uncommitted = !replacement_attempted
            || match read_record(dir, keys) {
                Ok(record) => old.as_ref() == Some(&record),
                Err(_) => false, // ambiguous I/O: retain the key, remain off
            };
        if definitely_uncommitted {
            keys.delete(&new.key_name)?;
        }
        return Err(error);
    }
    if let Some(old) = old
        && (hook(RotationStage::OldKeyDelete).is_err() || keys.delete(&old.key_name).is_err())
    {
        return Err(GenerationError::RetirementFailed);
    }
    Ok(new)
}
