//! The kanban board in the real workspace on GPUI's headless platform
//! (native QA, docs/10 §12): keyboard only, the scripted podd double
//! ([`TestBackend`]), no mouse events. Run with `--nocapture` for the
//! `[native-qa]` step log.
use core::prelude::v1::test;
use gpui_kit::*;
use podd::protocol::{GateInfo, GateState};
use serde_json::json;

use super::kanban::{Board, Column};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceEvent, TestBackend};

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

fn board(h: &mut Harness) -> Entity<Board> {
    h.cx.update(|_, cx| super::kanban::board(cx).expect("the board was shown"))
}

fn card_focused(h: &mut Harness, id: &str) -> bool {
    let b = board(h);
    let f =
        h.cx.update(|_, cx| b.update(cx, |b, cx| b.card_focus(id, cx)));
    h.is_focused(&f)
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

fn pending_gate(id: &str, terminal: &str) -> GateInfo {
    GateInfo {
        id: id.into(),
        title: "배포 승인".into(),
        message: "main에 병합할까요?".into(),
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
        options: Vec::new(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}

/// Extra answers: `gate.resolve` (approved/rejected) and `terminal.read` (one Korean line).
fn answers(b: &mut TestBackend) {
    b.rpc = Some(Box::new(|method, params| match method {
        "gate.resolve" => {
            let mut g = pending_gate(params["id"].as_str().unwrap_or_default(), "b1");
            g.state = if params["decision"] == "approve" {
                GateState::Approved
            } else {
                GateState::Rejected
            };
            Some(Ok(serde_json::to_value(g).unwrap()))
        }
        "terminal.read" => Some(Ok(
            json!({"id": params["id"], "mode": "screen", "lines": ["", "셀 경계 테스트 추가 중"], "cursor": [0, 0], "state": "running"}),
        )),
        _ => None,
    }));
}

#[test]
fn switching_layouts_keeps_terminals_attached_and_the_dock_as_it_was() {
    let mut h = Harness::new("kanban-switch", seed_two_tasks);
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let (panels, dock) = (panel_ids(&mut h), dock_state(&mut h));

    set_layout(&mut h, "kanban");
    assert!(h.bounds("kanban-board").is_some(), "the board renders");
    assert!(h.bounds("kanban-card-w1").is_some() && h.bounds("kanban-card-b1").is_some());
    // The focused terminal moved into the drawer as it was: same panel, same attach, still focused.
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "focus stays in the user's terminal"
    );
    assert_eq!(panel_ids(&mut h), panels, "no new panel or attach");
    let b = board(&mut h);
    assert!(h.cx.update(|_, cx| b.read(cx).own_panel().is_none()));
    assert_eq!(
        h.cx.update(|_, cx| b.read(cx).model.opened.clone())
            .as_deref(),
        Some("w1")
    );

    // F6 to the board, → to the blocked card, Enter opens it in the drawer.
    h.keys("f6");
    assert!(card_focused(&mut h, "w1"), "F6 moves to the selected card");
    h.keys("right");
    assert!(card_focused(&mut h, "b1"));
    h.keys("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "Enter: the card's terminal in the drawer, focused"
    );
    assert!(
        h.cx.update(|_, cx| b.read(cx).own_panel().is_some()),
        "a drawer-owned panel for a terminal without one"
    );
    assert_eq!(panel_ids(&mut h).len(), panels.len() + 1);
    h.keys("f6");
    assert!(card_focused(&mut h, "b1"), "F6 back to the board");

    // Back to the workbench: the dock is exactly as it was, the drawer's own panel is gone.
    set_layout(&mut h, "workbench");
    assert!(h.bounds("kanban-board").is_none());
    assert_eq!(dock_state(&mut h), dock, "dock state restored");
    assert_eq!(
        panel_ids(&mut h),
        panels,
        "the workbench's panels kept, the drawer's dropped"
    );
    assert!(h.cx.update(|_, cx| b.read(cx).own_panel().is_none()));

    // And again: the board remembers its selection.
    set_layout(&mut h, "kanban");
    assert_eq!(
        h.cx.update(|_, cx| b.read(cx).model.selected.clone())
            .as_deref(),
        Some("b1")
    );
}

#[test]
fn keyboard_approve_through_the_gate_path_and_rerun_a_failed_worker() {
    let mut h = Harness::new("kanban-actions", |b| {
        seed_two_tasks(b);
        let mut x = TestBackend::terminal("x1", "설치 서명", Some("sample/review-bot"), "working");
        x.live = crate::vm::Liveness::Exited {
            code: Some(1),
            killed: false,
        };
        b.terminals.push(x);
        answers(b);
    });
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(vec![pending_gate("g1", "b1")]));
    h.settle();
    set_layout(&mut h, "kanban");
    let b = board(&mut h);
    // Entering with nothing focused: the first card takes focus.
    assert!(card_focused(&mut h, "w1"), "first card focused on entry");
    let cards = h.cx.update(|_, cx| b.read(cx).cards(cx));
    let col = |id: &str| {
        cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.column, c.failed))
    };
    assert_eq!(
        (col("w1"), col("b1"), col("x1")),
        (
            Some((Column::Working, false)),
            Some((Column::NeedsYou, false)),
            Some((Column::NeedsYou, true))
        )
    );
    assert_eq!(
        h.cx.update(|_, cx| b.read(cx).line("w1").map(str::to_owned))
            .as_deref(),
        Some("셀 경계 테스트 추가 중"),
        "last line read once, no polling"
    );

    // → b1, Tab to 승인, Enter.
    h.keys("right");
    assert!(card_focused(&mut h, "b1"));
    h.keys("tab");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("gate.resolve") && c.contains("approve") && c.contains("g1")),
        "{:?}",
        h.backend.calls
    );
    let ws = h.ws.clone();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).orch.pending().is_empty()),
        "the workspace's gate state updated"
    );

    // The 승인 button is gone with the gate: focus is back on the card, not lost.
    assert!(card_focused(&mut h, "b1"), "focus returns to the card");
    // ↓ to the failed worker, Tab 로그, Tab 다시 실행, Enter.
    h.keys("down");
    assert!(
        card_focused(&mut h, "x1"),
        "down from b1 reaches the failed card"
    );
    h.keys("tab");
    h.keys("tab");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("terminal.run")),
        "{:?}",
        h.backend.calls
    );
    let opened = h.cx.update(|_, cx| b.read(cx).model.opened.clone());
    assert!(
        opened.is_some_and(|o| o != "x1"),
        "the new run opens in the drawer"
    );
}

#[test]
fn korean_card_text_fits_at_100_to_175_percent_and_large_fonts() {
    for (scale, font) in [
        (1.0, 13.),
        (1.25, 13.),
        (1.5, 13.),
        (1.75, 13.),
        (1.75, 20.),
    ] {
        let mut h = Harness::sized(
            "kanban-dpi",
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
        set_layout(&mut h, "kanban");
        let view = h.viewport();
        for id in ["w1", "b1"] {
            let card = h
                .bounds(&format!("kanban-card-{id}"))
                .expect("card painted");
            let status = h
                .bounds(&format!("kanban-status-{id}"))
                .expect("status row painted");
            assert!(
                status.bottom() <= card.bottom() && status.top() >= card.top(),
                "{scale} {font}: status row inside the card {status:?} {card:?}"
            );
            assert!(
                card.right() <= view.width && card.bottom() <= view.height,
                "{scale}: card inside the window"
            );
        }
        let drawer = h.bounds("kanban-drawer").expect("drawer");
        assert!(
            drawer.bottom() <= view.height + px(1.),
            "{scale}: drawer inside the window"
        );
    }
}

#[test]
fn preset_12_maps_to_the_board() {
    let p = pod_ui::themes::PRESETS
        .into_iter()
        .find(|p| p.number == "12")
        .expect("preset 12");
    assert_eq!(p.layout, "kanban");
    let r = crate::layout_mode::LayoutRegistry::new();
    assert!(
        r.available(crate::layout_mode::LayoutMode::Kanban),
        "settings show preset 12 as ready, not 준비 중"
    );
    assert_eq!(
        r.resolve(p.layout).mode(),
        crate::layout_mode::LayoutMode::Kanban
    );
}

/// v17: a gate with options shows one approve button per option; the one
/// picked by keyboard sends its `choice` (podd refuses an approve without one).
#[test]
fn option_gates_approve_with_the_picked_choice() {
    let mut h = Harness::new("kanban-options", |b| {
        seed_two_tasks(b);
        answers(b);
    });
    let mut g = pending_gate("g2", "b1");
    g.options = vec!["스테이징 먼저".into(), "바로 운영".into()];
    let _ = h.backend.events.unbounded_send(SourceEvent::Gates(vec![g]));
    h.settle();
    set_layout(&mut h, "kanban");
    assert!(h.bounds("kanban-card-b1").is_some());
    h.keys("right");
    assert!(card_focused(&mut h, "b1"));
    // Tab: first option, Tab: second option, Enter.
    h.keys("tab");
    h.keys("tab");
    h.keys("enter");
    let call = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("gate.resolve"))
        .cloned()
        .expect("gate.resolve sent");
    assert!(
        call.contains("\"choice\":\"바로 운영\"") && call.contains("approve"),
        "{call}"
    );
}

/// A panel opened while the board shows (settings) switches the drawer to its
/// 패널 view; F6 from a card goes into the dock (through the host's focus slot).
#[test]
fn panels_opened_on_the_board_show_in_the_drawer_and_f6_reaches_the_dock() {
    let mut h = Harness::new("kanban-panels", seed_two_tasks);
    set_layout(&mut h, "kanban");
    let b = board(&mut h);
    assert!(card_focused(&mut h, "w1"), "first card focused on entry");
    assert!(h.cx.update(|_, cx| !b.read(cx).show_dock));

    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_settings(window, cx)));
    h.settle();
    assert!(
        h.cx.update(|_, cx| b.read(cx).drawer_open && b.read(cx).show_dock),
        "the drawer shows the panels"
    );
    assert!(h.bounds("kanban-drawer").is_some());

    // From a card, F6 moves into the dock the drawer shows.
    h.cx.update(|window, cx| b.update(cx, |b, cx| b.select("w1", window, cx)));
    h.settle();
    assert!(card_focused(&mut h, "w1"));
    h.keys("f6");
    let dock =
        h.cx.update(|_, cx| ws.read(cx).dock.read(cx).focus_handle(cx));
    assert!(
        h.cx.update(|window, cx| dock.contains_focused(window, cx)),
        "F6: the dock has focus"
    );
    assert!(!card_focused(&mut h, "w1"));
    assert!(
        h.cx.update(|_, cx| b.read(cx).show_dock),
        "the drawer keeps showing the panels"
    );
}

#[test]
fn switching_in_reads_each_card_once_and_the_board_is_the_windows_state() {
    // A-T2c: the board is made by the narrow renderer while the workspace
    // renders and kept in the window's kit state slot. Requests as before:
    // one `terminal.read` per card when the board first shows, none when
    // switching away and back (same board, lines kept).
    let mut h = Harness::new("kanban-state-slot", |b| {
        seed_two_tasks(b);
        // Last lines as in the UI fixture.
        b.rpc = Some(Box::new(super::kanban::fixture_answer));
    });
    let before = h.backend.calls.len();
    set_layout(&mut h, "kanban");
    let b = board(&mut h);
    let cards: Vec<String> =
        h.cx.update(|_, cx| b.read(cx).cards(cx).into_iter().map(|c| c.id).collect());
    assert_eq!(cards.len(), 2, "{cards:?}");
    let calls = h.backend.calls[before..].to_vec();
    assert_eq!(calls.len(), cards.len(), "{calls:?}");
    for (call, id) in calls.iter().zip(&cards) {
        assert!(
            call.starts_with("terminal.read") && call.contains(&format!("\"{id}\"")),
            "card order: {call} for {id}"
        );
    }
    let lines: Vec<Option<String>> = h.cx.update(|_, cx| {
        cards
            .iter()
            .map(|id| b.read(cx).line(id).map(str::to_owned))
            .collect()
    });
    assert!(lines.iter().all(Option::is_some), "{lines:?}");

    let seen = h.backend.calls.len();
    set_layout(&mut h, "workbench");
    set_layout(&mut h, "kanban");
    assert_eq!(board(&mut h).entity_id(), b.entity_id(), "the same board");
    assert_eq!(
        h.backend.calls.len(),
        seen,
        "{:?}",
        &h.backend.calls[seen..]
    );

    let ws = h.ws.entity_id();
    let slot = h.cx.update(|_, cx| {
        crate::layouts::kit::state::<Board>(crate::layouts::kit::WindowKey(ws), cx)
    });
    assert_eq!(
        slot.map(|s| s.entity_id()),
        Some(b.entity_id()),
        "the window's state slot holds the board"
    );
}

// ───────────────────────────── fx-layouts ─────────────────────────────

/// Opens palette entry `id` of the orchestration screens, as the palette does.
fn open_orch(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    let id = id.to_string();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| assert!(crate::orch_ui::run(w, &id, window, cx)))
    });
    h.settle();
}

/// The keyboard is in the workspace's dock (the drawer's 패널 view).
fn dock_focused(h: &mut Harness) -> bool {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.read(cx)
            .dock
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    })
}

/// F6 cycles board → the drawer's dock → board. The dock is drawn inside the
/// board's root, so F6 from the dock (or a panel in it) used to land on the
/// dock again.
#[test]
fn f6_from_the_drawers_dock_returns_to_the_board() {
    let mut h = Harness::new("kanban-dock-f6", seed_two_tasks);
    set_layout(&mut h, "kanban");
    let b = board(&mut h);
    assert!(card_focused(&mut h, "w1"), "first card focused on entry");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_settings(window, cx)));
    h.settle();
    assert!(h.cx.update(|_, cx| b.read(cx).show_dock));
    assert!(dock_focused(&mut h), "the settings page has the keyboard");
    h.keys("f6");
    assert!(card_focused(&mut h, "w1"), "F6 from a panel: the board");
    h.keys("f6");
    assert!(dock_focused(&mut h), "F6: the dock");
    h.keys("f6");
    assert!(card_focused(&mut h, "w1"), "F6 from the dock: the board");
    h.keys("f6");
    assert!(dock_focused(&mut h), "and round again");
    assert!(h.cx.update(|_, cx| b.read(cx).show_dock));
}

/// An orchestration page opened on the board shows in the drawer's 패널 view:
/// the first one, one next to another while the drawer shows a terminal, one
/// asked for again, and one while the drawer is closed.
#[test]
fn orch_pages_opened_on_the_board_show_in_the_drawer() {
    let mut h = Harness::new("kanban-orch", seed_two_tasks);
    set_layout(&mut h, "kanban");
    let b = board(&mut h);
    let shows = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            let b = b.read(cx);
            b.drawer_open && b.show_dock
        })
    };
    // The drawer's 터미널 tab (as its click does).
    let to_terminal = |h: &mut Harness| {
        h.cx.update(|window, cx| {
            b.update(cx, |b, cx| {
                b.show_dock = false;
                b.switch_region(window, cx);
            })
        });
        h.settle();
    };
    assert!(!shows(&mut h));
    open_orch(&mut h, "orch-board");
    assert!(shows(&mut h), "the board page shows in the drawer");
    assert!(dock_focused(&mut h));
    to_terminal(&mut h);
    assert!(!shows(&mut h));
    open_orch(&mut h, "orch-settings");
    assert!(shows(&mut h), "a page next to another shows too");
    to_terminal(&mut h);
    open_orch(&mut h, "orch-board");
    assert!(shows(&mut h), "a page asked for again shows");
    // The drawer's close button, then the inbox: the drawer opens on it.
    h.cx.update(|window, cx| {
        b.update(cx, |b, cx| {
            b.drawer_open = false;
            b.switch_region(window, cx);
        })
    });
    h.settle();
    assert!(h.cx.update(|_, cx| !b.read(cx).drawer_open));
    open_orch(&mut h, "orch-inbox");
    assert!(shows(&mut h), "the inbox opens the drawer on 패널");
    assert!(dock_focused(&mut h), "with the keyboard in it");
}
