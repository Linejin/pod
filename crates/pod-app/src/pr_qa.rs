//! Headless UI tests of the PR section (docs/15), native-QA style: the real
//! workspace, Review panel and PR section on GPUI's test platform; podd's
//! `task.pr.*` answered by the **fake forge** (pod-forge) through the scripted
//! backend. Keyboard only; nothing contacts GitHub or a daemon.

use std::path::Path;
use std::rc::Rc;

use core::prelude::v1::test;
use gpui_kit::*;
use pod_forge::Forge as _;
use pod_forge::fake::{FakeForge, FakeState};
use podd::protocol::{self as p, ForgeRemote, MergeMethod, PrState, RepoSlug};
use serde_json::Value;

use crate::native_qa::{Harness, seed_two_tasks};
use crate::pr_panel::PrSection;
use crate::source::{SourceError, TestBackend};

const TASK: &str = "sample/로그인-수정";
const BRANCH: &str = "pod/로그인-수정";

fn remote() -> ForgeRemote {
    ForgeRemote {
        name: "origin".into(),
        url: "https://github.com/pod-fixtures/sample.git".into(),
        slug: RepoSlug {
            host: "github.com".into(),
            owner: "pod-fixtures".into(),
            name: "sample".into(),
        },
    }
}

/// `task.pr.*` the way podd answers them, over a fake forge.
fn serve_pr(
    forge: Rc<FakeForge>,
    ahead: u32,
) -> impl FnMut(&str, &Value) -> Option<Result<Value, SourceError>> {
    move |method, params| {
        let dir = Path::new(".");
        let fail = |e: pod_forge::ForgeError| {
            Some(Err(SourceError {
                code: Some(p::pr_codes::FORGE_FAILED),
                message: format!("the GitHub command failed: {}", e.message),
            }))
        };
        let info = |st: pod_forge::PrStatus| p::TaskPrInfo {
            task: TASK.into(),
            branch: BRANCH.into(),
            default_base: Some("main".into()),
            ahead,
            unpushed: 0,
            dirty: 1,
            cached: st.remote.is_none(),
            status: st,
            refreshed_ms: 0,
        };
        let st = match method {
            "task.pr.status" => pod_forge::status::collect(&*forge, dir, BRANCH, None),
            "task.pr.create" => {
                let req = pod_forge::CreatePr {
                    head: BRANCH.into(),
                    base: params["base"].as_str().unwrap_or("main").into(),
                    title: params["title"].as_str().unwrap_or("").into(),
                    body: params["body"].as_str().unwrap_or("").into(),
                    draft: params["draft"].as_bool().unwrap_or(false),
                };
                let r = match forge.detect(dir) {
                    Ok(r) => r,
                    Err(e) => return fail(e),
                };
                if let Err(e) = forge
                    .push(dir, &r, BRANCH)
                    .and_then(|_| forge.create(dir, &r, &req))
                {
                    return fail(e);
                }
                pod_forge::status::collect(&*forge, dir, BRANCH, None)
            }
            "task.pr.merge" => {
                let m = params["method"]
                    .as_str()
                    .and_then(MergeMethod::parse)
                    .unwrap_or(MergeMethod::Merge);
                let n = forge
                    .state()
                    .pr_for(BRANCH)
                    .map(|x| x.view.number)
                    .unwrap_or(0);
                if let Err(e) = forge.merge(dir, &remote(), n, m) {
                    return fail(e);
                }
                pod_forge::status::collect(&*forge, dir, BRANCH, None)
            }
            _ => return None,
        };
        Some(Ok(serde_json::to_value(info(st)).unwrap()))
    }
}

fn harness(name: &'static str, forge: Rc<FakeForge>, ahead: u32) -> Harness {
    Harness::new(name, move |b: &mut TestBackend| {
        seed_two_tasks(b);
        b.rpc = Some(Box::new(serve_pr(forge, ahead)));
    })
}

fn open_pr(h: &mut Harness, form: bool) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_pr(TASK, form, window, cx)));
    h.settle();
}

fn section(h: &mut Harness) -> Entity<PrSection> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == TASK)
            .expect("review panel")
            .read(cx)
            .pr_section()
    })
}

fn info(h: &mut Harness) -> p::TaskPrInfo {
    let s = section(h);
    h.cx.update(|_, cx| s.read(cx).info().cloned())
        .expect("PR info loaded")
}

#[test]
fn pr_without_a_remote_shows_the_connect_guidance() {
    let forge = Rc::new(FakeForge::new(FakeState {
        no_remote: true,
        ..FakeState::default()
    }));
    let mut h = harness("pr-no-remote", forge.clone(), 2);
    open_pr(&mut h, false);
    let i = info(&mut h);
    assert!(i.status.no_remote(), "{i:?}");
    assert!(h.bounds("pr-problem").is_some(), "empty state card drawn");
    assert!(
        h.bounds("pr-create").is_none() && h.bounds("pr-form").is_none(),
        "no create button without a remote"
    );
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("task.pr.status") && c.contains("\"cached\":false")),
        "shown tab asks GitHub: {:?}",
        h.backend.calls
    );
    assert!(
        forge.state().calls.iter().all(|c| c == "detect"),
        "nothing but detection: {:?}",
        forge.state().calls
    );
}

#[test]
fn pr_create_by_keyboard_prefills_and_pushes() {
    let forge = Rc::new(FakeForge::new(FakeState {
        remote: Some(remote()),
        ..FakeState::default()
    }));
    let mut h = harness("pr-create", forge.clone(), 2);
    open_pr(&mut h, true);
    // The form is open with the title focused and prefilled from the task.
    assert!(h.bounds("pr-form").is_some(), "form drawn");
    let name = h.focus_name();
    assert!(
        name.starts_with("review panel text input (value \"로그인-수정\")"),
        "title focused and prefilled: {name}"
    );
    // Title → base → body → draft → cancel → submit, by Tab.
    let mut stops = vec![name];
    for _ in 0..5 {
        h.keys("tab");
        stops.push(h.focus_name());
    }
    assert!(
        stops[1].contains("value \"main\""),
        "base prefilled with the task base: {stops:?}"
    );
    assert!(
        stops[2].contains("Pod 작업 `sample/로그인-수정`에서 만들었습니다."),
        "body prefilled: {stops:?}"
    );
    h.press("enter");
    let created = forge.state();
    assert_eq!(
        created.prs.len(),
        1,
        "a pull request was created via the keyboard: calls {:?} / stops {stops:?}",
        h.backend.calls
    );
    assert!(
        created.calls.iter().position(|c| c.starts_with("push"))
            < created.calls.iter().position(|c| c.starts_with("create")),
        "push first"
    );
    let i = info(&mut h);
    assert_eq!(
        i.status.pr.as_ref().map(|p| (p.number, p.state)),
        Some((1, PrState::Open))
    );
    assert!(
        h.bounds("pr-card").is_some() && h.bounds("pr-checks").is_some(),
        "PR card with checks"
    );
}

#[test]
fn pr_failed_checks_then_merge_after_confirmation() {
    let forge = Rc::new(FakeForge::new(FakeState {
        remote: Some(remote()),
        new_checks: Some(pod_forge::fixtures::checks_failed()),
        ..FakeState::default()
    }));
    forge.push(Path::new("."), &remote(), BRANCH).unwrap();
    forge
        .create(
            Path::new("."),
            &remote(),
            &pod_forge::CreatePr {
                head: BRANCH.into(),
                base: "main".into(),
                title: "t".into(),
                body: String::new(),
                draft: false,
            },
        )
        .unwrap();
    let mut h = harness("pr-merge", forge.clone(), 2);
    open_pr(&mut h, false);
    let i = info(&mut h);
    assert_eq!(i.status.summary.verdict(), "failed");
    assert!(
        h.bounds("pr-merge").is_some(),
        "merge controls for an open, non-draft PR"
    );
    // Merge asks first; Escape cancels without merging.
    let s = section(&mut h);
    h.cx.update(|window, cx| s.update(cx, |s, cx| s.confirm_merge(window, cx)));
    h.settle();
    assert!(h.dialog_open(), "confirmation dialog");
    h.keys("escape");
    assert!(!h.dialog_open());
    assert_eq!(
        forge.state().prs[0].view.state,
        PrState::Open,
        "not merged after cancel"
    );
    // Confirm by keyboard: Tab to 병합 (after the close and 취소 buttons) and Enter.
    let mut merged = false;
    for tabs in 1..=5 {
        h.cx.update(|window, cx| s.update(cx, |s, cx| s.confirm_merge(window, cx)));
        h.settle();
        for _ in 0..tabs {
            h.keys("tab");
        }
        h.press("enter");
        if forge.state().prs[0].view.state == PrState::Merged {
            merged = true;
            println!("[native-qa] pr-merge | merged after {tabs} Tab(s) + Enter");
            break;
        }
        assert!(
            !forge.state().calls.iter().any(|c| c.starts_with("merge")),
            "only 병합 merges"
        );
        if h.dialog_open() {
            h.keys("escape");
        }
    }
    assert!(
        merged,
        "merged through the dialog by keyboard: {:?}",
        forge.state().calls
    );
    assert_eq!(
        forge.state().prs[0].merged_with,
        Some(MergeMethod::Squash),
        "default method is squash"
    );
    assert_eq!(info(&mut h).status.pr.unwrap().state, PrState::Merged);
}
