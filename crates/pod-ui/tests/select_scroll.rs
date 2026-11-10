//! `pod_ui::select` in a real GPUI test window (headless): with more options than the
//! 8-row list shows, the keyboard highlight is always scrolled into view — Open on a late
//! value, ↓ past the bottom, End/Home, type-ahead — and Enter picks the visible row.
//! Shift+Tab closes the list. (ui-editor-04)
#![recursion_limit = "512"]

use ::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::*;
use pod_ui::{Mode, SelectEvent};
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

const N: usize = 20;

struct Host {
    options: Vec<SharedString>,
    selected: Option<usize>,
    open: Option<usize>,
    scroll: ScrollHandle,
    parent_focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity().downgrade();
        let parent = self.parent_focus.clone();
        div()
            .size_full()
            .p(px(8.))
            .track_focus(&self.parent_focus)
            .capture_key_down(move |e, window, cx| {
                if e.keystroke.key == "escape" {
                    parent.focus(window, cx);
                    cx.stop_propagation();
                }
            })
            .child(pod_ui::select(
                "select-style",
                "스타일",
                &self.options,
                self.selected,
                self.open,
                &self.scroll,
                true,
                Mode::Dark,
                move |ev, _, cx| {
                    let _ = me.update(cx, |h, cx| {
                        match ev {
                            SelectEvent::Open(i) | SelectEvent::Move(i) => h.open = Some(i),
                            SelectEvent::Pick(i) => {
                                h.selected = Some(i);
                                h.open = None;
                            }
                            SelectEvent::Close => h.open = None,
                        }
                        cx.notify();
                    });
                },
            ))
    }
}

struct T {
    _app: TestAppContext,
    cx: VisualTestContext,
    host: Entity<Host>,
}

impl T {
    fn new(selected: usize) -> Self {
        let mut app = TestAppContext::single();
        app.update(|cx| {
            gpui_kit::init(cx);
            pod_ui::apply_theme(Mode::Dark, true, cx);
        });
        let handle = app.add_window(|_, cx| Host {
            // a-style … t-style: distinct first letters for type-ahead.
            options: (0..N)
                .map(|i| format!("{}-style", (b'a' + i as u8) as char).into())
                .collect(),
            selected: Some(selected),
            open: None,
            scroll: ScrollHandle::new(),
            parent_focus: cx.focus_handle(),
        });
        let host = handle.root(&mut app).expect("host");
        let mut cx = VisualTestContext::from_window(handle.into(), &app);
        cx.simulate_resize(size(px(800.), px(600.)));
        cx.update(|window, cx| {
            window.activate_window();
            window.focus_next(cx);
        });
        let mut t = T {
            _app: app,
            cx,
            host,
        };
        t.settle();
        t
    }

    fn settle(&mut self) {
        for _ in 0..2 {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        }
    }

    fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    fn state(&mut self) -> (Option<usize>, Option<usize>) {
        let h = self.host.clone();
        self.cx.update(|_, cx| {
            let h = h.read(cx);
            (h.selected, h.open)
        })
    }

    /// Is option `i` fully inside the list's viewport at the current scroll offset?
    fn visible(&mut self, i: usize) -> bool {
        let h = self.host.clone();
        self.cx.update(|_, cx| {
            let s = &h.read(cx).scroll;
            let (view, off) = (s.bounds(), s.offset());
            let row = s.bounds_for_item(i).expect("option laid out");
            assert!(view.size.height > px(0.), "the list is laid out");
            row.top() + off.y >= view.top() - px(0.5)
                && row.bottom() + off.y <= view.bottom() + px(0.5)
        })
    }
}

#[test]
fn focus_loss_closes_without_picking_or_stealing_focus() {
    let mut t = T::new(0);
    t.keys("enter down");
    assert_eq!(t.state(), (Some(0), Some(1)));
    t.cx.deactivate_window();
    t.settle();
    assert_eq!(t.state(), (Some(0), None), "Alt+Tab closes without picking");
    t.cx.update(|w, _| w.activate_window());
    t.settle();
    assert_eq!(t.state().1, None, "returning does not reopen");

    t.keys("enter");
    let host = t.host.clone();
    let parent = t.cx.update(|_, cx| host.read(cx).parent_focus.clone());
    t.cx.update(|w, cx| parent.focus(w, cx));
    t.settle();
    assert_eq!(t.state(), (Some(0), None), "programmatic focus closes");
    assert!(t.cx.update(|w, _| parent.is_focused(w)), "no focus theft");

    t.cx.update(|w, cx| w.focus_next(cx));
    t.keys("enter");
    assert_eq!(t.state().1, Some(0));
    t.cx.update(|w, cx| w.blur(cx));
    t.settle();
    assert_eq!(t.state().1, None, "clearing focus closes");

    t.cx.update(|w, cx| w.focus_next(cx));
    t.keys("enter escape");
    assert_eq!(t.state(), (Some(0), None));
    assert!(
        t.cx.update(|w, _| parent.is_focused(w)),
        "parent Escape receives focus"
    );
}

#[test]
fn mouse_down_out_still_dismisses_the_list() {
    let mut t = T::new(0);
    t.keys("enter down");
    t.cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(700.), px(500.)),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    t.settle();
    assert_eq!(
        t.state(),
        (Some(0), None),
        "outside mouse-down does not pick"
    );
}

#[test]
fn end_down_and_open_keep_the_highlight_in_view() {
    let mut t = T::new(0);
    t.keys("enter");
    assert_eq!(t.state(), (Some(0), Some(0)), "Enter opens on the value");
    assert!(t.visible(0));
    assert!(!t.visible(N - 1), "more options than the 8-row list shows");

    t.keys("end");
    assert_eq!(t.state().1, Some(N - 1));
    assert!(
        t.visible(N - 1),
        "End: the last option is scrolled into view"
    );
    assert!(!t.visible(0));

    t.keys("home");
    assert!(t.visible(0), "Home: back to the top");

    // ↓ repeated past the bottom edge: the highlight never leaves the view.
    for i in 1..12 {
        t.keys("down");
        assert_eq!(t.state().1, Some(i));
        assert!(t.visible(i), "↓ to {i}: highlight visible");
    }

    t.keys("end enter");
    assert_eq!(
        t.state(),
        (Some(N - 1), None),
        "Enter picks the visible last row"
    );

    // Reopening on a late value starts with it in view.
    t.keys("enter");
    assert_eq!(t.state().1, Some(N - 1));
    assert!(t.visible(N - 1), "Open on the last value shows it");
}

#[test]
fn type_ahead_scrolls_and_shift_tab_closes() {
    let mut t = T::new(0);
    t.keys("enter q");
    assert_eq!(t.state(), (Some(0), Some(16)), "q → q-style");
    assert!(t.visible(16), "type-ahead scrolls the match into view");
    t.keys("b");
    assert!(t.visible(1), "and back up");
    t.keys("shift-tab");
    assert_eq!(t.state(), (Some(0), None), "Shift+Tab closes like Tab");
    // Closed: type-ahead changes the value directly.
    t.keys("s");
    assert_eq!(t.state(), (Some(18), None));
}
