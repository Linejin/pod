//! `sessions_ui::tests`: the pure helpers, and the page in the headless
//! workspace against a scripted podd on fixture data (no session folders,
//! no agent configs). The scripted-request tests ran on the unsplit page
//! first (A-P7) and pin its requests, busy / error transitions and resume
//! target.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
use podd::protocol::{
    CwdMatch, DiscoveredAgent, DiscoveredSession, SessionRunning, SessionState, SessionsList,
    UnknownRunning, sessions_codes,
};
use serde_json::{Value, json};

use super::copy;
use super::model::{
    Actions, actions, ago, empty_line, facts_line, heading, link_line, no_list_line, notes_line,
};
use super::{SessionsUi, open_page, page_open, s};
use crate::native_qa::Harness;
use crate::source::{SourceError, TestBackend};

fn session(state: SessionState) -> DiscoveredSession {
    DiscoveredSession {
        key: "claude:0f8b6a3e-1111-4222-8333-944445555666".into(),
        agent: DiscoveredAgent::Claude,
        id: "0f8b6a3e-1111-4222-8333-944445555666".into(),
        cwd: r"C:\src\demo".into(),
        title: None,
        preview: None,
        model: None,
        branch: None,
        version: None,
        origin: None,
        started_ms: None,
        last_ms: None,
        messages: None,
        state,
        running: None,
        matched: CwdMatch::default(),
        link: None,
        hidden: false,
        cwd_exists: true,
        resume_argv: vec![],
    }
}

/// The page's cards carry the tool's mark and accessible name (docs/10 §D5.11).
#[test]
fn session_cards_show_agent_marks() {
    let mut codex = session(SessionState::Resumable);
    codex.key = "codex:1".into();
    codex.agent = DiscoveredAgent::Codex;
    let list = podd::protocol::SessionsList {
        enabled: true,
        preview: false,
        scanned_ms: Some(1),
        truncated: false,
        sessions: vec![session(SessionState::Resumable), codex],
        hidden: 0,
        unknown_running: vec![],
    };
    let answer = serde_json::to_value(&list).unwrap();
    let mut h = crate::native_qa::Harness::new("sessions-agent-marks", move |b| {
        let answer = answer.clone();
        b.rpc = Some(Box::new(move |method, _| {
            (method == "sessions.list").then(|| Ok(answer.clone()))
        }));
    });
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| open_page(w, window, cx)));
    h.settle();
    assert!(
        h.bounds("agent-mark:session-claude:0f8b6a3e-1111-4222-8333-944445555666:Claude 에이전트")
            .is_some(),
        "claude session"
    );
    assert!(
        h.bounds("agent-mark:session-codex:1:Codex 에이전트")
            .is_some(),
        "codex session"
    );
}

#[test]
fn actions_follow_state() {
    let x = session(SessionState::Resumable);
    assert_eq!(
        actions(&x),
        Actions {
            open_terminal: None,
            resume: true,
            register: Some(true)
        }
    );
    let mut y = session(SessionState::RunningOther);
    y.running = Some(SessionRunning {
        pid: 5,
        pod_terminal: None,
        guessed: false,
    });
    y.matched = CwdMatch {
        repo: Some("pod".into()),
        task: None,
    };
    assert_eq!(
        actions(&y),
        Actions {
            open_terminal: None,
            resume: false,
            register: Some(false)
        }
    );
    let mut z = session(SessionState::RunningPod);
    z.running = Some(SessionRunning {
        pid: 5,
        pod_terminal: Some("t3".into()),
        guessed: false,
    });
    z.link = Some(CwdMatch {
        repo: Some("pod".into()),
        task: None,
    });
    assert_eq!(
        actions(&z),
        Actions {
            open_terminal: Some("t3".into()),
            resume: false,
            register: None
        }
    );
    let mut gone = session(SessionState::Stale);
    gone.cwd_exists = false;
    assert_eq!(
        actions(&gone),
        Actions {
            open_terminal: None,
            resume: false,
            register: None
        }
    );
}

#[test]
fn headings_and_ages() {
    let mut x = session(SessionState::Resumable);
    assert_eq!(heading(&x), "Claude Code 0f8b6a3e");
    x.preview = Some("첫 프롬프트".into());
    assert_eq!(heading(&x), "첫 프롬프트");
    x.title = Some("제목".into());
    assert_eq!(heading(&x), "제목");
    assert_eq!(ago(Some(1_000), 30_000), "방금");
    assert_eq!(ago(Some(0), 3 * 3600 * 1000), "3시간 전");
    assert_eq!(ago(None, 0), "-");
}

// ---- scripted requests (A-P7: the same requests, busy/error transitions
// and resume target before and after the split) ----------------------------

const KEY: &str = "claude:0f8b6a3e-1111-4222-8333-944445555666";

/// Resumable, folder present, in no repository: resume and "저장소 등록".
fn resumable() -> DiscoveredSession {
    let mut x = session(SessionState::Resumable);
    x.title = Some("fix login".into());
    x.resume_argv = vec!["claude".into(), "--resume".into(), x.id.clone()];
    x
}

/// Running in Pod's terminal t3 and linked: only "터미널 열기".
fn in_pod() -> DiscoveredSession {
    let mut x = session(SessionState::RunningPod);
    x.key = "claude:2".into();
    x.id = "2".into();
    x.running = Some(SessionRunning {
        pid: 7,
        pod_terminal: Some("t3".into()),
        guessed: false,
    });
    x.link = Some(CwdMatch {
        repo: Some("demo".into()),
        task: None,
    });
    x
}

/// Codex in another app's terminal, folder in repository `demo`: "작업으로 등록".
fn elsewhere() -> DiscoveredSession {
    let mut x = session(SessionState::RunningOther);
    x.key = "codex:3".into();
    x.id = "3".into();
    x.agent = DiscoveredAgent::Codex;
    x.running = Some(SessionRunning {
        pid: 8,
        pod_terminal: None,
        guessed: false,
    });
    x.matched = CwdMatch {
        repo: Some("demo".into()),
        task: None,
    };
    x
}

fn listed(sessions: Vec<DiscoveredSession>) -> SessionsList {
    SessionsList {
        enabled: true,
        preview: false,
        scanned_ms: Some(1),
        truncated: false,
        sessions,
        hidden: 0,
        unknown_running: vec![],
    }
}

/// A scripted `sessions.*` podd on fixture data (no session folders, no
/// agent configs): answers from `list`, changes it the way podd does, and
/// fails the methods in `fail`.
struct Daemon {
    list: SessionsList,
    fail: Vec<&'static str>,
}

type Shared = std::rc::Rc<std::cell::RefCell<Daemon>>;

impl Daemon {
    fn shared(list: SessionsList) -> Shared {
        std::rc::Rc::new(std::cell::RefCell::new(Self { list, fail: vec![] }))
    }

    fn answer(&mut self, method: &str, p: &Value) -> Result<Value, SourceError> {
        if self.fail.contains(&method) {
            return Err(SourceError {
                code: Some(sessions_codes::PERSIST_FAILED),
                message: format!("{method}: fixture failure"),
            });
        }
        let flag = |k: &str| p.get(k).and_then(Value::as_bool);
        let key = p.get("session").and_then(Value::as_str).unwrap_or_default();
        let ix = self.list.sessions.iter().position(|x| x.key == key);
        match (method, ix) {
            ("sessions.list", _) => {}
            ("sessions.scan", _) => {
                if flag("enable") == Some(true) {
                    self.list.enabled = true;
                }
                self.list.scanned_ms = Some(2);
            }
            ("sessions.settings", _) => {
                if let Some(on) = flag("enabled") {
                    self.list.enabled = on;
                }
                if let Some(on) = flag("preview") {
                    self.list.preview = on;
                }
            }
            ("sessions.hide", Some(i)) => {
                let x = &mut self.list.sessions[i];
                x.hidden = flag("hidden").unwrap_or(true);
                return Ok(serde_json::to_value(&*x).unwrap());
            }
            ("sessions.register", Some(i)) => {
                let x = &mut self.list.sessions[i];
                x.link = Some(CwdMatch {
                    repo: Some("demo".into()),
                    task: None,
                });
                return Ok(serde_json::to_value(&*x).unwrap());
            }
            ("sessions.resume", Some(i)) => {
                let x = &self.list.sessions[i];
                return Ok(json!({
                    "id": "t9", "argv": x.resume_argv, "cwd": x.cwd,
                    "state": "running", "cols": 80, "rows": 24, "created_ms": 0
                }));
            }
            _ => {
                return Err(SourceError {
                    code: Some(sessions_codes::SESSION_NOT_FOUND),
                    message: format!("{method}: no session {key}"),
                });
            }
        }
        Ok(serde_json::to_value(&self.list).unwrap())
    }
}

fn harness(name: &'static str, daemon: &Shared) -> Harness {
    let d = daemon.clone();
    Harness::new(name, move |b| {
        b.rpc = Some(Box::new(move |method, params| {
            method
                .starts_with("sessions.")
                .then(|| d.borrow_mut().answer(method, params))
        }));
    })
}

/// The `sessions.*` requests sent since the last look, in order.
fn sent(h: &mut Harness) -> Vec<(String, Value)> {
    std::mem::take(&mut h.backend.calls)
        .into_iter()
        .filter(|c| c.starts_with("sessions."))
        .map(|c| {
            let (m, p) = c.split_once(' ').expect("method and params");
            (m.to_string(), serde_json::from_str(p).expect("params JSON"))
        })
        .collect()
}

fn req(method: &str, params: Value) -> (String, Value) {
    (method.to_string(), params)
}

fn open(h: &mut Harness) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| open_page(w, window, cx)));
}

fn click(h: &mut Harness, id: &str) {
    let b = h
        .bounds(id)
        .unwrap_or_else(|| panic!("{id} is not on the page"));
    let at = point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    );
    h.cx.simulate_click(at, Modifiers::none());
}

/// (loading, scanning, failed, show_hidden, pending_open)
fn flags(h: &mut Harness) -> (bool, bool, bool, bool, Option<String>) {
    h.cx.update(|_, cx| {
        let s = cx.global::<SessionsUi>();
        (
            s.loading,
            s.scanning,
            s.failed,
            s.show_hidden,
            s.pending_open.clone(),
        )
    })
}

fn last_toast(h: &mut Harness) -> Option<(String, String)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .last()
            .map(|t| (t.title.to_string(), t.body.to_string()))
    })
}

fn toast(title: &str, body: &str) -> Option<(String, String)> {
    Some((title.to_string(), body.to_string()))
}

const IDLE: (bool, bool, bool, bool, Option<String>) = (false, false, false, false, None);

/// Opening reads `sessions.list` and never scans; discovery stays off. Opens
/// while that read is in flight make one follow-up read after it, however
/// many there are (fx-app: they used to be dropped).
#[test]
fn opening_reads_the_list_once_and_does_not_scan() {
    let d = Daemon::shared(SessionsList::default());
    let mut h = harness("sessions-open", &d);
    open(&mut h);
    open(&mut h);
    open(&mut h);
    assert!(flags(&mut h).0, "loading until podd answers");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.list", json!({"include_hidden": false})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    assert_eq!(flags(&mut h), IDLE);
    assert!(h.cx.update(|_, cx| page_open(cx)));
    assert!(h.bounds("sessions-enable").is_some(), "off: enable button");
    assert!(h.bounds("sessions-rescan").is_none(), "off: no toolbar");
    open(&mut h);
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req("sessions.list", json!({"include_hidden": false}))],
        "re-opening re-reads the list once"
    );
    assert!(!d.borrow().list.enabled);
}

/// fx-app regression: showing hidden sessions while a list read is in flight
/// is not lost. The in-flight read answers with the old filter, then one
/// follow-up read goes out with the new one and the page ends idle.
#[test]
fn a_reread_during_a_read_is_queued_once() {
    let mut list = listed(vec![resumable()]);
    list.hidden = 1;
    let d = Daemon::shared(list);
    let mut h = harness("sessions-reread", &d);
    open(&mut h);
    h.settle();
    sent(&mut h);

    open(&mut h);
    assert!(flags(&mut h).0, "a read is in flight");
    let ws = h.ws.downgrade();
    h.cx.update(|_, cx| super::controller::toggle_hidden(&ws, cx));
    assert!(h.cx.update(|_, cx| cx.global::<SessionsUi>().reread));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.list", json!({"include_hidden": false})),
            req("sessions.list", json!({"include_hidden": true})),
        ]
    );
    assert_eq!(flags(&mut h), (false, false, false, true, None));
    assert!(!h.cx.update(|_, cx| cx.global::<SessionsUi>().reread));
    assert!(last_toast(&mut h).is_none(), "no errors");
}

/// Enabling saves the setting, then scans once (without `enable`).
#[test]
fn enabling_saves_the_setting_then_scans_once() {
    let d = Daemon::shared(SessionsList::default());
    d.borrow_mut().list.sessions = vec![resumable()];
    let mut h = harness("sessions-enable", &d);
    open(&mut h);
    h.settle();
    sent(&mut h);
    click(&mut h, "sessions-enable");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.settings", json!({"enabled": true})),
            req(
                "sessions.scan",
                json!({"enable": false, "include_hidden": false})
            ),
        ]
    );
    assert_eq!(flags(&mut h), IDLE);
    assert!(h.bounds("sessions-rescan").is_some());
    assert!(h.bounds(&format!("session-{KEY}-resume")).is_some());
}

/// Rescan is busy until podd answers; the preview, hidden and disable
/// buttons send the same settings and re-read the list after each.
#[test]
fn toolbar_requests_and_busy_state() {
    let mut list = listed(vec![resumable()]);
    list.hidden = 1;
    let d = Daemon::shared(list);
    let mut h = harness("sessions-toolbar", &d);
    open(&mut h);
    h.settle();
    sent(&mut h);

    click(&mut h, "sessions-rescan");
    assert!(flags(&mut h).1, "scanning until podd answers");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req(
            "sessions.scan",
            json!({"enable": false, "include_hidden": false})
        )]
    );
    assert_eq!(flags(&mut h), IDLE);

    click(&mut h, "sessions-preview");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.settings", json!({"preview": true})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    assert!(d.borrow().list.preview);
    // Off again (podd deletes the stored previews).
    click(&mut h, "sessions-preview");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.settings", json!({"preview": false})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );

    click(&mut h, "sessions-hidden");
    assert!(flags(&mut h).3, "show hidden at once");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req("sessions.list", json!({"include_hidden": true}))]
    );
    click(&mut h, "sessions-rescan");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req(
            "sessions.scan",
            json!({"enable": false, "include_hidden": true})
        )]
    );

    click(&mut h, "sessions-disable");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.settings", json!({"enabled": false})),
            req("sessions.list", json!({"include_hidden": true})),
        ]
    );
    assert!(h.bounds("sessions-enable").is_some(), "off again");
    assert!(last_toast(&mut h).is_none(), "no errors");
}

/// Hide / unhide / register send the session's key, then re-read the list.
#[test]
fn card_requests_carry_the_session_key() {
    let d = Daemon::shared(listed(vec![resumable(), in_pod(), elsewhere()]));
    let mut h = harness("sessions-cards", &d);
    open(&mut h);
    h.settle();
    sent(&mut h);
    assert!(h.bounds("session-claude:2-register").is_none(), "linked");
    assert!(h.bounds("session-claude:2-resume").is_none(), "running");
    assert!(h.bounds("session-codex:3-resume").is_none(), "elsewhere");

    click(&mut h, &format!("session-{KEY}-hide"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.hide", json!({"session": KEY, "hidden": true})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    click(&mut h, &format!("session-{KEY}-hide"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.hide", json!({"session": KEY, "hidden": false})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );

    click(&mut h, "session-codex:3-register");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req(
                "sessions.register",
                json!({"session": "codex:3", "add_repo": false})
            ),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    click(&mut h, &format!("session-{KEY}-register"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req(
                "sessions.register",
                json!({"session": KEY, "add_repo": true})
            ),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    assert_eq!(last_toast(&mut h), toast(s::REGISTERED, "fix login → demo"));
    assert!(
        h.bounds(&format!("session-{KEY}-register")).is_none(),
        "linked now"
    );
    assert_eq!(flags(&mut h), IDLE);
}

/// Resume names the session, then opens the new terminal once the
/// workspace lists it; "터미널 열기" opens the session's Pod terminal.
#[test]
fn resume_opens_the_terminal_once_listed() {
    let d = Daemon::shared(listed(vec![resumable(), in_pod()]));
    let mut h = harness("sessions-resume", &d);
    h.backend.terminals = vec![TestBackend::terminal("t3", "claude", None, "idle")];
    h.backend.publish();
    open(&mut h);
    h.settle();
    sent(&mut h);

    click(&mut h, &format!("session-{KEY}-resume"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.resume", json!({"session": KEY})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    assert_eq!(last_toast(&mut h), toast(s::RESUMED, "fix login · t9"));
    assert_eq!(flags(&mut h).4.as_deref(), Some("t9"), "not listed yet");
    assert_ne!(h.focused_terminal().as_deref(), Some("t9"));

    h.backend
        .terminals
        .push(TestBackend::terminal("t9", "claude resume", None, "idle"));
    h.backend.publish();
    h.settle();
    assert_eq!(flags(&mut h), IDLE);
    assert_eq!(h.focused_terminal().as_deref(), Some("t9"));

    open(&mut h);
    h.settle();
    sent(&mut h);
    click(&mut h, "session-claude:2-open");
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("t3"));
    assert!(sent(&mut h).is_empty(), "opening asks podd nothing");
}

/// A failed read shows "unavailable" only while there is no list; any
/// failure raises one error toast; failed hide / settings do not re-read,
/// failed register / resume still do, and nothing waits to open.
#[test]
fn failures_toast_and_keep_the_page() {
    let d = Daemon::shared(listed(vec![resumable(), elsewhere()]));
    d.borrow_mut().fail = vec!["sessions.list"];
    let mut h = harness("sessions-failures", &d);
    open(&mut h);
    h.settle();
    assert_eq!(sent(&mut h).len(), 1);
    assert_eq!(flags(&mut h), (false, false, true, false, None));
    assert!(h.bounds("sessions-rescan").is_none(), "no list yet");

    d.borrow_mut().fail.clear();
    open(&mut h);
    h.settle();
    sent(&mut h);
    assert_eq!(flags(&mut h), IDLE);

    d.borrow_mut().fail = vec![
        "sessions.list",
        "sessions.scan",
        "sessions.settings",
        "sessions.hide",
        "sessions.register",
        "sessions.resume",
    ];
    click(&mut h, "sessions-preview");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req("sessions.settings", json!({"preview": true}))]
    );
    assert_eq!(flags(&mut h), IDLE, "the list stays");

    click(&mut h, "sessions-rescan");
    assert!(flags(&mut h).1);
    h.settle();
    assert_eq!(sent(&mut h).len(), 1);
    assert_eq!(flags(&mut h), IDLE, "not busy after a failed scan");

    click(&mut h, &format!("session-{KEY}-hide"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![req(
            "sessions.hide",
            json!({"session": KEY, "hidden": true})
        )]
    );

    click(&mut h, "session-codex:3-register");
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req(
                "sessions.register",
                json!({"session": "codex:3", "add_repo": false})
            ),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );

    click(&mut h, &format!("session-{KEY}-resume"));
    h.settle();
    assert_eq!(
        sent(&mut h),
        vec![
            req("sessions.resume", json!({"session": KEY})),
            req("sessions.list", json!({"include_hidden": false})),
        ]
    );
    assert_eq!(flags(&mut h), IDLE, "nothing to open, the list stays");
    assert!(
        h.bounds(&format!("agent-mark:session-{KEY}:Claude 에이전트"))
            .is_some()
    );
    // One error toast per failure; the same visible toast is not repeated.
    let ws = h.ws.clone();
    let all: Vec<(String, String)> = h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .iter()
            .map(|t| (t.title.to_string(), t.body.to_string()))
            .collect()
    });
    let fail = |m: &str| {
        (
            s::FAILED.to_string(),
            format!("sessions.{m}: fixture failure"),
        )
    };
    assert_eq!(
        all,
        vec![
            fail("list"),
            fail("settings"),
            fail("scan"),
            fail("hide"),
            fail("register"),
            fail("resume"),
        ]
    );
}

// ---- the split's pure helpers (A-P7): same lines as the unsplit page --------

/// A card's facts and link and the page's notes and placeholder lines.
#[test]
fn card_and_page_lines() {
    let now = 3 * 3600 * 1000;
    let mut x = session(SessionState::Resumable);
    assert_eq!(
        facts_line(&x, now),
        r"Claude Code · C:\src\demo · 마지막 활동 -"
    );
    x.branch = Some("main".into());
    x.last_ms = Some(0);
    x.messages = Some(12);
    x.model = Some("opus".into());
    assert_eq!(
        facts_line(&x, now),
        r"Claude Code · C:\src\demo · 브랜치 main · 마지막 활동 3시간 전 · 메시지 약 12개 · opus"
    );
    assert_eq!(link_line(&x), "Pod에 등록되지 않은 폴더");
    x.matched = CwdMatch {
        repo: Some("pod".into()),
        task: None,
    };
    assert_eq!(link_line(&x), "일치: pod");
    x.link = Some(CwdMatch {
        repo: Some("pod".into()),
        task: Some("pod/fix".into()),
    });
    assert_eq!(link_line(&x), "연결됨: pod/fix");
    x.link = Some(CwdMatch::default());
    assert_eq!(link_line(&x), "연결됨: ");

    let mut list = listed(vec![x]);
    list.scanned_ms = Some(0);
    list.unknown_running = vec![UnknownRunning::default()];
    assert_eq!(
        notes_line(&list, now),
        "마지막으로 찾은 시각: 3시간 전 · 세션 1개 · 세션을 알 수 없는 실행 중 에이전트 1개"
    );
    assert_eq!(empty_line(&list), s::EMPTY);
    list.scanned_ms = None;
    list.sessions.clear();
    list.unknown_running.clear();
    assert_eq!(notes_line(&list, now), "세션 0개");
    assert_eq!(empty_line(&list), s::NOT_SCANNED);
    assert_eq!(no_list_line(true, false), s::UNAVAILABLE);
    assert_eq!(no_list_line(true, true), s::LOADING);
    assert_eq!(no_list_line(false, false), s::LOADING);
    assert_eq!(ago(Some(0), 61_000), "1분 전");
    assert_eq!(ago(Some(0), 2 * 86_400_000), "2일 전");
}

/// UIA names of the buttons and cards.
#[test]
fn button_and_card_names() {
    assert_eq!(copy::rescan_name(), "발견된 세션 — 다시 찾기");
    assert_eq!(copy::preview_label(true), s::PREVIEW_ON);
    assert_eq!(
        copy::preview_name(true),
        "첫 프롬프트 표시: 켜짐. 누르면 끕니다"
    );
    assert_eq!(
        copy::preview_name(false),
        "첫 프롬프트 표시: 꺼짐. 누르면 켭니다"
    );
    assert_eq!(copy::hidden_label(false, 2), "숨긴 세션 보기 (2)");
    assert_eq!(copy::hidden_name(false, 2), "숨긴 세션 보기 2개");
    assert_eq!(copy::hidden_label(true, 2), s::HIDE_HIDDEN);
    assert_eq!(copy::hidden_name(true, 2), s::HIDE_HIDDEN);
    assert_eq!(
        copy::card_name("fix login", "Claude 에이전트", s::RESUMABLE),
        "fix login, Claude 에이전트, 재개 가능"
    );
    assert_eq!(
        copy::action_name("fix login", s::RESUME),
        "fix login 이 세션 재개"
    );
}

/// A list-shaped reply ends loading and scanning; a failure marks the page
/// unavailable only while it has no list, and the list stays.
#[test]
fn replies_end_busy_and_keep_the_list() {
    let err = |m: &str| {
        Err(SourceError {
            code: Some(sessions_codes::PERSIST_FAILED),
            message: m.into(),
        })
    };
    let mut st = SessionsUi {
        loading: true,
        scanning: true,
        ..Default::default()
    };
    assert_eq!(st.answered(err("first")), Some("first".into()));
    assert!(!st.loading && !st.scanning && st.failed, "no list yet");
    assert_eq!(st.answered(Ok(listed(vec![]))), None);
    assert!(!st.failed && st.list.is_some());
    st.scanning = true;
    assert_eq!(st.answered(err("again")), Some("again".into()));
    assert!(
        !st.scanning && !st.failed && st.list.is_some(),
        "a failure keeps the list"
    );
}
