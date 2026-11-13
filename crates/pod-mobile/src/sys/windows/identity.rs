//! Listener keys in the current user's CNG key storage provider and the
//! self-signed certificate, built and read by CryptoAPI.

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use windows_sys::Win32::Foundation::{FILETIME, NTE_BAD_KEYSET, NTE_NO_MORE_ITEMS, SYSTEMTIME};
use windows_sys::Win32::Security::Cryptography::{
    CERT_ALT_NAME_ENTRY, CERT_ALT_NAME_ENTRY_0, CERT_ALT_NAME_INFO, CERT_BASIC_CONSTRAINTS2_INFO,
    CERT_CONTEXT, CERT_DIGITAL_SIGNATURE_KEY_USAGE, CERT_EXTENSION, CERT_EXTENSIONS, CERT_INFO,
    CERT_KEY_PROV_INFO_PROP_ID, CERT_PUBLIC_KEY_INFO, CERT_X500_NAME_STR,
    CRYPT_ALGORITHM_IDENTIFIER, CRYPT_BIT_BLOB, CRYPT_INTEGER_BLOB, CRYPT_KEY_PROV_INFO,
    CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT, CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT, CTL_USAGE,
    CertCreateCertificateContext, CertCreateSelfSignCertificate, CertFindExtension,
    CertFreeCertificateContext, CertSetCertificateContextProperty, CertStrToNameW,
    CryptExportPublicKeyInfoEx, CryptVerifyCertificateSignatureEx, MS_KEY_STORAGE_PROVIDER,
    NCRYPT_ALLOW_SIGNING_FLAG, NCRYPT_ECDSA_P256_ALGORITHM, NCRYPT_EXPORT_POLICY_PROPERTY,
    NCRYPT_KEY_USAGE_PROPERTY, NCRYPT_PERSIST_FLAG, NCRYPT_SILENT_FLAG, NCryptCreatePersistedKey,
    NCryptDeleteKey, NCryptEnumKeys, NCryptFinalizeKey, NCryptFreeBuffer, NCryptFreeObject,
    NCryptKeyName, NCryptOpenKey, NCryptOpenStorageProvider, NCryptSetProperty,
    X509_ALTERNATE_NAME, X509_ASN_ENCODING, X509_BASIC_CONSTRAINTS2, X509_ENHANCED_KEY_USAGE,
    X509_KEY_USAGE, X509_PUBLIC_KEY_INFO, szOID_BASIC_CONSTRAINTS2, szOID_ECDSA_SHA256,
    szOID_ENHANCED_KEY_USAGE, szOID_KEY_USAGE, szOID_PKIX_KP_SERVER_AUTH, szOID_SUBJECT_ALT_NAME2,
};
use windows_sys::Win32::System::Time::FileTimeToSystemTime;
use windows_sys::core::{PCSTR, PSTR, PWSTR};

pub(crate) use super::storage;
use super::{blob, blob_bytes, decode, encode, last_error, oid_str, wide};
use crate::crypto::CryptoError;
use crate::identity::CertificateInfo;

/// wincrypt.h `CERT_ALT_NAME_IP_ADDRESS` (not in windows-sys).
const CERT_ALT_NAME_IP_ADDRESS: u32 = 8;
/// wincrypt.h `CERT_CREATE_SELFSIGN_NO_KEY_INFO`: do not attach key
/// properties to the temporary context (the caller rebinds from DER).
const CERT_CREATE_SELFSIGN_NO_KEY_INFO: u32 = 2;
/// The subject and issuer of the listener certificate.
const SUBJECT: &str = "CN=Pod mobile";
/// Seconds between 1601-01-01 (FILETIME) and 1970-01-01.
const FILETIME_UNIX_OFFSET: i64 = 11_644_473_600;

fn hr_err(what: &'static str, hr: i32) -> CryptoError {
    CryptoError::Os {
        what,
        code: i64::from(hr),
    }
}

/// An open handle to the Microsoft Software Key Storage Provider.
pub(crate) struct Provider(pub(crate) usize);

impl Provider {
    pub(crate) fn open() -> Result<Provider, CryptoError> {
        let mut h = 0usize;
        // SAFETY: `h` receives a provider handle we free.
        let hr = unsafe { NCryptOpenStorageProvider(&mut h, MS_KEY_STORAGE_PROVIDER, 0) };
        if hr != 0 {
            return Err(hr_err("NCryptOpenStorageProvider", hr));
        }
        Ok(Provider(h))
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        // SAFETY: a handle from NCryptOpenStorageProvider.
        unsafe { NCryptFreeObject(self.0) };
    }
}

/// An NCrypt key handle.
pub(crate) struct Key(pub(crate) usize);

impl Drop for Key {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: a handle from NCryptCreatePersistedKey / NCryptOpenKey.
            unsafe { NCryptFreeObject(self.0) };
        }
    }
}

/// The current user's key storage provider (per-user, DPAPI-protected).
pub(crate) struct Store {
    prov: Provider,
}

// SAFETY: NCrypt provider handles may be used from any thread.
unsafe impl Send for Store {}
// SAFETY: as above; NCrypt serializes access to the provider.
unsafe impl Sync for Store {}

impl Store {
    pub(crate) fn current_user() -> Result<Store, CryptoError> {
        Ok(Store {
            prov: Provider::open()?,
        })
    }

    /// A new persisted, non-exportable, signing-only P-256 user key.
    pub(crate) fn create(&self, name: &str) -> Result<Key, CryptoError> {
        let wname = wide(name);
        let mut h = 0usize;
        // SAFETY: flags 0 = a user key that must not exist yet.
        let hr = unsafe {
            NCryptCreatePersistedKey(
                self.prov.0,
                &mut h,
                NCRYPT_ECDSA_P256_ALGORITHM,
                wname.as_ptr(),
                0,
                0,
            )
        };
        if hr != 0 {
            return Err(hr_err("NCryptCreatePersistedKey", hr));
        }
        let key = Key(h);
        let finish = || -> Result<(), CryptoError> {
            let export: u32 = 0;
            let usage: u32 = NCRYPT_ALLOW_SIGNING_FLAG;
            for (prop, value, what) in [
                (
                    NCRYPT_EXPORT_POLICY_PROPERTY,
                    export,
                    "NCryptSetProperty(export policy)",
                ),
                (
                    NCRYPT_KEY_USAGE_PROPERTY,
                    usage,
                    "NCryptSetProperty(key usage)",
                ),
            ] {
                let bytes = value.to_le_bytes();
                // SAFETY: a not yet finalized key; a 4-byte DWORD property.
                let hr = unsafe {
                    NCryptSetProperty(
                        key.0,
                        prop,
                        bytes.as_ptr(),
                        4,
                        NCRYPT_PERSIST_FLAG | NCRYPT_SILENT_FLAG,
                    )
                };
                if hr != 0 {
                    return Err(hr_err(what, hr));
                }
            }
            // SAFETY: finalizes (and persists) the key.
            let hr = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
            if hr != 0 {
                return Err(hr_err("NCryptFinalizeKey", hr));
            }
            Ok(())
        };
        if let Err(e) = finish() {
            let mut key = key;
            // SAFETY: deletes the half-made key; the call frees the handle.
            unsafe { NCryptDeleteKey(key.0, NCRYPT_SILENT_FLAG) };
            key.0 = 0;
            return Err(e);
        }
        Ok(key)
    }

    /// Opens the user key `name`; `None` if it does not exist.
    pub(crate) fn open(&self, name: &str) -> Result<Option<Key>, CryptoError> {
        let wname = wide(name);
        let mut h = 0usize;
        // SAFETY: `h` receives a key handle we free.
        let hr =
            unsafe { NCryptOpenKey(self.prov.0, &mut h, wname.as_ptr(), 0, NCRYPT_SILENT_FLAG) };
        match hr {
            0 => Ok(Some(Key(h))),
            NTE_BAD_KEYSET => Ok(None),
            other => Err(hr_err("NCryptOpenKey", other)),
        }
    }

    pub(crate) fn delete(&self, name: &str) -> Result<bool, CryptoError> {
        let Some(mut key) = self.open(name)? else {
            return Ok(false);
        };
        // SAFETY: deletes the persisted key; on success the handle is freed.
        let hr = unsafe { NCryptDeleteKey(key.0, NCRYPT_SILENT_FLAG) };
        if hr != 0 {
            return Err(hr_err("NCryptDeleteKey", hr));
        }
        key.0 = 0;
        Ok(true)
    }

    /// Names of every user key in the provider.
    pub(crate) fn list(&self) -> Result<Vec<String>, CryptoError> {
        let mut names = Vec::new();
        let mut state: *mut c_void = ptr::null_mut();
        let result = loop {
            let mut item: *mut NCryptKeyName = ptr::null_mut();
            // SAFETY: enumeration with a state pointer we free below.
            let hr = unsafe {
                NCryptEnumKeys(
                    self.prov.0,
                    ptr::null(),
                    &mut item,
                    &mut state,
                    NCRYPT_SILENT_FLAG,
                )
            };
            if hr == NTE_NO_MORE_ITEMS {
                break Ok(());
            }
            if hr != 0 {
                break Err(hr_err("NCryptEnumKeys", hr));
            }
            // SAFETY: `item` is a valid NCryptKeyName until freed.
            names.push(unsafe { from_wide((*item).pszName) });
            // SAFETY: `item` came from NCryptEnumKeys.
            unsafe { NCryptFreeBuffer(item.cast()) };
        };
        if !state.is_null() {
            // SAFETY: the enumeration state from NCryptEnumKeys.
            unsafe { NCryptFreeBuffer(state) };
        }
        result.map(|()| names)
    }
}

/// # Safety
/// `p` is null or a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: walks to the terminating NUL.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` UTF-16 units are readable.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// A certificate context.
pub(crate) struct CertCtx(pub(crate) *const CERT_CONTEXT);

// SAFETY: certificate contexts are reference counted and immutable after
// the key property is set; CryptoAPI allows use from any thread.
unsafe impl Send for CertCtx {}
// SAFETY: as above.
unsafe impl Sync for CertCtx {}

impl CertCtx {
    pub(crate) fn from_der(der: &[u8]) -> Result<CertCtx, CryptoError> {
        let len = u32::try_from(der.len()).map_err(|_| CryptoError::BadInput("certificate"))?;
        // SAFETY: `der` is readable for `len` bytes; the context copies it.
        let ctx = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), len) };
        if ctx.is_null() {
            return Err(CryptoError::BadInput("certificate"));
        }
        Ok(CertCtx(ctx))
    }

    fn info(&self) -> &CERT_INFO {
        // SAFETY: a valid context always has pCertInfo.
        unsafe { &*(*self.0).pCertInfo }
    }
}

impl Drop for CertCtx {
    fn drop(&mut self) {
        // SAFETY: a context we own a reference to.
        unsafe { CertFreeCertificateContext(self.0) };
    }
}

/// The listener certificate bound to its persisted key by name, as
/// SChannel takes it.
pub(crate) struct Credential {
    pub(crate) cert: CertCtx,
}

impl Credential {
    pub(crate) fn new(
        _store: &Store,
        key_name: &str,
        _key: Key,
        cert_der: &[u8],
    ) -> Result<Credential, CryptoError> {
        let cert = CertCtx::from_der(cert_der)?;
        let mut container = wide(key_name);
        let mut provider = wide("Microsoft Software Key Storage Provider");
        let prov_info = CRYPT_KEY_PROV_INFO {
            pwszContainerName: container.as_mut_ptr(),
            pwszProvName: provider.as_mut_ptr(),
            dwProvType: 0,
            dwFlags: NCRYPT_SILENT_FLAG,
            cProvParam: 0,
            rgProvParam: ptr::null_mut(),
            dwKeySpec: 0,
        };
        // SAFETY: the property data is copied into the context.
        let ok = unsafe {
            CertSetCertificateContextProperty(
                cert.0,
                CERT_KEY_PROV_INFO_PROP_ID,
                0,
                ptr::from_ref(&prov_info).cast(),
            )
        };
        if ok == 0 {
            return Err(last_error("CertSetCertificateContextProperty"));
        }
        Ok(Credential { cert })
    }
}

/// DER SubjectPublicKeyInfo of an NCrypt key.
pub(crate) fn key_spki_der(key: &Key) -> Result<Vec<u8>, CryptoError> {
    let mut len = 0u32;
    // SAFETY: size query; dwKeySpec is ignored for NCrypt keys.
    let ok = unsafe {
        CryptExportPublicKeyInfoEx(
            key.0,
            0,
            X509_ASN_ENCODING,
            ptr::null(),
            0,
            ptr::null(),
            ptr::null_mut(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptExportPublicKeyInfoEx"));
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<CERT_PUBLIC_KEY_INFO>();
    // SAFETY: `buf` holds `len` aligned bytes.
    let ok = unsafe {
        CryptExportPublicKeyInfoEx(
            key.0,
            0,
            X509_ASN_ENCODING,
            ptr::null(),
            0,
            ptr::null(),
            info,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptExportPublicKeyInfoEx"));
    }
    // SAFETY: `info` was filled above and points into `buf`.
    unsafe { encode(X509_PUBLIC_KEY_INFO, info.cast_const().cast()) }
}

fn system_time(unix: i64) -> Result<SYSTEMTIME, CryptoError> {
    let ticks = u64::try_from(unix + FILETIME_UNIX_OFFSET)
        .map_err(|_| CryptoError::BadInput("certificate time"))?
        * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    // SAFETY: plain conversion into a local.
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid locals.
    if unsafe { FileTimeToSystemTime(&ft, &mut st) } == 0 {
        return Err(last_error("FileTimeToSystemTime"));
    }
    Ok(st)
}

fn unix_time(ft: &FILETIME) -> i64 {
    let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    (ticks / 10_000_000) as i64 - FILETIME_UNIX_OFFSET
}

fn name_blob(x500: &str) -> Result<Vec<u8>, CryptoError> {
    let w = wide(x500);
    let mut len = 0u32;
    // SAFETY: size query.
    let ok = unsafe {
        CertStrToNameW(
            X509_ASN_ENCODING,
            w.as_ptr(),
            CERT_X500_NAME_STR,
            ptr::null(),
            ptr::null_mut(),
            &mut len,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("CertStrToNameW"));
    }
    let mut out = vec![0u8; len as usize];
    // SAFETY: `out` has `len` bytes.
    let ok = unsafe {
        CertStrToNameW(
            X509_ASN_ENCODING,
            w.as_ptr(),
            CERT_X500_NAME_STR,
            ptr::null(),
            out.as_mut_ptr(),
            &mut len,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("CertStrToNameW"));
    }
    out.truncate(len as usize);
    Ok(out)
}

fn extension(oid: PCSTR, critical: bool, value: &mut [u8]) -> CERT_EXTENSION {
    CERT_EXTENSION {
        pszObjId: oid.cast_mut(),
        fCritical: i32::from(critical),
        Value: blob(value),
    }
}

/// The self-signed listener certificate for `key` (DER).
pub(crate) fn self_signed(
    key: &Key,
    ip: IpAddr,
    not_before: i64,
    not_after: i64,
) -> Result<Vec<u8>, CryptoError> {
    let mut subject = name_blob(SUBJECT)?;
    let subject_blob = blob(&mut subject);

    let mut ip_bytes: Vec<u8> = match ip {
        IpAddr::V4(a) => a.octets().to_vec(),
        IpAddr::V6(a) => a.octets().to_vec(),
    };
    let mut entry = CERT_ALT_NAME_ENTRY {
        dwAltNameChoice: CERT_ALT_NAME_IP_ADDRESS,
        Anonymous: CERT_ALT_NAME_ENTRY_0 {
            IPAddress: blob(&mut ip_bytes),
        },
    };
    let san_info = CERT_ALT_NAME_INFO {
        cAltEntry: 1,
        rgAltEntry: &mut entry,
    };
    let mut eku_oids: [PSTR; 1] = [szOID_PKIX_KP_SERVER_AUTH.cast_mut()];
    let eku_info = CTL_USAGE {
        cUsageIdentifier: 1,
        rgpszUsageIdentifier: eku_oids.as_mut_ptr(),
    };
    let bc_info = CERT_BASIC_CONSTRAINTS2_INFO {
        fCA: 0,
        fPathLenConstraint: 0,
        dwPathLenConstraint: 0,
    };
    let mut ku_bits = [CERT_DIGITAL_SIGNATURE_KEY_USAGE as u8];
    let ku_info = CRYPT_BIT_BLOB {
        cbData: 1,
        pbData: ku_bits.as_mut_ptr(),
        cUnusedBits: 7,
    };
    // SAFETY: each structure and what it points to lives through its call.
    let (mut san, mut eku, mut bc, mut ku) = unsafe {
        (
            encode(X509_ALTERNATE_NAME, ptr::from_ref(&san_info).cast())?,
            encode(X509_ENHANCED_KEY_USAGE, ptr::from_ref(&eku_info).cast())?,
            encode(X509_BASIC_CONSTRAINTS2, ptr::from_ref(&bc_info).cast())?,
            encode(X509_KEY_USAGE, ptr::from_ref(&ku_info).cast())?,
        )
    };
    let mut exts = [
        extension(szOID_BASIC_CONSTRAINTS2, true, &mut bc),
        extension(szOID_KEY_USAGE, true, &mut ku),
        extension(szOID_ENHANCED_KEY_USAGE, false, &mut eku),
        extension(szOID_SUBJECT_ALT_NAME2, false, &mut san),
    ];
    let exts_info = CERT_EXTENSIONS {
        cExtension: exts.len() as u32,
        rgExtension: exts.as_mut_ptr(),
    };
    let alg = CRYPT_ALGORITHM_IDENTIFIER {
        pszObjId: szOID_ECDSA_SHA256.cast_mut(),
        Parameters: CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        },
    };
    let start = system_time(not_before)?;
    let end = system_time(not_after)?;
    // SAFETY: every input is a live local; the key signs (Windows picks a
    // random serial); the returned context is freed below.
    let ctx = unsafe {
        CertCreateSelfSignCertificate(
            key.0,
            &subject_blob,
            CERT_CREATE_SELFSIGN_NO_KEY_INFO,
            ptr::null(),
            &alg,
            &start,
            &end,
            &exts_info,
        )
    };
    if ctx.is_null() {
        return Err(last_error("CertCreateSelfSignCertificate"));
    }
    let ctx = CertCtx(ctx);
    // SAFETY: the context's encoded bytes are valid while it lives.
    let der = unsafe {
        std::slice::from_raw_parts((*ctx.0).pbCertEncoded, (*ctx.0).cbCertEncoded as usize).to_vec()
    };
    Ok(der)
}

/// The extension `oid` of `info`, if present.
fn find_extension(info: &CERT_INFO, oid: PCSTR) -> Option<&CERT_EXTENSION> {
    // SAFETY: rgExtension holds cExtension entries of a live context.
    let ext = unsafe { CertFindExtension(oid, info.cExtension, info.rgExtension) };
    // SAFETY: a pointer into the same array, or null.
    unsafe { ext.as_ref() }
}

/// DER SubjectPublicKeyInfo of a decoded certificate.
pub(crate) fn spki_der(info: &CERT_INFO) -> Result<Vec<u8>, CryptoError> {
    // SAFETY: a live certificate's key info.
    unsafe {
        encode(
            X509_PUBLIC_KEY_INFO,
            ptr::from_ref(&info.SubjectPublicKeyInfo).cast(),
        )
    }
}

pub(crate) fn parse_certificate(der: &[u8]) -> Result<CertificateInfo, CryptoError> {
    use windows_sys::Win32::Security::Cryptography::{CERT_SIGNED_CONTENT_INFO, X509_CERT};
    let decoded = decode(X509_CERT, der).map_err(|_| CryptoError::BadInput("certificate"))?;
    // SAFETY: X509_CERT decodes to CERT_SIGNED_CONTENT_INFO in live storage.
    let signed: &CERT_SIGNED_CONTENT_INFO = unsafe { decoded.get() };
    // SAFETY: native re-encoding rejects trailing/noncanonical input without
    // implementing an ASN.1 parser here.
    if unsafe { encode(X509_CERT, ptr::from_ref(signed).cast()) }? != der {
        return Err(CryptoError::BadInput("certificate"));
    }
    let cert = CertCtx::from_der(der)?;
    let info = cert.info();
    // SAFETY: blobs of a live context.
    let (serial_le, subject, issuer) = unsafe {
        (
            blob_bytes(&info.SerialNumber),
            blob_bytes(&info.Subject),
            blob_bytes(&info.Issuer),
        )
    };
    let mut serial: Vec<u8> = serial_le.iter().rev().copied().collect();
    while serial.len() > 1 && serial[0] == 0 {
        serial.remove(0);
    }

    let mut san_ips = Vec::new();
    let mut san_other = 0;
    // SAFETY (all `blob_bytes` / `get` below): the extension values and the
    // decoded structures are live for the scope they are read in.
    if let Some(ext) = find_extension(info, szOID_SUBJECT_ALT_NAME2) {
        let decoded = decode(X509_ALTERNATE_NAME, unsafe { blob_bytes(&ext.Value) })?;
        let names: &CERT_ALT_NAME_INFO = unsafe { decoded.get() };
        let entries = if names.rgAltEntry.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(names.rgAltEntry, names.cAltEntry as usize) }
        };
        for e in entries {
            let ip = if e.dwAltNameChoice == CERT_ALT_NAME_IP_ADDRESS {
                match unsafe { blob_bytes(&e.Anonymous.IPAddress) } {
                    b if b.len() == 4 => Some(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))),
                    b if b.len() == 16 => <[u8; 16]>::try_from(b)
                        .ok()
                        .map(|o| IpAddr::V6(Ipv6Addr::from(o))),
                    _ => None,
                }
            } else {
                None
            };
            match ip {
                Some(ip) => san_ips.push(ip),
                None => san_other += 1,
            }
        }
    }
    let ca = match find_extension(info, szOID_BASIC_CONSTRAINTS2) {
        Some(ext) => {
            let decoded = decode(X509_BASIC_CONSTRAINTS2, unsafe { blob_bytes(&ext.Value) })?;
            let bc: &CERT_BASIC_CONSTRAINTS2_INFO = unsafe { decoded.get() };
            bc.fCA != 0
        }
        None => false,
    };
    let mut server_auth = false;
    if let Some(ext) = find_extension(info, szOID_ENHANCED_KEY_USAGE) {
        let decoded = decode(X509_ENHANCED_KEY_USAGE, unsafe { blob_bytes(&ext.Value) })?;
        let usage: &CTL_USAGE = unsafe { decoded.get() };
        if !usage.rgpszUsageIdentifier.is_null() {
            let oids = unsafe {
                std::slice::from_raw_parts(
                    usage.rgpszUsageIdentifier,
                    usage.cUsageIdentifier as usize,
                )
            };
            // SAFETY: NUL-terminated OID strings inside `decoded`.
            let want = unsafe { oid_str(szOID_PKIX_KP_SERVER_AUTH) };
            server_auth = oids.iter().any(|o| unsafe { oid_str(*o) } == want);
        }
    }
    let mut digital_signature = false;
    if let Some(ext) = find_extension(info, szOID_KEY_USAGE) {
        let decoded = decode(X509_KEY_USAGE, unsafe { blob_bytes(&ext.Value) })?;
        let bits: &CRYPT_BIT_BLOB = unsafe { decoded.get() };
        digital_signature = bits.cbData > 0
            && unsafe { *bits.pbData } & (CERT_DIGITAL_SIGNATURE_KEY_USAGE as u8) != 0;
    }
    // SAFETY: subject and issuer are the same live context.
    let signature_ok = unsafe {
        CryptVerifyCertificateSignatureEx(
            0,
            X509_ASN_ENCODING,
            CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT,
            cert.0.cast(),
            CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT,
            cert.0.cast(),
            0,
            ptr::null_mut(),
        )
    } != 0;
    Ok(CertificateInfo {
        serial,
        not_before: unix_time(&info.NotBefore),
        not_after: unix_time(&info.NotAfter),
        san_ips,
        san_other,
        ca: Some(ca),
        server_auth,
        digital_signature,
        self_signed: signature_ok && subject == issuer,
        spki_der: spki_der(info)?,
    })
}
