//! One bounded HTTP/1.1 exchange per call.
//!
//! A fresh connection per request: no pool, no cookies, no proxy, no
//! automatic retry, and redirects are reported, never followed, so a request
//! (and any credential on it) only ever reaches the endpoint that was
//! checked. TLS is rustls with the OS trust store; plain TCP only for
//! loopback endpoints that `Endpoint::parse` admitted.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST, LOCATION, USER_AGENT,
    WWW_AUTHENTICATE,
};
use http::{HeaderValue, Method, Request};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::error::{A2aError, Delivery};
use crate::model::PROTOCOL_VERSION;
use crate::sanitize_line;
use crate::target::{Endpoint, Scheme, is_loopback_ip};

/// Header carrying the protocol version (spec §3.2.6, §3.6.1).
pub const A2A_VERSION_HEADER: &str = "A2A-Version";
const USER_AGENT_VALUE: &str = concat!("pod-a2a/", env!("CARGO_PKG_VERSION"));

/// Limits for one exchange.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub connect_timeout: Duration,
    /// From the first request byte to the last response byte.
    pub response_timeout: Duration,
    pub max_response_bytes: usize,
}

pub(crate) struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// A bearer credential. Never printed: `Debug` is redacted.
#[derive(Clone)]
pub struct BearerToken(HeaderValue);

impl BearerToken {
    /// Checks `token` (visible ASCII, no spaces, at most 8 KiB) and builds
    /// the sensitive `Authorization: Bearer` value.
    pub fn new(token: &str) -> Result<Self, A2aError> {
        let t = token.trim();
        if t.is_empty() {
            return Err(A2aError::Invalid("the bearer token is empty".into()));
        }
        if t.len() > 8192 {
            return Err(A2aError::Invalid(
                "the bearer token is longer than 8 KiB".into(),
            ));
        }
        if !t.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(A2aError::Invalid(
                "the bearer token must be visible ASCII without spaces".into(),
            ));
        }
        let mut v = HeaderValue::from_str(&format!("Bearer {t}")).map_err(|_| {
            A2aError::Invalid("the bearer token is not a valid header value".into())
        })?;
        v.set_sensitive(true);
        Ok(Self(v))
    }
}

impl std::fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

/// rustls client configuration: ring provider, safe default protocol
/// versions (TLS 1.2/1.3), roots from the OS trust store, ALPN http/1.1.
pub(crate) fn tls_config() -> Result<Arc<rustls::ClientConfig>, A2aError> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(loaded.certs);
    if added == 0 {
        // SSL_CERT_FILE / SSL_CERT_DIR replace the OS store when set.
        let overridden = ["SSL_CERT_FILE", "SSL_CERT_DIR"]
            .into_iter()
            .filter(|v| std::env::var_os(v).is_some())
            .collect::<Vec<_>>();
        return Err(A2aError::Transport {
            delivery: Delivery::NotSent,
            detail: format!(
                "no trusted root certificates could be loaded{}{}",
                if overridden.is_empty() {
                    " from the OS trust store".to_string()
                } else {
                    format!(
                        " ({} is set and replaces the OS trust store)",
                        overridden.join(", ")
                    )
                },
                loaded
                    .errors
                    .first()
                    .map(|e| format!(": {}", sanitize_line(&e.to_string(), 300)))
                    .unwrap_or_default()
            ),
        });
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| A2aError::Transport {
            delivery: Delivery::NotSent,
            detail: format!("TLS setup: {e}"),
        })?
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

pub(crate) struct Exchange<'a> {
    pub method: Method,
    pub endpoint: &'a Endpoint,
    pub accept: &'static str,
    pub body: Option<Vec<u8>>,
    pub bearer: Option<&'a BearerToken>,
}

/// Performs one exchange. Errors before the request is written are
/// `Delivery::NotSent`; any later failure is `Delivery::Unknown`.
pub(crate) async fn exchange(
    tls: Option<&Arc<rustls::ClientConfig>>,
    x: Exchange<'_>,
    limits: Limits,
) -> Result<HttpResponse, A2aError> {
    let ep = x.endpoint;
    let not_sent = |detail: String| A2aError::Transport {
        delivery: Delivery::NotSent,
        detail,
    };
    let tcp = match tokio::time::timeout(limits.connect_timeout, connect_tcp(ep)).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(A2aError::Timeout {
                delivery: Delivery::NotSent,
                after: limits.connect_timeout,
            });
        }
    };
    let request = build_request(&x).map_err(not_sent)?;
    match ep.scheme {
        Scheme::Http => send_over(tcp, request, limits).await,
        Scheme::Https => {
            let tls =
                tls.ok_or_else(|| not_sent("no TLS configuration for an https endpoint".into()))?;
            let name = ServerName::try_from(ep.host.clone())
                .map_err(|_| not_sent(format!("{:?} is not a valid TLS server name", ep.host)))?;
            let connector = TlsConnector::from(tls.clone());
            let stream =
                match tokio::time::timeout(limits.connect_timeout, connector.connect(name, tcp))
                    .await
                {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        return Err(not_sent(format!("TLS handshake with {}: {e}", ep.origin())));
                    }
                    Err(_) => {
                        return Err(A2aError::Timeout {
                            delivery: Delivery::NotSent,
                            after: limits.connect_timeout,
                        });
                    }
                };
            send_over(stream, request, limits).await
        }
    }
}

async fn connect_tcp(ep: &Endpoint) -> Result<TcpStream, A2aError> {
    let not_sent = |detail: String| A2aError::Transport {
        delivery: Delivery::NotSent,
        detail,
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((ep.host.as_str(), ep.port))
        .await
        .map_err(|e| not_sent(format!("cannot resolve {}: {e}", ep.host)))?
        .collect();
    // Plain HTTP was admitted for loopback only: connect to loopback
    // addresses only, whatever `localhost` resolves to.
    let addrs: Vec<SocketAddr> = if ep.scheme == Scheme::Http {
        addrs
            .into_iter()
            .filter(|a| is_loopback_ip(&a.ip()))
            .collect()
    } else {
        addrs
    };
    if addrs.is_empty() {
        return Err(not_sent(format!(
            "{} resolves to no usable address",
            ep.host
        )));
    }
    let mut last = None;
    for a in addrs {
        match TcpStream::connect(a).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(not_sent(format!(
        "cannot connect to {}: {}",
        ep.origin(),
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

fn build_request(x: &Exchange<'_>) -> Result<Request<Full<Bytes>>, String> {
    let body = x.body.clone().unwrap_or_default();
    let mut b = Request::builder()
        .method(x.method.clone())
        .uri(x.endpoint.path_and_query.as_str())
        .header(HOST, x.endpoint.host_header())
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT, x.accept)
        .header(A2A_VERSION_HEADER, PROTOCOL_VERSION);
    if x.body.is_some() {
        b = b
            .header(CONTENT_TYPE, "application/json")
            .header(CONTENT_LENGTH, body.len());
    }
    if let Some(token) = x.bearer {
        b = b.header(AUTHORIZATION, token.0.clone());
    }
    b.body(Full::new(Bytes::from(body)))
        .map_err(|e| format!("cannot build the request: {e}"))
}

async fn send_over<IO>(
    io: IO,
    request: Request<Full<Bytes>>,
    limits: Limits,
) -> Result<HttpResponse, A2aError>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let unknown = |detail: String| A2aError::Transport {
        delivery: Delivery::Unknown,
        detail,
    };
    let work = async {
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
            .await
            .map_err(|e| unknown(format!("HTTP handshake: {e}")))?;
        let driver = tokio::spawn(conn);
        let resp = sender
            .send_request(request)
            .await
            .map_err(|e| unknown(format!("HTTP exchange: {e}")))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if let Some(len) = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            && len > limits.max_response_bytes as u64
        {
            driver.abort();
            return Err(A2aError::TooLarge {
                limit: limits.max_response_bytes,
            });
        }
        let collected = Limited::new(resp.into_body(), limits.max_response_bytes)
            .collect()
            .await;
        driver.abort();
        let body = match collected {
            Ok(c) => c.to_bytes().to_vec(),
            Err(e) if e.is::<http_body_util::LengthLimitError>() => {
                return Err(A2aError::TooLarge {
                    limit: limits.max_response_bytes,
                });
            }
            Err(e) => return Err(unknown(format!("reading the response: {e}"))),
        };
        if (300..400).contains(&status) {
            return Err(A2aError::Redirect {
                status,
                location: headers
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|l| sanitize_line(l, 200)),
            });
        }
        if status == 401 || status == 403 {
            return Err(A2aError::AuthRequired {
                status,
                challenges: headers
                    .get_all(WWW_AUTHENTICATE)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .map(|c| {
                        // The scheme name only: parameters may echo tokens.
                        sanitize_line(c.split_whitespace().next().unwrap_or(""), 40)
                    })
                    .filter(|c| !c.is_empty())
                    .collect(),
            });
        }
        Ok(HttpResponse {
            status,
            content_type: headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string()),
            body,
        })
    };
    match tokio::time::timeout(limits.response_timeout, work).await {
        Ok(r) => r,
        Err(_) => Err(A2aError::Timeout {
            delivery: Delivery::Unknown,
            after: limits.response_timeout,
        }),
    }
}

/// The media type of a `Content-Type` value, lower case, parameters dropped.
pub(crate) fn media_type(ct: Option<&str>) -> String {
    ct.unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}
