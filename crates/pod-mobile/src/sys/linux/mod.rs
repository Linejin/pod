//! Linux backend: the system OpenSSL 3 (libssl/libcrypto, dynamically
//! linked through the `openssl` crate; never vendored). Compile-checked in
//! CI; its runtime is unverified in M2 (no Linux machine in the loop).

pub(crate) mod crypto;
pub(crate) mod identity;
pub(crate) mod storage;
pub(crate) mod tls;

use openssl::error::ErrorStack;

use crate::crypto::CryptoError;

/// First OpenSSL error code of a failed call (0 when the stack is empty).
pub(super) fn ossl_code(e: &ErrorStack) -> i64 {
    e.errors().first().map_or(0, |x| x.code() as i64)
}

pub(super) fn ossl(what: &'static str) -> impl Fn(ErrorStack) -> CryptoError {
    move |e| CryptoError::Os {
        what,
        code: ossl_code(&e),
    }
}
