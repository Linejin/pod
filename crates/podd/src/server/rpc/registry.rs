//! The method registry: each method name with how it uses the connection
//! ([`Mode`]) and its handler, plus the domain dispatches that answer names
//! nobody registered ([`Claim`]). The tables are constants (`domains.rs`);
//! a name registered twice fails the build there ([`duplicate`]). The lookup
//! index is built once and only read afterwards.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::protocol::{Method, RpcError, codes};
use crate::server::Daemon;

/// What a handler returns.
pub(in crate::server) type RpcFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send + 'a>>;

/// A method handler: the daemon, the method name (a domain dispatch matches
/// on it) and the raw params.
pub(in crate::server) type RpcHandler = for<'a> fn(Arc<Daemon>, &'a str, Value) -> RpcFuture<'a>;

/// How a method uses its connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::server) enum Mode {
    /// One request, one response ([`Daemon::handle_request`]).
    Unary,
    /// `terminal.attach`: the connection becomes the terminal's output stream.
    Attach,
    /// `status.subscribe`: the connection becomes a status event stream.
    Subscribe,
}

/// One registered method.
#[derive(Clone, Copy)]
pub(in crate::server) struct MethodRegistration {
    pub(in crate::server) name: &'static str,
    pub(in crate::server) mode: Mode,
    /// Serves the method as one request. For a stream method this answers
    /// what the connection does not switch to a stream (a notification).
    pub(in crate::server) handler: RpcHandler,
}

impl MethodRegistration {
    pub(in crate::server) const fn new(
        name: &'static str,
        mode: Mode,
        handler: RpcHandler,
    ) -> Self {
        Self {
            name,
            mode,
            handler,
        }
    }

    /// `M` as a one-request method.
    pub(in crate::server) const fn unary<M: Method>(handler: RpcHandler) -> Self {
        Self::new(M::NAME, Mode::Unary, handler)
    }

    /// `M` as a stream method.
    pub(in crate::server) const fn stream<M: Method>(mode: Mode, handler: RpcHandler) -> Self {
        Self::new(M::NAME, mode, handler)
    }
}

/// A domain dispatch that also answers the names it claims but nobody
/// registered, with the domain's own answer (as before the registry). Only an
/// unregistered name reaches a claim.
#[derive(Clone, Copy)]
pub(in crate::server) struct Claim {
    /// The domain's own test of its names.
    pub(in crate::server) handles: fn(&str) -> bool,
    pub(in crate::server) handler: RpcHandler,
}

/// The error for a name no handler serves.
pub(in crate::server) fn unknown_method(method: &str) -> RpcError {
    RpcError::new(
        codes::METHOD_NOT_FOUND,
        format!("unknown method {method:?}"),
    )
}

/// The first name registered twice across `tables`, for a build-time check.
pub(in crate::server) const fn duplicate(tables: &[&[MethodRegistration]]) -> Option<&'static str> {
    // Every pair (table a, entry i) < (table b, entry j), in order.
    let mut a = 0;
    while a < tables.len() {
        let mut i = 0;
        while i < tables[a].len() {
            let name = tables[a][i].name;
            let mut b = a;
            let mut j = i + 1;
            while b < tables.len() {
                while j < tables[b].len() {
                    if same(name, tables[b][j].name) {
                        return Some(name);
                    }
                    j += 1;
                }
                b += 1;
                j = 0;
            }
            i += 1;
        }
        a += 1;
    }
    None
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The lookup index over the tables.
pub(in crate::server) struct RpcRegistry {
    methods: HashMap<&'static str, MethodRegistration>,
    claims: &'static [Claim],
}

impl RpcRegistry {
    pub(in crate::server) fn new(
        tables: &[&'static [MethodRegistration]],
        claims: &'static [Claim],
    ) -> Self {
        let mut methods = HashMap::new();
        for r in tables.iter().flat_map(|t| t.iter()) {
            let earlier = methods.insert(r.name, *r);
            assert!(
                earlier.is_none(),
                "rpc method {:?} registered twice",
                r.name
            );
        }
        Self { methods, claims }
    }

    pub(in crate::server) fn get(&self, method: &str) -> Option<&MethodRegistration> {
        self.methods.get(method)
    }

    /// The claim that answers `method` when nobody registered it.
    pub(in crate::server) fn claim(&self, method: &str) -> Option<&Claim> {
        self.claims.iter().find(|c| (c.handles)(method))
    }

    /// The handler of `method`: its registration, else the claim of its
    /// domain; `None` for an unknown name.
    pub(in crate::server) fn handler(&self, method: &str) -> Option<RpcHandler> {
        match self.get(method) {
            Some(r) => Some(r.handler),
            None => self.claim(method).map(|c| c.handler),
        }
    }

    /// The registered names.
    #[cfg(test)]
    pub(in crate::server) fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.methods.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(_: Arc<Daemon>, _: &str, _: Value) -> RpcFuture<'_> {
        Box::pin(async { Ok(Value::Null) })
    }

    const fn reg(name: &'static str) -> MethodRegistration {
        MethodRegistration::new(name, Mode::Unary, ok)
    }

    const A: &[MethodRegistration] = &[reg("a.one"), reg("a.two")];
    const B: &[MethodRegistration] = &[reg("b.one"), reg("b.two"), reg("b.three")];
    const CLAIMS: &[Claim] = &[Claim {
        handles: |m| m.starts_with("b."),
        handler: ok,
    }];

    #[test]
    fn duplicate_names_are_found_within_and_across_tables() {
        assert_eq!(duplicate(&[A, B]), None);
        assert_eq!(duplicate(&[]), None);
        assert_eq!(duplicate(&[A, &[reg("x"), reg("a.two")]]), Some("a.two"));
        assert_eq!(duplicate(&[&[reg("x"), reg("y"), reg("x")]]), Some("x"));
        assert_eq!(duplicate(&[A, B, &[reg("b.three")]]), Some("b.three"));
        // Equal length, different bytes; a prefix is not a duplicate.
        assert_eq!(
            duplicate(&[&[reg("a.one"), reg("a.two"), reg("a.on")]]),
            None
        );
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn the_index_refuses_a_duplicate() {
        const AGAIN: &[MethodRegistration] = &[reg("a.one")];
        RpcRegistry::new(&[A, AGAIN], &[]);
    }

    #[test]
    fn a_registration_comes_before_a_claim() {
        let r = RpcRegistry::new(&[A, B], CLAIMS);
        assert_eq!(r.get("b.two").map(|m| m.name), Some("b.two"));
        assert!(r.handler("b.two").is_some());
        // Unregistered: the claim answers, or nobody.
        assert!(r.get("b.nope").is_none());
        assert!(r.claim("b.nope").is_some());
        assert!(r.handler("b.nope").is_some());
        assert!(r.handler("a.nope").is_none());
        assert!(r.handler("").is_none());
        let mut names: Vec<_> = r.names().collect();
        names.sort();
        assert_eq!(names, ["a.one", "a.two", "b.one", "b.three", "b.two"]);
    }

    #[test]
    fn unknown_method_text() {
        let e = unknown_method("x.y");
        assert_eq!(e.code, codes::METHOD_NOT_FOUND);
        assert_eq!(e.message, "unknown method \"x.y\"");
    }
}
