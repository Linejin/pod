//! Listener keys as PKCS#8 files and the self-signed certificate, built and
//! read by OpenSSL.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::x509::extension::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
};
use openssl::x509::{X509, X509Builder, X509ExtendedKeyUsage, X509KeyUsage, X509NameBuilder};

use super::crypto::generate_p256;
use super::ossl;
pub(crate) use super::storage;
use crate::crypto::CryptoError;
use crate::identity::CertificateInfo;

// Linux uid_t is unsigned int. No dependency is needed for this libc call.
unsafe extern "C" {
    fn geteuid() -> u32;
}

pub(super) fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or preconditions.
    unsafe { geteuid() }
}

// Linux O_NOFOLLOW: reject a symlink at the key path on open. The value is
// per architecture (asm-generic vs arm/arm64); a wrong one would silently mean
// another flag, so unknown architectures do not build.
#[cfg(any(target_arch = "x86_64", target_arch = "x86", target_arch = "riscv64"))]
pub(super) const O_NOFOLLOW: i32 = 0o400000;
#[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
pub(super) const O_NOFOLLOW: i32 = 0o100000;
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "riscv64",
    target_arch = "aarch64",
    target_arch = "arm"
)))]
compile_error!("pod-mobile: add this architecture's O_NOFOLLOW value");

/// The subject and issuer common name of the listener certificate.
const SUBJECT_CN: &str = "Pod mobile";

fn io_err(what: &'static str, e: &io::Error) -> CryptoError {
    CryptoError::Os {
        what,
        code: i64::from(e.raw_os_error().unwrap_or(0)),
    }
}

/// A private key loaded from (or written to) the store.
pub(crate) struct Key(pub(crate) PKey<Private>);

/// A directory of PKCS#8 PEM files, one per key, readable by the owner only.
pub(crate) struct Store {
    dir: PathBuf,
}

impl Store {
    pub(crate) fn directory(dir: &Path) -> Result<Store, CryptoError> {
        if !dir.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| io_err("mkdir", &e))?;
        }
        let meta = fs::symlink_metadata(dir).map_err(|e| io_err("stat", &e))?;
        if !meta.is_dir()
            || meta.uid() != effective_uid()
            || meta.permissions().mode() & 0o7777 != 0o700
        {
            return Err(CryptoError::BadInput(
                "key directory (a 0700 directory, not a link)",
            ));
        }
        Ok(Store {
            dir: dir.to_path_buf(),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.p8"))
    }

    pub(crate) fn create(&self, name: &str) -> Result<Key, CryptoError> {
        let key = generate_p256()?;
        let pem = key
            .private_key_to_pem_pkcs8()
            .map_err(ossl("PEM_write_bio_PKCS8PrivateKey"))?;
        let path = self.path(name);
        let write = || -> io::Result<()> {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(&pem)?;
            f.sync_all()
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&path);
            return Err(io_err("write key", &e));
        }
        Ok(Key(key))
    }

    pub(crate) fn open(&self, name: &str) -> Result<Option<Key>, CryptoError> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(self.path(name))
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("open key", &e)),
        };
        let meta = file.metadata().map_err(|e| io_err("stat key", &e))?;
        if !meta.is_file()
            || meta.uid() != effective_uid()
            || meta.permissions().mode() & 0o7777 != 0o600
        {
            return Err(CryptoError::BadInput("key file owner or mode"));
        }
        let mut pem = Vec::new();
        file.read_to_end(&mut pem)
            .map_err(|e| io_err("read key", &e))?;
        PKey::private_key_from_pem(&pem)
            .map(|k| Some(Key(k)))
            .map_err(|_| CryptoError::BadInput("stored key"))
    }

    pub(crate) fn delete(&self, name: &str) -> Result<bool, CryptoError> {
        match fs::remove_file(self.path(name)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_err("remove key", &e)),
        }
    }

    pub(crate) fn list(&self) -> Result<Vec<String>, CryptoError> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(|e| io_err("read_dir", &e))? {
            let entry = entry.map_err(|e| io_err("read_dir", &e))?;
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".p8"))
            {
                out.push(name.to_string());
            }
        }
        Ok(out)
    }
}

/// The certificate and key OpenSSL serves with.
pub(crate) struct Credential {
    pub(crate) cert: X509,
    pub(crate) key: PKey<Private>,
}

impl Credential {
    pub(crate) fn new(
        _store: &Store,
        _key_name: &str,
        key: Key,
        cert_der: &[u8],
    ) -> Result<Credential, CryptoError> {
        let cert = X509::from_der(cert_der).map_err(|_| CryptoError::BadInput("certificate"))?;
        Ok(Credential { cert, key: key.0 })
    }
}

pub(crate) fn key_spki_der(key: &Key) -> Result<Vec<u8>, CryptoError> {
    key.0.public_key_to_der().map_err(ossl("i2d_PUBKEY"))
}

// `time_t` is i64 on x86_64 Linux (a no-op conversion there) but 32 bits on
// some targets.
#[allow(clippy::useless_conversion)]
fn asn1_time(unix: i64) -> Result<Asn1Time, CryptoError> {
    let t = unix
        .try_into()
        .map_err(|_| CryptoError::BadInput("certificate time"))?;
    Asn1Time::from_unix(t).map_err(ossl("ASN1_TIME_set"))
}

pub(crate) fn self_signed(
    key: &Key,
    ip: IpAddr,
    not_before: i64,
    not_after: i64,
) -> Result<Vec<u8>, CryptoError> {
    let e = ossl("X509 build");
    let mut b = X509Builder::new().map_err(&e)?;
    b.set_version(2).map_err(&e)?;
    let mut serial = BigNum::new().map_err(&e)?;
    serial.rand(127, MsbOption::MAYBE_ZERO, false).map_err(&e)?;
    // Bound first: `&expr?` does not deref-coerce to the *Ref parameter type.
    let serial = serial.to_asn1_integer().map_err(&e)?;
    b.set_serial_number(&serial).map_err(&e)?;
    let mut name = X509NameBuilder::new().map_err(&e)?;
    name.append_entry_by_nid(Nid::COMMONNAME, SUBJECT_CN)
        .map_err(&e)?;
    let name = name.build();
    b.set_subject_name(&name).map_err(&e)?;
    b.set_issuer_name(&name).map_err(&e)?;
    b.set_pubkey(&key.0).map_err(&e)?;
    let (not_before, not_after) = (asn1_time(not_before)?, asn1_time(not_after)?);
    b.set_not_before(&not_before).map_err(&e)?;
    b.set_not_after(&not_after).map_err(&e)?;
    b.append_extension(BasicConstraints::new().critical().build().map_err(&e)?)
        .map_err(&e)?;
    b.append_extension(
        KeyUsage::new()
            .critical()
            .digital_signature()
            .build()
            .map_err(&e)?,
    )
    .map_err(&e)?;
    b.append_extension(ExtendedKeyUsage::new().server_auth().build().map_err(&e)?)
        .map_err(&e)?;
    let san = SubjectAlternativeName::new()
        .ip(&ip.to_string())
        .build(&b.x509v3_context(None, None))
        .map_err(&e)?;
    b.append_extension(san).map_err(&e)?;
    b.sign(&key.0, MessageDigest::sha256()).map_err(&e)?;
    b.build().to_der().map_err(&e)
}

pub(crate) fn parse_certificate(der: &[u8]) -> Result<CertificateInfo, CryptoError> {
    let e = ossl("X509 read");
    let x = X509::from_der(der).map_err(|_| CryptoError::BadInput("certificate"))?;
    if x.to_der().map_err(&e)? != der {
        return Err(CryptoError::BadInput("certificate"));
    }
    let mut serial = x.serial_number().to_bn().map_err(&e)?.to_vec();
    if serial.is_empty() {
        serial.push(0);
    }
    let epoch = Asn1Time::from_unix(0).map_err(&e)?;
    let secs = |t: &openssl::asn1::Asn1TimeRef| -> Result<i64, CryptoError> {
        let d = epoch.diff(t).map_err(&e)?;
        Ok(i64::from(d.days) * 86_400 + i64::from(d.secs))
    };
    let mut san_ips = Vec::new();
    let mut san_other = 0;
    if let Some(names) = x.subject_alt_names() {
        for n in &names {
            match n.ipaddress() {
                Some(b) if b.len() == 4 => {
                    san_ips.push(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3])))
                }
                Some(b) if b.len() == 16 => match <[u8; 16]>::try_from(b) {
                    Ok(o) => san_ips.push(IpAddr::V6(Ipv6Addr::from(o))),
                    Err(_) => san_other += 1,
                },
                _ => san_other += 1,
            }
        }
    }
    let public = x.public_key().map_err(&e)?;
    let same_name =
        x.subject_name().to_der().map_err(&e)? == x.issuer_name().to_der().map_err(&e)?;
    Ok(CertificateInfo {
        serial,
        not_before: secs(x.not_before())?,
        not_after: secs(x.not_after())?,
        san_ips,
        san_other,
        ca: None,
        server_auth: x
            .extended_key_usage()
            .is_some_and(|u| u.contains(X509ExtendedKeyUsage::SSL_SERVER)),
        digital_signature: x
            .key_usage()
            .is_some_and(|u| u.contains(X509KeyUsage::DIGITAL_SIGNATURE)),
        self_signed: same_name && x.verify(&public).unwrap_or(false),
        spki_der: public.public_key_to_der().map_err(&e)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{chown, symlink};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let suffix = crate::identity::hex(&crate::crypto::random_256().unwrap()[..8]);
            Self(std::env::temp_dir().join(format!("PodMobileTest-perms-{suffix}")))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn key_open_refuses_symlinks_and_non_private_modes() {
        let f = Fixture::new();
        let store = Store::directory(&f.0).unwrap();
        drop(store.create("PodMobileTest-key").unwrap());
        assert!(store.open("PodMobileTest-key").unwrap().is_some());
        symlink(
            store.path("PodMobileTest-key"),
            store.path("PodMobileTest-link"),
        )
        .unwrap();
        assert!(store.open("PodMobileTest-link").is_err());
        for mode in [0o644, 0o400, 0o1600] {
            fs::set_permissions(
                store.path("PodMobileTest-key"),
                fs::Permissions::from_mode(mode),
            )
            .unwrap();
            assert!(store.open("PodMobileTest-key").is_err(), "mode {mode:o}");
        }
    }

    #[test]
    fn directory_refuses_links_and_non_private_modes() {
        let f = Fixture::new();
        let _store = Store::directory(&f.0).unwrap();
        let link = Fixture::new();
        symlink(&f.0, &link.0).unwrap();
        assert!(Store::directory(&link.0).is_err());
        fs::remove_file(&link.0).unwrap();
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::directory(&f.0).is_err());
    }

    #[test]
    fn foreign_owned_private_directory_and_key_are_refused() {
        // chown requires privilege. This case runs in root Linux CI/containers;
        // unprivileged runs explicitly leave ownership runtime validation open.
        if effective_uid() != 0 {
            eprintln!("unverified: foreign-owner fixture requires root");
            return;
        }
        let f = Fixture::new();
        let store = Store::directory(&f.0).unwrap();
        drop(store.create("PodMobileTest-key").unwrap());
        chown(store.path("PodMobileTest-key"), Some(1), None).unwrap();
        assert!(store.open("PodMobileTest-key").is_err());
        chown(&f.0, Some(1), None).unwrap();
        assert!(Store::directory(&f.0).is_err());
    }
}
