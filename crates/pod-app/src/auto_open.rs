//! Auto-open of new tasks (docs/19 §3.1 step 7).
//!
//! podd's `task.changed created` carries an `open_hint`. A task a person
//! made (`focus: true`) opens its agent terminal and takes focus; a worker
//! task (`focus: false`) gets a tab in the center, and keyboard focus and
//! the visible tab stay where the user was. `POD_APP_AUTO_OPEN=0` turns it
//! off.

use gpui_kit::component::dock::PanelId;
use gpui_kit::*;
use podd::protocol as p;
use serde_json::Value;

use crate::source::SourceEvent;
use crate::workspace::Workspace;

/// `task.changed created` with an `open_hint` -> [`SourceEvent::TaskCreated`].
pub fn created_event(n: &p::Notification) -> Option<SourceEvent> {
    if n.method != p::NOTIFY_TASK_CHANGED
        || n.params.get("change").and_then(Value::as_str) != Some("created")
    {
        return None;
    }
    let hint: p::OpenHint = serde_json::from_value(n.params.get("open_hint")?.clone()).ok()?;
    let id = n.params.get("id").and_then(Value::as_str)?.to_string();
    let task = n.params.get("task");
    let terminal = task
        .and_then(|t| t.get("agent_terminal").and_then(Value::as_str))
        .or_else(|| task?.get("terminals")?.get(0)?.get("id")?.as_str())
        .map(str::to_string);
    Some(SourceEvent::TaskCreated { id, terminal, hint })
}

fn enabled() -> bool {
    std::env::var("POD_APP_AUTO_OPEN")
        .map(|v| v.trim() != "0")
        .unwrap_or(true)
}

pub fn on_task_created(
    ws: &mut Workspace,
    terminal: Option<String>,
    hint: p::OpenHint,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(term) = terminal else { return };
    if !enabled() {
        return;
    }
    if hint.focus {
        ws.open_terminal(&term, false, window, cx);
        return;
    }
    if ws
        .live_panels()
        .iter()
        .any(|p| p.read(cx).terminal_id == term)
    {
        return;
    }
    // Where the user is now: keyboard focus and the terminal panel holding it.
    let focused = window.focused(cx);
    let current = ws.live_panels().into_iter().find(|p| {
        p.read(cx)
            .view
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    ws.add_panel(term, window, cx);
    if let Some(p) = current {
        let pid = PanelId::from(p.entity_id());
        ws.dock.update(cx, |a, cx| a.select_panel(pid, window, cx));
    }
    if let Some(f) = focused {
        window.focus(&f, cx);
    }
    ws.schedule_save(cx);
}

#[cfg(test)]
mod tests {
    use super::{SourceEvent, Value, created_event, p};
    use crate::native_qa::{Harness, seed_two_tasks};
    use crate::source::TestBackend;
    use serde_json::json;

    #[test]
    fn created_event_reads_the_hint() {
        let n = |params: Value| p::Notification {
            jsonrpc: "2.0".into(),
            method: p::NOTIFY_TASK_CHANGED.into(),
            params,
        };
        let ev = created_event(&n(
            json!({"seq": 1, "id": "r/a", "change": "created", "open_hint": {"focus": false, "reason": "worker"},
            "task": {"agent_terminal": "t7", "terminals": [{"id": "t6"}]}}),
        ));
        assert_eq!(
            ev,
            Some(SourceEvent::TaskCreated {
                id: "r/a".into(),
                terminal: Some("t7".into()),
                hint: p::OpenHint {
                    focus: false,
                    reason: p::OpenReason::Worker
                }
            })
        );
        let ev = created_event(&n(
            json!({"seq": 1, "id": "r/a", "change": "created", "open_hint": {"focus": true, "reason": "user"}, "task": {"terminals": [{"id": "t6"}]}}),
        ));
        assert!(
            matches!(ev, Some(SourceEvent::TaskCreated { terminal: Some(t), .. }) if t == "t6")
        );
        // Older daemons send no hint: nothing opens by itself.
        assert_eq!(
            created_event(&n(json!({"seq": 1, "id": "r/a", "change": "created"}))),
            None
        );
        assert_eq!(
            created_event(&n(
                json!({"seq": 1, "id": "r/a", "change": "git", "open_hint": {"focus": true, "reason": "user"}})
            )),
            None
        );
    }

    fn new_task(h: &mut Harness, id: &str, term: &str) {
        let name = id.rsplit('/').next().unwrap();
        h.backend
            .tasks
            .push(TestBackend::task(id, name, "sample", &[term]));
        h.backend
            .terminals
            .push(TestBackend::terminal(term, name, Some(id), "working"));
        h.backend.publish();
        h.settle();
    }

    fn panel_open(h: &mut Harness, term: &str) -> bool {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_panels()
                .iter()
                .any(|p| p.read(cx).terminal_id == term)
        })
    }

    /// docs/19 §10 S1: a worker task gets a tab, focus stays; a user task takes focus.
    #[test]
    fn worker_tabs_open_without_stealing_focus() {
        let mut h = Harness::new("auto-open", seed_two_tasks);
        let ws = h.ws.clone();
        h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
        h.settle();
        assert_eq!(h.focused_terminal().as_deref(), Some("w1"));

        new_task(&mut h, "sample/worker-1", "k1");
        let hint = p::OpenHint {
            focus: false,
            reason: p::OpenReason::Worker,
        };
        let _ = h.backend.events.unbounded_send(SourceEvent::TaskCreated {
            id: "sample/worker-1".into(),
            terminal: Some("k1".into()),
            hint,
        });
        h.settle();
        assert!(panel_open(&mut h, "k1"), "worker tab opened");
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("w1"),
            "focus unchanged: {}",
            h.focus_name()
        );
        h.type_text("x");
        assert_eq!(h.backend.typed_text("w1"), "x", "typing still goes to w1");
        assert_eq!(h.backend.typed_text("k1"), "");

        new_task(&mut h, "sample/mine", "m1");
        let hint = p::OpenHint {
            focus: true,
            reason: p::OpenReason::User,
        };
        let _ = h.backend.events.unbounded_send(SourceEvent::TaskCreated {
            id: "sample/mine".into(),
            terminal: Some("m1".into()),
            hint,
        });
        h.settle();
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("m1"),
            "a user task takes focus"
        );
    }
}
