//! The block terminal layout in the real workspace on GPUI's headless platform
//! (native QA, docs/10 §12): keyboard only, the scripted podd double
//! ([`TestBackend`]), output delivered as the attach stream would deliver it.
//! Run with `--nocapture` for the `[native-qa]` step log.
use core::prelude::v1::test;
use gpui_kit::*;
use podd::protocol::{GateInfo, GateState};
use serde_json::json;

use super::blocks::Feed;
use super::blocks_model::{Approval, BlockKey, decision};
use crate::attach::Chunk;
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceEvent, TestBackend};

const A: &str = "\x1b]133;A\x07";
const B: &str = "\x1b]133;B\x07";
const C: &str = "\x1b]133;C\x07";

fn set_layout(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let s = crate::theme::Settings {
            layout: id.into(),
            ..cx.global::<crate::theme::Settings>().clone()
        };
        s.apply_with(false, cx);
        ws.update(cx, |_, cx| cx.notify());
    });
    h.settle();
    h.settle();
}

fn feed(h: &mut Harness) -> Entity<Feed> {
    h.cx.update(|_, cx| super::blocks::feed(cx).expect("the feed was shown"))
}

fn info(id: &str) -> Box<podd::protocol::TerminalInfo> {
    Box::new(serde_json::from_value(json!({"id": id, "argv": ["powershell.exe"], "cwd": "C:/fixtures/sample", "state": "running", "cols": 100, "rows": 30, "created_ms": 0})).expect("terminal info"))
}

/// Output for `id`, as its attach stream would deliver it (first a snapshot).
fn output(h: &mut Harness, id: &str, text: &str) {
    let f = feed(h);
    let (id, text) = (id.to_string(), text.to_string());
    h.cx.update(|_, cx| {
        f.update(cx, |f, cx| {
            if !f.attached().contains(&id) {
                return;
            }
            f.ingest(
                &id,
                Chunk::Reset {
                    snapshot: Vec::new(),
                    info: info(&id),
                },
                cx,
            );
            f.ingest(&id, Chunk::Output(text.into_bytes()), cx);
        })
    });
    h.settle();
}

fn more(h: &mut Harness, id: &str, text: &str) {
    let f = feed(h);
    let (id, text) = (id.to_string(), text.to_string());
    h.cx.update(|_, cx| {
        f.update(cx, |f, cx| {
            f.ingest(&id, Chunk::Output(text.into_bytes()), cx)
        })
    });
    h.settle();
}

fn block_focused(h: &mut Harness, terminal: &str, seq: u64) -> bool {
    let f = feed(h);
    let key = BlockKey {
        terminal: terminal.into(),
        seq,
    };
    let handle =
        h.cx.update(|_, cx| f.update(cx, |f, cx| f.block_focus(&key, cx)));
    h.is_focused(&handle)
}

fn input_focused(h: &mut Harness) -> bool {
    let f = feed(h);
    let handle =
        h.cx.update(|_, cx| f.read(cx).input.read(cx).focus_handle(cx));
    h.is_focused(&handle)
}

fn panel_ids(h: &mut Harness) -> Vec<EntityId> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .iter()
            .map(|p| p.entity_id())
            .collect()
    })
}

fn dock_state(h: &mut Harness) -> String {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        serde_json::to_string(&ws.read(cx).dock.read(cx).dump(cx)).expect("dock state")
    })
}

fn pending_gate(id: &str, terminal: &str, options: &[&str]) -> GateInfo {
    GateInfo {
        id: id.into(),
        title: "명령 실행".into(),
        message: "cargo clippy 실행 승인이 필요합니다.".into(),
        task: None,
        group: None,
        terminal: Some(terminal.into()),
        state: GateState::Pending,
        created_ms: 1,
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: options.iter().map(|s| s.to_string()).collect(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}

/// `gate.resolve` answers approved/rejected.
fn answers(b: &mut TestBackend) {
    b.rpc = Some(Box::new(|method, params| match method {
        "gate.resolve" => {
            let mut g = pending_gate(params["id"].as_str().unwrap_or_default(), "b1", &[]);
            g.state = if params["decision"] == "approve" {
                GateState::Approved
            } else {
                GateState::Rejected
            };
            g.choice = params["choice"].as_str().map(str::to_owned);
            Some(Ok(serde_json::to_value(g).unwrap()))
        }
        _ => None,
    }));
}

#[test]
fn switching_layouts_keeps_terminals_attached_and_the_dock_as_it_was() {
    let mut h = Harness::new("blocks-switch", seed_two_tasks);
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let (panels, dock) = (panel_ids(&mut h), dock_state(&mut h));

    set_layout(&mut h, "block-terminal");
    assert!(h.bounds("blocks-layout").is_some(), "the layout renders");
    let f = feed(&mut h);
    // The focused terminal's task is shown and that terminal is the input target.
    assert_eq!(
        h.cx.update(|_, cx| (f.read(cx).task.clone(), f.read(cx).target.clone())),
        (Some("sample/로그인-수정".into()), Some("w1".into()))
    );
    assert!(
        input_focused(&mut h),
        "focus moves to the input that types into the user's terminal"
    );
    assert_eq!(
        h.cx.update(|_, cx| f.read(cx).attached()),
        ["w1"],
        "only the task's terminals stream"
    );
    assert_eq!(panel_ids(&mut h), panels, "no new panel");

    // A command with marks, then a full-screen program: one live block.
    output(
        &mut h,
        "w1",
        &format!(
            "{A}PS C:\\src> {B}git status --short\r\n{C} M src/로그인.rs\r\n\x1b]133;D;0\x07{A}PS C:\\src> {B}vim 메모.md\r\n{C}\x1b[?1049h\x1b[H\x1b[2J편집 중인 화면"
        ),
    );
    assert!(h.bounds("blocks-block-w1-0").is_some(), "the command block");
    let tui = h.cx.update(|_, cx| {
        f.read(cx)
            .feed(cx)
            .into_iter()
            .find(|b| b.block.kind == super::blocks_model::Kind::Tui)
            .expect("tui block")
    });
    assert!(
        tui.block.open && tui.block.lines == ["편집 중인 화면"],
        "{tui:?}"
    );
    // Esc: from the input to the newest block; Tab: its 전체 터미널로 보기; Enter: the raw terminal.
    h.keys("escape");
    assert!(
        block_focused(&mut h, "w1", tui.key.seq),
        "Esc goes to the newest block"
    );
    h.keys("tab");
    h.keys("enter");
    assert!(
        h.bounds("blocks-raw-view").is_some(),
        "the raw terminal is shown"
    );
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "the full-screen program keeps the real terminal"
    );
    assert_eq!(
        panel_ids(&mut h),
        panels,
        "the workbench's own panel, no new attach"
    );
    assert!(h.cx.update(|_, cx| f.read(cx).own_panel().is_none()));
    // F6 out of the terminal (Tab belongs to the program), Tab to the raw toggle, Enter: blocks again.
    h.keys("f6");
    h.keys("tab tab tab");
    h.keys("enter");
    assert!(
        h.bounds("blocks-raw-view").is_none() && h.bounds("blocks-feed").is_some(),
        "back to the blocks"
    );

    // Back to the workbench: dock and panels as they were, the feed's streams gone.
    set_layout(&mut h, "workbench");
    assert!(h.bounds("blocks-layout").is_none());
    assert_eq!(dock_state(&mut h), dock, "dock state restored");
    assert_eq!(panel_ids(&mut h), panels);
    assert!(
        h.cx.update(|_, cx| f.read(cx).attached().is_empty()),
        "the feed detached"
    );
    // And again: the feed remembers its task.
    set_layout(&mut h, "block-terminal");
    assert_eq!(
        h.cx.update(|_, cx| f.read(cx).task.clone()).as_deref(),
        Some("sample/로그인-수정")
    );
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), ["w1"]);
}

fn seed_review(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    b.tasks.push(TestBackend::task(
        "sample/로그인-수정",
        "로그인-수정",
        "sample",
        &["w1"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/review-bot",
        "review-bot",
        "sample",
        &["b1", "b2"],
    ));
    b.terminals.push(TestBackend::terminal(
        "w1",
        "로그인 작업",
        Some("sample/로그인-수정"),
        "working",
    ));
    let mut b1 = TestBackend::terminal("b1", "리뷰 봇", Some("sample/review-bot"), "blocked");
    b1.program = "codex".into();
    b.terminals.push(b1);
    b.terminals.push(TestBackend::terminal(
        "b2",
        "빌드",
        Some("sample/review-bot"),
        "working",
    ));
    answers(b);
}

#[test]
fn keyboard_approval_in_the_block_and_input_to_one_agent() {
    let mut h = Harness::new("blocks-actions", seed_review);
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(vec![pending_gate(
            "g1",
            "b1",
            &["이번만 허용", "항상 허용"],
        )]));
    h.settle();
    set_layout(&mut h, "block-terminal");
    let f = feed(&mut h);
    // F6: 블록 → 입력 → 작업; ↓ picks the next task, Enter goes to its blocks.
    h.keys("f6");
    assert!(input_focused(&mut h));
    h.keys("f6");
    h.keys("down");
    assert_eq!(
        h.cx.update(|_, cx| f.read(cx).task.clone()).as_deref(),
        Some("sample/review-bot")
    );
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), ["b1", "b2"]);
    output(&mut h, "b1", "Allow `cargo clippy`? 승인 기다리는 중\r\n");
    output(
        &mut h,
        "b2",
        &format!("{A}PS> {B}cargo build\r\n{C}   Compiling pod-ui\r\n"),
    );
    h.keys("enter");
    let feed_now = h.cx.update(|_, cx| f.read(cx).feed(cx));
    let gate_block = feed_now
        .iter()
        .find(|b| b.gate.is_some())
        .expect("the gate sits on b1's newest block")
        .key
        .clone();
    assert_eq!(gate_block.terminal, "b1");
    // ↑/↓ reach the approval block; Tab → 복사, Tab → 이번만 허용, Enter.
    let last = feed_now.last().unwrap().key.clone();
    assert!(
        block_focused(&mut h, &last.terminal, last.seq),
        "Enter in the task list: the newest block"
    );
    while !block_focused(&mut h, &gate_block.terminal, gate_block.seq) {
        h.keys("up");
    }
    assert!(
        h.cx.update(|_, cx| f.read(cx).is_expanded(&gate_block, cx)),
        "a waiting block is open"
    );
    h.keys("tab tab");
    h.keys("enter");
    assert!(
        h.backend.calls.iter().any(|c| c.starts_with("gate.resolve")
            && c.contains("approve")
            && c.contains("이번만 허용")),
        "{:?}",
        h.backend.calls
    );
    let ws = h.ws.clone();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).orch.pending().is_empty()),
        "the workspace's gate state updated"
    );
    assert!(
        block_focused(&mut h, &gate_block.terminal, gate_block.seq),
        "focus stays on the block after its buttons left"
    );

    // A second request: N on the block rejects it (no choice), A only when offered.
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gate(Box::new(pending_gate("g2", "b1", &[]))));
    h.settle();
    assert!(decision(&pending_gate("g2", "b1", &[]), Approval::Always).is_none());
    h.keys("a");
    assert!(
        !h.backend.calls.iter().any(|c| c.contains("g2")),
        "항상 허용 is not offered: nothing sent"
    );
    h.keys("n");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("gate.resolve") && c.contains("g2") && c.contains("reject")),
        "{:?}",
        h.backend.calls
    );

    // The input: F6 to it, Tab changes the target (b1 → b2), Enter sends to that agent only.
    h.keys("f6");
    assert!(input_focused(&mut h));
    assert_eq!(
        h.cx.update(|_, cx| f.read(cx).target.clone()).as_deref(),
        Some("b1")
    );
    h.keys("tab");
    assert_eq!(
        h.cx.update(|_, cx| f.read(cx).target.clone()).as_deref(),
        Some("b2"),
        "Tab changes the target"
    );
    assert!(input_focused(&mut h), "Tab stays in the input");
    h.type_text("ls");
    h.keys("enter");
    assert_eq!(h.backend.typed_text("b2"), "ls\r");
    assert_eq!(h.backend.typed_text("b1"), "");
    // Copy from the keyboard.
    h.keys("escape");
    h.keys("c");
    let clip =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert!(
        clip.as_deref()
            .is_some_and(|c| c.contains("cargo build") && c.contains("Compiling pod-ui")),
        "{clip:?}"
    );
    // Status transitions of an agent without marks cut a new step block.
    let before = h.cx.update(|_, cx| {
        f.read(cx)
            .feed(cx)
            .iter()
            .filter(|b| b.key.terminal == "b1")
            .count()
    });
    h.backend.set_status("b1", "working");
    h.settle();
    more(&mut h, "b1", "• 수정 crates/pod-ui/src/terminal.rs\r\n");
    let after: Vec<_> = h.cx.update(|_, cx| {
        f.read(cx)
            .feed(cx)
            .into_iter()
            .filter(|b| b.key.terminal == "b1")
            .collect()
    });
    assert_eq!(after.len(), before + 1, "a new step");
    assert_eq!(
        (
            after.last().unwrap().block.title.as_str(),
            after.last().unwrap().block.lines.clone()
        ),
        (
            "작업 중",
            vec!["• 수정 crates/pod-ui/src/terminal.rs".to_string()]
        )
    );
}

#[test]
fn korean_block_text_fits_at_100_to_175_percent_and_large_fonts() {
    for (scale, font) in [
        (1.0, 13.),
        (1.25, 13.),
        (1.5, 13.),
        (1.75, 13.),
        (1.75, 20.),
    ] {
        let mut h = Harness::sized(
            "blocks-dpi",
            scale,
            size(px(1440. / scale), px(960. / scale)),
            seed_two_tasks,
        );
        h.cx.update(|_, cx| {
            crate::theme::Settings {
                font_size: font,
                ..cx.global::<crate::theme::Settings>().clone()
            }
            .apply_with(false, cx)
        });
        set_layout(&mut h, "block-terminal");
        output(
            &mut h,
            "w1",
            &format!(
                "{A}PS> {B}cargo test -p pod-ui 넓은_글자\r\n{C}테스트 결과: 통과 1개, 실패 0개\r\n\x1b]133;D;0\x07{A}PS> {B}"
            ),
        );
        let view = h.viewport();
        let block = h.bounds("blocks-block-w1-0").expect("block painted");
        let status = h.bounds("blocks-status-w1-0").expect("status painted");
        assert!(
            status.top() >= block.top()
                && status.bottom() <= block.bottom()
                && status.right() <= block.right(),
            "{scale} {font}: status inside the block {status:?} {block:?}"
        );
        assert!(
            block.right() <= view.width,
            "{scale}: block inside the window"
        );
        let input = h.bounds("blocks-input").expect("input");
        assert!(
            input.bottom() <= view.height + px(1.) && input.right() <= view.width,
            "{scale} {font}: input inside the window"
        );
        let task = h
            .bounds("blocks-task-sample/로그인-수정")
            .expect("task row");
        assert!(
            task.bottom() > task.top() + px(font * 1.2),
            "{scale} {font}: task row tall enough for the text"
        );
    }
}

#[test]
fn preset_11_maps_to_the_block_terminal() {
    let p = pod_ui::themes::PRESETS
        .into_iter()
        .find(|p| p.number == "11")
        .expect("preset 11");
    assert_eq!(p.layout, "block-terminal");
    let r = crate::layout_mode::LayoutRegistry::new();
    assert!(
        r.available(crate::layout_mode::LayoutMode::BlockTerminal),
        "settings show preset 11 as ready, not 준비 중"
    );
    assert_eq!(
        r.resolve(p.layout).mode(),
        crate::layout_mode::LayoutMode::BlockTerminal
    );
}

/// A-P13 (reads through the layout frame): frames, a status event and a
/// layout round trip neither attach a terminal twice nor send a gate decision
/// or the input twice.
#[test]
fn frames_and_round_trips_neither_reattach_nor_resend() {
    let mut h = Harness::new("blocks-once", seed_review);
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(vec![pending_gate(
            "g1",
            "b1",
            &["이번만 허용", "항상 허용"],
        )]));
    h.settle();
    set_layout(&mut h, "block-terminal");
    let f = feed(&mut h);
    // F6: 블록 → 입력 → 작업; ↓ picks review-bot.
    h.keys("f6");
    h.keys("f6");
    h.keys("down");
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), ["b1", "b2"]);
    output(
        &mut h,
        "b2",
        &format!("{A}PS> {B}cargo build\r\n{C}   Compiling pod-ui\r\n"),
    );
    let build = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            f.read(cx)
                .feed(cx)
                .into_iter()
                .find(|b| b.key.terminal == "b2" && b.block.title == "cargo build")
                .map(|b| (b.key.seq, b.block.lines))
        })
    };
    let before = build(&mut h).expect("the command block");
    assert_eq!(before.1, ["   Compiling pod-ui"]);

    // Frames and a status event: the same streams (a new attach starts empty).
    let ws = h.ws.clone();
    let frames = |h: &mut Harness| {
        for _ in 0..4 {
            h.cx.update(|_, cx| ws.update(cx, |_, cx| cx.notify()));
            h.settle();
        }
    };
    frames(&mut h);
    h.backend.set_status("b2", "blocked");
    h.settle();
    frames(&mut h);
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), ["b1", "b2"]);
    assert_eq!(
        build(&mut h),
        Some(before.clone()),
        "the stream kept its blocks"
    );

    // Y on the approval: one gate.resolve, however many frames follow.
    let gate_block = h.cx.update(|_, cx| {
        f.read(cx)
            .feed(cx)
            .into_iter()
            .find(|b| b.gate.is_some())
            .expect("the gate sits on b1")
            .key
    });
    h.cx.update(|window, cx| f.update(cx, |f, cx| f.select(&gate_block, window, cx)));
    h.settle();
    h.keys("y");
    frames(&mut h);
    let resolves = |h: &Harness| {
        h.backend
            .calls
            .iter()
            .filter(|c| c.starts_with("gate.resolve"))
            .count()
    };
    assert_eq!(resolves(&h), 1, "{:?}", h.backend.calls);

    // Enter in the input: the text goes once to the target.
    h.keys("f6");
    assert!(input_focused(&mut h));
    let target =
        h.cx.update(|_, cx| f.read(cx).target.clone())
            .expect("a running terminal");
    h.type_text("ls");
    h.keys("enter");
    frames(&mut h);
    assert_eq!(h.backend.typed_text(&target), "ls\r");

    // Away and back: one attach per terminal again, nothing sent again.
    set_layout(&mut h, "workbench");
    assert!(h.cx.update(|_, cx| f.read(cx).attached().is_empty()));
    set_layout(&mut h, "block-terminal");
    frames(&mut h);
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), ["b1", "b2"]);
    assert_eq!(resolves(&h), 1);
    assert_eq!(h.backend.typed_text(&target), "ls\r");
}

/// A second window with its own workspace on its own scripted backend
/// (seeded by `seed`), drawn until its data arrived (A-T2d slot tests).
pub(super) struct SecondWindow {
    pub(super) handle: AnyWindowHandle,
    pub(super) key: crate::layouts::kit::WindowKey,
    pub(super) workspace: WeakEntity<crate::workspace::Workspace>,
}

pub(super) fn second_window(h: &mut Harness, seed: impl FnOnce(&mut TestBackend)) -> SecondWindow {
    let (source, events, mut backend) = crate::source::PoddSource::scripted();
    seed(&mut backend);
    backend.connect();
    let (handle, workspace) = h.cx.update(|_, cx| {
        let slot = std::cell::RefCell::new(None);
        let handle = cx
            .open_window(WindowOptions::default(), |window, cx| {
                let ws = cx.new(|cx| {
                    crate::workspace::Workspace::new(
                        source,
                        events,
                        crate::workspace::StartOptions::default(),
                        window,
                        cx,
                    )
                });
                *slot.borrow_mut() = Some(ws.downgrade());
                cx.new(|cx| gpui_kit::base::Root::new(ws, window, cx))
            })
            .expect("second window");
        (
            AnyWindowHandle::from(handle),
            slot.into_inner().expect("second workspace"),
        )
    });
    for _ in 0..4 {
        h.cx.run_until_parked();
        backend.serve();
        h.cx.run_until_parked();
        h.cx.update(|_, cx| {
            let _ = cx.update_window(handle, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        });
    }
    h.cx.run_until_parked();
    SecondWindow {
        handle,
        key: crate::layouts::kit::WindowKey(workspace.entity_id()),
        workspace,
    }
}

impl SecondWindow {
    /// Closes the window; its workspace (the host) is released with it.
    pub(super) fn close(self, h: &mut Harness) {
        h.cx.update(|_, cx| {
            let _ = cx.update_window(self.handle, |_, window, _| window.remove_window());
        });
        h.settle();
        assert!(
            self.workspace.upgrade().is_none(),
            "the second workspace is gone"
        );
    }
}

/// A-T2d: the layout draws through its kit renderer (no Workspace adapter)
/// and the window's state slot keeps the feed: the same feed, attached to
/// the same terminals, across a layout round trip; one feed per window,
/// dropped (attach streams and all) with its window.
#[test]
fn the_kit_renderer_keeps_one_feed_per_window_in_its_state_slot() {
    use crate::layout_mode::{LayoutMode, LayoutRegistry};
    use crate::layouts::kit::{self, WindowKey};
    assert_eq!(
        LayoutRegistry::new()
            .resolve(LayoutMode::BlockTerminal.id())
            .mode(),
        LayoutMode::BlockTerminal,
        "the block terminal is registered (every module has a kit renderer)"
    );
    let mut h = Harness::new("blocks-slot", seed_review);
    let key = WindowKey(h.ws.entity_id());
    let in_slot = |h: &mut Harness, key| h.cx.update(|_, cx| kit::state::<Feed>(key, cx));
    assert!(in_slot(&mut h, key).is_none(), "no feed before the layout");
    set_layout(&mut h, "block-terminal");
    assert!(h.bounds("blocks-layout").is_some(), "the renderer drew it");
    let f = feed(&mut h);
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(f.entity_id()),
        "the feed sits in this window's slot"
    );
    let attached = h.cx.update(|_, cx| f.read(cx).attached());
    assert_eq!(
        attached,
        ["w1"],
        "the window's first task, read through the host"
    );

    set_layout(&mut h, "workbench");
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(f.entity_id()),
        "the slot keeps the feed while another layout shows"
    );
    assert!(h.cx.update(|_, cx| f.read(cx).attached().is_empty()));
    set_layout(&mut h, "block-terminal");
    assert_eq!(
        feed(&mut h).entity_id(),
        f.entity_id(),
        "and shows it again"
    );
    assert_eq!(h.cx.update(|_, cx| f.read(cx).attached()), attached);

    // A second window: its own feed over its own workspace, gone with it.
    let second = second_window(&mut h, seed_review);
    let other = in_slot(&mut h, second.key).expect("the second window's feed");
    assert_ne!(other.entity_id(), f.entity_id(), "one feed per window");
    assert_eq!(h.cx.update(|_, cx| other.read(cx).attached()), ["w1"]);
    assert_eq!(h.cx.update(|_, cx| kit::states::<Feed>(cx).len()), 2);
    let weak = other.downgrade();
    drop(other);
    let other = weak;
    let second_key = second.key;
    second.close(&mut h);
    assert!(in_slot(&mut h, second_key).is_none(), "its slot is dropped");
    assert!(other.upgrade().is_none(), "and its feed released");
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(f.entity_id()),
        "the first window keeps its feed"
    );
}
