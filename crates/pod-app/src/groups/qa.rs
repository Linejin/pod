//! The Group panel and the group dialogs on GPUI's headless platform (native
//! QA, docs/10 §12) against a scripted podd double that keeps one group.
//! These scenarios pin what the A-P4 split moves: the exact request params
//! (J), request and toast counts and order, what a refusal keeps, the check
//! field and line, the compare debounce, focus, and the dialogs' keyboard
//! paths (B). Buttons are reached with Tab and pressed with Enter, as a
//! keyboard user would; no mouse event is simulated. Run with `--nocapture`
//! for the `[native-qa]` step log.

use core::prelude::v1::test;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::dock::Panel as _;
use gpui_kit::*;
use podd::protocol::{
    AgentKind, AgentSpec, CheckJob, CheckJobState, CheckResult, CompareMember, DiffFile,
    GroupCheckEvent, GroupCheckResult, GroupCompareResult, GroupCreateParams, GroupInfo,
    GroupMember, GroupPickResult, MemberCheck, MemberError, TaskRemoveResult, codes, orch_codes,
};
use serde_json::{Value, json};

use super::{GroupPanel, PickOutcome};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceError, SourceEvent, TestBackend};
use crate::vm::TaskGitVm;
use crate::workspace::ToastKind;

const GROUP: &str = "sample/fx";
const A: &str = "sample/로그인-수정";
const B: &str = "sample/review-bot";
const SHOW: &str = "group.show {\"group\":\"sample/fx\"}";
const COMPARE: &str = "group.compare {\"group\":\"sample/fx\"}";

fn member(task: &str, agent: AgentKind) -> GroupMember {
    GroupMember {
        task: task.into(),
        agent: AgentSpec::Kind(agent),
        info: None,
        removed: false,
        picked: false,
        check: None,
    }
}

fn argv(line: &str) -> Vec<String> {
    vec!["cmd".into(), "/c".into(), line.into()]
}

fn group() -> GroupInfo {
    GroupInfo {
        id: GROUP.into(),
        repo: "sample".into(),
        name: "fx".into(),
        base: "main".into(),
        base_commit: "0123456789abcdef".into(),
        prompt: "fix the login bug".into(),
        check: Some(argv("cargo test")),
        created_ms: 1,
        picked: None,
        picked_ms: None,
        members: vec![member(A, AgentKind::Claude), member(B, AgentKind::Codex)],
        check_job: None,
        gates: Vec::new(),
    }
}

fn compared(task: &str, agent: AgentKind, added: u64) -> CompareMember {
    CompareMember {
        task: task.into(),
        agent: AgentSpec::Kind(agent),
        removed: false,
        picked: false,
        status: None,
        files: vec![DiffFile {
            path: "src/login.rs".into(),
            old_path: None,
            status: "M".into(),
            added: Some(added),
            deleted: Some(1),
            binary: false,
        }],
        files_changed: 1,
        added,
        deleted: 1,
        ahead: Some(1),
        dirty: Some(0),
        check: None,
        error: None,
    }
}

fn comparison() -> GroupCompareResult {
    GroupCompareResult {
        group: GROUP.into(),
        base: "main".into(),
        check: None,
        members: vec![
            compared(A, AgentKind::Claude, 3),
            compared(B, AgentKind::Codex, 7),
        ],
    }
}

fn result(line: &str, passed: bool) -> CheckResult {
    CheckResult {
        argv: argv(line),
        passed,
        exit_code: Some(if passed { 0 } else { 1 }),
        timed_out: false,
        duration_ms: 1500,
        finished_ms: 2,
        output_tail: "running 3 tests\ntest result: ok\n".into(),
        error: None,
    }
}

fn job(state: CheckJobState, line: &str, done: &[&str]) -> CheckJob {
    CheckJob {
        id: "k1".into(),
        state,
        argv: argv(line),
        tasks: vec![A.into(), B.into()],
        done: done.iter().map(|s| s.to_string()).collect(),
        timeout_ms: 600_000,
        started_ms: 1,
        finished_ms: None,
        all_passed: None,
    }
}

fn err(code: Option<i64>, message: &str) -> SourceError {
    SourceError {
        code,
        message: message.into(),
    }
}

/// The group half of podd for [`GROUP`].
#[derive(Default)]
struct Groups {
    /// `group.show`; `None` answers GROUP_NOT_FOUND.
    info: Option<GroupInfo>,
    /// `group.compare`; `None` fails.
    compare: Option<GroupCompareResult>,
    /// Answers of the next `group.check` / `group.pick` / `group.create` calls, in order.
    checks: Vec<Result<CheckJob, SourceError>>,
    picks: Vec<Result<GroupPickResult, SourceError>>,
    creates: Vec<Result<GroupInfo, SourceError>>,
    /// `group.check_cancel` fails with this.
    cancel_error: Option<SourceError>,
}

fn next<T>(v: &mut Vec<Result<T, SourceError>>) -> Result<T, SourceError> {
    if v.is_empty() {
        Err(err(None, "no scripted answer"))
    } else {
        v.remove(0)
    }
}

fn serve(
    state: Rc<RefCell<Groups>>,
) -> impl FnMut(&str, &Value) -> Option<Result<Value, SourceError>> {
    move |method, params| {
        let mut s = state.borrow_mut();
        Some(match method {
            "group.show" => s
                .info
                .clone()
                .map(|g| json!(g))
                .ok_or_else(|| err(Some(orch_codes::GROUP_NOT_FOUND), "no group sample/fx")),
            "group.compare" => s
                .compare
                .clone()
                .map(|c| json!(c))
                .ok_or_else(|| err(None, "compare failed")),
            // Stores an edited command and the job, like podd.
            "group.check" => next(&mut s.checks).map(|job| {
                if let Some(g) = s.info.as_mut() {
                    if let Ok(check) = serde_json::from_value(params["check"].clone()) {
                        g.check = Some(check);
                    }
                    g.check_job = Some(job.clone());
                }
                json!(GroupCheckResult {
                    group: GROUP.into(),
                    results: Vec::new(),
                    all_passed: false,
                    job: Some(job),
                })
            }),
            "group.check_cancel" => match s.cancel_error.take() {
                Some(e) => Err(e),
                None => Ok(json!(GroupCheckResult {
                    group: GROUP.into(),
                    results: Vec::new(),
                    all_passed: false,
                    job: None,
                })),
            },
            "group.pick" => next(&mut s.picks).map(|r| json!(r)),
            "group.create" => {
                let r = next(&mut s.creates);
                if let Ok(g) = &r {
                    s.info = Some(g.clone());
                    s.compare = Some(comparison());
                }
                r.map(|g| json!(g))
            }
            _ => return None,
        })
    }
}

fn state(info: Option<GroupInfo>) -> Rc<RefCell<Groups>> {
    let compare = info.is_some().then(comparison);
    Rc::new(RefCell::new(Groups {
        info,
        compare,
        ..Default::default()
    }))
}

fn harness(name: &'static str, state: Rc<RefCell<Groups>>) -> Harness {
    Harness::new(name, move |b: &mut TestBackend| {
        seed_two_tasks(b);
        b.rpc = Some(Box::new(serve(state)));
    })
}

/// The group's panel, opened from outside (the agent tree / a restored layout).
fn open(h: &mut Harness) -> Entity<GroupPanel> {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_group(GROUP, None, window, cx)));
    h.settle();
    panel(h)
}

fn panels(h: &mut Harness) -> Vec<Entity<GroupPanel>> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| ws.read(cx).orch.live_groups())
}

fn panel(h: &mut Harness) -> Entity<GroupPanel> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .orch
            .live_groups()
            .into_iter()
            .find(|p| p.read(cx).group_id == GROUP)
    })
    .expect("the group's panel")
}

/// Group (and repo) requests since `from`, as podd received them.
fn requests(h: &Harness, from: usize) -> Vec<String> {
    h.backend.calls[from..]
        .iter()
        .filter(|c| c.starts_with("group.") || c.starts_with("repo.add"))
        .cloned()
        .collect()
}

fn toasts(h: &mut Harness) -> Vec<(ToastKind, String, String)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .iter()
            .map(|t| (t.kind, t.title.to_string(), t.body.to_string()))
            .collect()
    })
}

fn toast(kind: ToastKind, title: String, body: impl Into<String>) -> (ToastKind, String, String) {
    (kind, title, body.into())
}

fn check_text(h: &mut Harness, p: &Entity<GroupPanel>) -> String {
    h.cx.update(|_, cx| p.read(cx).check_input.read(cx).value().to_string())
}

fn check_error(h: &mut Harness, p: &Entity<GroupPanel>) -> Option<String> {
    h.cx.update(|_, cx| p.read(cx).check_error.clone())
}

fn busy(h: &mut Harness, p: &Entity<GroupPanel>) -> bool {
    h.cx.update(|_, cx| p.read(cx).check_busy)
}

/// Focus the check field, then `tabs` × Tab and Enter. In the header the
/// field is followed by Run check (skipped while disabled), Cancel check
/// (only while a check runs) and ⟳ Compare; then the first card's
/// Terminals, Review and Pick winner.
fn from_check_field(h: &mut Harness, p: &Entity<GroupPanel>, tabs: usize) {
    let f = h.cx.update(|_, cx| p.read(cx).check_input.focus_handle(cx));
    h.cx.update(|window, cx| window.focus(&f, cx));
    h.settle();
    for _ in 0..tabs {
        h.press("tab");
    }
    h.press("enter");
}

/// In an open dialog: `tabs` × Tab from the dialog's first stop, then Enter.
fn in_dialog(h: &mut Harness, tabs: usize) {
    assert!(h.dialog_open(), "a dialog is open");
    for _ in 0..tabs {
        h.press("tab");
    }
    h.press("enter");
}

fn send_check(h: &mut Harness, job: CheckJob, member: Option<(&str, CheckResult)>) {
    let e = GroupCheckEvent {
        seq: 1,
        group: GROUP.into(),
        job,
        member: member.map(|(task, result)| MemberCheck {
            task: task.into(),
            result,
        }),
    };
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::GroupCheck(Box::new(e)));
    h.settle();
}

/// J/B: opening reads the group, then the comparison, once each; the panel
/// takes focus, shows the stored check command without `cmd /c` and dumps
/// `{"group"}`. Opening it again only focuses it; ⟳ Compare (Tab, Tab from
/// the check field) reads both again, and a failed read is kept per part.
#[test]
fn opening_reads_the_group_then_the_comparison_once() {
    let s = state(Some(group()));
    let mut h = harness("group-qa-open", s.clone());
    let p = open(&mut h);
    assert_eq!(requests(&h, 0), [SHOW, COMPARE]);
    let f = h.cx.update(|_, cx| p.read(cx).focus_handle());
    assert!(h.is_focused(&f), "the new panel has focus");
    assert_eq!(check_text(&mut h, &p), "cargo test");
    let dump = h.cx.update(|_, cx| p.read(cx).dump(cx));
    assert_eq!(
        serde_json::to_value(&dump).unwrap(),
        json!({"panel_name": "PodGroup", "children": [], "info": {"panel": {"group": "sample/fx"}}})
    );
    let (loaded, compared, shown) = h.cx.update(|_, cx| {
        let p = p.read(cx);
        (p.info.is_some(), p.compare.is_some(), p.show_error.clone())
    });
    assert!(loaded && compared && shown.is_none());

    // Again: the same panel is selected and focused, nothing is re-read.
    let root = h.root_focus();
    h.cx.update(|window, cx| root.focus(window, cx));
    let mark = h.backend.calls.len();
    let again = open(&mut h);
    assert_eq!(again.entity_id(), p.entity_id());
    assert_eq!(panels(&mut h).len(), 1);
    assert!(h.is_focused(&f), "reopening focuses it");
    assert!(requests(&h, mark).is_empty(), "reopening reads nothing");

    // ⟳ Compare after podd lost the group: both are read again, both errors kept.
    s.borrow_mut().info = None;
    s.borrow_mut().compare = None;
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 2);
    assert_eq!(requests(&h, mark), [SHOW, COMPARE]);
    let (show_error, compare_error, still) = h.cx.update(|_, cx| {
        let p = p.read(cx);
        (
            p.show_error.clone(),
            p.compare_error.clone(),
            p.info.is_some() && p.compare.is_some(),
        )
    });
    assert_eq!(
        show_error.as_deref(),
        Some("GROUP_NOT_FOUND: no group sample/fx")
    );
    assert_eq!(compare_error.as_deref(), Some("compare failed"));
    assert!(still, "the last good group and comparison stay");
    assert!(toasts(&mut h).is_empty(), "read failures do not toast");
}

/// B: the first workspace update after opening only records the members'
/// git summaries; a change compares once 800 ms later however often it
/// changes meanwhile; asking while a compare runs compares once more after it.
#[test]
fn member_git_changes_compare_once_after_the_debounce() {
    let s = state(Some(group()));
    let mut h = harness("group-qa-debounce", s);
    let p = open(&mut h);
    let mark = h.backend.calls.len();
    h.backend.publish();
    h.settle();
    let git = |dirty, last: &str| {
        Some(TaskGitVm {
            ahead: 1,
            behind: 0,
            dirty,
            unpushed: 1,
            last_commit: Some(last.into()),
        })
    };
    h.backend.tasks[0].git = git(0, "abc first");
    h.backend.publish();
    h.settle();
    h.backend.tasks[1].git = git(2, "def second");
    h.backend.publish();
    h.settle();
    h.cx.executor().advance_clock(Duration::from_millis(799));
    h.settle();
    assert!(requests(&h, mark).is_empty(), "nothing before 800 ms");
    h.cx.executor().advance_clock(Duration::from_millis(1));
    h.settle();
    assert_eq!(
        requests(&h, mark),
        [COMPARE],
        "one compare for both changes"
    );

    // Unchanged summaries: no compare.
    let mark = h.backend.calls.len();
    h.backend.publish();
    h.settle();
    h.cx.executor().advance_clock(Duration::from_secs(2));
    h.settle();
    assert!(requests(&h, mark).is_empty());

    // Three asks while the first runs: the first, then one more.
    let mark = h.backend.calls.len();
    h.cx.update(|_, cx| {
        p.update(cx, |p, cx| {
            p.reload_compare(cx);
            p.reload_compare(cx);
            p.reload_compare(cx);
        })
    });
    h.settle();
    assert_eq!(requests(&h, mark), [COMPARE, COMPARE]);
}

/// J/B: Run check sends the edited command (stored by podd) or none when the
/// field still shows the stored one; member events update the cards without
/// a request, the job's end re-reads the group and toasts once; Cancel check
/// re-reads after success and failure; a refused run keeps the error line.
#[test]
fn check_runs_follows_events_and_cancels() {
    let s = state(Some(group()));
    let mut h = harness("group-qa-check", s.clone());
    let p = open(&mut h);

    // Edited command → stored with the run.
    s.borrow_mut()
        .checks
        .push(Ok(job(CheckJobState::Running, "cargo test --all", &[])));
    let f = h.cx.update(|_, cx| p.read(cx).check_input.focus_handle(cx));
    h.cx.update(|window, cx| window.focus(&f, cx));
    h.keys("ctrl-a");
    h.type_text("cargo test --all");
    assert_eq!(check_text(&mut h, &p), "cargo test --all");
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert_eq!(
        requests(&h, mark),
        [
            "group.check {\"group\":\"sample/fx\",\"check\":[\"cmd\",\"/c\",\"cargo test --all\"],\"wait\":false}"
        ]
    );
    assert!(busy(&mut h, &p));
    let stored =
        h.cx.update(|_, cx| p.read(cx).info.as_ref().unwrap().check.clone());
    assert_eq!(stored, Some(argv("cargo test --all")));

    // Per-member events: cards and the comparison change, no request, no toast.
    let mark = h.backend.calls.len();
    send_check(
        &mut h,
        job(CheckJobState::Running, "cargo test --all", &[A]),
        Some((A, result("cargo test --all", true))),
    );
    send_check(
        &mut h,
        job(CheckJobState::Running, "cargo test --all", &[A, B]),
        Some((B, result("cargo test --all", false))),
    );
    assert!(requests(&h, mark).is_empty());
    assert!(toasts(&mut h).is_empty());
    assert!(busy(&mut h, &p));
    let checks = h.cx.update(|_, cx| {
        let p = p.read(cx);
        let info: Vec<Option<bool>> = p
            .info
            .as_ref()
            .unwrap()
            .members
            .iter()
            .map(|m| m.check.as_ref().map(|c| c.passed))
            .collect();
        let cmp: Vec<Option<bool>> = p
            .compare
            .as_ref()
            .unwrap()
            .members
            .iter()
            .map(|m| m.check.as_ref().map(|c| c.passed))
            .collect();
        (info, cmp)
    });
    assert_eq!(checks.0, [Some(true), Some(false)]);
    assert_eq!(checks.1, [Some(true), Some(false)]);

    // The job's end: one re-read, one toast (the workspace's), not busy.
    {
        let mut st = s.borrow_mut();
        let g = st.info.as_mut().unwrap();
        g.members[0].check = Some(result("cargo test --all", true));
        g.members[1].check = Some(result("cargo test --all", false));
        let mut end = job(CheckJobState::Finished, "cargo test --all", &[A, B]);
        end.all_passed = Some(false);
        g.check_job = Some(end);
    }
    let mark = h.backend.calls.len();
    let mut end = job(CheckJobState::Finished, "cargo test --all", &[A, B]);
    end.all_passed = Some(false);
    send_check(&mut h, end, None);
    assert_eq!(requests(&h, mark), [SHOW]);
    assert_eq!(
        toasts(&mut h),
        [toast(
            ToastKind::Error,
            crate::tr!(CHECK_FAILED; ; GROUP),
            "cmd /c cargo test --all"
        )]
    );
    assert!(!busy(&mut h, &p));
    assert_eq!(check_text(&mut h, &p), "cargo test --all");

    // The field shows the stored command: run without `check`.
    s.borrow_mut()
        .checks
        .push(Ok(job(CheckJobState::Running, "cargo test --all", &[])));
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert_eq!(
        requests(&h, mark),
        ["group.check {\"group\":\"sample/fx\",\"wait\":false}"]
    );
    assert!(busy(&mut h, &p));

    // Running: Run check is disabled, Tab reaches Cancel check. A failed
    // cancel keeps its error and still re-reads the group.
    s.borrow_mut().cancel_error = Some(err(Some(orch_codes::INVALID_STATE), "no check running"));
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert_eq!(
        requests(&h, mark),
        ["group.check_cancel {\"group\":\"sample/fx\"}", SHOW]
    );
    assert_eq!(
        check_error(&mut h, &p).as_deref(),
        Some("INVALID_STATE: no check running")
    );
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert_eq!(
        requests(&h, mark),
        ["group.check_cancel {\"group\":\"sample/fx\"}", SHOW]
    );
    // The cancelled job's own event: one re-read, one info toast.
    s.borrow_mut().info.as_mut().unwrap().check_job =
        Some(job(CheckJobState::Cancelled, "cargo test --all", &[]));
    let mark = h.backend.calls.len();
    send_check(
        &mut h,
        job(CheckJobState::Cancelled, "cargo test --all", &[]),
        None,
    );
    assert_eq!(requests(&h, mark), [SHOW]);
    assert_eq!(toasts(&mut h).len(), 2);
    assert_eq!(
        toasts(&mut h)[1],
        toast(
            ToastKind::Info,
            crate::tr!(CHECK_CANCELLED; ; GROUP),
            "cmd /c cargo test --all"
        )
    );
    assert!(!busy(&mut h, &p));

    // podd refuses the run: not busy, the error line says why, nothing re-read.
    s.borrow_mut().checks.push(Err(err(
        Some(orch_codes::INVALID_STATE),
        "a check is running",
    )));
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert_eq!(
        requests(&h, mark),
        ["group.check {\"group\":\"sample/fx\",\"wait\":false}"]
    );
    assert!(!busy(&mut h, &p));
    assert_eq!(
        check_error(&mut h, &p).as_deref(),
        Some("INVALID_STATE: a check is running")
    );
    assert_eq!(toasts(&mut h).len(), 2, "a refused run does not toast");
}

/// B: without a stored command an empty field sends nothing and says so.
#[test]
fn check_without_any_command_sends_nothing() {
    let mut g = group();
    g.check = None;
    let s = state(Some(g));
    let mut h = harness("group-qa-no-check", s);
    let p = open(&mut h);
    assert_eq!(check_text(&mut h, &p), "");
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 1);
    assert!(requests(&h, mark).is_empty());
    assert_eq!(
        check_error(&mut h, &p).as_deref(),
        Some(crate::strings::ENTER_A_CHECK_COMMAND_FIRST)
    );
    assert!(!busy(&mut h, &p));
}

/// J/B: Pick winner from the card by keyboard; a refused pick toasts once and
/// keeps the reason in the banner; members podd kept open the force
/// confirmation ("Keep them" sends nothing), "Remove anyway" sends `force`
/// with the same branch choice and closes the removed members' terminals;
/// every pick re-reads the group and the comparison.
#[test]
fn pick_refusal_kept_members_and_force() {
    let s = state(Some(group()));
    let mut h = harness("group-qa-pick", s.clone());
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("b1", false, window, cx)));
    h.settle();
    let p = open(&mut h);
    let b1_open = |h: &mut Harness| {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_panels()
                .iter()
                .any(|t| t.read(cx).terminal_id == "b1")
        })
    };
    assert!(b1_open(&mut h));
    const PICK: &str = "group.pick {\"group\":\"sample/fx\",\"task\":\"sample/로그인-수정\",\"remove_others\":true,\"force\":false,\"delete_branch\":false}";

    // Card → Pick winner (5 Tabs from the check field) → the dialog's Pick winner.
    s.borrow_mut().picks.push(Err(err(
        Some(orch_codes::INVALID_STATE),
        "group sample/fx already has a winner",
    )));
    let mark = h.backend.calls.len();
    from_check_field(&mut h, &p, 5);
    // Dialog stops: close, remove the others, delete branches, Cancel, Pick winner.
    in_dialog(&mut h, 4);
    assert!(!h.dialog_open());
    assert_eq!(requests(&h, mark), [PICK, SHOW, COMPARE]);
    let refused = "INVALID_STATE: group sample/fx already has a winner";
    let task = A;
    assert_eq!(
        toasts(&mut h),
        [toast(
            ToastKind::Error,
            crate::tr!(COULD_NOT_PICK_TASK; task;),
            refused
        )]
    );
    let banner = h.cx.update(|_, cx| match &p.read(cx).pick {
        Some(PickOutcome::Failed(m)) => Some(m.clone()),
        _ => None,
    });
    assert_eq!(banner.as_deref(), Some(refused));

    // Delete branches on, then Pick: podd removes nothing and keeps B.
    let kept = vec![MemberError {
        task: B.into(),
        code: codes::TASK_DIRTY,
        message: "uncommitted changes".into(),
    }];
    let kept_reply = GroupPickResult {
        group: GROUP.into(),
        picked: A.into(),
        removed: Vec::new(),
        kept: kept.clone(),
    };
    s.borrow_mut().picks.push(Ok(kept_reply.clone()));
    let mark = h.backend.calls.len();
    h.cx.update(|window, cx| p.update(cx, |p, cx| p.open_pick_dialog(A.into(), window, cx)));
    h.settle();
    assert_eq!(
        h.bounds("dialog-0").map(|b| b.size.width),
        Some(px(558.)),
        "the pick dialog's width (560 px set)"
    );
    in_dialog(&mut h, 2);
    assert!(h.dialog_open(), "Enter on the checkbox keeps the dialog");
    h.press("tab");
    h.press("tab");
    h.press("enter");
    assert_eq!(
        requests(&h, mark),
        [
            PICK.replace("\"delete_branch\":false", "\"delete_branch\":true"),
            SHOW.into(),
            COMPARE.into()
        ]
    );
    assert!(
        h.dialog_open(),
        "podd kept a member: the force confirmation"
    );
    assert_eq!(toasts(&mut h).len(), 2);
    assert_eq!(
        toasts(&mut h)[1],
        toast(
            ToastKind::Success,
            crate::tr!(WINNER; ; A),
            crate::strings::NO_MEMBERS_REMOVED
        )
    );
    assert_eq!(
        h.bounds("dialog-0").map(|b| b.size.width),
        Some(px(598.)),
        "the force dialog's width (600 px set)"
    );
    // Force dialog stops: close, Keep them, Remove anyway.
    let mark = h.backend.calls.len();
    in_dialog(&mut h, 1);
    assert!(!h.dialog_open());
    assert!(requests(&h, mark).is_empty(), "Keep them sends nothing");

    // Again; this time Remove anyway.
    s.borrow_mut().picks.push(Ok(kept_reply));
    let mut removed = GroupPickResult {
        group: GROUP.into(),
        picked: A.into(),
        removed: vec![TaskRemoveResult {
            id: B.into(),
            terminals: vec!["b1".into()],
            worktree_removed: true,
            branch_deleted: true,
            branch_kept: None,
        }],
        kept: Vec::new(),
    };
    s.borrow_mut().picks.push(Ok(removed.clone()));
    let mark = h.backend.calls.len();
    h.cx.update(|window, cx| p.update(cx, |p, cx| p.open_pick_dialog(A.into(), window, cx)));
    h.settle();
    in_dialog(&mut h, 4);
    in_dialog(&mut h, 2);
    assert!(!h.dialog_open());
    assert_eq!(
        requests(&h, mark),
        [
            PICK.into(),
            SHOW.into(),
            COMPARE.into(),
            PICK.replace("\"force\":false", "\"force\":true"),
            SHOW.into(),
            COMPARE.into(),
        ]
    );
    assert!(!b1_open(&mut h), "the removed member's terminal closed");
    // The second "kept" winner toast equals the visible first one (not repeated).
    let all = toasts(&mut h);
    assert_eq!(all.len(), 3);
    assert_eq!(
        all[2],
        toast(
            ToastKind::Success,
            crate::tr!(WINNER; ; A),
            crate::tr!(REMOVED; ; B)
        )
    );
    let done = h.cx.update(|_, cx| match &p.read(cx).pick {
        Some(PickOutcome::Done(r)) => Some(r.clone()),
        _ => None,
    });
    removed.kept.clear();
    assert_eq!(done, Some(removed));

    // Remove the others off: the branch choice is disabled and not sent.
    s.borrow_mut().picks.push(Err(err(None, "x")));
    let mark = h.backend.calls.len();
    h.cx.update(|window, cx| p.update(cx, |p, cx| p.open_pick_dialog(A.into(), window, cx)));
    h.settle();
    in_dialog(&mut h, 1);
    assert!(h.dialog_open());
    // Stops now: close, remove the others, Cancel, Pick winner.
    h.press("tab");
    h.press("tab");
    h.press("enter");
    assert_eq!(
        requests(&h, mark)[0],
        PICK.replace("\"remove_others\":true", "\"remove_others\":false")
    );
}

/// J/B: Ctrl+Shift+N, keyboard only. Create with no prompt keeps the dialog
/// and sends nothing; with one it sends `group.create` once with the counts,
/// base and check command, toasts twice and opens the new group's panel with
/// the record podd returned (then reads it and its comparison).
#[test]
fn new_group_dialog_by_keyboard() {
    let s = state(None);
    s.borrow_mut().creates.push(Ok(group()));
    let mut h = harness("group-qa-new", s);
    h.keys("ctrl-shift-n");
    assert!(h.dialog_open());
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "the name field has focus: {}",
        h.focus_name()
    );
    assert_eq!(
        h.bounds("dialog-0").map(|b| b.size.width),
        Some(px(638.)),
        "the dialog's width (640 px set)"
    );
    // Stops from the name: base, prompt, 6 enabled steppers (− of a zero
    // count is disabled), check, Cancel, Create, close, the repo choices.
    h.type_text("fx");
    for _ in 0..11 {
        h.press("tab");
    }
    h.press("enter");
    assert!(h.dialog_open(), "no prompt: the dialog stays");
    assert!(requests(&h, 0).is_empty());
    // Create → close, sample, new repository, name, base: type a base.
    for _ in 0..5 {
        h.press("tab");
    }
    h.type_text("develop");
    h.press("tab");
    h.type_text("fix the login bug");
    // Claude −, Claude +, Codex −, Codex +, Shell + (Enter: one shell member).
    for _ in 0..5 {
        h.press("tab");
    }
    h.press("enter");
    // Custom +, then the check field.
    h.press("tab");
    h.press("tab");
    h.type_text("cargo test");
    h.press("tab");
    h.press("tab");
    h.press("enter");
    assert!(!h.dialog_open());
    assert_eq!(
        requests(&h, 0),
        [
            "group.create {\"repo\":\"sample\",\"name\":\"fx\",\"base\":\"develop\",\"prompt\":\"fix the login bug\",\"agents\":[\"claude\",\"codex\",\"shell\"],\"check\":[\"cmd\",\"/c\",\"cargo test\"]}",
            SHOW,
            COMPARE
        ]
    );
    let n = 3;
    assert_eq!(
        toasts(&mut h),
        [
            toast(
                ToastKind::Info,
                crate::tr!(CREATING_GROUP; ; "fx"),
                crate::tr!(N_MEMBER_TASK_S_WORKTREE_AGENT_EACH; n;)
            ),
            toast(
                ToastKind::Success,
                crate::tr!(GROUP_CREATED; ; GROUP),
                crate::tr!(MEMBERS_FROM; ; 2, "main")
            ),
        ]
    );
    let p = panel(&mut h);
    let f = h.cx.update(|_, cx| p.read(cx).focus_handle());
    assert!(h.is_focused(&f), "the new group's panel has focus");
    assert_eq!(check_text(&mut h, &p), "cargo test");
}

/// J/B: "new repository" adds the typed path first and creates the group in
/// it; podd refusing the group toasts once and opens nothing; without the
/// orchestration capability the dialog does not open.
#[test]
fn new_group_in_a_new_repository_and_refusals() {
    let s = state(None);
    s.borrow_mut().creates.push(Ok(group()));
    let mut h = harness("group-qa-new-repo", s.clone());
    h.keys("ctrl-shift-n");
    h.type_text("fx");
    // Shift+Tab from the name: "new repository"; Enter shows the path field.
    h.press("shift-tab");
    h.press("enter");
    h.press("tab");
    h.type_text("C:/fixtures/other");
    h.press("tab");
    h.press("tab");
    h.press("tab");
    h.type_text("go");
    // 6 steppers, the check field (left empty), Cancel, Create.
    for _ in 0..8 {
        h.press("tab");
    }
    h.press("tab");
    h.press("enter");
    assert!(!h.dialog_open());
    assert_eq!(
        requests(&h, 0),
        [
            "repo.add C:/fixtures/other",
            "group.create {\"repo\":\"other\",\"name\":\"fx\",\"prompt\":\"go\",\"agents\":[\"claude\",\"codex\"]}",
            SHOW,
            COMPARE
        ]
    );
    assert_eq!(panels(&mut h).len(), 1);

    // podd refuses: one error toast with the code's name, no second panel.
    s.borrow_mut().creates.push(Err(err(
        Some(orch_codes::GROUP_EXISTS),
        "group sample/fx exists",
    )));
    let before = toasts(&mut h).len();
    let mark = h.backend.calls.len();
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.create_group(
                None,
                GroupCreateParams {
                    repo: "sample".into(),
                    name: "fx".into(),
                    prompt: "go".into(),
                    agents: vec![AgentSpec::Kind(AgentKind::Shell)],
                    ..Default::default()
                },
                window,
                cx,
            )
        })
    });
    h.settle();
    assert_eq!(
        requests(&h, mark),
        [
            "group.create {\"repo\":\"sample\",\"name\":\"fx\",\"prompt\":\"go\",\"agents\":[\"shell\"]}"
        ]
    );
    let all = toasts(&mut h);
    assert_eq!(all.len(), before + 2);
    assert_eq!(all[before].0, ToastKind::Info);
    let title = GROUP;
    assert_eq!(
        all[before + 1],
        toast(
            ToastKind::Error,
            crate::tr!(COULD_NOT_CREATE_GROUP_TITLE; title;),
            "GROUP_EXISTS: group sample/fx exists"
        )
    );
    assert_eq!(panels(&mut h).len(), 1);

    // An older podd: a warning, no dialog.
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.caps.orch = false;
            w.open_new_group_dialog(window, cx)
        })
    });
    h.settle();
    assert!(!h.dialog_open());
    let all = toasts(&mut h);
    assert_eq!(
        all.last(),
        Some(&toast(
            ToastKind::Warning,
            crate::strings::GROUPS_NEED_A_NEWER_PODD.into(),
            crate::strings::PODD_PROTOCOL_V6_GROUP_IS_REQUIRED
        ))
    );
}
