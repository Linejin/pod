//! Direct lifecycle seams: no listener, agent, or user configuration.
#![cfg(feature = "mobile-lan")]

use std::sync::atomic::{AtomicU64, Ordering};

use podd::protocol::MobileCap;
use podd::server::mobile::registry::Registry;
use podd::server::mobile::session::{Clock, SESSION_ABSOLUTE_MS, SESSION_IDLE_MS, Sessions};

struct TestClock(AtomicU64);

impl TestClock {
    fn set(&self, now: u64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn registry() -> Registry {
    // Persisted fixture shape avoids depending on the debug pairing stub.
    serde_json::from_value(serde_json::json!({
        "version": 1, "counter": 1, "settings": {"audit_message_bodies": false},
        "devices": [{"id":"m1", "name":"fixture", "public_key":"fixture-key",
            "created_ms":0, "auth_epoch":1, "policy":{}}]
    }))
    .unwrap()
}

#[test]
fn backwards_clock_after_poll_ends_all_sessions_permanently() {
    let clock = TestClock(AtomicU64::new(100));
    let reg = registry();
    let mut sessions = Sessions::new(7);
    let a = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    let b = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    clock.set(200);
    sessions.check(&reg, &a, clock.now_ms(), false).unwrap();
    clock.set(199);
    assert!(sessions.check(&reg, &a, clock.now_ms(), false).is_err());
    assert_eq!(sessions.count("m1"), 0);
    clock.set(201);
    assert!(sessions.check(&reg, &b, clock.now_ms(), true).is_err());
}

#[test]
fn idle_absolute_sleep_and_off_boundaries() {
    let clock = TestClock(AtomicU64::new(1));
    let reg = registry();
    let mut sessions = Sessions::new(7);
    let id = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    clock.set(SESSION_IDLE_MS);
    sessions.check(&reg, &id, clock.now_ms(), false).unwrap();
    clock.set(SESSION_IDLE_MS + 1);
    assert!(sessions.check(&reg, &id, clock.now_ms(), false).is_err());

    let start = clock.now_ms();
    let id = sessions.open(&reg, "m1", start).unwrap();
    for elapsed in (60_000..SESSION_ABSOLUTE_MS).step_by(60_000) {
        clock.set(start + elapsed);
        sessions.check(&reg, &id, clock.now_ms(), true).unwrap();
    }
    clock.set(start + SESSION_ABSOLUTE_MS);
    assert!(sessions.check(&reg, &id, clock.now_ms(), true).is_err());

    let id = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    clock.set(clock.now_ms() + SESSION_IDLE_MS); // sleep counts as elapsed time
    assert!(sessions.check(&reg, &id, clock.now_ms(), true).is_err());
    let id = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    sessions.clear(); // off or uncertain resume
    assert!(sessions.check(&reg, &id, clock.now_ms(), true).is_err());
    let id = sessions.open(&reg, "m1", clock.now_ms()).unwrap();
    let mut restarted = Sessions::new(8);
    assert!(restarted.check(&reg, &id, clock.now_ms(), false).is_err());
}

#[test]
fn oldest_means_numeric_creation_order_when_timestamps_tie() {
    let reg = registry();
    let mut sessions = Sessions::new(7);
    let mut ids = Vec::new();
    for _ in 0..14 {
        ids.push(sessions.open(&reg, "m1", 1).unwrap());
    }
    assert_eq!(sessions.count("m1"), 4);
    for id in &ids[..10] {
        assert!(sessions.check(&reg, id, 1, false).is_err());
    }
    for id in &ids[10..] {
        sessions.check(&reg, id, 1, false).unwrap();
    }
}

#[test]
fn near_u64_clock_limit_does_not_wrap_expiry() {
    let reg = registry();
    let mut sessions = Sessions::new(7);
    let id = sessions.open(&reg, "m1", u64::MAX - 10).unwrap();
    sessions.check(&reg, &id, u64::MAX, false).unwrap();
}

#[test]
fn narrowing_bumps_epoch_but_widening_and_noop_do_not() {
    let mut reg = registry();
    reg.set_policy(
        "m1",
        "t1",
        &[MobileCap::Status, MobileCap::Approve],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(reg.device("m1").unwrap().auth_epoch, 1);
    let mut sessions = Sessions::new(7);
    let id = sessions.open(&reg, "m1", 0).unwrap();
    reg.set_policy("m1", "t1", &[], &[MobileCap::Approve], false)
        .unwrap();
    assert_eq!(reg.device("m1").unwrap().auth_epoch, 2);
    assert!(sessions.check(&reg, &id, 1, false).is_err());
    reg.set_policy("m1", "t1", &[], &[MobileCap::Approve], false)
        .unwrap();
    assert_eq!(reg.device("m1").unwrap().auth_epoch, 2);
    reg.set_policy("m1", "t1", &[], &[], true).unwrap();
    assert_eq!(reg.device("m1").unwrap().auth_epoch, 3);
}

#[test]
fn revoke_storage_failure_is_latched_and_not_serialized() {
    let mut reg = registry();
    let dir = std::env::temp_dir().join(format!(
        "pod-p2-registry-{}-{}",
        std::process::id(),
        podd::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    reg.save(&dir).unwrap();
    reg.set_policy("m1", "t1", &[MobileCap::Status], &[], false)
        .unwrap();
    reg.set_policy("m1", "t1", &[], &[], true).unwrap();
    reg.save(&dir).unwrap();
    assert_eq!(Registry::load(&dir).device("m1").unwrap().auth_epoch, 2);
    let mut other = reg.devices[0].clone();
    other.id = "m2".into();
    reg.devices.push(other);
    let mut sessions = Sessions::new(7);
    let id = sessions.open(&reg, "m2", 0).unwrap();
    reg.revoke("m1", 1).unwrap();
    // A directory at the temporary file path forces a deterministic write failure.
    let obstacle = Registry::path(&dir).with_extension("json.tmp");
    std::fs::create_dir(&obstacle).unwrap();
    assert!(reg.save(&dir).is_err());
    assert!(reg.storage_failed);
    assert!(sessions.check(&reg, &id, 1, false).is_err());
    assert_eq!(sessions.count("m1"), 0);
    assert_eq!(sessions.count("m2"), 0);
    assert!(sessions.open(&reg, "m2", 1).is_err());
    assert!(!reg.device("m1").unwrap().active());
    assert!(
        !serde_json::to_string(&reg)
            .unwrap()
            .contains("storage_failed")
    );
    std::fs::remove_dir(&obstacle).unwrap();
    reg.save(&dir).unwrap();
    assert!(
        reg.storage_failed,
        "a successful retry must not silently enable mobile"
    );
    assert!(sessions.open(&reg, "m2", 2).is_err());
    assert_eq!(Registry::load(&dir).device("m1").unwrap().auth_epoch, 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn epoch_exhaustion_does_not_mutate_policy_or_revocation() {
    let mut reg = registry();
    reg.set_policy("m1", "t1", &[MobileCap::Status], &[], false)
        .unwrap();
    reg.devices[0].auth_epoch = u64::MAX;
    let before = reg.devices[0].clone();
    assert!(reg.set_policy("m1", "t1", &[], &[], true).is_err());
    assert_eq!(reg.devices[0], before);
    assert!(reg.revoke("m1", 1).is_err());
    assert_eq!(reg.devices[0], before);
}

use base64::{Engine, engine::general_purpose::STANDARD};
use pod_mobile::crypto::SoftwareSigner;
use podd::protocol::mobile_v1 as wire;
use podd::server::mobile::{
    Mobile,
    auth::{Auth, Credentials},
};
use std::sync::Arc;

fn auth_fixture() -> (
    std::path::PathBuf,
    Arc<TestClock>,
    Arc<Mobile>,
    Auth,
    SoftwareSigner,
) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/mobile-auth-fixtures")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
    let clock = Arc::new(TestClock(AtomicU64::new(1000)));
    let mobile = Arc::new(Mobile::load(&dir, clock.clone()));
    let signer = SoftwareSigner::generate().unwrap();
    {
        let mut state = mobile.state.lock().unwrap();
        state.registry = registry();
        state.registry.devices[0].public_key = STANDARD.encode(signer.spki_der());
        state
            .registry
            .set_policy(
                "m1",
                "t1",
                &[MobileCap::Status, MobileCap::Approve],
                &[],
                false,
            )
            .unwrap();
    }
    let auth = Auth::new(
        mobile.clone(),
        wire::Id::new("machine").unwrap(),
        wire::Id::new("boot").unwrap(),
    );
    (dir, clock, mobile, auth, signer)
}
fn open_request(c: &wire::Challenge, signer: &SoftwareSigner) -> wire::SessionOpen {
    let mut r = wire::SessionOpen {
        device: c.subject.clone(),
        auth_epoch: c.auth_epoch,
        nonce: c.nonce.clone(),
        signature: wire::Text::new("").unwrap(),
    };
    r.signature =
        wire::Text::new(STANDARD.encode(signer.sign(&wire::session_preimage(c, &r)).unwrap()))
            .unwrap();
    r
}
fn open_session(auth: &Auth, signer: &SoftwareSigner, clock: &TestClock) -> wire::Session {
    let c = auth.challenge(&wire::Id::new("m1").unwrap()).unwrap();
    auth.open(open_request(&c, signer), c.csrf.as_str(), clock.now_ms())
        .unwrap()
}
fn credentials(s: &wire::Session) -> Credentials<'_> {
    Credentials {
        bearer: s.bearer.as_str(),
        csrf: s.csrf.as_str(),
    }
}

#[test]
fn bearer_and_csrf_are_separate_secrets_and_identifiers_never_authenticate() {
    let (_dir, clock, mobile, auth, signer) = auth_fixture();
    let s = open_session(&auth, &signer, &clock);
    assert_eq!(s.bearer.as_str().len(), 64);
    assert_eq!(s.csrf.as_str().len(), 64);
    assert!(s.bearer != s.csrf);
    let ctx = auth.authenticate(credentials(&s)).unwrap();
    assert_eq!(ctx.device(), "m1");
    assert!(ctx.session().starts_with("s3e8-"));
    for (bearer, csrf) in [
        ("0".repeat(64), s.csrf.as_str().into()),
        (s.bearer.as_str().into(), "0".repeat(64)),
        (s.csrf.as_str().into(), s.bearer.as_str().into()),
        (ctx.session().into(), s.csrf.as_str().into()),
        ("m1".into(), s.csrf.as_str().into()),
    ] {
        assert_eq!(
            auth.authenticate(Credentials {
                bearer: &bearer,
                csrf: &csrf
            })
            .unwrap_err()
            .code,
            wire::ErrorCode::Unauthenticated
        );
    }
    assert_eq!(mobile.state.lock().unwrap().sessions.count("m1"), 1);
    auth.recheck(&ctx, clock.now_ms()).unwrap();
}

#[test]
fn auth_recheck_never_refreshes_idle_and_enforces_epoch_scope_boot_and_absolute() {
    for case in ["idle", "absolute", "epoch", "scope", "boot", "off", "clock"] {
        let (_dir, clock, mobile, auth, signer) = auth_fixture();
        let s = open_session(&auth, &signer, &clock);
        let ctx = auth.authenticate(credentials(&s)).unwrap();
        match case {
            "idle" => {
                clock.set(1000 + SESSION_IDLE_MS - 1);
                auth.recheck(&ctx, clock.now_ms()).unwrap();
                clock.set(1000 + SESSION_IDLE_MS);
            }
            "absolute" => {
                for elapsed in (60_000..SESSION_ABSOLUTE_MS).step_by(60_000) {
                    clock.set(1000 + elapsed);
                    let mut state = mobile.state.lock().unwrap();
                    let podd::server::mobile::MobileState {
                        registry, sessions, ..
                    } = &mut *state;
                    sessions
                        .check(registry, ctx.session(), clock.now_ms(), true)
                        .unwrap();
                }
                clock.set(1000 + SESSION_ABSOLUTE_MS);
            }
            "epoch" => mobile.state.lock().unwrap().registry.devices[0].auth_epoch += 1,
            "scope" => {
                mobile.state.lock().unwrap().registry.devices[0]
                    .policy
                    .get_mut("t1")
                    .unwrap()
                    .remove(&MobileCap::Approve);
            }
            "boot" => {
                let other = Auth::new(
                    mobile.clone(),
                    wire::Id::new("machine").unwrap(),
                    wire::Id::new("otherboot").unwrap(),
                );
                assert!(other.authenticate(credentials(&s)).is_err());
                assert!(other.recheck(&ctx, clock.now_ms()).is_err());
                continue;
            }
            "off" => mobile.state.lock().unwrap().sessions.clear(),
            "clock" => clock.set(999),
            _ => unreachable!(),
        }
        assert!(auth.recheck(&ctx, clock.now_ms()).is_err(), "{case}");
    }
}

#[test]
fn session_signature_epoch_nonce_csrf_and_four_session_cap() {
    let (_dir, clock, mobile, auth, signer) = auth_fixture();
    for case in ["key", "epoch", "csrf", "expired", "unknown"] {
        let c = auth.challenge(&wire::Id::new("m1").unwrap()).unwrap();
        let mut r = open_request(&c, &signer);
        let mut csrf = c.csrf.as_str().to_owned();
        match case {
            "key" => r = open_request(&c, &SoftwareSigner::generate().unwrap()),
            "epoch" => {
                mobile.state.lock().unwrap().registry.devices[0].auth_epoch += 1;
            }
            "csrf" => csrf = "f".repeat(64),
            "expired" => clock.set(c.expires_ms),
            "unknown" => r.device = wire::Id::new("unregistered").unwrap(),
            _ => unreachable!(),
        }
        assert!(auth.open(r, &csrf, clock.now_ms()).is_err(), "{case}");
        assert_eq!(mobile.state.lock().unwrap().sessions.count("m1"), 0);
    }
    let c = auth.challenge(&wire::Id::new("m1").unwrap()).unwrap();
    let r = open_request(&c, &signer);
    let first = auth
        .open(r.clone(), c.csrf.as_str(), clock.now_ms())
        .unwrap();
    assert!(auth.open(r, c.csrf.as_str(), clock.now_ms()).is_err());
    for _ in 0..4 {
        open_session(&auth, &signer, &clock);
    }
    assert_eq!(mobile.state.lock().unwrap().sessions.count("m1"), 4);
    assert!(auth.authenticate(credentials(&first)).is_err());
}

#[test]
fn scope_widening_requires_new_session_and_digests_are_not_debugged() {
    let (_dir, clock, mobile, auth, signer) = auth_fixture();
    let s = open_session(&auth, &signer, &clock);
    {
        let mut state = mobile.state.lock().unwrap();
        state
            .registry
            .set_policy("m1", "t2", &[MobileCap::Status], &[], false)
            .unwrap();
        let debug = format!("{:?}", state.sessions);
        assert!(!debug.contains(s.bearer.as_str()));
        assert!(!debug.contains(s.csrf.as_str()));
    }
    let ctx = auth.authenticate(credentials(&s)).unwrap();
    assert_eq!(ctx.scopes().len(), 1);
    auth.recheck(&ctx, clock.now_ms()).unwrap();
    let fresh = open_session(&auth, &signer, &clock);
    assert_eq!(fresh.scopes.as_slice().len(), 2);
}

#[test]
fn off_discards_sessions_and_outstanding_session_challenges() {
    let (_dir, clock, _mobile, auth, signer) = auth_fixture();
    let session = open_session(&auth, &signer, &clock);
    let c = auth.challenge(&wire::Id::new("m1").unwrap()).unwrap();
    auth.off().unwrap();
    assert!(auth.authenticate(credentials(&session)).is_err());
    assert!(
        auth.open(open_request(&c, &signer), c.csrf.as_str(), clock.now_ms())
            .is_err()
    );
}

#[test]
fn clock_regression_during_challenge_invalidates_existing_sessions() {
    let (_dir, clock, _mobile, auth, signer) = auth_fixture();
    let session = open_session(&auth, &signer, &clock);
    clock.set(2000);
    auth.challenge(&wire::Id::new("m1").unwrap()).unwrap();
    clock.set(1999);
    assert!(auth.challenge(&wire::Id::new("m1").unwrap()).is_err());
    clock.set(2001);
    assert!(auth.authenticate(credentials(&session)).is_err());
}
