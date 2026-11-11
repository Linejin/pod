//! Rendering of the tab strip (tabs, drag and drop, overflow button) and of
//! the tab menu / open-files list.

use std::path::PathBuf;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types as lt;
use pod_ui::{Icon, Mode, Tokens, icon, tokens::color as tc};

use crate::ui::strings as txt;
use crate::uri::path_key;

use super::{DiskState, EditorWorkspace, OVERFLOW_BTN_W, TAB_STRIP_H, TabKey, file_name};

/// Drag payload of a tab.
#[derive(Clone)]
struct DraggedTab {
    path: PathBuf,
    title: String,
}

/// What follows the cursor while a tab is dragged.
struct TabDragPreview {
    title: String,
    mode: Mode,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::for_mode(self.mode);
        div()
            .h(px(28.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(tc(t.bg_raised))
            .border_1()
            .border_color(tc(t.border_subtle))
            .shadow(pod_ui::tokens::elevation(2, self.mode))
            .text_size(px(13.))
            .text_color(tc(t.text_primary))
            .whitespace_nowrap()
            .child(self.title.clone())
    }
}

impl EditorWorkspace {
    pub(super) fn render_tabs(
        &mut self,
        t: Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = self.buffers.len();
        let mode = self.mode;
        if std::mem::take(&mut self.scroll_active)
            && let Some(a) = self.tabs.active()
        {
            self.tab_scroll.scroll_to_item(a);
            self.scroll_retries = 2;
        }
        // Overflow, from the last layout. While the button shows, the strip
        // is narrower by its width, so that width is the threshold (no flicker).
        let overflowing = |this: &Self| {
            let over = this.tab_scroll.max_offset().x;
            this.buffers.len() > 1
                && if this.tab_overflow {
                    over > px(OVERFLOW_BTN_W)
                } else {
                    over > px(0.5)
                }
        };
        self.tab_overflow = overflowing(self);
        if n > 0 {
            // Layout changes (a new tab, a resize) can start or end overflow.
            // Deferred: runs after this frame's layout updated the scroll handle.
            cx.defer_in(window, move |this, _, cx| {
                if overflowing(this) != this.tab_overflow {
                    cx.notify();
                }
                // Not fully visible after a scroll request (the strip's
                // width changed meanwhile, e.g. the button appeared): again.
                if this.scroll_retries > 0 {
                    this.scroll_retries -= 1;
                    if this.tabs.active().is_some_and(|a| !this.tab_in_view(a)) {
                        this.scroll_active = true;
                        cx.notify();
                    }
                }
            });
        }
        if !cx.has_active_drag() {
            self.drop_slot = None;
        }
        let slot = self.drop_slot;
        let titles = self.tab_titles();
        let menu_for = self.tab_menu.as_ref().and_then(|m| m.target.clone());
        let list_open = self.tab_menu.as_ref().is_some_and(|m| m.target.is_none());
        let tabs: Vec<AnyElement> = self
            .buffers
            .iter()
            .zip(titles)
            .enumerate()
            .map(|(ix, (b, title))| {
                let active = self.tabs.active() == Some(ix);
                let preview = self.tabs.is_preview(&TabKey::of(&b.path));
                let name = file_name(&b.path);
                let has_err = self
                    .lsp
                    .read(cx)
                    .diagnostics_for(&b.path)
                    .iter()
                    .any(|d| d.severity == Some(lt::DiagnosticSeverity::ERROR));
                let disk = match b.disk {
                    DiskState::Same => None,
                    DiskState::Changed(_) => Some(txt::TabDisk::Changed),
                    DiskState::Deleted => Some(txt::TabDisk::Deleted),
                };
                let label = txt::tab_label_full(
                    &name,
                    &b.rel,
                    txt::TabFlags {
                        dirty: b.dirty,
                        preview,
                        disk,
                    },
                );
                let full = b.path.to_string_lossy().into_owned();
                let parent = title
                    .strip_prefix(name.as_str())
                    .and_then(|r| r.strip_prefix(" · "))
                    .map(str::to_string);
                let drag = DraggedTab {
                    path: b.path.clone(),
                    title: title.clone(),
                };
                let menu_open = menu_for
                    .as_ref()
                    .is_some_and(|p| path_key(p) == path_key(&b.path));
                div()
                    .id(("tab", ix))
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_description(full.clone())
                    .aria_selected(active)
                    .aria_expanded(menu_open)
                    .relative()
                    .h(px(TAB_STRIP_H))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .border_r_1()
                    .border_color(tc(t.border_subtle))
                    .when(active, |d| {
                        d.bg(tc(t.bg_surface))
                            .text_color(tc(t.text_primary))
                            .border_b_2()
                            .border_color(tc(t.accent_fg))
                    })
                    .when(!active, |d| {
                        d.text_color(tc(t.text_secondary))
                            .hover(|d| d.bg(tc(t.bg_hover)))
                    })
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            this.close_tab(ix, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_tab_menu_at(ix, e.position, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        if ix >= this.buffers.len() {
                            return;
                        }
                        this.activate(ix, window, cx);
                        if e.click_count() > 1 {
                            this.pin_active(cx);
                        }
                        this.focus_active(window, cx);
                    }))
                    .on_drag(drag, move |d: &DraggedTab, _, _, cx| {
                        cx.new(|_| TabDragPreview {
                            title: d.title.clone(),
                            mode,
                        })
                    })
                    // Left half of a tab: insert before it; right half: after it.
                    .on_drag_move(
                        cx.listener(move |this, e: &DragMoveEvent<DraggedTab>, _, cx| {
                            if e.bounds.contains(&e.event.position) {
                                let s = if e.event.position.x < e.bounds.center().x {
                                    ix
                                } else {
                                    ix + 1
                                };
                                if this.drop_slot != Some(s) {
                                    this.drop_slot = Some(s);
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                        let s = this.drop_slot.unwrap_or(ix);
                        this.drop_tab(&d.path, s, cx);
                    }))
                    // Drop indicator: a bar on the insertion edge.
                    .when(slot == Some(ix), |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(2.))
                                .bg(tc(t.accent_fg)),
                        )
                    })
                    .when(slot == Some(n) && ix + 1 == n, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(2.))
                                .bg(tc(t.accent_fg)),
                        )
                    })
                    .when_some(disk, |d, k| {
                        let col = if k == txt::TabDisk::Deleted {
                            t.danger_fg
                        } else {
                            t.warning_fg
                        };
                        d.child(icon(Icon::TriangleAlert, col).size(px(12.)))
                    })
                    .child(
                        div()
                            .when(preview, |d| d.italic())
                            .when(has_err, |d| d.text_color(tc(t.danger_fg)))
                            .child(name.clone()),
                    )
                    .when_some(parent, |d, p| {
                        d.child(
                            div()
                                .max_w(px(180.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(tc(t.text_muted))
                                .child(p),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-close", ix))
                            .role(Role::Button)
                            .aria_label(format!("{} {name}", txt::CLOSE_TAB))
                            .w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(tc(t.text_muted))
                            .hover(|d| d.bg(tc(t.bg_pressed)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, window, cx);
                            }))
                            .child(if b.dirty { "●" } else { "×" }),
                    )
                    .into_any_element()
            })
            .collect();
        // Space after the last tab: dropping there moves a tab to the end.
        let tail = div()
            .id("pod-editor-tabs-tail")
            .flex_1()
            .min_w(px(24.))
            .h_full()
            .on_drag_move(
                cx.listener(move |this, e: &DragMoveEvent<DraggedTab>, _, cx| {
                    if e.bounds.contains(&e.event.position) && this.drop_slot != Some(n) {
                        this.drop_slot = Some(n);
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| this.drop_tab(&d.path, n, cx)));
        let mut chevron = icon(Icon::ChevronRight, t.text_secondary).size(px(14.));
        chevron = chevron
            .with_transformation(Transformation::rotate(radians(std::f32::consts::FRAC_PI_2)));
        div()
            .h(px(TAB_STRIP_H))
            .flex_none()
            .flex()
            .bg(tc(t.bg_chrome))
            .border_b_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .id("pod-editor-tabs")
                    .role(Role::TabList)
                    .aria_label(txt::TABS_LABEL)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_end()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs)
                    .child(tail),
            )
            .when(self.tab_overflow, |d| {
                d.child(
                    div()
                        .id("pod-editor-tabs-overflow")
                        .role(Role::Button)
                        .aria_label(txt::OPEN_TABS_BUTTON)
                        .aria_expanded(list_open)
                        .tab_index(0)
                        .focus_visible(move |st| pod_ui::focus_style(st, mode))
                        .flex_none()
                        .w(px(OVERFLOW_BTN_W))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_l_1()
                        .border_color(tc(t.border_subtle))
                        .cursor_pointer()
                        .hover(|d| d.bg(tc(t.bg_hover)))
                        .tooltip(|window, cx| Tooltip::new(txt::OPEN_TABS_BUTTON).build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.open_tab_list(window, cx)))
                        .child(chevron),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_tab_menu(
        &mut self,
        t: Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = self.mode;
        let m = self.tab_menu.as_ref()?;
        let keys = cx.listener(|this, e: &KeyDownEvent, w, cx| {
            let k = e.keystroke.key.as_str();
            let handled = match k {
                "escape" | "tab" => {
                    this.close_menu(w, cx);
                    true
                }
                "up" => {
                    this.menu_move(-1, cx);
                    true
                }
                "down" => {
                    this.menu_move(1, cx);
                    true
                }
                "home" => {
                    this.menu_edge(false, cx);
                    true
                }
                "end" => {
                    this.menu_edge(true, cx);
                    true
                }
                "enter" | "space" => {
                    this.menu_activate(w, cx);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
            }
        });
        let mut rows: Vec<AnyElement> = Vec::with_capacity(m.items.len() * 2);
        for (i, it) in m.items.iter().enumerate() {
            if it.separator_before && i > 0 {
                rows.push(
                    div()
                        .my(px(4.))
                        .h(px(1.))
                        .bg(tc(t.border_subtle))
                        .into_any_element(),
                );
            }
            let sel = i == m.selected;
            let enabled = it.enabled;
            rows.push(
                div()
                    .id(("tab-menu-item", i))
                    .role(Role::MenuItem)
                    .aria_label(it.name.clone())
                    .when_some(it.shortcut, |d, s| d.aria_keyshortcuts(s))
                    .when(sel, |d| d.aria_active_descendant().bg(tc(t.bg_selected)))
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.))
                    .text_size(px(13.))
                    .whitespace_nowrap()
                    .text_color(tc(if enabled {
                        t.text_primary
                    } else {
                        t.text_muted
                    }))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .when(!sel, |d| d.hover(|d| d.bg(tc(t.bg_hover))))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                if let Some(m) = &mut this.tab_menu {
                                    m.selected = i;
                                }
                                this.menu_activate(w, cx);
                            }))
                    })
                    .child(it.label.clone())
                    .when_some(it.shortcut, |d, s| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(tc(t.text_muted))
                                .child(s),
                        )
                    })
                    .into_any_element(),
            );
        }
        let menu = div()
            .id("tab-menu")
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label(m.title.clone())
            .capture_key_down(keys)
            .on_mouse_down_out(cx.listener(|this, _, w, cx| this.close_menu(w, cx)))
            .min_w(px(220.))
            .max_w(px(480.))
            .max_h(px(400.))
            .overflow_y_scroll()
            .py(px(4.))
            .flex()
            .flex_col()
            .bg(tc(t.bg_raised))
            .border_1()
            .border_color(tc(t.border_subtle))
            .rounded(px(8.))
            .shadow(pod_ui::tokens::elevation(2, mode))
            .children(rows);
        Some(
            deferred(
                anchored()
                    .position(m.position)
                    .anchor(m.anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
