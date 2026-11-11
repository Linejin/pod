//! The extensions UI: the labels, the toast-click target and the preset size
//! rule (plain tests), and headless scenarios on GPUI's test platform against
//! a scripted podd (the native QA harness). The scenarios pin what the A-P6
//! split moves: each request's method and params (J), request and toast
//! counts and order, the list/log state each answer leaves, the palette rows
//! and the "확장" page's dock identity and focus (B). Layout presets end to
//! end are `native_qa::extensions::h_*`. The last group checks the plans in
//! `model` (palette ids, state and error lines, permission toggles, a card's
//! lines, the Settings summary, notice routing) with the same fixture list.
//! Run with `--nocapture` for the harness's step log.

// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use podd::protocol::{
    LayoutPreset, ModInfo, ModListResult, ModState, Notice, Permission, StopReason,
};
use serde_json::{Value, json};

use super::controller::{grant, reload, set_enabled, toggle_log};
use super::copy::{self, permission_label, stop_reason_label};
use super::layouts::Built;
use super::model::{self, click_terminal};
use super::{ExtensionsPanel, ModsUi, open_page, page_open, palette_entries, refresh, run};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::os_notify::Target;
use crate::source::{SourceError, SourceEvent, TestBackend};
use crate::workspace::ToastKind;

#[test]
fn a_collapsed_split_hands_its_size_to_the_remaining_child() {
    let lone = Built::Tabs(Vec::new(), Some(0.25), true);
    assert_eq!(lone.with_size(Some(0.6)).size(), Some(0.6));
    assert_eq!(
        Built::Tabs(Vec::new(), Some(0.25), true)
            .with_size(None)
            .size(),
        None,
        "an unsized split leaves the child unsized"
    );
}

#[test]
fn toast_click_shows_the_notice_terminal_or_its_task() {
    let terminals = vec!["t1".to_string(), "t2".to_string()];
    let tasks = vec![
        (
            "repo/a".to_string(),
            vec!["gone".to_string(), "t2".to_string()],
        ),
        ("repo/b".to_string(), vec![]),
    ];
    let t = |task: Option<&str>, terminal: Option<&str>| Target {
        task: task.map(Into::into),
        terminal: terminal.map(Into::into),
    };
    assert_eq!(
        click_terminal(&t(Some("repo/a"), Some("t1")), &terminals, &tasks).as_deref(),
        Some("t1"),
        "the notice's terminal first"
    );
    assert_eq!(
        click_terminal(&t(Some("repo/a"), Some("closed")), &terminals, &tasks).as_deref(),
        Some("t2"),
        "closed terminal: the task's first live one"
    );
    assert_eq!(
        click_terminal(&t(Some("repo/a"), None), &terminals, &tasks).as_deref(),
        Some("t2")
    );
    assert_eq!(
        click_terminal(&t(Some("repo/b"), None), &terminals, &tasks),
        None,
        "task without terminals: only bring Pod forward"
    );
    assert_eq!(click_terminal(&t(None, None), &terminals, &tasks), None);
}

#[test]
fn labels_cover_every_permission_and_reason() {
    for p in Permission::ALL {
        assert!(!permission_label(p).is_empty());
    }
    for r in [
        StopReason::Cpu,
        StopReason::Memory,
        StopReason::Errors,
        StopReason::Load,
    ] {
        assert!(!stop_reason_label(Some(r)).is_empty());
    }
}

#[test]
fn example_presets_parse() {
    for f in ["review-focus.json", "agents-quad.json"] {
        let text = std::fs::read_to_string(format!(
            "{}/../../examples/mods/pod-layouts/layouts/{f}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let p = LayoutPreset::parse(&text).unwrap();
        assert!(!p.title.is_empty());
    }
}

// ───────────────── headless scenarios (scripted podd) ─────────────────

/// What the scripted podd answers to the `mod.*` requests.
#[derive(Default)]
struct Script {
    /// `mod.list` fails while set.
    list_fails: bool,
    /// `mod.reload` fails while set.
    reload_fails: bool,
}

fn example_preset(f: &str) -> Value {
    let text = std::fs::read_to_string(format!(
        "{}/../../examples/mods/pod-layouts/layouts/{f}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("example preset");
    serde_json::to_value(LayoutPreset::parse(&text).expect("valid preset")).expect("json")
}

/// A preset without a description (the palette falls back to the mod's name).
fn bare_preset() -> Value {
    let p = LayoutPreset::parse(
        r#"{"id": "solo", "title": "하나", "center": {"panel": "terminal", "index": 0}}"#,
    )
    .expect("valid preset");
    serde_json::to_value(p).expect("json")
}

/// The running mod: two commands (one without detail), three presets (one
/// without description), two requested permissions and one not requested.
fn demo(enabled: bool, commands_granted: bool) -> Value {
    json!({
        "name": "demo", "version": "1.2.0", "title": "데모", "description": "fixture mod",
        "enabled": enabled, "state": if enabled { "running" } else { "disabled" },
        "permissions": [
            {"name": "notify", "requested": true, "granted": true},
            {"name": "commands", "requested": true, "granted": commands_granted},
            {"name": "gate", "requested": false, "granted": false}
        ],
        "checks": [{"name": "lint", "argv": ["cargo", "clippy"]}],
        "commands": [
            {"id": "hello", "title": "인사", "detail": "인사를 보냅니다"},
            {"id": "bye", "title": "작별"}
        ],
        "detectors": [{"match": ["claude", "codex"]}],
        "layouts": [example_preset("review-focus.json"), example_preset("agents-quad.json"), bare_preset()],
        "log_lines": 2, "memory_kib": 512
    })
}

/// `mod.list`: the running mod, a disabled one, a stopped one with commands
/// (not in the palette) and an invalid one.
fn mod_list() -> Value {
    json!({
        "enabled": true, "root": "C:/fixtures/mods", "dropped_events": 0,
        "mods": [
            demo(true, false),
            {"name": "sleepy", "version": "0.1.0", "enabled": false, "state": "disabled",
             "permissions": [], "commands": [{"id": "x", "title": "X"}], "log_lines": 0, "memory_kib": 0},
            {"name": "crashed", "version": "0.2.0", "title": "멈춘 모드", "enabled": true, "state": "stopped",
             "stop_reason": "cpu", "error": "time limit\nstack", "permissions": [],
             "commands": [{"id": "y", "title": "Y"}], "log_lines": 9, "memory_kib": 2048},
            {"name": "broken", "version": "0.1.0", "enabled": true, "state": "invalid",
             "error": "layout layouts/bad.json: a split needs 2-6 children", "permissions": [],
             "log_lines": 0, "memory_kib": 0}
        ]
    })
}

fn err(message: &str) -> SourceError {
    SourceError {
        code: None,
        message: message.into(),
    }
}

fn harness(name: &'static str, script: &Rc<RefCell<Script>>) -> Harness {
    let script = script.clone();
    Harness::new(name, move |b: &mut TestBackend| {
        seed_two_tasks(b);
        b.rpc = Some(Box::new(move |method, params| {
            let s = script.borrow();
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            Some(match method {
                "mod.list" if s.list_fails => Err(err("mod.list down")),
                "mod.list" => Ok(mod_list()),
                "mod.reload" if s.reload_fails => Err(err("reload refused")),
                "mod.reload" => {
                    let mut list = mod_list();
                    list["mods"].as_array_mut().expect("mods").truncate(1);
                    Ok(list)
                }
                "mod.set_enabled" if name == "ghost" => Err(err("no mod ghost")),
                "mod.grant" if name == "ghost" => Err(err("grant refused")),
                "mod.set_enabled" => Ok(demo(params["enabled"] == json!(true), false)),
                "mod.grant" => Ok(demo(true, params["granted"] == json!(true))),
                "mod.log" if name == "demo" => Ok(json!({"name": "demo", "lines": ["a", "b"]})),
                "mod.log" => Err(err("no log")),
                "mod.command" if params["command"] == json!("fail") => Err(err("boom")),
                "mod.command" => Ok(json!({})),
                _ => return None,
            })
        }));
    })
}

/// The `mod.*` requests since `from`, as `method params`.
fn mod_calls(h: &Harness, from: usize) -> Vec<String> {
    h.backend.calls[from..]
        .iter()
        .filter(|c| c.starts_with("mod."))
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

fn toast(kind: ToastKind, title: &str, body: &str) -> (ToastKind, String, String) {
    (kind, title.into(), body.into())
}

/// `(name, state, enabled, granted permissions)` of every listed mod.
fn listed(h: &mut Harness) -> Vec<(String, ModState, bool, Vec<Permission>)> {
    h.cx.update(|_, cx| {
        cx.global::<ModsUi>()
            .list
            .as_ref()
            .map(|l| {
                l.list
                    .mods
                    .iter()
                    .map(|m| {
                        let granted = m
                            .permissions
                            .iter()
                            .filter(|p| p.granted)
                            .map(|p| p.name)
                            .collect();
                        (m.name.clone(), m.state.clone(), m.enabled, granted)
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn run_id(h: &mut Harness, id: &str) -> bool {
    let ws = h.ws.clone();
    let id = id.to_string();
    let handled =
        h.cx.update(|window, cx| ws.update(cx, |w, cx| run(w, &id, window, cx)));
    h.settle();
    handled
}

#[test]
fn palette_rows_list_running_mods_commands_then_presets() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-palette-rows", &script);
    let rows = h.cx.update(|_, cx| palette_entries(cx));
    let row = |id: &str, title: &str, detail: &str| {
        (id.to_string(), title.to_string(), detail.to_string())
    };
    assert_eq!(
        rows,
        vec![
            row("mod:demo:hello", "데모: 인사", "인사를 보냅니다"),
            row("mod:demo:bye", "데모: 작별", "확장 demo"),
            row(
                "layout:demo:review-focus",
                "레이아웃: 리뷰 집중",
                "왼쪽에 변경 검토, 오른쪽 위에 작업의 첫 터미널, 아래에 승인 요청함"
            ),
            row(
                "layout:demo:agents-quad",
                "레이아웃: 4분할 에이전트",
                "현재 작업(없으면 열린 패널)의 터미널 네 개를 2×2로"
            ),
            row("layout:demo:solo", "레이아웃: 하나", "데모"),
        ],
        "running mods only: commands, then presets; detail falls back to the mod"
    );
}

#[test]
fn palette_commands_send_the_task_and_focused_terminal() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-command", &script);
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let from = h.backend.calls.len();

    assert!(run_id(&mut h, "mod:demo:hello"));
    assert!(
        run_id(&mut h, "mod:demo:a:b"),
        "the command keeps its colons"
    );
    assert_eq!(
        mod_calls(&h, from),
        vec![
            r#"mod.command {"name":"demo","command":"hello","task":"sample/로그인-수정","terminal":"w1"}"#,
            r#"mod.command {"name":"demo","command":"a:b","task":"sample/로그인-수정","terminal":"w1"}"#,
        ]
    );
    assert!(toasts(&mut h).is_empty(), "a command that worked is quiet");

    let from = h.backend.calls.len();
    assert!(run_id(&mut h, "mod:demo:fail"));
    assert_eq!(mod_calls(&h, from).len(), 1);
    assert_eq!(
        toasts(&mut h),
        vec![toast(ToastKind::Error, super::s::COMMAND_FAILED, "boom")],
        "a failed command: one error toast with podd's message"
    );
}

#[test]
fn malformed_and_foreign_palette_ids() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-ids", &script);
    let from = h.backend.calls.len();
    assert!(
        run_id(&mut h, "mod:demo"),
        "a mod id without a command is still ours"
    );
    assert!(
        run_id(&mut h, "layout:demo"),
        "so is a preset id without a preset"
    );
    assert!(!run_id(&mut h, "orch:board"), "other ids are not");
    assert!(!run_id(&mut h, "demo:hello"));
    assert!(mod_calls(&h, from).is_empty(), "no request");
    assert!(toasts(&mut h).is_empty(), "and no toast");

    assert!(run_id(&mut h, "layout:demo:gone"));
    assert_eq!(
        toasts(&mut h),
        vec![toast(
            ToastKind::Error,
            super::s::LAYOUT_MISSING,
            &format!("{} (demo: gone)", super::s::LAYOUT_MISSING_DETAIL)
        )]
    );
    assert!(
        mod_calls(&h, from).is_empty(),
        "a preset is looked up locally"
    );
}

#[test]
fn mod_list_is_asked_once_at_a_time_and_a_failure_keeps_the_list() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-list", &script);
    assert_eq!(
        mod_calls(&h, 0),
        vec!["mod.list {}"],
        "connecting (caps) asks once"
    );
    assert_eq!(listed(&mut h).len(), 4);

    script.borrow_mut().list_fails = true;
    let from = h.backend.calls.len();
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.update(cx, |w, cx| {
            refresh(w, cx);
            refresh(w, cx);
        })
    });
    h.settle();
    assert_eq!(
        mod_calls(&h, from),
        vec!["mod.list {}"],
        "no second request while one is in flight"
    );
    let (loading, failed) = h.cx.update(|_, cx| {
        let s = cx.global::<ModsUi>();
        (s.loading, s.failed)
    });
    assert!(!loading && failed);
    assert_eq!(
        listed(&mut h).len(),
        4,
        "a failed refresh keeps the last list"
    );
    assert!(
        toasts(&mut h).is_empty(),
        "a failed list read does not toast"
    );

    script.borrow_mut().list_fails = false;
    h.cx.update(|_, cx| ws.update(cx, refresh));
    h.settle();
    let failed = h.cx.update(|_, cx| cx.global::<ModsUi>().failed);
    assert!(!failed, "the next answer clears the failure");
}

#[test]
fn settings_actions_send_their_params_and_apply_the_answer() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-settings-actions", &script);
    let weak = h.ws.downgrade();
    let from = h.backend.calls.len();

    h.cx.update(|_, cx| set_enabled(&weak, "demo".into(), false, cx));
    h.settle();
    assert_eq!(
        listed(&mut h)[0].1,
        ModState::Disabled,
        "the answer replaces the card"
    );
    h.cx.update(|_, cx| grant(&weak, "demo".into(), Permission::Commands, true, cx));
    h.settle();
    assert_eq!(
        listed(&mut h)[0],
        (
            "demo".to_string(),
            ModState::Running,
            true,
            vec![Permission::Notify, Permission::Commands]
        )
    );
    assert_eq!(
        mod_calls(&h, from),
        vec![
            r#"mod.set_enabled {"name":"demo","enabled":false}"#,
            r#"mod.grant {"name":"demo","permission":"commands","granted":true}"#,
        ]
    );
    assert!(toasts(&mut h).is_empty());

    // Refused: one error toast each, the list stays.
    let before = listed(&mut h);
    h.cx.update(|_, cx| set_enabled(&weak, "ghost".into(), true, cx));
    h.settle();
    h.cx.update(|_, cx| grant(&weak, "ghost".into(), Permission::Notify, false, cx));
    h.settle();
    assert_eq!(listed(&mut h), before);
    assert_eq!(
        toasts(&mut h),
        vec![
            toast(ToastKind::Error, super::s::CHANGE_FAILED, "no mod ghost"),
            toast(ToastKind::Error, super::s::CHANGE_FAILED, "grant refused"),
        ]
    );

    // Reload replaces the whole list; a refused reload toasts and keeps it.
    let from = h.backend.calls.len();
    h.cx.update(|_, cx| reload(&weak, cx));
    h.settle();
    assert_eq!(listed(&mut h).len(), 1, "the reload answer is the new list");
    script.borrow_mut().reload_fails = true;
    h.cx.update(|_, cx| reload(&weak, cx));
    h.settle();
    assert_eq!(listed(&mut h).len(), 1);
    assert_eq!(mod_calls(&h, from), vec!["mod.reload {}", "mod.reload {}"]);
    assert_eq!(
        toasts(&mut h).last(),
        Some(&toast(
            ToastKind::Error,
            super::s::CHANGE_FAILED,
            "reload refused"
        ))
    );
}

#[test]
fn the_log_opens_one_mod_at_a_time_and_closing_sends_nothing() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-log", &script);
    let weak = h.ws.downgrade();
    let from = h.backend.calls.len();
    let log = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            let s = cx.global::<ModsUi>();
            (s.log_open.clone(), s.logs.clone())
        })
    };

    h.cx.update(|_, cx| toggle_log(&weak, "demo".into(), cx));
    h.settle();
    let (open, logs) = log(&mut h);
    assert_eq!(open.as_deref(), Some("demo"));
    assert_eq!(
        logs.get("demo"),
        Some(&vec!["a".to_string(), "b".to_string()])
    );

    h.cx.update(|_, cx| toggle_log(&weak, "crashed".into(), cx));
    h.settle();
    let (open, logs) = log(&mut h);
    assert_eq!(open.as_deref(), Some("crashed"), "one log at a time");
    assert_eq!(
        logs.get("crashed"),
        Some(&vec!["no log".to_string()]),
        "a failed read shows podd's message as the log"
    );

    h.cx.update(|_, cx| toggle_log(&weak, "crashed".into(), cx));
    h.settle();
    let (open, logs) = log(&mut h);
    assert_eq!(open, None, "the same button closes it");
    assert_eq!(logs.len(), 2, "closing keeps what was read");
    assert_eq!(
        mod_calls(&h, from),
        vec![
            r#"mod.log {"name":"demo","tail":60}"#,
            r#"mod.log {"name":"crashed","tail":60}"#,
        ],
        "closing sends nothing"
    );
    assert!(toasts(&mut h).is_empty());
}

#[test]
fn mod_events_replace_the_card_and_toast_with_the_mod_title() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-events", &script);
    // In-app toasts only: no Windows toast from a test.
    h.cx.update(|_, cx| cx.global_mut::<crate::theme::Settings>().os_notifications = false);
    let from = h.backend.calls.len();

    let changed: ModInfo = serde_json::from_value(demo(false, true)).expect("mod info");
    let mut fresh = changed.clone();
    fresh.name = "fresh".into();
    for m in [changed, fresh] {
        let _ = h
            .backend
            .events
            .unbounded_send(SourceEvent::ModChanged(Box::new(m)));
    }
    h.settle();
    let names: Vec<String> = listed(&mut h).into_iter().map(|m| m.0).collect();
    assert_eq!(
        names,
        ["demo", "sleepy", "crashed", "broken", "fresh"],
        "replaced in place; unknown appended"
    );
    assert_eq!(listed(&mut h)[0].1, ModState::Disabled);

    let notice = |name: &str, level: &str| Notice {
        mod_name: name.into(),
        title: "빌드 끝".into(),
        body: "본문".into(),
        level: level.into(),
        task: None,
        terminal: None,
    };
    for n in [
        notice("demo", "success"),
        notice("demo", "warning"),
        notice("demo", "error"),
        notice("demo", "info"),
        notice("ghost", "loud"),
        notice("sleepy", "info"),
    ] {
        let _ = h
            .backend
            .events
            .unbounded_send(SourceEvent::ModNotice(Box::new(n)));
    }
    h.settle();
    assert_eq!(
        toasts(&mut h),
        vec![
            toast(ToastKind::Success, "빌드 끝 · 데모", "본문"),
            toast(ToastKind::Warning, "빌드 끝 · 데모", "본문"),
            toast(ToastKind::Error, "빌드 끝 · 데모", "본문"),
            toast(ToastKind::Info, "빌드 끝 · 데모", "본문"),
            toast(ToastKind::Info, "빌드 끝 · ghost", "본문"),
            toast(ToastKind::Info, "빌드 끝 · sleepy", "본문"),
        ],
        "title · the mod's title (else its name); unknown levels are info"
    );
    assert!(mod_calls(&h, from).is_empty(), "events send nothing");
    let os = h.cx.update(|_, cx| cx.global::<ModsUi>().os.is_some());
    assert!(!os, "OS 알림 off: no Windows toast");
}

#[test]
fn the_extensions_page_opens_once_and_takes_focus() {
    let script = Rc::new(RefCell::new(Script::default()));
    let mut h = harness("mods-page", &script);
    let weak = h.ws.downgrade();
    assert!(!h.cx.update(|_, cx| page_open(cx)));
    // Exercise every card branch while the page renders: a log open, a
    // stopped and an invalid mod.
    h.cx.update(|_, cx| toggle_log(&weak, "demo".into(), cx));
    h.settle();
    let from = h.backend.calls.len();

    let ws = h.ws.clone();
    let open = |h: &mut Harness| {
        h.cx.update(|window, cx| ws.update(cx, |w, cx| open_page(w, window, cx)));
        h.settle();
    };
    open(&mut h);
    assert!(h.cx.update(|_, cx| page_open(cx)));
    assert_eq!(
        mod_calls(&h, from),
        vec!["mod.list {}"],
        "opening the page re-reads the list"
    );
    let panel = |h: &mut Harness| -> Entity<ExtensionsPanel> {
        h.cx.update(|_, cx| {
            cx.global::<ModsUi>()
                .panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .expect("page open")
        })
    };
    let first = panel(&mut h);
    let focused =
        h.cx.update(|window, cx| first.read(cx).focus.is_focused(window));
    assert!(focused, "the page takes focus");
    let center = |h: &mut Harness| {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| {
            let d = ws.read(cx).dock.read(cx).dump(cx);
            serde_json::to_string(&d.center).unwrap_or_default()
        })
    };
    assert_eq!(center(&mut h).matches("PodExtensions").count(), 1);

    // Focus elsewhere, then open again: the same page is selected and focused.
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    let from = h.backend.calls.len();
    open(&mut h);
    let again = panel(&mut h);
    assert_eq!(again.entity_id(), first.entity_id(), "no second page");
    assert_eq!(center(&mut h).matches("PodExtensions").count(), 1);
    let focused =
        h.cx.update(|window, cx| again.read(cx).focus.is_focused(window));
    assert!(focused, "re-opening focuses the page");
    assert!(
        mod_calls(&h, from).is_empty(),
        "re-opening does not re-read"
    );
    assert!(toasts(&mut h).is_empty());
}

// ───────────────── plans (model) ─────────────────

fn listed_mods() -> ModListResult {
    serde_json::from_value(mod_list()).expect("mod list")
}

fn listed_mod(name: &str) -> ModInfo {
    listed_mods()
        .list
        .mods
        .into_iter()
        .find(|m| m.name == name)
        .expect("listed")
}

#[test]
fn palette_ids_split_at_the_first_colon() {
    use model::PaletteId;
    assert_eq!(
        model::palette_id("mod:demo:hello"),
        Some(PaletteId::Command {
            name: "demo",
            command: "hello"
        })
    );
    assert_eq!(
        model::palette_id("mod:demo:a:b"),
        Some(PaletteId::Command {
            name: "demo",
            command: "a:b"
        })
    );
    assert_eq!(
        model::palette_id("layout:demo:review-focus"),
        Some(PaletteId::Layout {
            name: "demo",
            layout: "review-focus"
        })
    );
    assert_eq!(model::palette_id("mod:demo"), Some(PaletteId::Incomplete));
    assert_eq!(model::palette_id("layout:"), Some(PaletteId::Incomplete));
    assert_eq!(model::palette_id("orch:board"), None);
    assert_eq!(model::palette_id("demo:hello"), None);

    let list = listed_mods();
    assert_eq!(
        model::find_preset(Some(&list), "demo", "solo").map(|p| p.title),
        Some("하나".to_string())
    );
    assert_eq!(model::find_preset(Some(&list), "demo", "gone"), None);
    assert_eq!(model::find_preset(Some(&list), "ghost", "solo"), None);
    assert_eq!(model::find_preset(None, "demo", "solo"), None);
}

#[test]
fn state_and_error_lines_say_what_went_wrong_in_korean() {
    let demo = listed_mod("demo");
    let crashed = listed_mod("crashed");
    let broken = listed_mod("broken");
    assert_eq!(model::state_label(&demo), "실행 중");
    assert_eq!(model::state_label(&listed_mod("sleepy")), "꺼짐");
    assert_eq!(model::state_label(&crashed), "정지됨: CPU 시간 한도 초과");
    assert_eq!(model::state_label(&broken), "설정 오류");
    assert_eq!(model::display_name(&demo), "데모");
    assert_eq!(model::display_name(&broken), "broken", "no title: the name");

    assert_eq!(model::error_summary(&demo), None);
    assert_eq!(
        model::error_summary(&crashed).as_deref(),
        Some("CPU 시간 한도 초과: time limit"),
        "first line only"
    );
    assert_eq!(
        model::error_summary(&broken).as_deref(),
        Some("레이아웃 프리셋 오류: layout layouts/bad.json: a split needs 2-6 children")
    );
    let mut manifest = broken.clone();
    manifest.error = Some("missing field `version`".into());
    assert_eq!(
        model::error_summary(&manifest).as_deref(),
        Some("mod.json 오류: missing field `version`")
    );
    let mut running = demo.clone();
    running.error = Some("warn\nmore".into());
    assert_eq!(model::error_summary(&running).as_deref(), Some("warn"));
}

#[test]
fn a_card_offers_only_requested_permissions_and_lists_what_the_mod_provides() {
    let demo = listed_mod("demo");
    let toggles = model::permission_toggles(&demo);
    assert_eq!(
        toggles
            .iter()
            .map(|t| (t.permission, t.granted, t.label, t.verdict, t.next))
            .collect::<Vec<_>>(),
        vec![
            (Permission::Notify, true, "알림 표시", "허용", "거부"),
            (Permission::Commands, false, "팔레트 명령", "거부", "허용"),
        ],
        "gate is not requested: no toggle"
    );
    assert_eq!(
        copy::permission_toggle(
            "데모",
            toggles[1].label,
            toggles[1].verdict,
            toggles[1].next
        ),
        "데모 권한 팔레트 명령: 거부. 누르면 허용"
    );
    assert!(model::permission_toggles(&listed_mod("broken")).is_empty());

    assert_eq!(
        model::facts_line(&demo).as_deref(),
        Some(
            "명령 2개 · 상태 감지: claude, codex · 레이아웃: 리뷰 집중, 4분할 에이전트, 하나 · 메모리 512 KiB"
        )
    );
    assert_eq!(
        model::facts_line(&listed_mod("crashed")).as_deref(),
        Some("명령 1개"),
        "memory only while running"
    );
    assert_eq!(model::facts_line(&listed_mod("broken")), None);
    assert_eq!(
        model::checks_line(&demo).as_deref(),
        Some("검사 명령: lint = cargo clippy")
    );
    assert_eq!(model::checks_line(&listed_mod("sleepy")), None);
}

#[test]
fn settings_summary_counts_installed_and_enabled_mods() {
    assert_eq!(
        model::installed_summary(None),
        "",
        "nothing before mod.list"
    );
    assert_eq!(
        model::installed_summary(Some(&listed_mods())),
        "4개 설치 · 3개 켜짐"
    );
}

#[test]
fn notices_route_by_level_and_name_the_listed_mod() {
    assert_eq!(model::toast_kind("success"), ToastKind::Success);
    assert_eq!(model::toast_kind("warning"), ToastKind::Warning);
    assert_eq!(model::toast_kind("error"), ToastKind::Error);
    assert_eq!(model::toast_kind("info"), ToastKind::Info);
    assert_eq!(model::toast_kind("loud"), ToastKind::Info);
    let list = listed_mods();
    assert_eq!(model::notice_author(Some(&list), "demo"), "데모");
    assert_eq!(model::notice_author(Some(&list), "sleepy"), "sleepy");
    assert_eq!(model::notice_author(Some(&list), "ghost"), "ghost");
    assert_eq!(model::notice_author(None, "demo"), "demo");
}
