//! Drawing: the settings category (profiles, TM-13 statement), the machine
//! board in the sidebar, and the dialogs (profile form, removal, trust).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use pod_remote::Machine;

use super::board::MachineBoard;
use super::copy;
use super::form::{Field, FormError, ProfileForm};
use super::model::{LinkState, MachineVm, RemoteRef};
use super::terminal::clock_text;
use super::trust::{Confirm, Trust};
use crate::qa_marks::QaId;
use crate::theme::{mode, tokens};

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(tokens().text_muted))
        .child(text.into())
}

fn danger(text: impl Into<SharedString>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(tokens().danger_fg))
        .child(text.into())
}

/// `key: …, known_hosts: …` of a profile (paths shown as stored).
fn extras(m: &Machine) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(k) = &m.identity_file {
        parts.push(format!("키 {}", k.display()));
    }
    if let Some(k) = &m.known_hosts {
        parts.push(format!("known_hosts {}", k.display()));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn dot_color(vm: &MachineVm) -> u32 {
    let t = tokens();
    match &vm.link {
        LinkState::Connected if vm.needs_confirm() => t.warning_fg,
        LinkState::Connected => t.success_fg,
        LinkState::Connecting | LinkState::Lost { .. } => t.warning_fg,
        LinkState::Failed(_) => t.danger_fg,
        LinkState::Off => t.text_muted,
    }
}

// ───────────────────────────── settings category ─────────────────────────────

/// The 원격 머신 settings category: the full-rights statement, the profiles
/// and their add/edit/remove actions.
pub(crate) fn settings_section(board: Option<Entity<MachineBoard>>, cx: &mut App) -> Div {
    let m = mode();
    let t = tokens();
    let mut col = div()
        .id("machines-settings")
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(copy::SECTION_TITLE),
        )
        .child(
            div()
                .id("machines-full-rights")
                .qa_id(|| "machines-full-rights".into())
                .role(Role::Note)
                .aria_label(SharedString::from(format!(
                    "{} — {}",
                    copy::FULL_RIGHTS_TITLE,
                    copy::FULL_RIGHTS_DETAIL
                )))
                .child(
                    pod_ui::banner(
                        pod_ui::Tone::Warning,
                        m,
                        copy::FULL_RIGHTS_TITLE,
                        Some(copy::FULL_RIGHTS_DETAIL.into()),
                    )
                    .text_xs(),
                ),
        )
        .child(muted(copy::ONLY_THESE_FIELDS));
    let Some(board) = board else {
        return div().child(col);
    };
    let weak = board.downgrade();
    let b = board.read(cx);
    if let Some(e) = &b.load_error {
        col = col.child(
            pod_ui::banner(
                pod_ui::Tone::Danger,
                m,
                copy::STORE_ERROR,
                Some(e.clone().into()),
            )
            .text_xs(),
        );
    }
    {
        let weak = weak.clone();
        col = col.child(
            div().flex().child(
                pod_ui::semantic_button(
                    "machine-add",
                    copy::ADD,
                    pod_ui::Tone::Info,
                    m,
                    if b.load_error.is_some() {
                        pod_ui::ControlState::Disabled
                    } else {
                        pod_ui::ControlState::Ready
                    },
                    move |window, cx| open_profile_dialog(weak.clone(), None, window, cx),
                )
                .qa_id(|| "machine-add".into()),
            ),
        );
    }
    let mut list = div()
        .id("machine-profiles")
        .role(Role::List)
        .aria_label(SharedString::from(copy::CATEGORY))
        .flex()
        .flex_col()
        .gap_2();
    if b.machines.is_empty() && b.load_error.is_none() {
        list = list.child(muted(copy::NO_PROFILES));
    }
    for (i, vm) in b.machines.iter().enumerate() {
        let p = vm.profile.clone();
        let (w_edit, w_rm) = (weak.clone(), weak.clone());
        let (p_edit, name_rm, target_rm) = (p.clone(), p.name.clone(), p.target());
        let accessible = format!("{} · {} · {}", p.name, p.target(), vm.state_label());
        list = list.child(
            div()
                .id(SharedString::from(format!("machine-profile-{i}")))
                .qa_id(|| format!("machine-profile-{i}"))
                .role(Role::ListItem)
                .aria_label(SharedString::from(accessible))
                .flex()
                .items_center()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(t.border_subtle))
                .child(pod_ui::icon_sized(pod_ui::Icon::Server, 16., t.text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(p.name.clone()),
                                )
                                .child(pod_ui::status_dot(dot_color(vm)))
                                .child(muted(vm.state_label())),
                        )
                        .child(div().text_xs().truncate().child(p.target()))
                        .when_some(extras(&p), |d, x| d.child(muted(x).truncate())),
                )
                .child(
                    pod_ui::IconButton::new(
                        SharedString::from(format!("machine-edit-{i}")),
                        pod_ui::Icon::Pencil,
                        format!("{} {}", p.name, copy::EDIT),
                    )
                    .render(m, move |window, cx| {
                        open_profile_dialog(w_edit.clone(), Some(p_edit.clone()), window, cx)
                    })
                    .qa_id(|| format!("machine-edit-{i}")),
                )
                .child(
                    pod_ui::IconButton::new(
                        SharedString::from(format!("machine-remove-{i}")),
                        pod_ui::Icon::Trash2,
                        format!("{} {}", p.name, copy::REMOVE),
                    )
                    .render(m, move |window, cx| {
                        open_remove_dialog(
                            w_rm.clone(),
                            name_rm.clone(),
                            target_rm.clone(),
                            window,
                            cx,
                        )
                    })
                    .qa_id(|| format!("machine-remove-{i}")),
                ),
        );
    }
    div().child(col.child(list))
}

// ───────────────────────────── dialogs ─────────────────────────────

/// Add (`original` None) or edit a profile. Saving validates with
/// pod-remote's rules; the first bad field is named under the form.
pub(crate) fn open_profile_dialog(
    board: WeakEntity<MachineBoard>,
    original: Option<Machine>,
    window: &mut Window,
    cx: &mut App,
) {
    let start = original
        .as_ref()
        .map(ProfileForm::from_machine)
        .unwrap_or_default();
    let input = |value: &str, ph: &'static str, window: &mut Window, cx: &mut App| {
        let v = value.to_string();
        cx.new(|cx| InputState::new(window, cx).placeholder(ph).default_value(v))
    };
    let fields: Rc<[(Field, &'static str, Entity<InputState>); 6]> = Rc::new([
        (
            Field::Name,
            copy::FIELD_NAME,
            input(&start.name, copy::PH_NAME, window, cx),
        ),
        (
            Field::Host,
            copy::FIELD_HOST,
            input(&start.host, copy::PH_HOST, window, cx),
        ),
        (
            Field::User,
            copy::FIELD_USER,
            input(&start.user, copy::PH_USER, window, cx),
        ),
        (
            Field::Port,
            copy::FIELD_PORT,
            input(&start.port, copy::PH_PORT, window, cx),
        ),
        (
            Field::IdentityFile,
            copy::FIELD_KEY,
            input(&start.identity_file, copy::PH_KEY, window, cx),
        ),
        (
            Field::KnownHosts,
            copy::FIELD_KNOWN_HOSTS,
            input(&start.known_hosts, copy::PH_KNOWN_HOSTS, window, cx),
        ),
    ]);
    let error: Rc<RefCell<Option<FormError>>> = Rc::default();
    let original_name = original.as_ref().map(|m| m.name.clone());
    let submit: crate::chrome::Submit = {
        let (fields, error, board) = (fields.clone(), error.clone(), board.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let v = |i: usize| fields[i].2.read(cx).value().to_string();
            let form = ProfileForm {
                name: v(0),
                host: v(1),
                user: v(2),
                port: v(3),
                identity_file: v(4),
                known_hosts: v(5),
            };
            let r = board
                .update(cx, |b, cx| {
                    b.save_profile(original_name.as_deref(), &form, cx)
                })
                .unwrap_or(Ok(()));
            match r {
                Ok(()) => {
                    error.borrow_mut().take();
                    window.close_dialog(cx);
                }
                Err(e) => {
                    *error.borrow_mut() = Some(e);
                    window.refresh();
                }
            }
        })
    };
    for (_, _, f) in fields.iter() {
        crate::chrome::submit_on_enter(f, submit.clone(), window, cx);
    }
    let title = if original.is_some() {
        copy::EDIT_TITLE
    } else {
        copy::ADD_TITLE
    };
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog: Dialog, window, cx| {
        if first.replace(false) {
            fields[0].2.focus_handle(cx).focus(window, cx);
        }
        let err = error.borrow().clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                pod_ui::banner(
                    pod_ui::Tone::Warning,
                    mode(),
                    copy::FULL_RIGHTS_TITLE,
                    Some(copy::FULL_RIGHTS_DETAIL.into()),
                )
                .text_xs(),
            )
            .child(muted(copy::ONLY_THESE_FIELDS));
        for (field, label, state) in fields.iter() {
            let bad = err.as_ref().filter(|e| e.field == *field);
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(muted(*label))
                    .child(Input::new(state))
                    .when_some(bad, |d, e| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("machine-form-error-{field:?}")))
                                .qa_id(|| format!("machine-form-error-{field:?}"))
                                .role(Role::Alert)
                                .aria_label(SharedString::from(e.message.clone()))
                                .child(danger(e.message.clone())),
                        )
                    }),
            );
        }
        let submit = submit.clone();
        dialog.title(title).w(px(520.)).child(body).footer(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("machine-form-cancel")
                        .label(copy::CANCEL)
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    div().flex().qa_id(|| "machine-form-save".into()).child(
                        Button::new("machine-form-save")
                            .label(copy::SAVE)
                            .primary()
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
                ),
        )
    });
}

pub(crate) fn open_remove_dialog(
    board: WeakEntity<MachineBoard>,
    name: String,
    target: String,
    window: &mut Window,
    cx: &mut App,
) {
    let body = copy::remove_body(&name, &target);
    window.open_dialog(cx, move |dialog: Dialog, _, _| {
        let (board, name) = (board.clone(), name.clone());
        dialog
            .title(copy::REMOVE_TITLE)
            .w(px(460.))
            .child(div().text_sm().child(body.clone()))
            .footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("machine-remove-cancel")
                            .label(copy::CANCEL)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        div().flex().qa_id(|| "machine-remove-ok".into()).child(
                            Button::new("machine-remove-ok")
                                .label(copy::REMOVE)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = board.update(cx, |b, cx| b.remove_profile(&name, cx));
                                }),
                        ),
                    ),
            )
    });
}

/// Rows of the trust dialog: (label, value).
pub(crate) fn trust_rows(vm: &MachineVm) -> (String, &'static str, Vec<(&'static str, String)>) {
    let id = vm.identity.as_ref();
    let fps = id
        .map(|i| match &i.fingerprints {
            Ok(v) if !v.is_empty() => v.join("\n"),
            _ => copy::TRUST_NO_FINGERPRINT.to_string(),
        })
        .unwrap_or_default();
    let mut rows = vec![
        (copy::TRUST_TARGET, vm.profile.target()),
        (copy::TRUST_FINGERPRINTS, fps),
        (
            copy::TRUST_MACHINE_ID,
            id.map(|i| i.machine_id.clone()).unwrap_or_default(),
        ),
    ];
    let (title, body) = match &vm.trust {
        Some(Trust::Needs(Confirm::HostKey { old })) => {
            rows.push((
                copy::TRUST_OLD_FINGERPRINTS,
                if old.is_empty() {
                    copy::TRUST_NO_FINGERPRINT.into()
                } else {
                    old.join("\n")
                },
            ));
            (copy::TRUST_HOST_KEY_TITLE, copy::TRUST_HOST_KEY_BODY)
        }
        Some(Trust::Needs(Confirm::Machine { old })) => {
            rows.push((copy::TRUST_OLD_MACHINE_ID, old.clone()));
            (copy::TRUST_MACHINE_TITLE, copy::TRUST_MACHINE_BODY)
        }
        Some(Trust::Needs(Confirm::Profile { old_target })) => {
            rows.push((copy::TRUST_OLD_TARGET, old_target.clone()));
            (copy::TRUST_PROFILE_TITLE, copy::TRUST_PROFILE_BODY)
        }
        _ => (copy::TRUST_FIRST_TITLE, copy::TRUST_FIRST_BODY),
    };
    (format!("{title}: {}", vm.profile.name), body, rows)
}

/// "이 머신을 신뢰할까요?": the identity this connection showed, what
/// changed, and the TM-13 statement. Only the button confirms; closing the
/// dialog leaves the machine connected read-only (확인 필요).
pub(crate) fn open_trust_dialog(
    board: WeakEntity<MachineBoard>,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(b) = board.upgrade() else {
        return;
    };
    let Some(vm) = b.read(cx).vm(&name).cloned() else {
        return;
    };
    let changed = !matches!(vm.trust, Some(Trust::Needs(Confirm::First)));
    let (title, body, rows) = trust_rows(&vm);
    let failed: Rc<RefCell<Option<String>>> = Rc::default();
    let mono: SharedString = cx
        .global::<crate::theme::Settings>()
        .terminal_font
        .clone()
        .into();
    window.open_dialog(cx, move |dialog: Dialog, _, _| {
        let t = tokens();
        let mut table = div()
            .id("machine-trust-identity")
            .qa_id(|| "machine-trust-identity".into())
            .role(Role::Group)
            .aria_label(SharedString::from(title.clone()))
            .flex()
            .flex_col()
            .gap_2();
        for (label, value) in &rows {
            table = table.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(muted(*label))
                    .child(
                        div()
                            .text_sm()
                            .font_family(mono.clone())
                            .text_color(rgb(t.text_primary))
                            .child(value.clone()),
                    ),
            );
        }
        let (board_ok, name_ok, failed_ok) = (board.clone(), name.clone(), failed.clone());
        let (board_no, name_no) = (board.clone(), name.clone());
        dialog
            .title(title.clone())
            .w(px(560.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        pod_ui::banner(
                            if changed {
                                pod_ui::Tone::Danger
                            } else {
                                pod_ui::Tone::Warning
                            },
                            mode(),
                            copy::FULL_RIGHTS_TITLE,
                            Some(copy::FULL_RIGHTS_DETAIL.into()),
                        )
                        .text_xs(),
                    )
                    .child(div().text_sm().child(body))
                    .child(table)
                    .when_some(failed.borrow().clone(), |d, e| {
                        d.child(danger(format!("{}: {e}", copy::TRUST_SAVE_FAILED)))
                    }),
            )
            .footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("machine-trust-cancel")
                            .label(copy::DISCONNECT)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = board_no.update(cx, |b, cx| b.disconnect(&name_no, cx));
                            }),
                    )
                    .child(div().flex().qa_id(|| "machine-trust-ok".into()).child({
                        let b = Button::new("machine-trust-ok").label(if changed {
                            copy::TRUST_ACCEPT_CHANGED
                        } else {
                            copy::TRUST_ACCEPT
                        });
                        let b = if changed { b.danger() } else { b.primary() };
                        b.on_click(move |_, window, cx| {
                            let r = board_ok
                                .update(cx, |b, cx| b.confirm(&name_ok, cx))
                                .unwrap_or(Ok(()));
                            match r {
                                Ok(()) => window.close_dialog(cx),
                                Err(e) => {
                                    *failed_ok.borrow_mut() = Some(e);
                                    window.refresh();
                                }
                            }
                        })
                    })),
            )
    });
}

// ───────────────────────────── sidebar board ─────────────────────────────

impl Render for MachineBoard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.machines.is_empty() && self.load_error.is_none() {
            return div().id("machine-board-empty");
        }
        let m = mode();
        let t = tokens();
        let me = cx.entity().downgrade();
        let mut board = div()
            .id("machine-board")
            .qa_id(|| "machine-board".into())
            .role(Role::Region)
            .aria_label(SharedString::from(copy::BOARD_TITLE))
            .flex_shrink_0()
            // At most 40 % of the sidebar: the local tree keeps the rest.
            .max_h(relative(0.4))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(rgb(t.border_subtle))
            .py_1()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(pod_ui::icon_sized(pod_ui::Icon::Server, 14., t.text_muted))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_secondary))
                            .child(copy::BOARD_TITLE),
                    ),
            );
        if let Some(e) = &self.load_error {
            board = board.child(
                div()
                    .px_3()
                    .child(danger(format!("{}: {e}", copy::STORE_ERROR))),
            );
        }
        for (i, vm) in self.machines.iter().enumerate() {
            board = board.child(machine_card(i, vm, &me, m));
        }
        board
    }
}

fn machine_card(
    i: usize,
    vm: &MachineVm,
    me: &WeakEntity<MachineBoard>,
    m: pod_ui::Mode,
) -> Stateful<Div> {
    let t = tokens();
    let name = vm.name().to_string();
    let action =
        |id: &str,
         icon: pod_ui::Icon,
         label: &str,
         f: fn(&mut MachineBoard, &str, &mut Window, &mut Context<MachineBoard>)| {
            let (me, name) = (me.clone(), name.clone());
            pod_ui::IconButton::new(
                SharedString::from(format!("{id}-{i}")),
                icon,
                format!("{} {label}", vm.name()),
            )
            .render(m, move |window, cx| {
                let _ = me.update(cx, |b, cx| f(b, &name, window, cx));
            })
            .qa_id(move || format!("{id}-{i}"))
        };
    let mut buttons = div().flex().flex_shrink_0().items_center().gap(px(2.));
    match &vm.link {
        LinkState::Off => {
            buttons = buttons.child(action(
                "machine-connect",
                pod_ui::Icon::Plug,
                copy::CONNECT,
                |b, n, _, cx| b.connect(n, cx),
            ));
        }
        LinkState::Failed(_) => {
            buttons = buttons.child(action(
                "machine-retry",
                pod_ui::Icon::RefreshCw,
                copy::RETRY,
                |b, n, _, cx| b.connect(n, cx),
            ));
        }
        _ => {
            if vm.needs_confirm() {
                buttons = buttons.child(action(
                    "machine-confirm",
                    pod_ui::Icon::ShieldQuestion,
                    copy::CONFIRM_BUTTON,
                    |b, n, window, cx| {
                        let weak = cx.entity().downgrade();
                        let name = n.to_string();
                        // After this update: the dialog reads the board.
                        window.defer(cx, move |window, cx| {
                            open_trust_dialog(weak, name, window, cx)
                        });
                        b.pending_confirm = None;
                    },
                ));
            }
            buttons = buttons.child(action(
                "machine-disconnect",
                pod_ui::Icon::Unplug,
                copy::DISCONNECT,
                |b, n, _, cx| b.disconnect(n, cx),
            ));
        }
    }
    let last = copy::last_received_at(vm.last_rx.map(clock_text).as_deref());
    let accessible = format!("{} · {} · {}", vm.name(), vm.state_label(), last);
    let mut card = div()
        .id(SharedString::from(format!("machine-card-{i}")))
        .qa_id(|| format!("machine-card-{i}"))
        .role(Role::Group)
        .aria_label(SharedString::from(accessible))
        .px_3()
        .py_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(pod_ui::status_dot(dot_color(vm)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.clone()),
                )
                .child(buttons),
        )
        .child(muted(format!("{} · {}", vm.state_label(), vm.profile.target())).truncate())
        .child(muted(last).truncate());
    if let LinkState::Failed(f) = &vm.link {
        card = card.child(
            div()
                .id(SharedString::from(format!("machine-failure-{i}")))
                .qa_id(|| format!("machine-failure-{i}"))
                .role(Role::Alert)
                .aria_label(SharedString::from(format!("{} {}", f.summary, f.hint)))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(danger(f.summary.clone()))
                .when(!f.hint.is_empty(), |d| d.child(muted(f.hint.clone())))
                .when_some(f.sent_fingerprint.clone(), |d, fp| {
                    d.child(muted(format!("{}: {fp}", copy::FAILURE_SENT_KEY)))
                        .child(muted(copy::FAILURE_NO_ACCEPT))
                })
                .when(!f.detail.is_empty(), |d| d.child(muted(f.detail.clone())))
                .children(
                    f.stderr
                        .iter()
                        .take(4)
                        .map(|l| muted(format!("{} {l}", copy::FAILURE_SSH)).truncate()),
                ),
        );
    }
    if let LinkState::Lost { reason, .. } = &vm.link
        && !reason.is_empty()
    {
        card = card.child(muted(reason.clone()).truncate());
    }
    if let Some(n) = &vm.notice {
        card = card.child(div().text_xs().text_color(rgb(t.info_fg)).child(n.clone()));
    }
    if matches!(vm.link, LinkState::Connected | LinkState::Lost { .. }) && vm.hello.is_some() {
        card = card.child(muted(copy::counts(vm.tasks.len(), vm.terminals.len())));
        if vm.terminals.is_empty() {
            card = card.child(muted(copy::NO_TERMINALS));
        }
        let mut rows = div()
            .id(SharedString::from(format!("machine-terminals-{i}")))
            .role(Role::List)
            .aria_label(SharedString::from(format!(
                "{} {}",
                vm.name(),
                copy::TERMINALS
            )))
            .flex()
            .flex_col();
        // Tasks with their terminals, then the terminals of no task.
        let mut order: Vec<(Option<&str>, Vec<usize>)> = vm
            .tasks
            .iter()
            .map(|k| {
                let terms = (0..vm.terminals.len())
                    .filter(|j| vm.terminals[*j].task.as_deref() == Some(k.id.as_str()))
                    .collect();
                (Some(k.id.as_str()), terms)
            })
            .collect();
        order.push((
            None,
            (0..vm.terminals.len())
                .filter(|j| {
                    vm.terminals[*j]
                        .task
                        .as_deref()
                        .is_none_or(|k| !vm.tasks.iter().any(|x| x.id == k))
                })
                .collect(),
        ));
        for (task, terms) in order {
            if let Some(k) = task {
                rows = rows.child(
                    div()
                        .h(px(22.))
                        .pl_4()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::GitBranch,
                            12.,
                            t.text_muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(t.text_secondary))
                                .child(k.to_string()),
                        ),
                );
            }
            for j in terms {
                let term = &vm.terminals[j];
                let Some(target) = vm.remote_ref(&term.id) else {
                    continue;
                };
                let label = match &term.name {
                    Some(n) => format!("{} · {n}", term.id),
                    None => term.id.clone(),
                };
                let accessible = match &term.task {
                    Some(k) => format!("{label} · {k}"),
                    None => label.clone(),
                };
                let running = term.state == podd::protocol::TerminalState::Running;
                let (me_click, me_key) = (me.clone(), me.clone());
                let target_key = target.clone();
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("machine-terminal-{i}-{j}")))
                        .qa_id(|| format!("machine-terminal-{i}-{j}"))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(accessible))
                        .tab_index(0)
                        .h(px(24.))
                        .pl(px(if task.is_some() { 32. } else { 16. }))
                        .pr_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .focus_visible(|s| pod_ui::focus_style(s, m))
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::SquareTerminal,
                            12.,
                            if running {
                                t.text_secondary
                            } else {
                                t.text_muted
                            },
                        ))
                        .child(div().flex_1().min_w_0().truncate().text_xs().child(label))
                        .on_click(move |_, window, cx| {
                            open_terminal(&me_click, target.clone(), window, cx)
                        })
                        .on_key_down(move |e, window, cx| {
                            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                open_terminal(&me_key, target_key.clone(), window, cx);
                                cx.stop_propagation();
                            }
                        }),
                );
            }
        }
        card = card.child(rows);
    }
    card
}

/// Open a remote terminal panel (a row of the board).
fn open_terminal(
    board: &WeakEntity<MachineBoard>,
    target: RemoteRef,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(ws) = board.upgrade().map(|b| b.read(cx).workspace.clone()) else {
        return;
    };
    let _ = ws.update(cx, |w, cx| w.open_remote_terminal(target, window, cx));
}
