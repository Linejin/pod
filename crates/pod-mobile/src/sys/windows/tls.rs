//! SChannel over SSPI: credentials with `SCH_CREDENTIALS` (TLS 1.3 capable,
//! version and cipher restrictions), the handshake loops, the record layer
//! and close_notify.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::time::Instant;

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::{
    SEC_E_INCOMPLETE_MESSAGE, SEC_E_OK, SEC_I_CONTEXT_EXPIRED, SEC_I_CONTINUE_NEEDED,
    SEC_I_RENEGOTIATE,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    ASC_REQ_ALLOCATE_MEMORY, ASC_REQ_CONFIDENTIALITY, ASC_REQ_EXTENDED_ERROR,
    ASC_REQ_REPLAY_DETECT, ASC_REQ_SEQUENCE_DETECT, ASC_REQ_STREAM, AcceptSecurityContext,
    AcquireCredentialsHandleW, ApplyControlToken, CRYPTO_SETTINGS, DecryptMessage,
    DeleteSecurityContext, EncryptMessage, FreeContextBuffer, FreeCredentialsHandle,
    ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONFIDENTIALITY, ISC_REQ_EXTENDED_ERROR,
    ISC_REQ_MANUAL_CRED_VALIDATION, ISC_REQ_REPLAY_DETECT, ISC_REQ_SEQUENCE_DETECT, ISC_REQ_STREAM,
    ISC_REQ_USE_SUPPLIED_CREDS, InitializeSecurityContextW, LSA_UNICODE_STRING,
    QueryContextAttributesW, SCH_CRED_DISABLE_RECONNECTS, SCH_CRED_MANUAL_CRED_VALIDATION,
    SCH_CRED_NO_DEFAULT_CREDS, SCH_CRED_NO_SYSTEM_MAPPER, SCH_CREDENTIALS, SCH_CREDENTIALS_VERSION,
    SCH_USE_STRONG_CRYPTO, SCHANNEL_SHUTDOWN, SECBUFFER_ALERT, SECBUFFER_APPLICATION_PROTOCOLS,
    SECBUFFER_DATA, SECBUFFER_EMPTY, SECBUFFER_EXTRA, SECBUFFER_STREAM_HEADER,
    SECBUFFER_STREAM_TRAILER, SECBUFFER_TOKEN, SECBUFFER_VERSION, SECPKG_ATTR_APPLICATION_PROTOCOL,
    SECPKG_ATTR_CIPHER_INFO, SECPKG_ATTR_CONNECTION_INFO, SECPKG_ATTR_REMOTE_CERT_CONTEXT,
    SECPKG_ATTR_STREAM_SIZES, SECPKG_CRED_INBOUND, SECPKG_CRED_OUTBOUND, SP_PROT_TLS1_2_CLIENT,
    SP_PROT_TLS1_2_SERVER, SP_PROT_TLS1_3_CLIENT, SP_PROT_TLS1_3_SERVER,
    SecApplicationProtocolNegotiationExt_ALPN, SecApplicationProtocolNegotiationStatus_Success,
    SecBuffer, SecBufferDesc, SecPkgContext_ApplicationProtocol, SecPkgContext_CipherInfo,
    SecPkgContext_ConnectionInfo, SecPkgContext_StreamSizes, TLS_PARAMETERS,
    TlsParametersCngAlgUsageCipher, TlsParametersCngAlgUsageKeyExchange, UNISP_NAME_W,
};
use windows_sys::Win32::Security::Credentials::SecHandle;
use windows_sys::Win32::Security::Cryptography::CERT_CONTEXT;
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

use super::identity::{CertCtx, Credential, spki_der};
use crate::crypto::CryptoError;
use crate::tls::platform::Offer;
use crate::tls::{ALPN_HTTP11, MAX_HANDSHAKE_BYTES, SessionInfo, SpkiSha256, TlsError, TlsVersion};

/// One read from the transport (a full TLS record plus header room).
const READ_CHUNK: usize = 16 * 1024 + 1024;
/// Ciphertext buffered while assembling one record. A record is at most
/// 16 KiB plus overhead; more than a few means a misbehaving peer.
const MAX_CIPHERTEXT: usize = 4 * (16 * 1024 + 2048);
/// First Windows build with a TLS 1.3 server: Server 2022 (20348); Windows
/// 11 is 22000 and later. Windows 10 (19045 and before) has none.
const TLS13_SERVER_BUILD: u32 = 20348;

const ASC_FLAGS: u32 = ASC_REQ_SEQUENCE_DETECT
    | ASC_REQ_REPLAY_DETECT
    | ASC_REQ_CONFIDENTIALITY
    | ASC_REQ_EXTENDED_ERROR
    | ASC_REQ_ALLOCATE_MEMORY
    | ASC_REQ_STREAM;
const ISC_FLAGS: u32 = ISC_REQ_SEQUENCE_DETECT
    | ISC_REQ_REPLAY_DETECT
    | ISC_REQ_CONFIDENTIALITY
    | ISC_REQ_EXTENDED_ERROR
    | ISC_REQ_ALLOCATE_MEMORY
    | ISC_REQ_STREAM
    | ISC_REQ_MANUAL_CRED_VALIDATION
    | ISC_REQ_USE_SUPPLIED_CREDS;

fn os(what: &'static str, status: i32) -> TlsError {
    TlsError::Os {
        what,
        code: i64::from(status),
    }
}

fn io_os(what: &'static str, status: i32) -> io::Error {
    io::Error::other(os(what, status))
}

fn crypto_err(e: CryptoError) -> TlsError {
    match e {
        CryptoError::Os { what, code } => TlsError::Os { what, code },
        other => TlsError::Policy(other.to_string()),
    }
}

/// The OS build number (RtlGetVersion does not lie about compatibility).
fn windows_build() -> u32 {
    // SAFETY: plain struct initialised below.
    let mut v: OSVERSIONINFOW = unsafe { mem::zeroed() };
    v.dwOSVersionInfoSize = mem::size_of::<OSVERSIONINFOW>() as u32;
    // SAFETY: `v` is a valid OSVERSIONINFOW with its size set.
    if unsafe { RtlGetVersion(&mut v) } == 0 {
        v.dwBuildNumber
    } else {
        0
    }
}

pub(crate) fn has_tls13_server() -> bool {
    windows_build() >= TLS13_SERVER_BUILD
}

/// An SSPI credential handle.
struct Cred(SecHandle);

impl Drop for Cred {
    fn drop(&mut self) {
        // SAFETY: a handle from AcquireCredentialsHandleW.
        unsafe { FreeCredentialsHandle(&self.0) };
        #[cfg(test)]
        tests::record_drop("credential");
    }
}

/// An SSPI security context; deleted on drop once established.
struct Ctx {
    h: SecHandle,
    live: bool,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        if self.live {
            // SAFETY: a context created by ISC/ASC.
            unsafe { DeleteSecurityContext(&self.h) };
            #[cfg(test)]
            tests::record_drop("context");
        }
    }
}

/// An output buffer SSPI allocated (`*_REQ_ALLOCATE_MEMORY`).
struct OutBuf {
    ptr: *mut c_void,
    len: usize,
}

impl OutBuf {
    fn take(b: &SecBuffer) -> Option<OutBuf> {
        if b.pvBuffer.is_null() {
            return None;
        }
        Some(OutBuf {
            ptr: b.pvBuffer,
            len: b.cbBuffer as usize,
        })
    }

    fn bytes(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: SSPI allocated `len` bytes at `ptr`.
        unsafe { std::slice::from_raw_parts(self.ptr.cast(), self.len) }
    }
}

impl Drop for OutBuf {
    fn drop(&mut self) {
        // SAFETY: allocated by SSPI for this call.
        unsafe { FreeContextBuffer(self.ptr) };
    }
}

fn sec_buf(kind: u32, data: &mut [u8]) -> SecBuffer {
    SecBuffer {
        cbBuffer: data.len() as u32,
        BufferType: kind,
        pvBuffer: data.as_mut_ptr().cast(),
    }
}

fn empty_buf(kind: u32) -> SecBuffer {
    SecBuffer {
        cbBuffer: 0,
        BufferType: kind,
        pvBuffer: ptr::null_mut(),
    }
}

fn desc(bufs: &mut [SecBuffer]) -> SecBufferDesc {
    SecBufferDesc {
        ulVersion: SECBUFFER_VERSION,
        cBuffers: bufs.len() as u32,
        pBuffers: bufs.as_mut_ptr(),
    }
}

/// A counted UTF-16 string for `TLS_PARAMETERS` (kept alive by the owner).
struct Ucs(Vec<u16>);

impl Ucs {
    fn new(s: &str) -> Ucs {
        Ucs(s.encode_utf16().collect())
    }

    fn us(&mut self) -> LSA_UNICODE_STRING {
        let bytes = (self.0.len() * 2) as u16;
        LSA_UNICODE_STRING {
            Length: bytes,
            MaximumLength: bytes,
            Buffer: self.0.as_mut_ptr(),
        }
    }
}

fn crypto_setting(
    usage: i32,
    alg: LSA_UNICODE_STRING,
    modes: &mut [LSA_UNICODE_STRING],
) -> CRYPTO_SETTINGS {
    CRYPTO_SETTINGS {
        eAlgorithmUsage: usage,
        strCngAlgId: alg,
        cChainingModes: modes.len() as u32,
        rgstrChainingModes: if modes.is_empty() {
            ptr::null_mut()
        } else {
            modes.as_mut_ptr()
        },
        dwMinBitLength: 0,
        dwMaxBitLength: 0,
    }
}

/// `AcquireCredentialsHandleW` with `SCH_CREDENTIALS`: only the offered
/// versions, strong crypto, no static RSA / finite-field DH key exchange,
/// and AES-CBC disabled (or, for the refusing client, everything but CBC).
fn acquire(inbound: bool, cert: Option<&CertCtx>, offer: Offer) -> Result<Cred, TlsError> {
    let (p12, p13) = if inbound {
        (SP_PROT_TLS1_2_SERVER, SP_PROT_TLS1_3_SERVER)
    } else {
        (SP_PROT_TLS1_2_CLIENT, SP_PROT_TLS1_3_CLIENT)
    };
    let enabled = if offer.tls12 { p12 } else { 0 } | if offer.tls13 { p13 } else { 0 };
    if enabled == 0 {
        return Err(TlsError::Policy("no TLS version enabled".into()));
    }
    let (mut aes, mut cbc, mut gcm) = (
        Ucs::new("AES"),
        Ucs::new("ChainingModeCBC"),
        Ucs::new("ChainingModeGCM"),
    );
    let (mut chacha, mut rsa, mut dh) = (
        Ucs::new("CHACHA20_POLY1305"),
        Ucs::new("RSA"),
        Ucs::new("DH"),
    );
    let mut cbc_mode = [cbc.us()];
    let mut gcm_mode = [gcm.us()];
    let mut disabled = vec![
        crypto_setting(TlsParametersCngAlgUsageKeyExchange, rsa.us(), &mut []),
        crypto_setting(TlsParametersCngAlgUsageKeyExchange, dh.us(), &mut []),
    ];
    if offer.cbc_only {
        disabled.push(crypto_setting(
            TlsParametersCngAlgUsageCipher,
            aes.us(),
            &mut gcm_mode,
        ));
        disabled.push(crypto_setting(
            TlsParametersCngAlgUsageCipher,
            chacha.us(),
            &mut [],
        ));
    } else {
        disabled.push(crypto_setting(
            TlsParametersCngAlgUsageCipher,
            aes.us(),
            &mut cbc_mode,
        ));
    }
    let mut params = TLS_PARAMETERS {
        cAlpnIds: 0,
        rgstrAlpnIds: ptr::null_mut(),
        grbitDisabledProtocols: !enabled,
        cDisabledCrypto: disabled.len() as u32,
        pDisabledCrypto: disabled.as_mut_ptr(),
        dwFlags: 0,
    };
    let mut certs: [*mut CERT_CONTEXT; 1] = [cert.map_or(ptr::null_mut(), |c| c.0.cast_mut())];
    let flags = if inbound {
        SCH_USE_STRONG_CRYPTO | SCH_CRED_NO_SYSTEM_MAPPER | SCH_CRED_DISABLE_RECONNECTS
    } else {
        SCH_USE_STRONG_CRYPTO | SCH_CRED_NO_DEFAULT_CREDS | SCH_CRED_MANUAL_CRED_VALIDATION
    };
    let creds = SCH_CREDENTIALS {
        dwVersion: SCH_CREDENTIALS_VERSION,
        dwCredFormat: 0,
        cCreds: u32::from(cert.is_some()),
        paCred: if cert.is_some() {
            certs.as_mut_ptr()
        } else {
            ptr::null_mut()
        },
        hRootStore: ptr::null_mut(),
        cMappers: 0,
        aphMappers: ptr::null_mut(),
        dwSessionLifespan: 0,
        dwFlags: flags,
        cTlsParameters: 1,
        pTlsParameters: &mut params,
    };
    let mut h = SecHandle {
        dwLower: 0,
        dwUpper: 0,
    };
    // SAFETY: `creds` and everything it points to live through the call;
    // SChannel copies what it keeps.
    let st = unsafe {
        AcquireCredentialsHandleW(
            ptr::null(),
            UNISP_NAME_W,
            if inbound {
                SECPKG_CRED_INBOUND
            } else {
                SECPKG_CRED_OUTBOUND
            },
            ptr::null(),
            ptr::from_ref(&creds).cast(),
            None,
            ptr::null(),
            &mut h,
            ptr::null_mut(),
        )
    };
    if st != SEC_E_OK {
        return Err(os("AcquireCredentialsHandleW", st));
    }
    Ok(Cred(h))
}

/// The `SEC_APPLICATION_PROTOCOLS` input buffer offering `http/1.1`:
/// u32 list size, then one list: u32 ALPN extension, u16 length, the
/// length-prefixed names (packed, little-endian).
fn alpn_buffer() -> Vec<u8> {
    let mut wire = vec![ALPN_HTTP11.len() as u8];
    wire.extend_from_slice(ALPN_HTTP11);
    let mut v = Vec::with_capacity(10 + wire.len());
    v.extend_from_slice(&((4 + 2 + wire.len()) as u32).to_le_bytes());
    v.extend_from_slice(&(SecApplicationProtocolNegotiationExt_ALPN as u32).to_le_bytes());
    v.extend_from_slice(&(wire.len() as u16).to_le_bytes());
    v.extend_from_slice(&wire);
    v
}

/// Appends one read from `io` to `buf`; `Ok(0)` at end of stream.
fn read_more<S: Read>(
    io: &mut S,
    buf: &mut Vec<u8>,
    deadline: Option<Instant>,
) -> io::Result<usize> {
    let old = buf.len();
    buf.resize(old + READ_CHUNK, 0);
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            buf.truncate(old);
            return Err(io::ErrorKind::TimedOut.into());
        }
        match io.read(&mut buf[old..]) {
            Ok(n) => {
                buf.truncate(old + n);
                return Ok(n);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                buf.truncate(old);
                return Err(e);
            }
        }
    }
}

/// One ISC/ASC call's result.
struct Step {
    status: i32,
    token: Option<OutBuf>,
    /// Unprocessed bytes at the end of the input.
    extra: usize,
}

/// Drives ISC (client) or ASC (server) for one context.
struct Driver {
    // Fields drop in declaration order: the context must release before its credential.
    ctx: Ctx,
    cred: Arc<Cred>,
    server: bool,
    alpn: Vec<u8>,
}

impl Driver {
    fn new(cred: Arc<Cred>, server: bool) -> Driver {
        Driver {
            cred,
            ctx: Ctx {
                h: SecHandle {
                    dwLower: 0,
                    dwUpper: 0,
                },
                live: false,
            },
            server,
            alpn: alpn_buffer(),
        }
    }

    /// One call with `input` as the token (none for the first client call
    /// and for shutdown) and the ALPN offer when `with_alpn`.
    fn step(&mut self, input: Option<&mut [u8]>, with_alpn: bool) -> Step {
        let has_input = input.is_some();
        let mut inbufs: Vec<SecBuffer> = Vec::with_capacity(3);
        if let Some(data) = input {
            inbufs.push(sec_buf(SECBUFFER_TOKEN, data));
            inbufs.push(empty_buf(SECBUFFER_EMPTY));
        }
        if with_alpn {
            inbufs.push(sec_buf(SECBUFFER_APPLICATION_PROTOCOLS, &mut self.alpn));
        }
        let in_desc = desc(&mut inbufs);
        let in_ptr: *const SecBufferDesc = if inbufs.is_empty() {
            ptr::null()
        } else {
            &in_desc
        };
        let mut outbufs = [
            empty_buf(SECBUFFER_TOKEN),
            empty_buf(SECBUFFER_ALERT),
            empty_buf(SECBUFFER_EMPTY),
        ];
        let mut out_desc = desc(&mut outbufs);
        let mut attrs = 0u32;
        let ctx_in: *const SecHandle = if self.ctx.live {
            &self.ctx.h
        } else {
            ptr::null()
        };
        // SAFETY: the credential is live, the context handle is either the
        // live one or a fresh slot, and every buffer outlives the call.
        let status = unsafe {
            if self.server {
                AcceptSecurityContext(
                    &self.cred.0,
                    ctx_in,
                    in_ptr,
                    ASC_FLAGS,
                    0,
                    &mut self.ctx.h,
                    &mut out_desc,
                    &mut attrs,
                    ptr::null_mut(),
                )
            } else {
                InitializeSecurityContextW(
                    &self.cred.0,
                    ctx_in,
                    ptr::null(),
                    ISC_FLAGS,
                    0,
                    0,
                    in_ptr,
                    0,
                    &mut self.ctx.h,
                    &mut out_desc,
                    &mut attrs,
                    ptr::null_mut(),
                )
            }
        };
        let token = OutBuf::take(&outbufs[0]);
        drop((OutBuf::take(&outbufs[1]), OutBuf::take(&outbufs[2])));
        let extra = if has_input && inbufs[1].BufferType == SECBUFFER_EXTRA {
            inbufs[1].cbBuffer as usize
        } else {
            0
        };
        if status == SEC_E_OK || status == SEC_I_CONTINUE_NEEDED {
            self.ctx.live = true;
        }
        Step {
            status,
            token,
            extra,
        }
    }

    fn what(&self) -> &'static str {
        if self.server {
            "AcceptSecurityContext"
        } else {
            "InitializeSecurityContextW"
        }
    }

    /// Runs the handshake until `SEC_E_OK`, starting from `buf` (bytes
    /// already received) and returning the bytes received past it.
    fn handshake<S: Read + Write>(
        &mut self,
        io: &mut S,
        buf: Vec<u8>,
        deadline: Instant,
    ) -> Result<Vec<u8>, TlsError> {
        self.handshake_with(io, buf, deadline, |d, buf| d.step(Some(buf), true))
    }

    fn handshake_with<S: Read + Write>(
        &mut self,
        io: &mut S,
        mut buf: Vec<u8>,
        deadline: Instant,
        mut step: impl FnMut(&mut Self, &mut [u8]) -> Step,
    ) -> Result<Vec<u8>, TlsError> {
        let mut total = buf.len();
        let mut need_read = buf.is_empty();
        loop {
            if Instant::now() >= deadline {
                return Err(TlsError::Timeout);
            }
            if need_read {
                if total >= MAX_HANDSHAKE_BYTES {
                    return Err(TlsError::TooLarge);
                }
                let n = read_more(io, &mut buf, Some(deadline))?;
                total += n;
                if total > MAX_HANDSHAKE_BYTES {
                    return Err(TlsError::TooLarge);
                }
                if n == 0 {
                    return Err(TlsError::Io(io::ErrorKind::UnexpectedEof.into()));
                }
            }
            let step = step(self, &mut buf);
            match step.status {
                SEC_E_OK | SEC_I_CONTINUE_NEEDED => {
                    if let Some(t) = &step.token {
                        io.write_all(t.bytes())?;
                        io.flush()?;
                    }
                    buf.drain(..buf.len() - step.extra.min(buf.len()));
                    if step.status == SEC_E_OK {
                        return Ok(buf);
                    }
                    need_read = buf.is_empty();
                }
                SEC_E_INCOMPLETE_MESSAGE => need_read = true,
                err => {
                    if let Some(t) = &step.token {
                        let _ = io.write_all(t.bytes());
                    }
                    return Err(os(self.what(), err));
                }
            }
        }
    }

    fn query<T>(&self, attr: u32, what: &'static str) -> Result<T, TlsError> {
        // SAFETY: `T` is the documented output structure of `attr`.
        let mut v: T = unsafe { mem::zeroed() };
        // SAFETY: a live context and a buffer of the right type.
        let st =
            unsafe { QueryContextAttributesW(&self.ctx.h, attr, ptr::from_mut(&mut v).cast()) };
        if st != SEC_E_OK {
            return Err(os(what, st));
        }
        Ok(v)
    }

    fn session_info(&self) -> Result<SessionInfo, TlsError> {
        let conn: SecPkgContext_ConnectionInfo = self.query(
            SECPKG_ATTR_CONNECTION_INFO,
            "QueryContextAttributes(connection)",
        )?;
        let cipher: SecPkgContext_CipherInfo =
            self.query(SECPKG_ATTR_CIPHER_INFO, "QueryContextAttributes(cipher)")?;
        let alpn: SecPkgContext_ApplicationProtocol = self.query(
            SECPKG_ATTR_APPLICATION_PROTOCOL,
            "QueryContextAttributes(ALPN)",
        )?;
        let version = if conn.dwProtocol & (SP_PROT_TLS1_3_SERVER | SP_PROT_TLS1_3_CLIENT) != 0 {
            TlsVersion::Tls13
        } else if conn.dwProtocol & (SP_PROT_TLS1_2_SERVER | SP_PROT_TLS1_2_CLIENT) != 0 {
            TlsVersion::Tls12
        } else {
            return Err(TlsError::Policy(format!(
                "unexpected protocol 0x{:x}",
                conn.dwProtocol
            )));
        };
        let name_len = cipher
            .szCipherSuite
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(cipher.szCipherSuite.len());
        let alpn = (alpn.ProtoNegoStatus == SecApplicationProtocolNegotiationStatus_Success
            && alpn.ProtoNegoExt == SecApplicationProtocolNegotiationExt_ALPN)
            .then(|| {
                alpn.ProtocolId[..usize::from(alpn.ProtocolIdSize).min(alpn.ProtocolId.len())]
                    .to_vec()
            });
        Ok(SessionInfo {
            version,
            cipher_suite: u16::try_from(cipher.dwCipherSuite).unwrap_or(0),
            cipher_name: String::from_utf16_lossy(&cipher.szCipherSuite[..name_len]),
            alpn,
        })
    }

    /// The established session: sizes, version, suite, ALPN and (client)
    /// the server certificate.
    fn into_stream<S>(self, io: S, leftover: Vec<u8>) -> Result<Stream<S>, TlsError> {
        let sizes: SecPkgContext_StreamSizes = self.query(
            SECPKG_ATTR_STREAM_SIZES,
            "QueryContextAttributes(stream sizes)",
        )?;
        let info = self.session_info()?;
        let (peer_cert, peer_spki) = if self.server {
            (None, None)
        } else {
            let remote: *const CERT_CONTEXT = self.query(
                SECPKG_ATTR_REMOTE_CERT_CONTEXT,
                "QueryContextAttributes(remote cert)",
            )?;
            if remote.is_null() {
                return Err(TlsError::PinMismatch);
            }
            let remote = CertCtx(remote);
            // SAFETY: a live context returned by SChannel; freed by `remote`.
            let der = unsafe {
                std::slice::from_raw_parts(
                    (*remote.0).pbCertEncoded,
                    (*remote.0).cbCertEncoded as usize,
                )
                .to_vec()
            };
            // SAFETY: as above.
            let spki = spki_der(unsafe { &*(*remote.0).pCertInfo }).map_err(crypto_err)?;
            let pin = crate::crypto::spki_sha256(&spki).map_err(crypto_err)?;
            (Some(der), Some(pin))
        };
        Ok(Stream {
            io,
            d: self,
            sizes,
            enc: leftover,
            plain: Vec::new(),
            pos: 0,
            closed: false,
            read_deadline: None,
            empty_records: 0,
            info,
            peer_cert,
            peer_spki,
        })
    }
}

/// An SChannel server credential presenting the listener certificate.
pub(crate) struct Server {
    cred: Arc<Cred>,
}

// SAFETY: SChannel credential handles may be used by several threads; each
// connection gets its own context.
unsafe impl Send for Server {}
// SAFETY: as above.
unsafe impl Sync for Server {}

impl Server {
    pub(crate) fn new(credential: &Credential, offer: Offer) -> Result<Server, TlsError> {
        Ok(Server {
            cred: Arc::new(acquire(true, Some(&credential.cert), offer)?),
        })
    }

    pub(crate) fn accept<S: Read + Write>(
        &self,
        mut io: S,
        deadline: Instant,
    ) -> Result<Stream<S>, TlsError> {
        let mut d = Driver::new(self.cred.clone(), true);
        let leftover = d.handshake(&mut io, Vec::new(), deadline)?;
        d.into_stream(io, leftover)
    }
}

/// An SChannel client credential that validates nothing itself
/// (`SCH_CRED_MANUAL_CRED_VALIDATION`): the caller checks the pin.
pub(crate) struct Client {
    cred: Arc<Cred>,
}

// SAFETY: as for `Server`.
unsafe impl Send for Client {}
// SAFETY: as for `Server`.
unsafe impl Sync for Client {}

impl Client {
    pub(crate) fn new(offer: Offer) -> Result<Client, TlsError> {
        Ok(Client {
            cred: Arc::new(acquire(false, None, offer)?),
        })
    }

    pub(crate) fn connect<S: Read + Write>(
        &self,
        mut io: S,
        deadline: Instant,
    ) -> Result<Stream<S>, TlsError> {
        let mut d = Driver::new(self.cred.clone(), false);
        let hello = d.step(None, true);
        if hello.status != SEC_I_CONTINUE_NEEDED {
            return Err(os("InitializeSecurityContextW", hello.status));
        }
        if let Some(t) = &hello.token {
            io.write_all(t.bytes())?;
            io.flush()?;
        }
        drop(hello);
        let leftover = d.handshake(&mut io, Vec::new(), deadline)?;
        d.into_stream(io, leftover)
    }
}

/// An established SChannel session.
pub(crate) struct Stream<S> {
    io: S,
    d: Driver,
    sizes: SecPkgContext_StreamSizes,
    /// Ciphertext received and not yet decrypted.
    enc: Vec<u8>,
    plain: Vec<u8>,
    pos: usize,
    /// close_notify received.
    closed: bool,
    read_deadline: Option<Instant>,
    empty_records: usize,
    info: SessionInfo,
    peer_cert: Option<Vec<u8>>,
    peer_spki: Option<SpkiSha256>,
}

// SAFETY: the context and credential handles are plain values owned by
// this stream; SSPI allows using a context from any one thread at a time.
unsafe impl<S: Send> Send for Stream<S> {}

/// What one `DecryptMessage` call achieved.
enum Decrypted {
    Progress,
    NeedMore,
}

impl<S> Stream<S> {
    pub(crate) fn set_read_deadline(&mut self, deadline: Option<Instant>) {
        self.read_deadline = deadline;
    }

    fn check_read_deadline(&self) -> io::Result<()> {
        if self.read_deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn count_empty_record(&mut self) -> io::Result<()> {
        self.empty_records += 1;
        if self.empty_records > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many empty TLS records",
            ));
        }
        Ok(())
    }

    pub(crate) fn get_ref(&self) -> &S {
        &self.io
    }

    pub(crate) fn get_mut(&mut self) -> &mut S {
        &mut self.io
    }

    pub(crate) fn session(&self) -> &SessionInfo {
        &self.info
    }

    pub(crate) fn peer_certificate(&self) -> Option<&[u8]> {
        self.peer_cert.as_deref()
    }

    pub(crate) fn peer_spki_sha256(&self) -> Option<SpkiSha256> {
        self.peer_spki
    }
}

impl<S: Read + Write> Stream<S> {
    fn verify_session_unchanged(&self) -> io::Result<()> {
        let info = self.d.session_info().map_err(io::Error::other)?;
        if info != self.info {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TLS session changed after handshake",
            ));
        }
        Ok(())
    }

    fn read_more(&mut self) -> io::Result<usize> {
        self.check_read_deadline()?;
        read_more(&mut self.io, &mut self.enc, self.read_deadline)
    }

    /// Decrypts what `enc` holds: plaintext goes to `plain`, close_notify
    /// sets `closed`, a TLS 1.3 post-handshake message (ticket, key update)
    /// is fed back to ISC/ASC.
    fn decrypt(&mut self) -> io::Result<Decrypted> {
        let mut bufs = [
            sec_buf(SECBUFFER_DATA, &mut self.enc),
            empty_buf(SECBUFFER_EMPTY),
            empty_buf(SECBUFFER_EMPTY),
            empty_buf(SECBUFFER_EMPTY),
        ];
        let d = desc(&mut bufs);
        // SAFETY: a live context; the buffers describe `enc`, decrypted in place.
        let st = unsafe { DecryptMessage(&self.d.ctx.h, &d, 0, ptr::null_mut()) };
        match st {
            // close_notify: SChannel leaves the input buffer as it was (its
            // DATA entry is still the ciphertext), so nothing is read from
            // the buffers and nothing after the alert counts.
            SEC_I_CONTEXT_EXPIRED => {
                self.enc.clear();
                self.closed = true;
                Ok(Decrypted::Progress)
            }
            SEC_E_OK | SEC_I_RENEGOTIATE => {
                let (start, end) = (
                    self.enc.as_ptr() as usize,
                    self.enc.as_ptr() as usize + self.enc.len(),
                );
                let mut extra = 0;
                // bufs[0] turns into the record header; plaintext is a later
                // DATA entry pointing inside `enc`.
                for b in &bufs[1..] {
                    let p = b.pvBuffer as usize;
                    if b.BufferType == SECBUFFER_DATA
                        && b.cbBuffer > 0
                        && p >= start
                        && p + b.cbBuffer as usize <= end
                    {
                        // SAFETY: checked to lie inside `enc`.
                        let data = unsafe {
                            std::slice::from_raw_parts(b.pvBuffer.cast::<u8>(), b.cbBuffer as usize)
                        };
                        self.plain.extend_from_slice(data);
                    } else if b.BufferType == SECBUFFER_EXTRA {
                        extra = b.cbBuffer as usize;
                    }
                }
                let consumed = self.enc.len() - extra.min(self.enc.len());
                self.enc.drain(..consumed);
                if st == SEC_I_RENEGOTIATE {
                    self.post_handshake()?;
                }
                Ok(Decrypted::Progress)
            }
            SEC_E_INCOMPLETE_MESSAGE => Ok(Decrypted::NeedMore),
            err => Err(io_os("DecryptMessage", err)),
        }
    }

    /// `SEC_I_RENEGOTIATE`: TLS 1.3 post-handshake messages go back through
    /// ISC/ASC. A TLS 1.2 renegotiation is refused.
    fn post_handshake(&mut self) -> io::Result<()> {
        if self.info.version != TlsVersion::Tls13 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TLS 1.2 renegotiation refused",
            ));
        }
        loop {
            self.check_read_deadline()?;
            let mut input = std::mem::take(&mut self.enc);
            let mut dummy = [0u8; 1];
            let slice: &mut [u8] = if input.is_empty() {
                &mut dummy[..0]
            } else {
                &mut input
            };
            let step = self.d.step(Some(slice), false);
            match step.status {
                SEC_E_OK | SEC_I_CONTINUE_NEEDED => {
                    if let Some(t) = &step.token {
                        self.io.write_all(t.bytes())?;
                        self.io.flush()?;
                    }
                    input.drain(..input.len() - step.extra.min(input.len()));
                    self.enc = input;
                    if step.status == SEC_E_OK {
                        self.verify_session_unchanged()?;
                        return Ok(());
                    }
                    self.count_empty_record()?;
                    if self.enc.is_empty() && self.read_more()? == 0 {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
                SEC_E_INCOMPLETE_MESSAGE => {
                    self.enc = input;
                    if self.enc.len() >= MAX_CIPHERTEXT {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "TLS post-handshake message too large",
                        ));
                    }
                    if self.read_more()? == 0 {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
                err => return Err(io_os(self.d.what(), err)),
            }
        }
    }

    pub(crate) fn shutdown(&mut self) -> io::Result<()> {
        let mut token = SCHANNEL_SHUTDOWN.to_le_bytes();
        let mut b = [sec_buf(SECBUFFER_TOKEN, &mut token)];
        let d = desc(&mut b);
        // SAFETY: a live context; the token is a DWORD.
        let st = unsafe { ApplyControlToken(&self.d.ctx.h, &d) };
        if st != SEC_E_OK {
            return Err(io_os("ApplyControlToken", st));
        }
        let step = self.d.step(None, false);
        if step.status != SEC_E_OK && step.status != SEC_I_CONTINUE_NEEDED {
            return Err(io_os(self.d.what(), step.status));
        }
        if let Some(t) = &step.token {
            self.io.write_all(t.bytes())?;
        }
        self.io.flush()
    }
}

impl<S: Read + Write> Read for Stream<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            self.check_read_deadline()?;
            if self.pos < self.plain.len() {
                self.empty_records = 0;
                let n = out.len().min(self.plain.len() - self.pos);
                out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
                self.pos += n;
                if self.pos == self.plain.len() {
                    self.plain.clear();
                    self.pos = 0;
                }
                return Ok(n);
            }
            if self.closed {
                return Ok(0);
            }
            if !self.enc.is_empty() && matches!(self.decrypt()?, Decrypted::Progress) {
                if self.plain.is_empty() && !self.closed {
                    self.count_empty_record()?;
                }
                continue;
            }
            if self.enc.len() >= MAX_CIPHERTEXT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "TLS record too large",
                ));
            }
            if self.read_more()? == 0 {
                // End of stream without close_notify: possible truncation.
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
    }
}

impl<S: Read + Write> Write for Stream<S> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        self.write_record(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.io.flush()
    }
}

impl<S: Read + Write> Stream<S> {
    fn write_record(&mut self, data: &[u8]) -> io::Result<usize> {
        let (h, t) = (self.sizes.cbHeader as usize, self.sizes.cbTrailer as usize);
        let n = data.len().min(self.sizes.cbMaximumMessage as usize);
        let mut buf = vec![0u8; h + n + t];
        buf[h..h + n].copy_from_slice(&data[..n]);
        let (hdr, rest) = buf.split_at_mut(h);
        let (body, trl) = rest.split_at_mut(n);
        let mut bufs = [
            sec_buf(SECBUFFER_STREAM_HEADER, hdr),
            sec_buf(SECBUFFER_DATA, body),
            sec_buf(SECBUFFER_STREAM_TRAILER, trl),
            empty_buf(SECBUFFER_EMPTY),
        ];
        let d = desc(&mut bufs);
        // SAFETY: a live context; the buffers cover `buf`, encrypted in place.
        let st = unsafe { EncryptMessage(&self.d.ctx.h, 0, &d, 0) };
        if st != SEC_E_OK {
            return Err(io_os("EncryptMessage", st));
        }
        let (lh, lb, lt) = (
            bufs[0].cbBuffer as usize,
            bufs[1].cbBuffer as usize,
            bufs[2].cbBuffer as usize,
        );
        if lh == h && lb == n {
            self.io.write_all(&buf[..h + n + lt])?;
        } else {
            let mut out = Vec::with_capacity(lh + lb + lt);
            out.extend_from_slice(&buf[..lh]);
            out.extend_from_slice(&buf[h..h + lb]);
            out.extend_from_slice(&buf[h + n..h + n + lt]);
            self.io.write_all(&out)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    thread_local! {
        static DROPS: std::cell::RefCell<Option<Vec<&'static str>>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn record_drop(name: &'static str) {
        DROPS.with(|events| {
            if let Some(events) = &mut *events.borrow_mut() {
                events.push(name);
            }
        });
    }

    fn driver() -> Driver {
        let offer = Offer {
            tls12: true,
            tls13: false,
            cbc_only: false,
        };
        Driver::new(Arc::new(acquire(false, None, offer).unwrap()), false)
    }

    #[test]
    fn handshake_counts_consumed_bytes() {
        let mut d = driver();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut input, _) = listener.accept().unwrap();
        input
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let sender =
            std::thread::spawn(move || peer.write_all(&vec![0; MAX_HANDSHAKE_BYTES + READ_CHUNK]));
        let result = d.handshake_with(
            &mut input,
            Vec::new(),
            Instant::now() + Duration::from_secs(2),
            |_, _| Step {
                status: SEC_I_CONTINUE_NEEDED,
                token: None,
                extra: 0,
            },
        );
        assert!(matches!(result, Err(TlsError::TooLarge)));
        drop(input);
        let _ = sender.join().unwrap();
    }

    #[test]
    fn handshake_clamps_oversized_extra() {
        let mut d = driver();
        let mut input = io::Cursor::new(Vec::new());
        let result = d.handshake_with(
            &mut input,
            vec![0],
            Instant::now() + Duration::from_secs(2),
            |_, _| Step {
                status: SEC_E_OK,
                token: None,
                extra: usize::MAX,
            },
        );
        assert_eq!(result.unwrap(), vec![0]);
    }

    #[test]
    fn handshake_times_out_without_progress() {
        let mut d = driver();
        let mut input = io::Cursor::new(Vec::new());
        let mut calls = 0;
        let result = d.handshake_with(
            &mut input,
            vec![0],
            Instant::now() + Duration::from_millis(20),
            |_, _| {
                calls += 1;
                std::thread::sleep(Duration::from_millis(5));
                Step {
                    status: if calls < 20 {
                        SEC_I_CONTINUE_NEEDED
                    } else {
                        -1
                    },
                    token: None,
                    extra: 1,
                }
            },
        );
        assert!(matches!(result, Err(TlsError::Timeout)));
    }

    #[test]
    fn empty_records_are_bounded_over_loopback() {
        empty_record_peer(false);
        if has_tls13_server() {
            empty_record_peer(true);
        }
    }

    fn empty_record_peer(tls13: bool) {
        use crate::identity::{IdentityParams, KeyStore, ServerIdentity};
        use std::net::{TcpListener, TcpStream};
        let prefix = format!(
            "PodMobileTest-empty-{}-",
            crate::identity::hex(&crate::crypto::random_256().unwrap()[..4])
        );
        let store = KeyStore::current_user(&prefix).unwrap();
        struct Cleanup(KeyStore);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for key in self.0.keys().unwrap() {
                    self.0.delete(&key).unwrap();
                }
            }
        }
        let store = Cleanup(store);
        let id = ServerIdentity::generate(&store.0, &IdentityParams::new([127, 0, 0, 1].into()))
            .unwrap();
        let offer = Offer {
            tls12: !tls13,
            tls13,
            cbc_only: false,
        };
        let server = Server::new(id.credential(), offer).unwrap();
        let client = Client::new(offer).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let c = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (s, _) = listener.accept().unwrap();
        for socket in [&s, &c] {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
        }
        let peer = std::thread::spawn(move || {
            server
                .accept(s, Instant::now() + Duration::from_secs(2))
                .unwrap()
        });
        let mut c = client
            .connect(c, Instant::now() + Duration::from_secs(2))
            .unwrap();
        let mut s = peer.join().unwrap();
        s.verify_session_unchanged().unwrap();
        let suite = s.info.cipher_suite;
        s.info.cipher_suite ^= 1;
        assert_eq!(
            s.verify_session_unchanged().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        s.info.cipher_suite = suite;
        let version = s.info.version;
        s.info.version = if tls13 {
            TlsVersion::Tls12
        } else {
            TlsVersion::Tls13
        };
        assert_eq!(
            s.verify_session_unchanged().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        s.info.version = version;
        // Exercise the live session after its Server has already dropped.
        for _ in 0..32 {
            c.write_record(&[]).unwrap();
        }
        c.write_all(b"x").unwrap();
        assert_eq!(s.read(&mut [0]).unwrap(), 1);
        for _ in 0..33 {
            c.write_record(&[]).unwrap();
        }
        c.io.flush().unwrap();
        assert_eq!(
            s.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        DROPS.with(|events| *events.borrow_mut() = Some(Vec::new()));
        drop(s);
        let events = DROPS.with(|events| events.borrow_mut().take().unwrap());
        assert_eq!(events, ["context", "credential"]);
    }
}
