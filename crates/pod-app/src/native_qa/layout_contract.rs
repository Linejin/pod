//! Card T2 step 3 (docs/20-refactor/app.md): the seven layout modes in one
//! real window, cycled twice, on the headless platform with the scripted
//! podd double. Each step records what the user would notice and what podd
//! would see: which body drew, the focused terminal and focus, the terminal
//! panels (each new one is an attach), the Review and editor panels, the dock
//! JSON, the requests sent, and the mode's state in the window's kit slot.
//! The lines are pinned (taken on the pre-T2z code), so a renderer move that
//! adds an attach, a request or a focus change fails here.
//!
//! Also: each mode binds its keys once, the unknown-layout fallback and a
//! re-registered module keep working, and a second window gets its own state
//! slots, all dropped with that window.
use core::prelude::v1::test;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use gpui_kit::*;

use super::{Harness, seed_two_tasks};
use crate::layout_mode::{LayoutMode, LayoutRegistry};
use crate::layouts::kit::{self, WindowKey};
use crate::source::TestBackend;

const LOGIN: &str = "sample/로그인-수정";

/// The element each mode's body marks (`qa_id`); the workbench marks none.
const MARKERS: [(LayoutMode, &str); 6] = [
    (LayoutMode::FocusRail, "rail"),
    (LayoutMode::Queue, "queue-list"),
    (LayoutMode::Kanban, "kanban-board"),
    (LayoutMode::BlockTerminal, "blocks-layout"),
    (LayoutMode::Review, "review-layout"),
    (LayoutMode::DenseDetails, "dense-layout"),
];

/// The cycle: every mode once, back on the workbench last.
const CYCLE: [LayoutMode; 7] = [
    LayoutMode::FocusRail,
    LayoutMode::Queue,
    LayoutMode::Kanban,
    LayoutMode::BlockTerminal,
    LayoutMode::Review,
    LayoutMode::DenseDetails,
    LayoutMode::Workbench,
];

/// What each step showed, taken on the code before T2z (aff314e).
const EXPECTED: &[&str] = &[
    "start workbench: drawn=[] terminal=Some(\"w1\") focus=\"terminal w1\" panels=1 made=1 reviews=[] editor=kept dock=as-left requests=[\"mod.list\", \"terminal.ack\"]",
    "1 focus-rail: drawn=[\"rail\"] terminal=Some(\"w1\") focus=\"terminal w1\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[\"terminal.ack\"]",
    "1 queue: drawn=[\"queue-list\"] terminal=Some(\"w1\") focus=\"terminal w1\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "1 kanban: drawn=[\"kanban-board\"] terminal=Some(\"w1\") focus=\"terminal w1\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[\"terminal.read\", \"terminal.read\"]",
    "1 block-terminal: drawn=[\"blocks-layout\"] terminal=None focus=\"workspace text input (value \\\"\\\") [Root > PodBlocks > PodBlocksInput > Input]\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "1 review: drawn=[\"review-layout\"] terminal=None focus=\"workspace control [Root > PodReviewLayout]\" panels=1 made=0 reviews=[\"sample/로그인-수정\"] editor=kept dock=changed requests=[\"task.pr.status\", \"task.diff\", \"review.comments.list\", \"review.deliveries\"]",
    "1 dense-details: drawn=[\"dense-layout\"] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[\"terminal.read\"]",
    "1 workbench: drawn=[] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
    "2 focus-rail: drawn=[\"rail\"] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
    "2 queue: drawn=[\"queue-list\"] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "2 kanban: drawn=[\"kanban-board\"] terminal=None focus=\"workspace control [Root > PodKanban]\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "2 block-terminal: drawn=[\"blocks-layout\"] terminal=None focus=\"workspace control [Root > PodBlocks]\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "2 review: drawn=[\"review-layout\"] terminal=None focus=\"workspace control [Root > PodReviewLayout]\" panels=1 made=0 reviews=[\"sample/로그인-수정\"] editor=kept dock=changed requests=[\"task.pr.status\", \"task.diff\", \"review.comments.list\", \"review.deliveries\"]",
    "2 dense-details: drawn=[\"dense-layout\"] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=changed requests=[]",
    "2 workbench: drawn=[] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
    "unknown id: drawn=[] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
    "kanban re-registered: drawn=[\"kanban-board\"] terminal=None focus=\"workspace control [Root > PodKanban]\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
    "workbench again: drawn=[] terminal=None focus=\"workspace root\" panels=1 made=0 reviews=[] editor=kept dock=as-left requests=[]",
];

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

/// The mode's state entity in the window's kit slot (none for the workbench).
fn state_of(mode: LayoutMode, key: WindowKey, cx: &App) -> Option<EntityId> {
    use crate::layouts::{blocks, dense, kanban, queue, rail, review};
    match mode {
        LayoutMode::Workbench => None,
        LayoutMode::FocusRail => kit::state::<rail::RailState>(key, cx).map(|e| e.entity_id()),
        LayoutMode::Queue => kit::state::<queue::QueueColumn>(key, cx).map(|e| e.entity_id()),
        LayoutMode::Kanban => kit::state::<kanban::Board>(key, cx).map(|e| e.entity_id()),
        LayoutMode::BlockTerminal => kit::state::<blocks::Feed>(key, cx).map(|e| e.entity_id()),
        LayoutMode::Review => kit::state::<review::ReviewView>(key, cx).map(|e| e.entity_id()),
        LayoutMode::DenseDetails => kit::state::<dense::DenseView>(key, cx).map(|e| e.entity_id()),
    }
}

/// Key bindings, without pod-ui's three modal keys: `pod_ui::apply_theme`
/// binds those again on every settings apply (each layout switch here), a
/// pod-ui leak outside this card.
fn keymap_len(h: &mut Harness) -> usize {
    h.cx.update(|_, cx| {
        cx.key_bindings()
            .borrow()
            .bindings()
            .filter(|b| !b.action().name().starts_with("pod_modal::"))
            .count()
    })
}

/// Reads one step: the line pinned in [`EXPECTED`].
struct Probe {
    /// Every terminal panel seen so far: a new one is a new attach.
    seen: BTreeSet<EntityId>,
    /// Requests already counted.
    calls: usize,
    /// The workbench dock as the user left it.
    dock: String,
    /// The workbench's editor.
    editor: EntityId,
}

impl Probe {
    fn dock(h: &mut Harness) -> String {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| {
            serde_json::to_string(&ws.read(cx).dock.read(cx).dump(cx)).expect("dock state")
        })
    }

    fn step(&mut self, h: &mut Harness, label: &str) -> String {
        let drawn: Vec<&str> = MARKERS
            .iter()
            .map(|(_, m)| *m)
            .filter(|m| h.bounds(m).is_some())
            .collect();
        let ws = h.ws.clone();
        let (panels, reviews, editors) = h.cx.update(|_, cx| {
            let w = ws.read(cx);
            (
                w.live_panels()
                    .iter()
                    .map(|p| p.entity_id())
                    .collect::<Vec<_>>(),
                w.live_reviews()
                    .iter()
                    .map(|r| r.read(cx).task_id.clone())
                    .collect::<Vec<_>>(),
                w.live_editors()
                    .iter()
                    .map(|e| e.entity_id())
                    .collect::<Vec<_>>(),
            )
        });
        let made = panels.iter().filter(|p| self.seen.insert(**p)).count();
        let editor = if editors == [self.editor] {
            "kept"
        } else {
            "changed"
        };
        let dock = if Self::dock(h) == self.dock {
            "as-left"
        } else {
            "changed"
        };
        let requests: Vec<&str> = h.backend.calls[self.calls..]
            .iter()
            .map(|c| c.split(' ').next().unwrap_or_default())
            .collect();
        let requests = format!("{requests:?}");
        self.calls = h.backend.calls.len();
        let line = format!(
            "{label}: drawn={drawn:?} terminal={:?} focus={:?} panels={} made={made} \
             reviews={reviews:?} editor={editor} dock={dock} requests={requests}",
            h.focused_terminal(),
            h.focus_name(),
            panels.len(),
        );
        h.log(&line);
        line
    }
}

/// A second window over its own workspace (as `blocks_qa::second_window`).
fn second_window(
    h: &mut Harness,
    seed: impl FnOnce(&mut TestBackend),
) -> (
    AnyWindowHandle,
    WindowKey,
    WeakEntity<crate::workspace::Workspace>,
) {
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
    let draw = |h: &mut Harness, backend: &mut TestBackend| {
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
    };
    draw(h, &mut backend);
    // Every mode in this window too (the settings are app-wide).
    for m in CYCLE {
        set_layout(h, m.id());
        draw(h, &mut backend);
    }
    let key = WindowKey(workspace.entity_id());
    (handle, key, workspace)
}

#[test]
fn seven_modes_cycle_keeps_panels_dock_focus_requests_and_state() {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new("layout-cycle", |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert(LOGIN.into(), wt.clone());
    });
    let key = WindowKey(h.ws.entity_id());
    // The workbench as the user left it: an editor and w1, w1 focused.
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_editor(LOGIN, None, false, window, cx)));
    h.settle();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let editor = h.cx.update(|_, cx| {
        let editors = ws.read(cx).live_editors();
        assert_eq!(editors.len(), 1, "one editor");
        editors[0].entity_id()
    });
    let mut probe = Probe {
        seen: BTreeSet::new(),
        calls: 0,
        dock: Probe::dock(&mut h),
        editor,
    };
    let mut lines = vec![probe.step(&mut h, "start workbench")];
    let workbench_panels: Vec<EntityId> = probe.seen.iter().copied().collect();
    assert_eq!(workbench_panels.len(), 1, "w1's dock panel");

    let mut states: HashMap<LayoutMode, EntityId> = HashMap::new();
    let mut keys = Vec::new();
    for round in 1..=2 {
        for m in CYCLE {
            set_layout(&mut h, m.id());
            lines.push(probe.step(&mut h, &format!("{round} {}", m.id())));
            let drawn: Vec<&str> = MARKERS
                .iter()
                .filter(|(mode, _)| *mode == m)
                .map(|(_, id)| *id)
                .collect();
            for (_, id) in MARKERS {
                assert_eq!(
                    h.bounds(id).is_some(),
                    drawn.contains(&id),
                    "{m:?}: only its own body draws ({id})"
                );
            }
            let state = h.cx.update(|_, cx| state_of(m, key, cx));
            match (m, state) {
                (LayoutMode::Workbench, _) => {}
                (_, None) => panic!("{m:?} keeps its state in the window's slot"),
                (_, Some(id)) => {
                    let first = *states.entry(m).or_insert(id);
                    assert_eq!(first, id, "{m:?}: the same state entity every visit");
                }
            }
        }
        // Back on the workbench: the dock and its panels as the user left them.
        assert_eq!(Probe::dock(&mut h), probe.dock, "round {round}: dock JSON");
        let panels = h.cx.update(|_, cx| {
            ws.read(cx)
                .live_panels()
                .iter()
                .map(|p| p.entity_id())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            panels, workbench_panels,
            "round {round}: the workbench's panels only"
        );
        keys.push(keymap_len(&mut h));
    }
    assert_eq!(states.len(), 6, "six modes keep a state");
    assert_eq!(keys[0], keys[1], "each mode binds its keys once");

    // An unknown layout id draws the workbench.
    set_layout(&mut h, "nonsense");
    lines.push(probe.step(&mut h, "unknown id"));
    // A module registered again replaces the old one; the window's state stays.
    h.cx.update(|_, cx| {
        cx.default_global::<LayoutRegistry>()
            .register(Rc::new(crate::layouts::kanban::Kanban))
    });
    set_layout(&mut h, LayoutMode::Kanban.id());
    lines.push(probe.step(&mut h, "kanban re-registered"));
    assert_eq!(
        h.cx.update(|_, cx| state_of(LayoutMode::Kanban, key, cx)),
        states.get(&LayoutMode::Kanban).copied(),
        "the re-registered module finds the window's board"
    );
    set_layout(&mut h, LayoutMode::Workbench.id());
    lines.push(probe.step(&mut h, "workbench again"));
    assert_eq!(keymap_len(&mut h), keys[1], "no key bound twice");

    // A second window: its own six states, all dropped with it; this
    // window's states stay.
    let (handle, second, workspace) = second_window(&mut h, seed_two_tasks);
    let theirs: Vec<EntityId> = h.cx.update(|_, cx| {
        CYCLE
            .iter()
            .filter_map(|m| state_of(*m, second, cx))
            .collect()
    });
    assert_eq!(theirs.len(), 6, "the second window keeps its own six");
    assert!(
        theirs.iter().all(|id| !states.values().any(|s| s == id)),
        "none shared with the first window"
    );
    h.cx.update(|_, cx| {
        let _ = cx.update_window(handle, |_, window, _| window.remove_window());
    });
    h.settle();
    assert!(
        workspace.upgrade().is_none(),
        "the second workspace is gone"
    );
    let left = h.cx.update(|_, cx| {
        CYCLE
            .iter()
            .filter_map(|m| state_of(*m, second, cx))
            .count()
    });
    assert_eq!(left, 0, "its slots are dropped");
    for (m, id) in &states {
        assert_eq!(
            h.cx.update(|_, cx| state_of(*m, key, cx)),
            Some(*id),
            "{m:?}: the first window keeps its state"
        );
    }
    assert_eq!(keymap_len(&mut h), keys[1], "a second window binds no key");

    assert_eq!(
        lines.iter().map(String::as_str).collect::<Vec<_>>(),
        EXPECTED,
        "the cycle as before T2z"
    );
}
