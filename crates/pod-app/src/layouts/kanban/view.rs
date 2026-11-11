//! The board's render: header with filters, three columns of cards, the
//! terminal drawer, and the shell around the board. The model decides; this
//! file draws and hands events to the board's controller methods.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::GateDecision;

use super::model::{
    CARD_GAP, CardAction, CardButton, Stripe, card_buttons, drawer_height, shown_line, stripe,
};
use super::{
    Board, BoardModel, Card, Column, KEY_CONTEXT, SwitchRegion, card_name, copy as s,
    filter_choices,
};
use crate::layouts::kit::{BodyFlow, ConnectionShell, LayoutMetrics, ToastAnchor, color};
use crate::qa_marks::QaId;

/// The window body: the board under the connection warning and the hook
/// prompt, toasts top right over it.
pub(super) fn shell(
    warning: Option<String>,
    hook_banner: Option<AnyElement>,
    board: AnyElement,
    toasts: AnyElement,
) -> AnyElement {
    ConnectionShell {
        warning,
        hook_banner,
        canvas: Some(crate::theme::tokens().bg_canvas),
        flow: BodyFlow::Column,
        body: board,
        toasts,
        anchor: ToastAnchor::TOP_RIGHT,
    }
    .render()
    .into_any_element()
}

/// Payload of a card drag.
#[derive(Clone)]
pub(super) struct DragCard {
    pub(super) id: String,
    column: Column,
    title: SharedString,
}

struct DragGhost(SharedString);
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        div()
            .px_3()
            .py_2()
            .rounded(px(m.radius.card))
            .bg(color(t.bg_raised))
            .border_2()
            .border_color(color(t.accent_fg))
            .text_color(color(t.text_primary))
            .shadow(pod_ui::tokens::elevation(2, crate::theme::mode()))
            .child(self.0.clone())
    }
}

/// A small text button for card and drawer actions (Enter/Space, Korean name).
fn action_button(
    id: impl Into<ElementId>,
    label: &'static str,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    enabled: bool,
    tip: Option<&'static str>,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let key = on.clone();
    let (fg, bg, border) = if !enabled {
        (t.text_disabled, t.bg_surface, t.border_subtle)
    } else if primary {
        (
            t.action_primary_fg,
            t.action_primary_bg,
            t.action_primary_bg,
        )
    } else {
        (t.text_primary, t.bg_surface, t.border_control)
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .when(enabled, |d| d.tab_index(0))
        .h(px(pod_ui::hit_area()))
        .px(px(10.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(color(border))
        .bg(color(bg))
        .text_color(color(fg))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(icon, |d, i| d.child(pod_ui::icon_sized(i, 14., fg)))
        .child(label)
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| {
                    if primary {
                        s.bg(color(t.action_primary_hover))
                    } else {
                        s.bg(color(t.bg_hover))
                    }
                })
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on(window, cx);
                })
                .on_key_down(move |e, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        key(window, cx);
                    }
                })
        })
        .when_some(tip, |d, tip| {
            d.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
        })
}

/// A UI callback shared by click and key handlers.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// What one card element needs besides the card.
struct CardCtx {
    board: WeakEntity<Board>,
    selected: Option<String>,
    opened: Option<String>,
    pinned: HashSet<String>,
    lines: HashMap<String, Option<String>>,
    focus: HashMap<String, FocusHandle>,
    height: f32,
    gap: f32,
    mono: SharedString,
}

/// The button element of one card action.
fn card_button(b: CardButton, card: &str, board: &WeakEntity<Board>) -> Stateful<Div> {
    let id = SharedString::from(b.id);
    let open = |board: WeakEntity<Board>, card: String| {
        move |window: &mut Window, cx: &mut App| {
            let _ = board.update(cx, |b, cx| b.open(&card, true, window, cx));
        }
    };
    let (board, card) = (board.clone(), card.to_owned());
    match b.action {
        CardAction::Decide {
            gate,
            approve,
            choice,
            label,
        } => {
            let decision = if approve {
                GateDecision::Approve
            } else {
                GateDecision::Reject
            };
            let text: &'static str = if choice.is_some() {
                ""
            } else if approve {
                s::APPROVE
            } else {
                s::REJECT
            };
            let option = choice.is_some();
            let button = action_button(
                id,
                text,
                Some(if approve {
                    pod_ui::Icon::Check
                } else {
                    pod_ui::Icon::X
                }),
                approve,
                true,
                None,
                move |window, cx| {
                    let _ = board.update(cx, |b, cx| {
                        b.resolve(
                            card.clone(),
                            gate.clone(),
                            decision,
                            choice.clone(),
                            window,
                            cx,
                        )
                    });
                },
            );
            if option {
                button.aria_label(s::approve_option(&label)).child(label)
            } else {
                button
            }
        }
        CardAction::Answer => action_button(
            id,
            s::ANSWER,
            Some(pod_ui::Icon::SquareTerminal),
            false,
            true,
            Some(s::NO_GATE_TIP),
            open(board, card),
        ),
        CardAction::Log => action_button(
            id,
            s::LOG,
            Some(pod_ui::Icon::SquareTerminal),
            false,
            true,
            None,
            open(board, card),
        ),
        CardAction::Rerun => action_button(
            id,
            s::RERUN,
            Some(pod_ui::Icon::RotateCcw),
            false,
            true,
            None,
            move |_, cx| {
                let _ = board.update(cx, |b, cx| b.rerun(&card, cx));
            },
        ),
    }
}

fn render_card(c: &Card, ctx: &CardCtx) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    use pod_ui::themes::Surface;
    let selected = ctx.selected.as_deref() == Some(c.id.as_str());
    let opened = ctx.opened.as_deref() == Some(c.id.as_str());
    let pinned = ctx.pinned.contains(&c.id);
    let line = ctx.lines.get(&c.id).cloned().flatten();
    let name = card_name(c, line.as_deref(), pinned);
    let shown_line = shown_line(c, line);
    let (glyph, glyph_fg) = if c.failed {
        (pod_ui::Icon::OctagonX, t.danger_fg)
    } else {
        let b = pod_ui::StatusBadge::fixture(match c.column {
            Column::Working => pod_status::AgentState::Working,
            Column::NeedsYou => pod_status::AgentState::Blocked(pod_status::BlockedKind::Other),
            Column::Done => pod_status::AgentState::Done,
        });
        b.glyph(mode)
    };
    let stripe = stripe(c).map(|s| match s {
        Stripe::Danger => t.danger_fg,
        Stripe::Warning => t.warning_fg,
    });
    let card_bg = match m.surface {
        Surface::Flat => t.bg_surface,
        _ => t.bg_raised,
    };
    let border = if selected {
        t.accent_fg
    } else if m.surface == Surface::Mono {
        card_bg
    } else {
        t.border_subtle
    };
    let focus = ctx.focus.get(&c.id).cloned();
    let board = ctx.board.clone();
    let id = c.id.clone();
    let drag = DragCard {
        id: c.id.clone(),
        column: c.column,
        title: c.title.clone().into(),
    };
    let column = c.column;

    // Actions (only the selected card's are tab stops: one card at a time in the tab order).
    let mut actions = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .overflow_hidden();
    let tab = |d: Stateful<Div>| if selected { d } else { d.tab_stop(false) };
    for b in card_buttons(c) {
        actions = actions.child(tab(card_button(b, &id, &board)));
    }

    let mono = ctx.mono.clone();
    let card = div()
        .id(SharedString::from(format!("kanban-card-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("kanban-card-{id}")
        })
        .when_some(focus, |d, f| d.track_focus(&f.tab_stop(selected)))
        .role(Role::ListItem)
        .aria_label(name)
        .aria_selected(selected)
        .h(px(ctx.height - ctx.gap))
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(12.))
        .when(stripe.is_some(), |d| d.pl(px(14.)))
        .rounded(px(m.radius.card))
        .bg(color(if selected { t.bg_selected } else { card_bg }))
        .border(px(if selected {
            m.border_width.max(2.)
        } else {
            m.border_width
        }))
        .border_color(color(border))
        // Status stripe (응답 필요: warning, 실패: danger); never the only carrier: icon + status text too.
        .when_some(stripe, |d, s| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(4.))
                    .rounded_l(px(m.radius.card))
                    .bg(color(s)),
            )
        })
        .relative()
        .when(
            m.shadow != pod_ui::themes::Shadow::None && m.surface != Surface::Flat,
            |d| d.shadow(pod_ui::tokens::elevation(1, mode)),
        )
        .cursor_pointer()
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .hover(move |s| {
            if selected {
                s
            } else {
                s.border_color(color(t.border_strong))
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(pod_ui::icon(glyph, glyph_fg))
                .child(crate::agent_kind::mark_el(
                    &format!("kanban-{id}"),
                    &c.kind,
                    pod_ui::tokens::ICON_SIZE,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(t.text_primary))
                        .child(c.title.clone()),
                )
                .when(pinned, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .px_1()
                            .rounded(px(m.radius.control))
                            .bg(color(t.bg_hover))
                            .text_color(color(t.text_secondary))
                            .child(s::PINNED),
                    )
                })
                .when(opened, |d| {
                    d.child(pod_ui::icon_sized(
                        pod_ui::Icon::SquareTerminal,
                        14.,
                        t.accent_fg,
                    ))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("kanban-grip-{id}")))
                        .flex_shrink_0()
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::ChevronsUpDown,
                            14.,
                            t.text_muted,
                        ))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(s::DRAG_TIP)
                                .build(window, cx)
                        }),
                ),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(color(t.text_secondary))
                .child(c.task_title.clone()),
        )
        .child(
            div()
                .min_w_0()
                .px_2()
                .py(px(4.))
                .rounded(px(m.radius.control))
                .bg(color(t.bg_canvas))
                .font_family(mono)
                .text_color(color(t.text_secondary))
                .truncate()
                .child(shown_line),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(actions)
                .child(div().flex_1())
                .child(
                    div()
                        .qa_id({
                            let id = id.clone();
                            move || format!("kanban-status-{id}")
                        })
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(color(if c.failed { t.danger_fg } else { t.text_muted }))
                        .child(c.status.clone()),
                ),
        )
        .on_click({
            let (board, id) = (board.clone(), id.clone());
            move |_, window, cx| {
                let _ = board.update(cx, |b, cx| b.open(&id, false, window, cx));
                let _ = board.update(cx, |b, cx| b.select(&id, window, cx));
            }
        })
        .on_key_down({
            let (board, id) = (board.clone(), id.clone());
            move |e, window, cx| {
                if board
                    .update(cx, |b, cx| b.on_card_key(&id, e, window, cx))
                    .unwrap_or(false)
                {
                    cx.stop_propagation();
                }
            }
        })
        .on_drag(drag, |d, _, _, cx| cx.new(|_| DragGhost(d.title.clone())))
        .can_drop(move |v, _, _| {
            v.downcast_ref::<DragCard>()
                .is_some_and(|d| d.column == column)
        })
        .drag_over::<DragCard>(move |s, d, _, _| {
            if d.column == column {
                s.border_color(color(t.accent_fg))
            } else {
                s
            }
        })
        .on_drop::<DragCard>({
            let (board, id) = (board.clone(), id.clone());
            move |d, _, cx| {
                let _ = board.update(cx, |b, cx| b.drop_card(d, Some(&id), None, cx));
            }
        });
    div()
        .h(px(ctx.height))
        .w_full()
        .pb(px(ctx.gap))
        .child(card)
        .into_any_element()
}

impl Render for Board {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(frame) = self.sync(window, cx) else {
            return div().into_any_element();
        };
        let super::Synced {
            cards,
            cols,
            task_count,
            dock,
        } = frame;

        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let gap = m.gap.max(8.);
        let height = Board::card_height(cx) + CARD_GAP;
        let ctx = Rc::new(CardCtx {
            board: cx.entity().downgrade(),
            selected: self.model.selected.clone(),
            opened: self.model.opened.clone(),
            pinned: self.model.pinned.clone(),
            lines: self
                .lines
                .iter()
                .map(|(k, (_, l))| (k.clone(), l.clone()))
                .collect(),
            focus: self.card_focus.clone(),
            height,
            gap: CARD_GAP,
            mono: gpui_kit::component::theme::Theme::global(cx)
                .mono_font_family
                .clone(),
        });
        let me = cx.entity().downgrade();

        // ── header: title, counts, hint, filters ──
        let (repos, agents) = filter_choices(&cards);
        let filtered = self.model.repo.is_some() || self.model.agent.is_some();
        let filter_menu = |id: &'static str,
                           what: &'static str,
                           all: &'static str,
                           current: Option<String>,
                           choices: Vec<String>,
                           set: fn(&mut BoardModel, Option<String>)| {
            let me = me.clone();
            let text = s::filter(what, current.as_deref().unwrap_or(all));
            Button::new(id)
                .label(text.clone())
                .accessibility_label(text)
                .small()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let pick = |menu: PopupMenu,
                                label: String,
                                value: Option<String>,
                                me: WeakEntity<Board>| {
                        menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = me.update(cx, |b, cx| {
                                set(&mut b.model, value.clone());
                                b.notify_all(cx);
                            });
                        }))
                    };
                    let mut menu = pick(menu, all.to_string(), None, me.clone());
                    for c in &choices {
                        menu = pick(menu, c.clone(), Some(c.clone()), me.clone());
                    }
                    menu
                })
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px(px(gap + 4.))
            .pt(px(gap))
            .min_h(px(m.density.panel_header.max(36.)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::ALL_WORK),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(color(t.text_muted))
                    .child(s::counts(cards.len(), task_count)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(color(t.text_muted))
                    .child(s::HINT),
            )
            .child(filter_menu(
                "kanban-filter-repo",
                s::REPO,
                s::ALL_REPOS,
                self.model.repo.clone(),
                repos,
                |m, v| m.repo = v,
            ))
            .child(filter_menu(
                "kanban-filter-agent",
                s::AGENT,
                s::ALL_AGENTS,
                self.model.agent.clone(),
                agents,
                |m, v| m.agent = v,
            ))
            .when(filtered, |d| {
                let me = me.clone();
                d.child(action_button(
                    "kanban-filter-clear",
                    s::CLEAR_FILTERS,
                    Some(pod_ui::Icon::X),
                    false,
                    true,
                    None,
                    move |_, cx| {
                        let _ = me.update(cx, |b, cx| {
                            b.model.repo = None;
                            b.model.agent = None;
                            b.notify_all(cx);
                        });
                    },
                ))
            })
            .when(!self.drawer_open, |d| {
                let me = me.clone();
                d.child(
                    pod_ui::IconButton::new(
                        "kanban-drawer-open",
                        pod_ui::Icon::Rows2,
                        s::DRAWER_OPEN,
                    )
                    .render(mode, move |window, cx| {
                        let _ = me.update(cx, |b, cx| {
                            b.drawer_open = true;
                            b.switch_region(window, cx);
                        });
                    }),
                )
            });

        // ── columns ──
        let column_bg = match m.surface {
            Surface::Flat => t.bg_canvas,
            _ => t.bg_surface,
        };
        let mut row = div().flex().flex_1().min_h_0().gap(px(gap)).p(px(gap));
        for (ci, col) in cols.iter().enumerate() {
            let column = Column::ALL[ci];
            let n = col.len();
            let list_cards = Rc::new(col.clone());
            let wanted_board = me.clone();
            let ctx = ctx.clone();
            let list = if col.is_empty() {
                div()
                    .flex_1()
                    .flex()
                    .items_start()
                    .justify_center()
                    .pt_4()
                    .text_color(color(t.text_muted))
                    .child(s::NO_CARDS)
                    .into_any_element()
            } else {
                uniform_list(
                    SharedString::from(format!("kanban-list-{ci}")),
                    n,
                    move |range, _, cx| {
                        let wanted: Vec<(String, String)> = list_cards[range.clone()]
                            .iter()
                            .map(|c| (c.id.clone(), c.line_key.clone()))
                            .collect();
                        let _ = wanted_board.update(cx, |b, cx| b.want_lines(wanted, cx));
                        list_cards[range]
                            .iter()
                            .map(|c| render_card(c, &ctx))
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll[ci])
                .flex_1()
                .min_h_0()
                .px(px(8.))
                .into_any_element()
            };
            let header_board = me.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("kanban-column-{ci}")))
                    .qa_id(move || format!("kanban-column-{ci}"))
                    .role(Role::List)
                    .aria_label(s::column_name(column.label(), n))
                    .flex_1()
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .rounded(px(m.radius.card))
                    .bg(color(column_bg))
                    .when(
                        m.surface == Surface::Flat || m.surface == Surface::Islands,
                        |d| {
                            d.border(px(m.border_width))
                                .border_color(color(t.border_subtle))
                        },
                    )
                    .when(m.surface == Surface::Islands, |d| {
                        d.shadow(pod_ui::tokens::elevation(1, mode))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("kanban-column-head-{ci}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .min_h(px(m.density.panel_header.max(36.)))
                            .flex_shrink_0()
                            .child(pod_ui::icon(column.icon(), column.fg(&t)))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(column.label()),
                            )
                            .child(div().text_color(color(t.text_muted)).child(n.to_string()))
                            .can_drop(move |v, _, _| {
                                v.downcast_ref::<DragCard>()
                                    .is_some_and(|d| d.column == column)
                            })
                            .drag_over::<DragCard>(move |s, d, _, _| {
                                if d.column == column {
                                    s.bg(color(t.bg_hover))
                                } else {
                                    s
                                }
                            })
                            .on_drop::<DragCard>(move |d, _, cx| {
                                let _ = header_board
                                    .update(cx, |b, cx| b.drop_card(d, None, Some(column), cx));
                            }),
                    )
                    .child(list),
            );
        }

        // ── drawer ──
        let drawer = self.drawer_open.then(|| {
            let opened = self.model.opened.clone();
            let card = opened
                .as_ref()
                .and_then(|id| cards.iter().find(|c| &c.id == id))
                .cloned();
            let panel = if self.show_dock {
                None
            } else {
                self.drawer_panel(window, cx)
            };
            let h = drawer_height(
                panel.is_none() && !self.show_dock,
                self.drawer_max,
                m.density.panel_header,
                LayoutMetrics::current(cx).ui_font,
                f32::from(window.viewport_size().height),
            );
            let (b1, b2, b3, b4) = (me.clone(), me.clone(), me.clone(), me.clone());
            let seg = |id: &'static str, label: &'static str, on: bool, f: Action| {
                let key = f.clone();
                div()
                    .id(id)
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_selected(on)
                    .tab_index(0)
                    .px_2()
                    .h(px(pod_ui::hit_area()))
                    .flex()
                    .items_center()
                    .rounded(px(m.radius.control))
                    .text_color(color(if on { t.text_primary } else { t.text_secondary }))
                    .when(on, |d| {
                        d.bg(color(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                    })
                    .hover(|s| s.bg(color(t.bg_hover)))
                    .focus_visible(move |s| pod_ui::focus_style(s, mode))
                    .cursor_pointer()
                    .child(label)
                    .on_click(move |_, window, cx| f(window, cx))
                    .on_key_down(move |e, window, cx| {
                        if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            key(window, cx);
                        }
                    })
            };
            div()
                .id("kanban-drawer")
                .qa_id(|| "kanban-drawer".into())
                .role(Role::Region)
                .aria_label(s::DRAWER)
                .h(px(h))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .bg(color(t.bg_surface))
                .border_t(px(m.border_width))
                .border_color(color(t.border_subtle))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(m.density.panel_header.max(32.)))
                        .flex_shrink_0()
                        .border_b(px(m.border_width))
                        .border_color(color(t.border_subtle))
                        .when_some(card.clone().filter(|_| !self.show_dock), |d, c| {
                            d.child(pod_ui::icon(
                                c.column.icon(),
                                if c.failed {
                                    t.danger_fg
                                } else {
                                    c.column.fg(&t)
                                },
                            ))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(c.title.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(color(t.text_secondary))
                                    .child(c.task_title.clone()),
                            )
                        })
                        .when(card.is_none() || self.show_dock, |d| {
                            d.child(div().font_weight(FontWeight::SEMIBOLD).child(s::DRAWER))
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("kanban-drawer-tabs")
                                .flex()
                                .items_center()
                                .gap_1()
                                .role(Role::TabList)
                                .child(seg(
                                    "kanban-drawer-terminal",
                                    s::DRAWER_TERMINAL,
                                    !self.show_dock,
                                    Rc::new(move |window, cx| {
                                        let _ = b1.update(cx, |b, cx| {
                                            b.show_dock = false;
                                            b.switch_region(window, cx);
                                        });
                                    }),
                                ))
                                .child(seg(
                                    "kanban-drawer-panels",
                                    s::DRAWER_PANELS,
                                    self.show_dock,
                                    Rc::new(move |_, cx| {
                                        let _ = b2.update(cx, |b, cx| {
                                            b.show_dock = true;
                                            b.notify_all(cx);
                                        });
                                    }),
                                )),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "kanban-drawer-size",
                                if self.drawer_max {
                                    pod_ui::Icon::Minimize2
                                } else {
                                    pod_ui::Icon::Maximize2
                                },
                                if self.drawer_max {
                                    s::DRAWER_SHRINK
                                } else {
                                    s::DRAWER_GROW
                                },
                            )
                            .render(mode, move |_, cx| {
                                let _ = b3.update(cx, |b, cx| {
                                    b.drawer_max = !b.drawer_max;
                                    b.notify_all(cx);
                                });
                            }),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "kanban-drawer-close",
                                pod_ui::Icon::X,
                                s::DRAWER_CLOSE,
                            )
                            .render(mode, move |window, cx| {
                                let _ = b4.update(cx, |b, cx| {
                                    b.drawer_open = false;
                                    b.switch_region(window, cx);
                                });
                            }),
                        ),
                )
                .child(match (self.show_dock, panel) {
                    (true, _) => div().flex_1().min_h_0().child(dock).into_any_element(),
                    (false, Some(p)) => div().flex_1().min_h_0().child(p).into_any_element(),
                    (false, None) => div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(color(t.text_muted))
                        .child(s::DRAWER_EMPTY)
                        .into_any_element(),
                })
        });

        div()
            .id("kanban-board")
            .qa_id(|| "kanban-board".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|b, _: &SwitchRegion, window, cx| b.switch_region(window, cx)))
            .on_key_down(
                cx.listener(|b, e: &KeyDownEvent, window, cx| b.on_board_key(e, window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .text_color(color(t.text_primary))
            .child(header)
            .child(row)
            .when_some(drawer, |d, dr| d.child(dr))
            .into_any_element()
    }
}
