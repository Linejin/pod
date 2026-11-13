//! Fixture-only persistence and complete-detail receipt contract tests.
mod common;
use podd::server::mobile::approval::{
    ApprovalIdentity, ApprovalState, LockedApproval, decision_fingerprint,
};
use podd::server::orch_store::SavePhase;
use podd::{
    protocol::{GateInfo, GateState, mobile_v1 as wire},
    server::{
        mobile::{
            approval::{GateMetadata, ReadReceipts, project_gate},
            session::Clock,
        },
        orch_store::Store,
    },
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Barrier, Mutex};

struct ApprovalFixture {
    receipts: ReadReceipts,
    allowed: bool,
    expires: u64,
    commits: usize,
}
impl ApprovalState for ApprovalFixture {
    fn recheck(
        &self,
        identity: ApprovalIdentity<'_>,
        request: &wire::Decide,
        now: u64,
    ) -> Result<(), wire::Error> {
        if !self.allowed
            || identity.session != "s1"
            || identity.boot != "b1"
            || identity.auth_epoch != 1
            || !["t1", "t2"].contains(&request.task.as_str())
            || now >= self.expires
        {
            return Err(wire::Error {
                code: wire::ErrorCode::Denied,
                retry_after_ms: None,
            });
        }
        Ok(())
    }
    fn receipts(&mut self) -> &mut ReadReceipts {
        &mut self.receipts
    }
    fn committed(&mut self, _: ApprovalIdentity<'_>, _: u64) {
        self.commits += 1;
    }
}
fn identity(device: &str) -> ApprovalIdentity<'_> {
    ApprovalIdentity {
        device,
        session: "s1",
        boot: "b1",
        auth_epoch: 1,
    }
}
fn atomic_fixture(
    label: &str,
) -> (
    Mutex<ApprovalFixture>,
    Mutex<Store>,
    std::path::PathBuf,
    ClockAt,
) {
    let dir = std::env::var_os("POD_P3B_CRASH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pod-p3b-{}-{label}", std::process::id()))
        });
    let mut store = Store::default();
    let mut g = gate();
    g.expires_ms = Some(1000);
    store.gates.push(g);
    store.gate_metadata.insert(
        "g1".into(),
        GateMetadata {
            revision: 1,
            origin: wire::Origin::LocalUser,
        },
    );
    store.save(&dir).unwrap();
    let clock = ClockAt(AtomicU64::new(10));
    let mut state = ApprovalFixture {
        receipts: ReadReceipts::default(),
        allowed: true,
        expires: 2000,
        commits: 0,
    };
    let projection = project_gate(
        wire::Id::new("m1").unwrap(),
        wire::Id::new("b1").unwrap(),
        &store.gates[0],
        &store.gate_metadata["g1"],
    )
    .unwrap();
    state.receipts.record_detail("s1", &projection, &clock);
    (Mutex::new(state), Mutex::new(store), dir, clock)
}

#[test]
fn decision_replay_precedes_cas_and_lookup_is_device_scoped_and_authorized() {
    let (mobile, orch, dir, clock) = atomic_fixture("replay");
    let boundary = LockedApproval {
        mobile: &mobile,
        orch: &orch,
        machine: wire::Id::new("m1").unwrap(),
        state_dir: &dir,
    };
    let req = request("g1", 1);
    let first = boundary
        .decide_identity(identity("d1"), &req, &clock)
        .unwrap();
    assert!(
        !mobile
            .lock()
            .unwrap()
            .receipts
            .matches("s1", "b1", &req, 10)
    );
    {
        let mut s = orch.lock().unwrap();
        assert_eq!(s.mobile_decisions.len(), 1);
        let record = &s.mobile_decisions[0];
        assert_eq!(record.auth_epoch, 1);
        assert_eq!(record.previous, GateState::Pending);
        assert_eq!(record.new, GateState::Approved);
        assert_eq!(record.at_ms, 10);
        assert_eq!(record.scope_fingerprint.as_str().len(), 64);
        s.gates[0].message.push('!');
        s.save(&dir).unwrap();
        s.gates.clear();
        s.save(&dir).unwrap();
        *s = Store::load(&dir);
    }
    clock.0.store(1500, Ordering::SeqCst);
    assert_eq!(
        first,
        boundary
            .decide_identity(identity("d1"), &req, &clock)
            .unwrap()
    );
    for field in ["machine", "task", "gate", "revision", "decision", "choice"] {
        let mut changed = serde_json::to_value(&req).unwrap();
        changed[field] = match field {
            "revision" => serde_json::json!(2),
            "decision" => serde_json::json!("reject"),
            "task" => serde_json::json!("t2"),
            _ => serde_json::json!("different"),
        };
        let changed: wire::Decide = serde_json::from_value(changed).unwrap();
        let code = boundary
            .decide_identity(identity("d1"), &changed, &clock)
            .unwrap_err()
            .code;
        assert_eq!(code, wire::ErrorCode::Conflict);
    }
    let mut operation = wire::Operation {
        kind: wire::OperationKind::Decision,
        id: req.operation_id.clone(),
        body_fingerprint: decision_fingerprint(&req).unwrap(),
    };
    assert_eq!(
        first,
        boundary
            .lookup_identity(identity("d1"), &req.task, &operation, &clock)
            .unwrap()
    );
    assert_eq!(
        boundary
            .lookup_identity(identity("d2"), &req.task, &operation, &clock)
            .unwrap_err()
            .code,
        wire::ErrorCode::UnknownOperation
    );
    operation.id = wire::Text::new("unknown").unwrap();
    assert_eq!(
        boundary
            .lookup_identity(identity("d1"), &req.task, &operation, &clock)
            .unwrap_err()
            .code,
        wire::ErrorCode::UnknownOperation
    );
    operation.id = req.operation_id.clone();
    operation.body_fingerprint = wire::Fingerprint::new("0".repeat(64)).unwrap();
    assert_eq!(
        boundary
            .lookup_identity(identity("d1"), &req.task, &operation, &clock)
            .unwrap_err()
            .code,
        wire::ErrorCode::Conflict
    );
    mobile.lock().unwrap().allowed = false;
    assert_eq!(
        boundary
            .lookup_identity(identity("d1"), &req.task, &operation, &clock)
            .unwrap_err()
            .code,
        wire::ErrorCode::Denied
    );
    assert!(
        boundary
            .decide_identity(identity("d1"), &req, &clock)
            .is_err()
    );
    mobile.lock().unwrap().allowed = true;
    clock.0.store(2000, Ordering::SeqCst);
    assert_eq!(
        boundary
            .lookup_identity(identity("d1"), &req.task, &operation, &clock)
            .unwrap_err()
            .code,
        wire::ErrorCode::Denied
    );
    assert_eq!(mobile.lock().unwrap().commits, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_duplicate_has_one_commit_and_identical_results() {
    let (mobile, orch, dir, clock) = atomic_fixture("duplicate");
    let boundary = LockedApproval {
        mobile: &mobile,
        orch: &orch,
        machine: wire::Id::new("m1").unwrap(),
        state_dir: &dir,
    };
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let run = || {
            barrier.wait();
            boundary
                .decide_identity(identity("d1"), &request("g1", 1), &clock)
                .unwrap()
        };
        let a = scope.spawn(run);
        let b = scope.spawn(run);
        assert_eq!(a.join().unwrap(), b.join().unwrap());
    });
    assert_eq!(mobile.lock().unwrap().commits, 1);
    assert_eq!(orch.lock().unwrap().mobile_decisions.len(), 1);
    assert_eq!(orch.lock().unwrap().gate_metadata["g1"].revision, 2);
    assert_eq!(Store::load(&dir).mobile_decisions.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn save_failures_rollback_and_crash_checkpoints_recover_exactly_one_outcome() {
    for phase in [
        SavePhase::BeforeWrite,
        SavePhase::AfterTempWrite,
        SavePhase::AfterReplace,
    ] {
        for crash in [false, true] {
            let (mobile, orch, dir, clock) = atomic_fixture(&format!("{phase:?}-{crash}"));
            let boundary = LockedApproval {
                mobile: &mobile,
                orch: &orch,
                machine: wire::Id::new("m1").unwrap(),
                state_dir: &dir,
            };
            let req = request("g1", 1);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                boundary.decide_identity_with_save(identity("d1"), &req, &clock, |s| {
                    s.save_with_checkpoint(&dir, |at| {
                        if at == phase {
                            if crash {
                                panic!("injected crash at {phase:?}");
                            }
                            return Err(std::io::Error::other("injected storage failure"));
                        }
                        Ok(())
                    })
                })
            }));
            if !crash {
                assert_eq!(outcome.unwrap().is_ok(), phase == SavePhase::AfterReplace);
                let state = mobile.lock().unwrap();
                assert_eq!(state.commits, usize::from(phase == SavePhase::AfterReplace));
                drop(state);
                if phase != SavePhase::AfterReplace {
                    assert!(
                        mobile
                            .lock()
                            .unwrap()
                            .receipts
                            .matches("s1", "b1", &req, 10)
                    );
                    assert!(orch.lock().unwrap().mobile_decisions.is_empty());
                    assert!(orch.lock().unwrap().gate_ops.is_empty());
                    assert_eq!(orch.lock().unwrap().gate_metadata["g1"].revision, 1);
                    assert_eq!(orch.lock().unwrap().gates[0].state, GateState::Pending);
                }
            } else {
                assert!(outcome.is_err());
            }
            let recovered = Mutex::new(Store::load(&dir));
            assert!(recovered.lock().unwrap().mobile_recovered);
            assert_eq!(
                recovered.lock().unwrap().mobile_decisions.len(),
                usize::from(phase == SavePhase::AfterReplace)
            );
            // Restart uses fresh locks and session state; re-read detail only
            // when the original operation was never committed.
            let mut state = ApprovalFixture {
                receipts: ReadReceipts::default(),
                allowed: true,
                expires: 2000,
                commits: 0,
            };
            let s = recovered.lock().unwrap();
            state.receipts.record_detail(
                "s1",
                &project_gate(
                    wire::Id::new("m1").unwrap(),
                    wire::Id::new("b1").unwrap(),
                    &s.gates[0],
                    &s.gate_metadata["g1"],
                )
                .unwrap(),
                &clock,
            );
            drop(s);
            let restarted = Mutex::new(state);
            let boundary = LockedApproval {
                mobile: &restarted,
                orch: &recovered,
                machine: wire::Id::new("m1").unwrap(),
                state_dir: &dir,
            };
            let first = boundary
                .decide_identity(identity("d1"), &req, &clock)
                .unwrap();
            assert_eq!(
                first,
                boundary
                    .decide_identity(identity("d1"), &req, &clock)
                    .unwrap()
            );
            assert_eq!(Store::load(&dir).mobile_decisions.len(), 1);
            assert_eq!(Store::load(&dir).gate_ops.len(), 1);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn invalid_recovery_stays_closed_across_restarts() {
    let (_, _, dir, _) = atomic_fixture("corrupt");
    std::fs::write(Store::path(&dir), b"{broken").unwrap();
    for _ in 0..2 {
        let mut store = Store::load(&dir);
        assert!(!store.mobile_recovered);
        assert!(store.save(&dir).is_err());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn duplicate_or_tampered_durable_outcome_fences_recovery() {
    for duplicate in [false, true] {
        let (mobile, orch, dir, clock) = atomic_fixture(&format!("bad-record-{duplicate}"));
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        boundary
            .decide_identity(identity("d1"), &request("g1", 1), &clock)
            .unwrap();
        let path = Store::path(&dir);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if duplicate {
            let record = value["mobile_decisions"][0].clone();
            value["mobile_decisions"]
                .as_array_mut()
                .unwrap()
                .push(record);
        } else {
            value["mobile_decisions"][0]["request"]["revision"] = serde_json::json!(7);
        }
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let store = Mutex::new(Store::load(&dir));
        assert!(!store.lock().unwrap().mobile_recovered);
        let blocked = LockedApproval {
            mobile: &mobile,
            orch: &store,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        assert_eq!(
            blocked
                .decide_identity(identity("d1"), &request("g1", 1), &clock)
                .unwrap_err()
                .code,
            wire::ErrorCode::Storage
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn expiry_is_rechecked_immediately_before_save() {
    struct AdvancingClock(AtomicU64);
    impl Clock for AdvancingClock {
        fn now_ms(&self) -> u64 {
            if self.0.fetch_add(1, Ordering::SeqCst) < 2 {
                10
            } else {
                1000
            }
        }
    }
    for session in [false, true] {
        let (mobile, orch, dir, _) = atomic_fixture(&format!("last-expiry-{session}"));
        if session {
            mobile.lock().unwrap().expires = 1000;
            orch.lock().unwrap().gates[0].expires_ms = None;
        }
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        let mut called = false;
        assert!(
            boundary
                .decide_identity_with_save(
                    identity("d1"),
                    &request("g1", 1),
                    &AdvancingClock(AtomicU64::new(0)),
                    |_| {
                        called = true;
                        Ok(())
                    }
                )
                .is_err()
        );
        assert!(!called);
        assert!(orch.lock().unwrap().mobile_decisions.is_empty());
        assert!(orch.lock().unwrap().gate_ops.is_empty());
        assert_eq!(orch.lock().unwrap().gates[0].state, GateState::Pending);
        assert_eq!(mobile.lock().unwrap().commits, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn crash_child() {
    let Ok(phase) = std::env::var("POD_P3B_CRASH_PHASE") else {
        return;
    };
    let (mobile, orch, dir, clock) = atomic_fixture("child");
    let boundary = LockedApproval {
        mobile: &mobile,
        orch: &orch,
        machine: wire::Id::new("m1").unwrap(),
        state_dir: &dir,
    };
    let _ = boundary.decide_identity_with_save(identity("d1"), &request("g1", 1), &clock, |s| {
        s.save_with_checkpoint(&dir, |at| {
            if format!("{at:?}") == phase {
                std::process::exit(91);
            }
            Ok(())
        })
    });
    panic!("crash checkpoint not reached");
}

#[test]
fn abrupt_process_exit_preserves_commit_boundary() {
    for phase in [
        SavePhase::BeforeWrite,
        SavePhase::AfterTempWrite,
        SavePhase::AfterReplace,
    ] {
        let dir =
            std::env::temp_dir().join(format!("pod-p3b-process-{}-{phase:?}", std::process::id()));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture"])
            .env("POD_P3B_CRASH_DIR", &dir)
            .env("POD_P3B_CRASH_PHASE", format!("{phase:?}"))
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(91),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let s = Store::load(&dir);
        assert!(s.mobile_recovered);
        assert_eq!(
            s.mobile_decisions.len(),
            usize::from(phase == SavePhase::AfterReplace)
        );
        assert_eq!(
            s.gates[0].state,
            if phase == SavePhase::AfterReplace {
                GateState::Approved
            } else {
                GateState::Pending
            }
        );
        assert_eq!(
            s.gate_metadata["g1"].revision,
            if phase == SavePhase::AfterReplace {
                2
            } else {
                1
            }
        );
        assert_eq!(s.gate_ops.len(), s.mobile_decisions.len());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct ClockAt(AtomicU64);
impl Clock for ClockAt {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn gate() -> GateInfo {
    serde_json::from_value(serde_json::json!({"id":"g1","task":"t1","title":"title","message":"body","state":"pending","created_ms":1})).unwrap()
}
fn request(gate: &str, revision: u64) -> wire::Decide {
    serde_json::from_value(serde_json::json!({"machine":"m1","task":"t1","gate":gate,"revision":revision,"operation_id":"op1","decision":"approve","choice":null})).unwrap()
}

#[test]
fn local_and_verified_mod_creation_persist_server_origin() {
    use pod_test_support::git::git_ok;
    use podd::protocol::*;
    let daemon = common::Daemon::start("gate-provenance");
    let repo = daemon.state_dir.join("fixture-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    git_ok(&repo, &["config", "user.name", "Pod Fixture"]);
    git_ok(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git_ok(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    git_ok(&repo, &["add", "README.md"]);
    git_ok(&repo, &["commit", "-qm", "fixture"]);
    let mut client = daemon.client();
    client
        .call::<RepoAdd>(&RepoAddParams {
            path: repo.display().to_string(),
            name: Some("demo".into()),
        })
        .unwrap();
    let task = client
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "demo".into(),
            name: "provenance".into(),
            agent: Some(AgentSpec::Argv {
                argv: vec![
                    common::fixture_bin("examples/fake-agent.exe")
                        .display()
                        .to_string(),
                ],
            }),
            ..Default::default()
        })
        .unwrap();
    let local = client
        .call::<GateCreate>(&GateCreateParams {
            task: Some(task.id.clone()),
            title: Some("[trusted-mod] title".into()),
            message: "origin=orch run=r1 attempt=a1".into(),
            ..Default::default()
        })
        .unwrap();
    let mod_dir = daemon.state_dir.join("extensions").join("fixture-mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(mod_dir.join("mod.json"), r#"{"pod":1,"name":"fixture-mod","version":"0.1.0","entry":"main.js","permissions":["commands","gate"]}"#).unwrap();
    std::fs::write(mod_dir.join("main.js"), r#"pod.command({id:"ask",title:"Ask"}, ctx => pod.gate.request(ctx.task,"title","origin=local_user"));"#).unwrap();
    client
        .call::<ModReload>(&ModReloadParams::default())
        .unwrap();
    client
        .call::<ModSetEnabled>(&ModEnableParams {
            name: "fixture-mod".into(),
            enabled: true,
        })
        .unwrap();
    client
        .call::<ModCommand>(&ModCommandParams {
            name: "fixture-mod".into(),
            command: "ask".into(),
            task: Some(task.id),
            terminal: None,
        })
        .unwrap();
    let store = Store::load(&daemon.state_dir);
    assert_eq!(
        store.gate_metadata[&local.id],
        GateMetadata {
            revision: 1,
            origin: wire::Origin::LocalUser
        }
    );
    let mod_gate = store.gates.iter().find(|g| g.id != local.id).unwrap();
    assert_eq!(
        store.gate_metadata[&mod_gate.id],
        GateMetadata {
            revision: 1,
            origin: wire::Origin::Mod {
                id: wire::Id::new("fixture-mod").unwrap()
            }
        }
    );
}

#[test]
fn legacy_migration_and_every_revision_field() {
    let dir = std::env::temp_dir().join(format!("pod-mobile-atomic-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("orch")).unwrap();
    std::fs::write(
        Store::path(&dir),
        serde_json::to_vec(&serde_json::json!({"gates":[gate()]})).unwrap(),
    )
    .unwrap();
    let mut store = Store::load(&dir);
    assert_eq!(store.gate_metadata["g1"], GateMetadata::default());
    assert!(
        !project_gate(
            wire::Id::new("m1").unwrap(),
            wire::Id::new("b1").unwrap(),
            &store.gates[0],
            &store.gate_metadata["g1"]
        )
        .unwrap()
        .complete
    );
    store.save(&dir).unwrap();
    assert_eq!(store.gate_metadata["g1"].revision, 0);
    for n in 1..=7 {
        match n {
            1 => store.gates[0].title.push('!'),
            2 => store.gates[0].message.push('!'),
            3 => store.gates[0].options.push("yes".into()),
            4 => store.gates[0].task = Some("t2".into()),
            5 => store.gates[0].expires_ms = Some(100),
            6 => store.gate_metadata.get_mut("g1").unwrap().origin = wire::Origin::LocalUser,
            7 => store.gates[0].state = GateState::Rejected,
            _ => unreachable!(),
        }
        store.save(&dir).unwrap();
        assert_eq!(store.gate_metadata["g1"].revision, n);
        store.counters.insert("unrelated".into(), n);
        store.save(&dir).unwrap();
        assert_eq!(Store::load(&dir).gate_metadata["g1"].revision, n);
    }
    assert_eq!(store.next_id("g"), "g2");
    store.gates.clear();
    store.counters.remove("g");
    assert_eq!(
        store.next_id("g"),
        "g2",
        "persisted tombstone prevents reuse"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn receipts_are_complete_bound_scoped_and_strictly_younger_than_thirty_seconds() {
    let clock = ClockAt(AtomicU64::new(100));
    let mut receipts = ReadReceipts::default();
    let mut projected = project_gate(
        wire::Id::new("m1").unwrap(),
        wire::Id::new("b1").unwrap(),
        &gate(),
        &GateMetadata {
            revision: 1,
            origin: wire::Origin::LocalUser,
        },
    )
    .unwrap();
    projected.complete = false;
    receipts.record_detail("s1", &projected, &clock);
    assert!(!receipts.matches("s1", "b1", &request("g1", 1), 100));
    projected.complete = true;
    receipts.record_detail("s1", &projected, &clock);
    assert!(!receipts.matches("s2", "b1", &request("g1", 1), 100));
    assert!(!receipts.matches("s1", "b2", &request("g1", 1), 100));
    assert!(!receipts.matches("s1", "b1", &request("g1", 2), 100));
    assert!(receipts.matches("s1", "b1", &request("g1", 1), 30_099));
    assert!(!receipts.matches("s1", "b1", &request("g1", 1), 30_100));
    for i in 1..=17 {
        projected.gate = wire::Id::new(format!("g{i}")).unwrap();
        receipts.record_detail("s1", &projected, &clock);
    }
    assert!(!receipts.matches("s1", "b1", &request("g1", 1), 100));
    assert!(receipts.matches("s1", "b1", &request("g2", 1), 100));
    receipts.forget_session("s1");
    assert!(!receipts.matches("s1", "b1", &request("g2", 1), 100));
}
