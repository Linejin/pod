//! Endpoint policy and interface selection: which URLs the client will
//! contact, and that an Agent Card cannot redirect it (or a credential) to
//! a host the user did not name.

use pod_a2a::model::{AgentCard, AgentInterface};
use pod_a2a::target::{Endpoint, Scheme, TargetPolicy};
use pod_a2a::{A2aError, Discovery};

const STRICT: TargetPolicy = TargetPolicy {
    allow_http_loopback: false,
};
const DEV: TargetPolicy = TargetPolicy {
    allow_http_loopback: true,
};

fn refused(url: &str, policy: TargetPolicy) -> String {
    match Endpoint::parse(url, policy) {
        Err(A2aError::Target(why)) => why,
        other => panic!("{url}: expected a refusal, got {other:?}"),
    }
}

#[test]
fn https_is_accepted_and_normalised() {
    let e = Endpoint::parse("HTTPS://Agent.Example.COM/a2a/v1?x=1", STRICT).unwrap();
    assert_eq!(e.scheme, Scheme::Https);
    assert_eq!(e.host, "agent.example.com");
    assert_eq!(e.port, 443);
    assert_eq!(e.path_and_query, "/a2a/v1?x=1");
    assert_eq!(e.origin().to_string(), "https://agent.example.com:443");
    assert_eq!(e.to_string(), "https://agent.example.com/a2a/v1?x=1");
    let v6 = Endpoint::parse("https://[2001:db8::1]:8443/", STRICT).unwrap();
    assert_eq!(v6.host, "2001:db8::1");
    assert_eq!(v6.host_header(), "[2001:db8::1]:8443");
}

#[test]
fn unsafe_urls_are_refused() {
    assert!(refused("http://agent.example.com/", DEV).contains("non-loopback"));
    assert!(refused("http://10.0.0.5:8080/", DEV).contains("non-loopback"));
    assert!(refused("http://127.0.0.1:8080/", STRICT).contains("--allow-http-loopback"));
    assert!(refused("https://user:pw@agent.example.com/", STRICT).contains("user info"));
    assert!(refused("https://token@agent.example.com/", STRICT).contains("user info"));
    assert!(refused("https://agent.example.com/#frag", STRICT).contains("fragment"));
    assert!(refused("ftp://agent.example.com/", STRICT).contains("https"));
    assert!(!refused("file:///C:/x", STRICT).is_empty());
    assert!(refused("agent.example.com/a2a", STRICT).contains("absolute"));
    assert!(refused("https://agent.exam ple.com/", STRICT).contains("whitespace"));
    assert!(refused("https://bücher.example/", STRICT).contains("ASCII"));
    assert!(refused("https://agent.example.com:0/", STRICT).contains("port 0"));
}

#[test]
fn loopback_http_only_with_the_switch() {
    for url in [
        "http://127.0.0.1:9/",
        "http://127.8.9.10:9/",
        "http://[::1]:9/",
        "http://localhost:9/",
    ] {
        let e = Endpoint::parse(url, DEV).unwrap();
        assert!(e.is_loopback_host(), "{url}");
        assert_eq!(e.scheme, Scheme::Http);
    }
}

#[test]
fn well_known_card_location() {
    let at = |u: &str| {
        Endpoint::parse(u, STRICT)
            .unwrap()
            .well_known_card()
            .to_string()
    };
    assert_eq!(
        at("https://a.example"),
        "https://a.example/.well-known/agent-card.json"
    );
    assert_eq!(
        at("https://a.example/"),
        "https://a.example/.well-known/agent-card.json"
    );
    assert_eq!(
        at("https://a.example:8443/agents/x/?q=1"),
        "https://a.example:8443/agents/x/.well-known/agent-card.json"
    );
}

fn iface(url: &str, binding: &str, version: &str) -> AgentInterface {
    AgentInterface {
        url: url.into(),
        protocol_binding: binding.into(),
        protocol_version: version.into(),
        tenant: String::new(),
    }
}

fn discovery(card_url: &str, ifaces: Vec<AgentInterface>) -> Discovery {
    Discovery {
        card_url: Endpoint::parse(card_url, DEV).unwrap(),
        card: AgentCard {
            name: "a".into(),
            supported_interfaces: ifaces,
            ..AgentCard::default()
        },
    }
}

#[test]
fn selection_takes_the_first_jsonrpc_1_0_interface_on_the_card_origin() {
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![
            iface("https://a.example/grpc", "GRPC", "1.0"),
            iface("https://a.example/v03", "JSONRPC", "0.3"),
            iface("https://a.example/rest", "HTTP+JSON", "1.0"),
            iface("https://a.example/rpc", "JSONRPC", "1.0.1"),
            iface("https://a.example/rpc2", "JSONRPC", "1.0"),
        ],
    );
    let s = d.select(None, STRICT).unwrap();
    assert_eq!((s.index, s.url.as_str()), (3, "https://a.example/rpc"));
    assert!(!s.explicit);
    // An explicit endpoint picks a later advertised one.
    let s = d.select(Some("https://a.example/rpc2"), STRICT).unwrap();
    assert_eq!((s.index, s.explicit), (4, true));
}

#[test]
fn tenant_of_the_selected_interface_is_kept() {
    let mut i = iface("https://a.example/rpc", "JSONRPC", "1.0");
    i.tenant = "acme".into();
    let d = discovery("https://a.example/.well-known/agent-card.json", vec![i]);
    assert_eq!(d.select(None, STRICT).unwrap().tenant, "acme");
}

#[test]
fn a_card_cannot_send_the_client_to_another_origin() {
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![iface("https://evil.example/rpc", "JSONRPC", "1.0")],
    );
    let e = d.select(None, STRICT).unwrap_err();
    assert!(
        matches!(e, A2aError::CrossOrigin { ref interface, .. } if interface == "https://evil.example/rpc")
    );
    // Other port, other scheme: other origin.
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![iface("https://a.example:8443/rpc", "JSONRPC", "1.0")],
    );
    assert!(matches!(
        d.select(None, STRICT),
        Err(A2aError::CrossOrigin { .. })
    ));
    // Named explicitly by the user, it is allowed.
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![iface("https://rpc.a.example/rpc", "JSONRPC", "1.0")],
    );
    let s = d.select(Some("https://rpc.a.example/rpc"), STRICT).unwrap();
    assert_eq!(s.url, "https://rpc.a.example/rpc");
}

#[test]
fn explicit_endpoint_must_be_advertised_and_compatible() {
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![
            iface("https://a.example/rpc", "JSONRPC", "1.0"),
            iface("https://a.example/grpc", "GRPC", "1.0"),
        ],
    );
    let e = d
        .select(Some("https://other.example/rpc"), STRICT)
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("not a JSONRPC 1.0 interface advertised")
    );
    assert!(d.select(Some("https://a.example/grpc"), STRICT).is_err());
}

#[test]
fn no_https_downgrade_and_no_unsafe_interface_urls() {
    // A loopback https card advertising an http interface on loopback.
    let d = discovery(
        "https://127.0.0.1:8443/.well-known/agent-card.json",
        vec![iface("http://127.0.0.1:8080/rpc", "JSONRPC", "1.0")],
    );
    let e = d
        .select(Some("http://127.0.0.1:8080/rpc"), DEV)
        .unwrap_err();
    assert!(e.to_string().contains("downgrade"), "{e}");
    // Interface URLs go through the same policy.
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![iface("https://u:p@a.example/rpc", "JSONRPC", "1.0")],
    );
    assert!(matches!(d.select(None, STRICT), Err(A2aError::Target(_))));
}

#[test]
fn only_0_3_or_other_bindings_is_no_compatible_interface() {
    let d = discovery(
        "https://a.example/.well-known/agent-card.json",
        vec![
            iface("https://a.example/v03", "JSONRPC", "0.3"),
            iface("https://a.example/grpc", "GRPC", "1.0"),
        ],
    );
    let A2aError::NoCompatibleInterface { advertised } = d.select(None, STRICT).unwrap_err() else {
        panic!()
    };
    assert_eq!(advertised.len(), 2);
}
