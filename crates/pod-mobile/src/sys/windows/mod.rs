//! Windows backend: SChannel (SSPI) for TLS, CNG (BCrypt/NCrypt) for keys,
//! hashing, randomness and signatures, CryptoAPI for DER encoding and
//! decoding. All of it is the OS; nothing is bundled.

pub(crate) mod crypto;
pub(crate) mod identity;
pub(crate) mod tls;

use std::ffi::{CStr, c_void};
use std::ptr;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptDecodeObjectEx, CryptEncodeObjectEx, X509_ASN_ENCODING,
};
use windows_sys::core::PCSTR;

use crate::crypto::CryptoError;

/// NUL-terminated UTF-16.
pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The thread's last Win32 error as a [`CryptoError`].
pub(super) fn last_error(what: &'static str) -> CryptoError {
    // SAFETY: no preconditions.
    let code = unsafe { GetLastError() };
    CryptoError::Os {
        what,
        code: i64::from(code),
    }
}

/// The bytes of a CryptoAPI blob (empty for a null pointer).
///
/// # Safety
/// `blob` must describe `cbData` readable bytes at `pbData` (or be null).
pub(super) unsafe fn blob_bytes(blob: &CRYPT_INTEGER_BLOB) -> &[u8] {
    if blob.pbData.is_null() || blob.cbData == 0 {
        return &[];
    }
    // SAFETY: per the contract above.
    unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }
}

/// A blob over `bytes` (CryptoAPI takes `*mut` for inputs it only reads).
pub(super) fn blob(bytes: &mut [u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: if bytes.is_empty() {
            ptr::null_mut()
        } else {
            bytes.as_mut_ptr()
        },
    }
}

/// The OID string at `oid` (null gives empty).
///
/// # Safety
/// `oid` is null or a NUL-terminated string that outlives the result.
pub(super) unsafe fn oid_str<'a>(oid: *const u8) -> &'a [u8] {
    if oid.is_null() {
        return &[];
    }
    // SAFETY: per the contract above.
    unsafe { CStr::from_ptr(oid.cast()).to_bytes() }
}

/// DER-encodes the CryptoAPI structure `value` of type `struct_type`.
///
/// # Safety
/// `value` must point to the structure `struct_type` names, with every
/// pointer in it valid for the call.
pub(super) unsafe fn encode(
    struct_type: PCSTR,
    value: *const c_void,
) -> Result<Vec<u8>, CryptoError> {
    let mut len = 0u32;
    // SAFETY: size query; `value` per the contract.
    let ok = unsafe {
        CryptEncodeObjectEx(
            X509_ASN_ENCODING,
            struct_type,
            value,
            0,
            ptr::null(),
            ptr::null_mut(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptEncodeObjectEx"));
    }
    let mut out = vec![0u8; len as usize];
    // SAFETY: `out` has `len` writable bytes.
    let ok = unsafe {
        CryptEncodeObjectEx(
            X509_ASN_ENCODING,
            struct_type,
            value,
            0,
            ptr::null(),
            out.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptEncodeObjectEx"));
    }
    out.truncate(len as usize);
    Ok(out)
}

/// A decoded CryptoAPI structure: an 8-byte aligned buffer that starts with
/// the structure and holds everything it points to.
pub(super) struct Decoded(Vec<u64>);

impl Decoded {
    /// The structure, typed by the caller.
    ///
    /// # Safety
    /// `T` must be the structure the decode produced.
    pub(super) unsafe fn get<T>(&self) -> &T {
        // SAFETY: the buffer is aligned for pointers and starts with `T`
        // (the decoder's contract), per the caller.
        unsafe { &*self.0.as_ptr().cast::<T>() }
    }
}

/// Decodes DER `der` as `struct_type`.
pub(super) fn decode(struct_type: PCSTR, der: &[u8]) -> Result<Decoded, CryptoError> {
    let len_in = u32::try_from(der.len()).map_err(|_| CryptoError::BadInput("DER length"))?;
    let mut len = 0u32;
    // SAFETY: size query over `der`.
    let ok = unsafe {
        CryptDecodeObjectEx(
            X509_ASN_ENCODING,
            struct_type,
            der.as_ptr(),
            len_in,
            0,
            ptr::null(),
            ptr::null_mut(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptDecodeObjectEx"));
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` has at least `len` writable, 8-byte aligned bytes.
    let ok = unsafe {
        CryptDecodeObjectEx(
            X509_ASN_ENCODING,
            struct_type,
            der.as_ptr(),
            len_in,
            0,
            ptr::null(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("CryptDecodeObjectEx"));
    }
    Ok(Decoded(buf))
}
