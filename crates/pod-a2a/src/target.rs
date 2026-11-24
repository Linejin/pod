//! Which URLs this client will contact.
//!
//! HTTPS everywhere; plain HTTP only to a loopback address and only when the
//! caller turned on the explicit development/test switch. No user info, no
//! fragments, ASCII hosts only (use punycode), and an origin that the rest of
//! the client compares exactly.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use http::Uri;

use crate::error::A2aError;

/// The well-known Agent Card path (spec §8.2).
pub const WELL_KNOWN_AGENT_CARD: &str = "/.well-known/agent-card.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Https,
    /// Loopback development/test endpoints only.
    Http,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
        }
    }
}

/// The endpoint policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetPolicy {
    /// Allow `http://` to 127.0.0.0/8, `::1` or `localhost` (dev/test only).
    pub allow_http_loopback: bool,
}

/// `scheme://host:port`, lower case, port always explicit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin(String);

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A URL this client is allowed to contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub scheme: Scheme,
    /// Lower-case host; IPv6 without brackets.
    pub host: String,
    pub port: u16,
    /// Path and query, at least `/`.
    pub path_and_query: String,
}

impl Endpoint {
    /// Parses and checks `url` against `policy`.
    pub fn parse(url: &str, policy: TargetPolicy) -> Result<Self, A2aError> {
        let refuse = |why: &str| A2aError::Target(format!("{why}: {url:?}"));
        if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(refuse("URL contains whitespace or control characters"));
        }
        if !url.is_ascii() {
            return Err(refuse("URL must be ASCII (use punycode for the host)"));
        }
        if url.contains('#') {
            return Err(refuse("URL must not contain a fragment"));
        }
        let uri: Uri = url
            .parse()
            .map_err(|_| refuse("not a valid absolute URL"))?;
        let scheme = match uri.scheme_str().map(str::to_ascii_lowercase).as_deref() {
            Some("https") => Scheme::Https,
            Some("http") => Scheme::Http,
            Some(_) => return Err(refuse("only https URLs are allowed")),
            None => return Err(refuse("URL must be absolute (https://...)")),
        };
        let authority = uri.authority().ok_or_else(|| refuse("URL has no host"))?;
        if authority.as_str().contains('@') {
            return Err(refuse(
                "URL must not contain user info (credentials in URLs are refused)",
            ));
        }
        let raw_host = authority.host();
        let host = raw_host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(raw_host)
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err(refuse("URL has no host"));
        }
        let port = match authority.port_u16() {
            Some(0) => return Err(refuse("port 0 is not allowed")),
            Some(p) => p,
            None => match scheme {
                Scheme::Https => 443,
                Scheme::Http => 80,
            },
        };
        let path_and_query = uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/".into());
        let ep = Self {
            scheme,
            host,
            port,
            path_and_query: if path_and_query.starts_with('/') {
                path_and_query
            } else {
                format!("/{path_and_query}")
            },
        };
        if ep.scheme == Scheme::Http {
            if !ep.is_loopback_host() {
                return Err(refuse(
                    "plain http is refused for non-loopback hosts; use https",
                ));
            }
            if !policy.allow_http_loopback {
                return Err(refuse(
                    "plain http to loopback needs the explicit development switch (--allow-http-loopback)",
                ));
            }
        }
        Ok(ep)
    }

    pub fn origin(&self) -> Origin {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        Origin(format!("{}://{host}:{}", self.scheme.as_str(), self.port))
    }

    /// The `Host` header value (port only when not the scheme default).
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = match self.scheme {
            Scheme::Https => 443,
            Scheme::Http => 80,
        };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// The literal host is loopback (127.0.0.0/8, ::1) or `localhost`;
    /// `localhost` is then only connected to through loopback addresses.
    pub fn is_loopback_host(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<IpAddr>()
                .is_ok_and(|ip| is_loopback_ip(&ip))
    }

    /// The well-known Agent Card URL under this URL's path: the A2A SDKs
    /// resolve `{base}/.well-known/agent-card.json`, so `https://h/` and
    /// `https://h/agents/a` give `https://h/.well-known/agent-card.json` and
    /// `https://h/agents/a/.well-known/agent-card.json`. A query is dropped.
    pub fn well_known_card(&self) -> Self {
        let path = self
            .path_and_query
            .split('?')
            .next()
            .unwrap_or("/")
            .trim_end_matches('/');
        Self {
            path_and_query: format!("{path}{WELL_KNOWN_AGENT_CARD}"),
            ..self.clone()
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = match self.scheme {
            Scheme::Https => 443,
            Scheme::Http => 80,
        };
        if self.port == default {
            write!(
                f,
                "{}://{host}{}",
                self.scheme.as_str(),
                self.path_and_query
            )
        } else {
            write!(
                f,
                "{}://{host}:{}{}",
                self.scheme.as_str(),
                self.port,
                self.path_and_query
            )
        }
    }
}

pub(crate) fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4: Ipv4Addr| v4.is_loopback())
                || *v6 == Ipv6Addr::LOCALHOST
        }
    }
}
