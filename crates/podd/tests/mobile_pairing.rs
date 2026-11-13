#![cfg(feature = "mobile-lan")]
use base64::{Engine, engine::general_purpose::STANDARD};
use pod_mobile::crypto::SoftwareSigner;
use podd::{
    protocol::mobile_v1 as wire,
    server::mobile::{
        Mobile,
        pairing::{OpenedWindow, Pairing},
        session::Clock,
    },
};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicU64, Ordering},
};

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn setup() -> (std::path::PathBuf, Arc<TestClock>, Arc<Pairing>) {
    let (dir, clock, pairing, _) = setup_full();
    (dir, clock, pairing)
}
fn setup_full() -> (
    std::path::PathBuf,
    Arc<TestClock>,
    Arc<Pairing>,
    Arc<Mobile>,
) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/mobile-pairing-fixtures")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
    let clock = Arc::new(TestClock(AtomicU64::new(1_000)));
    let mobile = Arc::new(Mobile::load(&dir, clock.clone()));
    let pairing = Arc::new(Pairing::new(
        mobile.clone(),
        wire::Id::new("machine").unwrap(),
        wire::Id::new("boot").unwrap(),
    ));
    (dir, clock, pairing, mobile)
}
fn request(w: &OpenedWindow, c: &wire::Challenge, signer: &SoftwareSigner) -> wire::Claim {
    let mut p = wire::Claim {
        pairing_id: w.pairing_id.clone(),
        secret: w.secret.clone(),
        code: w.code.clone(),
        nonce: c.nonce.clone(),
        public_key: wire::Text::new(STANDARD.encode(signer.spki_der())).unwrap(),
        signature: wire::Text::new("").unwrap(),
        name: wire::Text::new("self-claimed phone").unwrap(),
        scopes: serde_json::from_str(r#"[{"task":"t1","caps":["status","approve"]}]"#).unwrap(),
    };
    sign(&mut p, c, signer);
    p
}
fn sign(p: &mut wire::Claim, c: &wire::Challenge, signer: &SoftwareSigner) {
    p.signature =
        wire::Text::new(STANDARD.encode(signer.sign(&wire::claim_preimage(c, p)).unwrap()))
            .unwrap();
}
fn challenge(p: &Pairing, w: &OpenedWindow) -> wire::Challenge {
    p.challenge(wire::Purpose::Claim, w.pairing_id.clone(), 0)
        .unwrap()
}
fn refused(result: Result<wire::Claimed, wire::Error>) {
    assert!(matches!(result, Err(e) if e == wire::Error::refusal()));
}

#[test]
fn parallel_claims_have_one_winner() {
    let (_dir, clock, p) = setup();
    let w = p.open_window().unwrap();
    let signer = SoftwareSigner::generate().unwrap();
    let c = challenge(&p, &w);
    let request = request(&w, &c, &signer);
    let barrier = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (1..=8)
        .map(|i| {
            let (p, barrier, r, csrf) =
                (p.clone(), barrier.clone(), request.clone(), c.csrf.clone());
            std::thread::spawn(move || {
                barrier.wait();
                p.claim(r, [192, 0, 2, i].into(), csrf.as_str(), 1_000)
            })
        })
        .collect();
    let mut wins = 0;
    for t in workers {
        match t.join().unwrap() {
            Ok(claimed) => {
                wins += 1;
                let pending = p.pending().unwrap().unwrap();
                assert_eq!(
                    pending.ticket_hash,
                    <[u8; 32]>::from(Sha256::digest(claimed.ticket.as_str().as_bytes()))
                );
                assert_eq!(pending.public_key, request.public_key);
                assert_eq!(pending.name, request.name);
                assert_eq!(pending.scopes, request.scopes);
                assert_eq!(claimed.expires_ms, w.expires_at);
            }
            Err(e) => assert_eq!(e, wire::Error::refusal()),
        }
    }
    assert_eq!(wins, 1);
    clock.0.store(w.expires_at, Ordering::SeqCst);
    assert!(p.pending().unwrap().is_none());
}

#[test]
fn all_invalid_claims_use_the_same_error() {
    for case in [
        "secret",
        "code",
        "signature",
        "reused",
        "challenge_expired",
        "purpose",
        "window_expired",
        "replaced",
        "csrf",
        "epoch",
        "subject",
        "off",
        "scope",
        "clock_backwards",
    ] {
        let (_dir, clock, p) = setup();
        let w = p.open_window().unwrap();
        let signer = SoftwareSigner::generate().unwrap();
        let c = match case {
            "purpose" => p
                .challenge(wire::Purpose::Finish, w.pairing_id.clone(), 0)
                .unwrap(),
            "epoch" => p
                .challenge(wire::Purpose::Claim, w.pairing_id.clone(), 1)
                .unwrap(),
            "subject" => p
                .challenge(wire::Purpose::Claim, wire::Id::new("other").unwrap(), 0)
                .unwrap(),
            _ => challenge(&p, &w),
        };
        let mut r = request(&w, &c, &signer);
        let mut csrf = c.csrf.as_str().to_owned();
        match case {
            "secret" => {
                r.secret = secret_value("0".repeat(64)).unwrap();
                sign(&mut r, &c, &signer);
            }
            "code" => {
                r.code = wire::Text::new(if w.code.as_str() == "00000000" {
                    "00000001"
                } else {
                    "00000000"
                })
                .unwrap();
                sign(&mut r, &c, &signer);
            }
            "signature" => sign(&mut r, &c, &SoftwareSigner::generate().unwrap()),
            "reused" => {
                let mut bad = r.clone();
                sign(&mut bad, &c, &SoftwareSigner::generate().unwrap());
                refused(p.claim(bad, [192, 0, 2, 1].into(), &csrf, clock.now_ms()));
            }
            "challenge_expired" => clock.0.store(c.expires_ms, Ordering::SeqCst),
            "window_expired" => clock.0.store(w.expires_at, Ordering::SeqCst),
            "replaced" => {
                p.open_window().unwrap();
            }
            "off" => p.off().unwrap(),
            "csrf" => csrf = "0".repeat(64),
            "scope" => {
                r.scopes = serde_json::from_str(r#"[{"task":"t1","caps":["approve"]}]"#).unwrap();
                sign(&mut r, &c, &signer);
            }
            "clock_backwards" => clock.0.store(999, Ordering::SeqCst),
            _ => {}
        }
        refused(p.claim(r, [192, 0, 2, 1].into(), &csrf, clock.now_ms()));
        assert!(p.pending().unwrap().is_none(), "{case}");
    }
}

#[test]
fn five_bad_codes_close_the_window_and_reopen_does_not_reset_limits() {
    let (_dir, clock, p) = setup();
    let w = p.open_window().unwrap();
    let c = challenge(&p, &w);
    let signer = SoftwareSigner::generate().unwrap();
    let good = request(&w, &c, &signer);
    let mut bad = good.clone();
    bad.code = wire::Text::new("bad-code").unwrap();
    for i in 1..=5 {
        refused(p.claim(
            bad.clone(),
            [192, 0, 2, i].into(),
            c.csrf.as_str(),
            clock.now_ms(),
        ));
    }
    refused(p.claim(good, [192, 0, 2, 6].into(), c.csrf.as_str(), clock.now_ms()));
    let w = p.open_window().unwrap();
    let c = challenge(&p, &w);
    let mut bad = request(&w, &c, &signer);
    bad.secret = secret_value("x").unwrap();
    for _ in 0..4 {
        refused(p.claim(
            bad.clone(),
            "::ffff:192.0.2.1".parse().unwrap(),
            c.csrf.as_str(),
            clock.now_ms(),
        ));
    }
    let error = p
        .claim(
            bad.clone(),
            [192, 0, 2, 1].into(),
            c.csrf.as_str(),
            clock.now_ms(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, wire::ErrorCode::RateLimited);
    assert_eq!(error.retry_after_ms, Some(60_000));
    for i in 7..=16 {
        refused(p.claim(
            bad.clone(),
            [192, 0, 2, i].into(),
            c.csrf.as_str(),
            clock.now_ms(),
        ));
    }
    assert_eq!(
        p.claim(
            bad.clone(),
            [192, 0, 2, 99].into(),
            c.csrf.as_str(),
            clock.now_ms()
        )
        .err()
        .unwrap()
        .code,
        wire::ErrorCode::RateLimited
    );
    clock.0.fetch_add(60_000, Ordering::SeqCst);
    refused(p.claim(bad, [192, 0, 2, 99].into(), c.csrf.as_str(), clock.now_ms()));
}

#[test]
fn claim_wire_fixture() {
    let f: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mobile-m2-wire.json")).unwrap();
    let c: wire::Challenge = serde_json::from_value(f["claim_challenge"].clone()).unwrap();
    let r: wire::Claim = serde_json::from_value(f["claim_request"].clone()).unwrap();
    let bytes = wire::claim_preimage(&c, &r);
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        f["claim_preimage"].as_str().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        f["claim_sha256"].as_str().unwrap()
    );
}

fn secret_value(s: impl Into<String>) -> Result<wire::Secret, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(s.into()))
}

fn finish_request(
    ticket: &wire::Secret,
    c: &wire::Challenge,
    signer: &SoftwareSigner,
) -> wire::Finish {
    let mut r = wire::Finish {
        ticket: ticket.clone(),
        nonce: c.nonce.clone(),
        signature: wire::Text::new("").unwrap(),
        wait_ms: None,
    };
    r.signature =
        wire::Text::new(STANDARD.encode(signer.sign(&wire::finish_preimage(c, &r)).unwrap()))
            .unwrap();
    r
}

#[test]
fn approval_finish_is_single_use_and_scopes_are_intersected() {
    let (dir, clock, p, mobile) = setup_full();
    let w = p.open_window().unwrap();
    let signer = SoftwareSigner::generate().unwrap();
    let c = challenge(&p, &w);
    let claim = request(&w, &c, &signer);
    let ticket = p
        .claim(
            claim.clone(),
            [192, 0, 2, 1].into(),
            c.csrf.as_str(),
            clock.now_ms(),
        )
        .unwrap()
        .ticket;
    let c = p.finish_challenge(ticket.as_str()).unwrap();
    assert_eq!(c.subject, w.pairing_id);
    assert_eq!(c.auth_epoch, 1);
    let r = finish_request(&ticket, &c, &signer);
    assert!(
        p.finish(r.clone(), &"f".repeat(64), clock.now_ms())
            .is_err()
    );
    assert!(
        matches!(p.finish(r.clone(), c.csrf.as_str(), clock.now_ms()), Err(e) if e == Pairing::pending_refusal())
    );
    let choice = serde_json::from_str(
        r#"[{"task":"t1","caps":["status","message"]},{"task":"unrequested","caps":["status"]}]"#,
    )
    .unwrap();
    p.approve(&w.pairing_id, &choice).unwrap();
    let session = p
        .finish(r.clone(), c.csrf.as_str(), clock.now_ms())
        .unwrap();
    assert_eq!(session.scopes.as_slice().len(), 1);
    assert_eq!(
        session.scopes.as_slice()[0].caps.as_slice(),
        &[podd::protocol::MobileCap::Status]
    );
    assert!(p.finish(r, c.csrf.as_str(), clock.now_ms()).is_err());
    assert!(p.approve(&w.pairing_id, &choice).is_err());
    assert!(
        p.claim(
            claim,
            [192, 0, 2, 1].into(),
            c.csrf.as_str(),
            clock.now_ms()
        )
        .is_err()
    );
    let registry = podd::server::mobile::registry::Registry::load(&dir);
    assert_eq!(registry.devices.len(), 1);
    assert_eq!(
        mobile
            .state
            .lock()
            .unwrap()
            .sessions
            .count(session.device.as_str()),
        1
    );
    let auth = podd::server::mobile::auth::Auth::new(
        mobile,
        wire::Id::new("machine").unwrap(),
        wire::Id::new("boot").unwrap(),
    );
    let ctx = auth
        .authenticate(podd::server::mobile::auth::Credentials {
            bearer: session.bearer.as_str(),
            csrf: session.csrf.as_str(),
        })
        .unwrap();
    assert_eq!(ctx.device(), session.device.as_str());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn finish_rejects_wrong_ticket_key_csrf_and_reused_nonce() {
    for case in ["ticket", "key", "csrf", "nonce", "timeout", "reject", "off"] {
        let (dir, clock, p) = setup();
        let w = p.open_window().unwrap();
        let signer = SoftwareSigner::generate().unwrap();
        let c = challenge(&p, &w);
        let claim = request(&w, &c, &signer);
        let scopes = claim.scopes.clone();
        let ticket = p
            .claim(
                claim,
                [192, 0, 2, 1].into(),
                c.csrf.as_str(),
                clock.now_ms(),
            )
            .unwrap()
            .ticket;
        p.approve(&w.pairing_id, &scopes).unwrap();
        let c = p.finish_challenge(ticket.as_str()).unwrap();
        let mut r = finish_request(&ticket, &c, &signer);
        let mut csrf = c.csrf.as_str().to_owned();
        match case {
            "ticket" => r.ticket = secret_value("f".repeat(64)).unwrap(),
            "key" => r = finish_request(&ticket, &c, &SoftwareSigner::generate().unwrap()),
            "csrf" => csrf = "f".repeat(64),
            "nonce" => {
                let bad = finish_request(&ticket, &c, &SoftwareSigner::generate().unwrap());
                assert!(p.finish(bad, &csrf, clock.now_ms()).is_err());
            }
            "timeout" => clock.0.store(w.expires_at, Ordering::SeqCst),
            "reject" => p.reject(&w.pairing_id).unwrap(),
            "off" => p.off().unwrap(),
            _ => unreachable!(),
        }
        assert!(p.finish(r, &csrf, clock.now_ms()).is_err(), "{case}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn approval_failure_timeout_rejection_and_device_limit_close_window() {
    for case in ["storage", "timeout", "reject", "off", "capacity"] {
        let (dir, clock, p, mobile) = setup_full();
        if case == "capacity" {
            // Fill the registry through three independently approved keys.
            for _ in 0..3 {
                let w = p.open_window().unwrap();
                let c = challenge(&p, &w);
                let r = request(&w, &c, &SoftwareSigner::generate().unwrap());
                let scopes = r.scopes.clone();
                p.claim(r, [192, 0, 2, 1].into(), c.csrf.as_str(), clock.now_ms())
                    .unwrap();
                p.approve(&w.pairing_id, &scopes).unwrap();
            }
        }
        let w = p.open_window().unwrap();
        let c = challenge(&p, &w);
        let r = request(&w, &c, &SoftwareSigner::generate().unwrap());
        let scopes = r.scopes.clone();
        p.claim(r, [192, 0, 2, 1].into(), c.csrf.as_str(), clock.now_ms())
            .unwrap();
        match case {
            "storage" => {
                std::fs::create_dir_all(dir.join("mobile/devices.json.tmp")).unwrap();
            }
            "timeout" => clock.0.store(w.expires_at, Ordering::SeqCst),
            "reject" => p.reject(&w.pairing_id).unwrap(),
            "off" => p.off().unwrap(),
            _ => {}
        }
        assert!(p.approve(&w.pairing_id, &scopes).is_err());
        assert!(p.pending().unwrap().is_none());
        assert_eq!(
            mobile.state.lock().unwrap().registry.devices.len(),
            if case == "capacity" { 3 } else { 0 }
        );
        let registry = podd::server::mobile::registry::Registry::load(&dir);
        assert_eq!(
            registry.devices.len(),
            if case == "capacity" { 3 } else { 0 }
        );
        if dir.exists() {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn finish_wire_fixture() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mobile-m2-wire.json")).unwrap();
    let challenge: wire::Challenge =
        serde_json::from_value(fixture["finish_challenge"].clone()).unwrap();
    let request: wire::Finish = serde_json::from_value(fixture["finish_request"].clone()).unwrap();
    let bytes = wire::finish_preimage(&challenge, &request);
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        fixture["finish_preimage"].as_str().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        fixture["finish_sha256"].as_str().unwrap()
    );
}
