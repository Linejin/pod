//! The editor in the workspace: (f) status-bar language server menu, (g)
//! protocol v9 save/removal hooks and snippets, (h) tabs, tree delete prompt
//! and the tab menu (docs/13 §9).

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::{link_worktree, seed_link_task};
use super::harness::Harness;

#[test]
fn f_status_bar_language_server_menu_off_restart_log() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("f-lsp-menu", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("docs/notes.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    assert!(
        h.bounds("status-lsp").is_none(),
        "no server for .txt: no status item"
    );
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.lsp_turn_off(cx)));
    h.settle();
    assert!(
        h.bounds("status-lsp").is_some(),
        "the off state stays visible so it can be turned back on"
    );
    let enabled = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            ws.read(cx)
                .lsp_target(cx)
                .map(|(s, _)| s.read(cx).is_enabled())
        })
    };
    assert_eq!(enabled(&mut h), Some(false));
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.lsp_show_log(window, cx)));
    h.settle();
    assert!(h.dialog_open(), "log dialog");
    h.keys("escape");
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.lsp_restart(cx)));
    h.settle();
    assert_eq!(enabled(&mut h), Some(true), "restart turns it back on");
}

// ---- (g) protocol v9: task.touch on save, task.removing ----

fn open_notes(h: &mut Harness) -> Entity<pod_editor::EditorWorkspace> {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("docs/notes.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == "sample/links")
            .map(|e| e.read(cx).editor.clone())
    })
    .expect("editor")
}

fn type_in_editor(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>, text: &'static str) {
    h.cx.update(|window, cx| {
        if let Some(st) = ed.read(cx).active_editor() {
            st.update(cx, |s, cx| s.insert(text, window, cx));
        }
    });
    h.settle();
}

#[test]
fn g_saving_in_the_editor_touches_the_task() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-touch", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    type_in_editor(&mut h, &ed, "추가 ");
    h.cx.update(|window, cx| ed.update(cx, |e, cx| e.save_all(window, cx)));
    h.settle();
    let touch = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("task.touch"))
        .cloned();
    h.log(&format!("{touch:?}"));
    let touch = touch.expect("task.touch sent after save");
    assert!(
        touch.contains("\"task\":\"sample/links\"") && touch.contains("docs/notes.txt"),
        "{touch}"
    );
}

#[test]
fn g_task_removing_saves_unsaved_edits_or_closes_a_clean_editor() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-removing", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    type_in_editor(&mut h, &ed, "에이전트가 지우기 전 ");
    assert_eq!(h.cx.update(|_, cx| ed.read(cx).dirty_paths().len()), 1);
    // `pod task remove` from a CLI: podd announces the removal first.
    let _ = h
        .backend
        .events
        .unbounded_send(crate::source::SourceEvent::TaskRemoving {
            id: "sample/links".into(),
            worktree: wt.clone(),
        });
    h.settle();
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).dirty_paths().len()),
        0,
        "unsaved edits were saved (podd then stops with TASK_DIRTY)"
    );
    let saved =
        std::fs::read_to_string(std::path::Path::new(&wt).join("docs/notes.txt")).expect("file");
    assert!(saved.contains("에이전트가 지우기 전"), "{saved}");
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == crate::strings::TASK_REMOVING_SAVED),
        "{:?}",
        h.toast_titles()
    );
    let ws = h.ws.clone();
    let open = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .iter()
                .any(|e| e.read(cx).task_id == "sample/links")
        })
    };
    assert!(open(&mut h), "editor kept while the removal stops");
    // Clean now: a second announcement closes the editor (language servers stop).
    let _ = h
        .backend
        .events
        .unbounded_send(crate::source::SourceEvent::TaskRemoving {
            id: "sample/links".into(),
            worktree: wt.clone(),
        });
    h.settle();
    assert!(
        !open(&mut h),
        "clean editor closed before the worktree goes"
    );
}

/// A snippet completion with an auto-import, inserted the way the
/// completion menu does (`insert_completion`): the import lands above, the
/// first placeholder is selected, Tab / Shift+Tab walk the stops, typing
/// replaces a placeholder, and Esc ends the session (Tab indents again).
#[test]
fn g_snippet_completion_tab_stops_and_auto_import() {
    use pod_editor::lsp::types as lt;
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-snippet", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    let item = lt::CompletionItem {
        label: "pair".into(),
        insert_text: Some("pair(${1:left}, ${2:right})$0".into()),
        insert_text_format: Some(lt::InsertTextFormat::SNIPPET),
        additional_text_edits: Some(vec![lt::TextEdit {
            range: lt::Range::default(),
            new_text: "use pairs::pair;\n".into(),
        }]),
        ..Default::default()
    };
    let extra = pod_editor::lsp::store::completion_extra(&item, false).expect("extra");
    let mut shown = item.clone();
    pod_editor::lsp::store::expand_snippet_item(&mut shown);
    let state =
        h.cx.update(|_, cx| ed.read(cx).active_editor())
            .expect("editor state");
    h.cx.update(|window, cx| {
        let path = ed.read(cx).active_path().expect("path").to_path_buf();
        let lsp = ed.read(cx).lsp().clone();
        lsp.update(cx, |s, _| s.note_completion(&path, 0, vec![extra]));
        state.update(cx, |s, cx| s.insert_completion(&shown, 0..0, window, cx));
    });
    h.settle();
    let read = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            let s = state.read(cx);
            let text = s.value().to_string();
            let sel = s.selected_range();
            (
                text.clone(),
                text[sel].to_string(),
                ed.read(cx).snippet_active(),
            )
        })
    };
    let (text, selected, active) = read(&mut h);
    h.log(&format!("after insert: selected {selected:?}"));
    assert!(
        text.starts_with("use pairs::pair;\npair(left, right)"),
        "{text:?}"
    );
    assert_eq!(
        (selected.as_str(), active),
        ("left", true),
        "first placeholder selected"
    );
    // Focus the code editor so keys go through its bindings (Tab = IndentInline).
    h.cx.update(|window, cx| state.update(cx, |s, cx| s.focus(window, cx)));
    h.keys("tab");
    assert_eq!(read(&mut h).1, "right");
    h.keys("shift-tab");
    assert_eq!(read(&mut h).1, "left");
    h.type_text("a");
    let (text, _, active) = read(&mut h);
    assert!(
        text.starts_with("use pairs::pair;\npair(a, right)"),
        "typing replaced the placeholder: {text:?}"
    );
    assert!(active);
    h.keys("tab");
    assert_eq!(read(&mut h).1, "right");
    h.keys("escape");
    assert!(!read(&mut h).2, "Esc ends the session");
    let before = read(&mut h).0;
    h.keys("tab");
    assert_ne!(
        read(&mut h).0,
        before,
        "Tab edits (indents) again after the session"
    );
}

// ---- (h) editor tabs: preview, MRU, close prompt, reopen, restore (docs/13 §9) ----

fn tabs_worktree() -> (tempfile::TempDir, String) {
    let (dir, wt) = link_worktree();
    for n in ["alpha", "beta", "gamma", "delta"] {
        std::fs::write(
            std::path::Path::new(&wt).join(format!("{n}.txt")),
            format!("{n} 첫 줄\n"),
        )
        .expect("file");
    }
    (dir, wt)
}

/// Tabs as `name`, `name~` (preview), `name*` (unsaved); the active one in [ ].
fn tab_strip(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> String {
    h.cx.update(|_, cx| {
        ed.read(cx)
            .tab_states()
            .iter()
            .map(|t| {
                let mut s = t
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_end_matches(".txt")
                    .to_string();
                if t.preview {
                    s.push('~');
                }
                if t.dirty {
                    s.push('*');
                }
                if t.active { format!("[{s}]") } else { s }
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Whether keyboard focus is in the active tab's code editor.
fn editor_focused(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> bool {
    h.cx.update(|window, cx| {
        ed.read(cx)
            .active_editor()
            .is_some_and(|s| s.focus_handle(cx).is_focused(window))
    })
}

/// Quick open by keyboard: Ctrl+P, the name, Enter.
fn quick_open(h: &mut Harness, name: &str) {
    h.keys("ctrl-p");
    h.type_text(name);
    h.keys("enter");
}

#[test]
fn h_editor_tabs_keyboard_preview_mru_close_reopen() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-tabs", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "[alpha]",
        "an explicit open is a pinned tab"
    );
    assert!(editor_focused(&mut h, &ed));

    // Quick open makes a preview tab; the next one replaces it in place.
    quick_open(&mut h, "beta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [beta~]");
    quick_open(&mut h, "gamma.txt");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma~]",
        "preview replaced, not added"
    );
    assert!(editor_focused(&mut h, &ed), "focus follows the opened file");
    // Typing pins it.
    h.type_text("x");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*]");
    quick_open(&mut h, "delta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");

    // Ctrl+Tab: previous file; release Ctrl and Ctrl+Tab again toggles back.
    h.keys("ctrl-tab");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*] delta~");
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    h.keys("ctrl-tab");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha gamma* [delta~]",
        "Ctrl+Tab, release, Ctrl+Tab toggles"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    // Held Ctrl: Tab Tab walks further back (delta → gamma → alpha).
    h.keys("ctrl-tab ctrl-tab");
    assert_eq!(tab_strip(&mut h, &ed), "[alpha] gamma* delta~");
    h.keys("ctrl-shift-tab");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma*] delta~",
        "Ctrl+Shift+Tab steps back while held"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    assert!(editor_focused(&mut h, &ed));

    // Ctrl+PageDown / Ctrl+PageUp: by position, wrapping.
    h.keys("ctrl-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");
    h.keys("ctrl-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "[alpha] gamma* delta~");
    h.keys("ctrl-pageup");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");
    h.keys("ctrl-pageup");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*] delta~");

    // Ctrl+W on an unsaved tab asks; Esc cancels and focus returns to the editor.
    h.keys("ctrl-w");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_open()),
        "save/discard/cancel prompt"
    );
    h.keys("escape");
    assert!(!h.cx.update(|_, cx| ed.read(cx).close_prompt_open()));
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma*] delta~",
        "cancel keeps the tab"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus back in the editor after cancel: {}",
        h.focus_name()
    );
    // Ctrl+W, Enter: save and close; focus moves to the most recent other tab.
    h.keys("ctrl-w");
    h.keys("enter");
    let saved =
        std::fs::read_to_string(std::path::Path::new(&wt).join("gamma.txt")).expect("gamma");
    assert!(saved.starts_with('x'), "saved before closing: {saved:?}");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [delta~]",
        "most recently used tab (delta) is active"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus in the next tab's editor: {}",
        h.focus_name()
    );

    // Ctrl+Shift+T reopens gamma where it was (pinned), even though the app
    // binds Ctrl+Shift+T to "new terminal" outside the editor.
    h.keys("ctrl-shift-t");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma] delta~");
    assert!(editor_focused(&mut h, &ed));

    // Restart: the panel's saved state restores order, preview, active tab and MRU.
    let state =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|p| gpui_kit::base::dock::Panel::dump(p.read(cx), cx))
        })
        .expect("panel");
    let session =
        crate::editor_panel::EditorPanel::session_of_state(&state).expect("session saved");
    h.log(&format!(
        "saved session {}",
        serde_json::to_string(&session).unwrap()
    ));
    assert_eq!(
        session
            .tabs
            .iter()
            .map(|t| (t.path.as_str(), t.preview))
            .collect::<Vec<_>>(),
        [
            ("alpha.txt", false),
            ("gamma.txt", false),
            ("delta.txt", true)
        ]
    );
    let weak = ws.downgrade();
    let root = std::path::PathBuf::from(&wt);
    let restored = h.cx.update(|window, cx| {
        cx.new(|cx| {
            crate::editor_panel::EditorPanel::new(
                "sample/links".into(),
                root,
                Some(&session),
                weak,
                window,
                cx,
            )
        })
    });
    h.settle();
    let red = h.cx.update(|_, cx| restored.read(cx).editor.clone());
    assert_eq!(tab_strip(&mut h, &red), "alpha [gamma] delta~");
    h.cx.update(|window, cx| red.update(cx, |e, cx| e.switch_mru(true, window, cx)));
    h.settle();
    assert_eq!(
        tab_strip(&mut h, &red),
        "alpha gamma [delta~]",
        "Ctrl+Tab order survived the restart"
    );

    // Ctrl+W on clean tabs closes at once; the last one returns focus to the tree.
    // (The restored copy above took focus; give it back to the live editor.)
    drop(restored);
    h.cx.update(|window, cx| {
        if let Some(st) = ed.read(cx).active_editor() {
            st.update(cx, |s, cx| s.focus(window, cx));
        }
    });
    h.settle();
    h.keys("ctrl-w ctrl-w ctrl-w");
    assert_eq!(tab_strip(&mut h, &ed), "");
    let tree_focused = h.cx.update(|window, cx| {
        ed.read(cx).focus_handle(cx).contains_focused(window, cx)
            && ed.read(cx).active_editor().is_none()
    });
    assert!(
        tree_focused,
        "focus stays in the files panel (tree): {}",
        h.focus_name()
    );
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).closed_paths().len()),
        3,
        "all three can be reopened"
    );
}

/// The tree's delete prompt answers Esc and Enter from the keyboard (it
/// takes focus while shown; before, the keys went nowhere).
#[test]
fn h_tree_delete_prompt_esc_cancels_enter_deletes() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-delete", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    let alpha = std::path::Path::new(&wt).join("alpha.txt");
    // Alt+1: tree; Home is docs/, Down is alpha.txt.
    h.keys("alt-1 home down");
    h.keys("delete");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).has_overlay()),
        "delete prompt shown"
    );
    h.keys("escape");
    assert!(
        !h.cx.update(|_, cx| ed.read(cx).has_overlay()),
        "Esc closed the prompt"
    );
    assert!(alpha.exists(), "Esc kept the file");
    h.keys("alt-1");
    h.keys("delete");
    h.keys("enter");
    assert!(!alpha.exists(), "Enter deleted the file");
    assert_eq!(tab_strip(&mut h, &ed), "", "its tab closed");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).closed_paths().is_empty()),
        "a deleted file is not offered by Ctrl+Shift+T"
    );
}

/// Selected item of the open tab menu.
fn menu_selected(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> Option<String> {
    h.cx.update(|_, cx| {
        ed.read(cx)
            .tab_menu_items()
            .into_iter()
            .find(|i| i.selected)
            .map(|i| i.label)
    })
}

/// Walks the open tab menu with Down until `label` is selected, then Enter.
fn menu_pick(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>, label: &str) {
    for _ in 0..12 {
        if menu_selected(h, ed).as_deref() == Some(label) {
            h.keys("enter");
            return;
        }
        h.keys("down");
    }
    panic!(
        "menu item {label:?} not reachable; items {:?}",
        h.cx.update(|_, cx| ed.read(cx).tab_menu_items())
    );
}

fn menu_focused(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> bool {
    h.cx.update(|_, cx| ed.read(cx).tab_menu_open())
}

/// Tab menu and tab moving by keyboard only (docs/13 §9): Ctrl+Shift+
/// PageUp/PageDown move the active tab (a preview is pinned, the order is
/// saved), Shift+F10 / the menu key open the tab menu, arrows skip
/// unavailable items, Esc returns focus, bulk closes ask once for all
/// unsaved files, and the path / tree commands work.
#[test]
fn h_editor_tab_menu_and_move_keyboard() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-tab-menu", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    // The editor holds the canonical root; on CI the temp dir is spelled with
    // 8.3 short names (RUNNER~1), so compare against the same spelling.
    let root = pod_editor::uri::simplify(&std::fs::canonicalize(&wt).expect("root"));
    for n in ["beta", "gamma"] {
        let p = root.join(format!("{n}.txt"));
        h.cx.update(|window, cx| ed.update(cx, |e, cx| e.open_file(&p, window, cx)))
            .expect("open");
    }
    quick_open(&mut h, "delta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha beta gamma [delta~]");

    // Ctrl+Shift+PageUp moves the active tab left (and pins the preview).
    h.keys("ctrl-shift-pageup");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha beta [delta] gamma",
        "moved left and pinned"
    );
    h.keys("ctrl-shift-pageup ctrl-shift-pageup ctrl-shift-pageup");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "[delta] alpha beta gamma",
        "stops at the first place (no wrap)"
    );
    h.keys("ctrl-shift-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta gamma");
    assert!(
        editor_focused(&mut h, &ed),
        "moving keeps focus in the editor"
    );
    // The order is what the layout saves.
    let state =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|p| gpui_kit::base::dock::Panel::dump(p.read(cx), cx))
        })
        .expect("panel");
    let session = crate::editor_panel::EditorPanel::session_of_state(&state).expect("session");
    assert_eq!(
        session
            .tabs
            .iter()
            .map(|t| t.path.as_str())
            .collect::<Vec<_>>(),
        ["alpha.txt", "delta.txt", "beta.txt", "gamma.txt"]
    );

    // Shift+F10: the active tab's menu takes focus; Esc gives it back.
    h.keys("shift-f10");
    assert!(menu_focused(&mut h, &ed), "menu open");
    let items = h.cx.update(|_, cx| ed.read(cx).tab_menu_items());
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    h.log(&format!("menu {labels:?}"));
    assert_eq!(
        labels,
        [
            "닫기",
            "다른 탭 닫기",
            "오른쪽 탭 닫기",
            "저장된 탭 닫기",
            "모두 닫기",
            "고정 해제",
            "경로 복사",
            "상대 경로 복사",
            "파일 트리에서 보기"
        ]
    );
    assert_eq!(menu_selected(&mut h, &ed).as_deref(), Some("닫기"));
    assert!(
        h.cx.update(|window, cx| ed.read(cx).tab_menu_has_focus(window)),
        "focus on the menu: {}",
        h.focus_name()
    );
    h.keys("up");
    assert_eq!(
        menu_selected(&mut h, &ed).as_deref(),
        Some("파일 트리에서 보기"),
        "Up wraps to the last item"
    );
    h.keys("home");
    assert_eq!(menu_selected(&mut h, &ed).as_deref(), Some("닫기"));
    h.keys("escape");
    assert!(!menu_focused(&mut h, &ed));
    assert!(
        editor_focused(&mut h, &ed),
        "Esc returns focus to the editor: {}",
        h.focus_name()
    );

    // Copy path / relative path (menu key opens the menu too).
    h.keys("menu");
    menu_pick(&mut h, &ed, "상대 경로 복사");
    assert_eq!(
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .as_deref(),
        Some("delta.txt")
    );
    assert!(editor_focused(&mut h, &ed), "focus back after a command");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "경로 복사");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default();
    // The editor may hold the canonical spelling of the root (8.3 short names
    // such as RUNNER~1 resolve to the long name), so compare resolved paths.
    assert_eq!(
        std::fs::canonicalize(&copied).ok(),
        std::fs::canonicalize(root.join("delta.txt")).ok(),
        "copied {copied}"
    );

    // Reveal in the file tree: the tree gets focus with the file selected.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "파일 트리에서 보기");
    let (tree_sel, in_tree) = h.cx.update(|window, cx| {
        let e = ed.read(cx);
        (
            e.tree().selected().map(|p| p.to_path_buf()),
            e.focus_handle(cx).contains_focused(window, cx)
                && !e
                    .active_editor()
                    .is_some_and(|s| s.focus_handle(cx).is_focused(window)),
        )
    });
    assert_eq!(tree_sel, Some(root.join("delta.txt")));
    assert!(in_tree, "focus in the tree: {}", h.focus_name());
    h.keys("alt-2");

    // 고정 해제 → the tab is the preview again; 고정 pins it back.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "고정 해제");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta~] beta gamma");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "고정");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta gamma");

    // Two unsaved tabs; "다른 탭 닫기" asks once, listing both files.
    for n in ["beta", "gamma"] {
        let p = root.join(format!("{n}.txt"));
        h.cx.update(|window, cx| ed.update(cx, |e, cx| e.open_file(&p, window, cx)))
            .expect("open");
        type_in_editor(&mut h, &ed, "z");
    }
    h.cx.update(|window, cx| {
        ed.update(cx, |e, cx| e.open_file(&root.join("delta.txt"), window, cx))
    })
    .expect("open");
    h.settle();
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta* gamma*");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "다른 탭 닫기");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_open()),
        "one combined prompt"
    );
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_files()),
        ["beta.txt", "gamma.txt"]
    );
    h.keys("escape");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [delta] beta* gamma*",
        "cancel closes nothing"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus back after cancel: {}",
        h.focus_name()
    );
    // "저장된 탭 닫기" never asks: alpha and delta go, the unsaved ones stay.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "저장된 탭 닫기");
    assert!(!h.cx.update(|_, cx| ed.read(cx).close_prompt_open()));
    assert_eq!(
        tab_strip(&mut h, &ed).replace(['[', ']'], ""),
        "beta* gamma*"
    );
    assert!(editor_focused(&mut h, &ed));
    // "모두 닫기", Enter: both saved, then all closed; Ctrl+Shift+T brings them back in order.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "모두 닫기");
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_files()),
        ["beta.txt", "gamma.txt"]
    );
    h.keys("enter");
    assert_eq!(tab_strip(&mut h, &ed), "");
    for n in ["beta", "gamma"] {
        let saved = std::fs::read_to_string(root.join(format!("{n}.txt"))).expect("file");
        assert!(
            saved.starts_with('z'),
            "{n} saved before closing: {saved:?}"
        );
    }
    h.keys("ctrl-shift-t ctrl-shift-t");
    assert_eq!(tab_strip(&mut h, &ed).replace(['[', ']'], ""), "beta gamma");

    // "오른쪽 탭 닫기" is unavailable on the last tab: arrows skip it.
    h.keys("shift-f10");
    let items = h.cx.update(|_, cx| ed.read(cx).tab_menu_items());
    let right = items
        .iter()
        .find(|i| i.label == "오른쪽 탭 닫기")
        .expect("item");
    assert!(
        !right.enabled,
        "nothing to the right of the active (last) tab"
    );
    h.keys("down down");
    assert_eq!(
        menu_selected(&mut h, &ed).as_deref(),
        Some("저장된 탭 닫기"),
        "Down skipped the unavailable item"
    );
    h.keys("escape");
    assert!(editor_focused(&mut h, &ed));
}
