//! (f) Terminal links: the keyboard picker and Ctrl+click.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::{link_worktree, seed_link_task};
use super::harness::{Harness, center, jump_to, palette};

fn terminal_view(h: &mut Harness, id: &str) -> Entity<pod_terminal_view::TerminalView> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
            .map(|p| p.read(cx).view.clone())
    })
    .expect("terminal panel")
}

/// Editor of `task`: (active path, 0-based cursor line).
fn editor_cursor(h: &mut Harness, task: &str) -> Option<(String, u32)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let e = ws
            .read(cx)
            .live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == task)?;
        let ed = e.read(cx).editor.read(cx);
        let path = ed.active_path()?.to_string_lossy().replace('\\', "/");
        let line = ed.active_editor()?.read(cx).cursor_position().line;
        Some((path, line))
    })
}

#[test]
fn f_terminal_links_keyboard_picker_and_ctrl_click() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("f-links", |b| seed_link_task(b, &wt));
    jump_to(&mut h, "링크 터미널");
    assert_eq!(h.focused_terminal().as_deref(), Some("L1"));
    let view = terminal_view(&mut h, "L1");
    h.cx.update(|_, cx| {
        view.update(cx, |v, cx| v.feed("error: docs/notes.txt:2:4 참고, https://example.com/docs.\r\nmissing.rs:9 은 없는 파일\r\n".as_bytes(), cx));
    });
    h.settle();
    let links = h.cx.update(|_, cx| view.read(cx).visible_links());
    let labels: Vec<String> = links.iter().map(|l| l.target.label()).collect();
    h.log(&format!("visible links {labels:?}"));
    assert_eq!(
        labels,
        vec![
            "docs/notes.txt:2:4".to_string(),
            "https://example.com/docs".into()
        ],
        "missing.rs is not a file: not offered"
    );

    // Keyboard: palette, 터미널에서 링크 열기, Enter on the first link.
    palette(&mut h, "터미널에서 링크 열기", "");
    assert!(h.dialog_open(), "the link picker is open");
    h.keys("enter");
    assert!(!h.dialog_open());
    let (path, line) = editor_cursor(&mut h, "sample/links").expect("the task editor opened");
    assert!(path.ends_with("docs/notes.txt"), "{path}");
    assert_eq!(line, 1, "cursor on line 2 (0-based 1)");

    // Mouse: Ctrl+hover underlines, Ctrl+click opens the URL; a plain click does not.
    // (Ctrl+K inside the editor is its chord prefix, so return to the terminal directly.)
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("L1", false, window, cx)));
    h.settle();
    let url = links[1].clone();
    let cell =
        h.cx.update(|_, cx| view.read(cx).cell_bounds(url.start.col + 3, 0))
            .expect("painted");
    let at = center(cell);
    h.cx.simulate_mouse_move(at, None, Modifiers::none());
    h.settle();
    assert!(
        h.cx.update(|_, cx| view.read(cx).hovered_link().is_none()),
        "no underline without Ctrl"
    );
    h.cx.simulate_modifiers_change(Modifiers::control());
    h.settle();
    let by_modifiers =
        h.cx.update(|_, cx| view.read(cx).hovered_link().map(|l| l.target.label()));
    assert_eq!(
        by_modifiers.as_deref(),
        Some("https://example.com/docs"),
        "pressing Ctrl over the link underlines it"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    assert!(
        h.cx.update(|_, cx| view.read(cx).hovered_link().is_none()),
        "releasing Ctrl removes the underline"
    );
    h.cx.simulate_mouse_move(at, None, Modifiers::control());
    h.settle();
    let hovered =
        h.cx.update(|_, cx| view.read(cx).hovered_link().map(|l| l.target.label()));
    assert_eq!(
        hovered.as_deref(),
        Some("https://example.com/docs"),
        "moving with Ctrl held underlines too"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.cx.simulate_click(at, Modifiers::none());
    h.settle();
    crate::term_links::OPENED_URLS
        .with(|o| assert!(o.borrow().is_empty(), "plain click opened nothing"));
    h.cx.simulate_click(at, Modifiers::control());
    h.settle();
    h.log("ctrl+click on the URL");
    crate::term_links::OPENED_URLS.with(|o| {
        assert_eq!(
            o.borrow().as_slice(),
            ["https://example.com/docs".to_string()]
        )
    });
    assert!(
        h.backend.typed_text("L1").is_empty(),
        "link clicks never reach the program"
    );
}
