#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use gpui_kit::component::{
    Disableable,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};
use gpui_kit::*;
use pod_status::{AgentState as S, BlockedKind as B};
use pod_ui::tokens::color;
use pod_ui::*;
struct Gallery {
    mode: Mode,
    query: Entity<InputState>,
    modal: bool,
    owner: FocusHandle,
    close_focus: FocusHandle,
    open_focus: FocusHandle,
    returns: FocusReturn,
    reduced: bool,
}
fn fixtures() -> Vec<StatusBadge> {
    let mut rows = [
        S::Idle,
        S::Working,
        S::Blocked(B::Permission),
        S::Blocked(B::Question),
        S::Blocked(B::Auth),
        S::Blocked(B::Other),
        S::Done,
        S::Error,
        S::Unknown,
        S::Exited,
    ]
    .into_iter()
    .map(StatusBadge::fixture)
    .collect::<Vec<_>>();
    rows[9].exit_code = Some(0);
    let mut failed = StatusBadge::fixture(S::Exited);
    failed.exit_code = Some(1);
    failed.final_state = Some(S::Working);
    rows.push(failed);
    let mut stale = StatusBadge::fixture(S::Working);
    stale.stale_since = Some("2분 전".into());
    rows.push(stale);
    let mut held = StatusBadge::fixture(S::Done);
    held.running_children = 2;
    rows.push(held);
    let mut estimated = StatusBadge::fixture(S::Idle);
    estimated.confidence = 0.4;
    estimated.source = Some(pod_status::Source::Heuristic);
    rows.push(estimated);
    rows
}
impl Render for Gallery {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let t = Tokens::for_mode(mode);
        let rows = fixtures();
        let summary = StatusSummary::from_terminals(rows.iter().enumerate().map(|(i, b)| {
            (
                match i {
                    0 => "a",
                    1 => "b",
                    2 => "c",
                    3 => "d",
                    4 => "e",
                    5 => "f",
                    6 => "g",
                    7 => "h",
                    8 => "i",
                    9 => "j",
                    10 => "k",
                    11 => "l",
                    12 => "m",
                    _ => "n",
                },
                b,
            )
        }));
        let statuses = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(rows.iter().map(|b| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(b.render_with_motion(mode, self.reduced))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(color(t.text_muted))
                            .child(match b.source {
                                Some(pod_status::Source::Hook) => "hook",
                                Some(pod_status::Source::Osc) => "osc",
                                Some(pod_status::Source::Heuristic) => "heuristic",
                                Some(pod_status::Source::Mod) => "mod",
                                Some(pod_status::Source::Process) => "process",
                                None => "unknown",
                            }),
                    )
            }));
        let mut list = div().flex().flex_col().gap(px(4.));
        for i in 0..3 {
            list = list.child(
                TreeRow {
                    id: ("tree", i).into(),
                    label: "한글 이름이 아주 긴 작업 — 로그인 오류를 재현하고 접근성을 확인합니다"
                        .into(),
                    depth: i,
                    selected: i == 1,
                    state: if i == 2 {
                        ControlState::Disabled
                    } else {
                        ControlState::Ready
                    },
                    badge: None,
                    glyph: Some(pod_ui::Icon::Bot),
                    mark: None,
                }
                .render(mode, |_, _| {}),
            );
        }
        let weak = cx.entity().downgrade();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                IconButton::new("sun", Icon::Sun, "테마 전환").render(mode, move |_, cx| {
                    let _ = weak.update(cx, |s, cx| {
                        s.mode = if s.mode == Mode::Dark {
                            Mode::Light
                        } else {
                            Mode::Dark
                        };
                        apply_theme(s.mode, s.reduced, cx);
                        cx.notify();
                    });
                }),
            )
            .child(
                Button::new("theme")
                    .label("테마 전환")
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.mode = if s.mode == Mode::Dark {
                            Mode::Light
                        } else {
                            Mode::Dark
                        };
                        apply_theme(s.mode, s.reduced, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("dialog")
                    .track_focus(&self.open_focus)
                    .primary()
                    .label("대화상자 열기")
                    .on_click(cx.listener(|s, _, w, cx| {
                        s.open_focus.focus(w, cx);
                        s.modal = true;
                        s.returns.enter(&s.close_focus, w, cx);
                        cx.notify();
                    })),
            )
            .child(Button::new("disabled").label("사용 불가").disabled(true))
            .child(
                Button::new("motion")
                    .label(if self.reduced {
                        "동작 줄이기: 켬"
                    } else {
                        "동작 줄이기: 끔"
                    })
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.reduced = !s.reduced;
                        apply_theme(s.mode, s.reduced, cx);
                        cx.notify();
                    })),
            );
        let palette = Frame::new(
            "palette",
            FrameKind::CommandPalette,
            "명령 / 작업 검색",
            "Ctrl+K · 한글 입력 및 IME 확인용 fixture",
        )
        .render(
            mode,
            [
                Input::new(&self.query).into_any_element(),
                div()
                    .child("검색 결과 0 · 검색어를 바꿔 보세요")
                    .into_any_element(),
            ],
        );
        let mut loading = Frame::new(
            "loading",
            FrameKind::List,
            "불러오는 중",
            "빈 결과와 로딩을 구분합니다",
        );
        loading.state = ControlState::Loading;
        let mut error = Frame::new(
            "error",
            FrameKind::Toast,
            "연결을 확인해 주세요",
            "입력과 마지막 상태를 보존합니다",
        );
        error.state = ControlState::Error("연결 실패 · 다시 시도할 수 있습니다".into());
        let terminal = terminal::TerminalPalette::for_mode(mode);
        let swatches =
            div()
                .flex()
                .gap(px(4.))
                .children(terminal.ansi.into_iter().enumerate().map(|(i, c)| {
                    div()
                        .w(px(22.))
                        .h(px(24.))
                        .bg(color(c))
                        .text_color(color(if tokens::contrast(c, 0xffffff) > 4.5 {
                            0xffffff
                        } else {
                            0x000000
                        }))
                        .child(i.to_string())
                }));
        let left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                Frame::new(
                    "statuses",
                    FrameKind::List,
                    "상태 / 형태 / 문구",
                    "색을 제거해도 의미가 남습니다 · source는 별도 메타데이터",
                )
                .render(
                    mode,
                    [
                        statuses.into_any_element(),
                        summary.render(mode).into_any_element(),
                    ],
                ),
            )
            .child(
                Frame::new(
                    "tree",
                    FrameKind::List,
                    "Repository > Task > Terminal",
                    "선택 · hover · keyboard focus · disabled",
                )
                .render(mode, [list.into_any_element()]),
            )
            .child(
                Frame::new(
                    "semantic",
                    FrameKind::List,
                    "Semantic actions / switch",
                    "Hover border / pressed inset / checked position",
                )
                .render(
                    mode,
                    [div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .children(
                            [
                                (Tone::Success, "Success"),
                                (Tone::Warning, "Attention"),
                                (Tone::Danger, "Delete"),
                                (Tone::Info, "Info"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(i, (tone, label))| {
                                semantic_button(
                                    ("semantic-action", i),
                                    label,
                                    tone,
                                    mode,
                                    ControlState::Ready,
                                    |_, _| {},
                                )
                            }),
                        )
                        .child(gpui_kit::component::switch::Switch::new("switch-off").label("Off"))
                        .child(
                            gpui_kit::component::switch::Switch::new("switch-on")
                                .checked(true)
                                .label("On"),
                        )
                        .into_any_element()],
                ),
            );
        let right = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(palette)
            .child(loading.render(mode, []))
            .child(error.render(mode, []))
            .child(
                Frame::new(
                    "empty",
                    FrameKind::Empty,
                    "첫 저장소를 열어 보세요",
                    "작업마다 공간을 나누고 터미널을 함께 관리합니다.",
                )
                .render(
                    mode,
                    [div()
                        .child(icon(Icon::FolderOpen, t.text_secondary).size(px(32.)))
                        .child(Button::new("open").label("저장소 열기 (fixture)"))
                        .into_any_element()],
                ),
            )
            .child(
                Frame::new(
                    "terminal",
                    FrameKind::List,
                    "VT 기본 팔레트",
                    "ANSI 0–15 · UI 상태색과 별개 · truecolor 보존",
                )
                .render(mode, [swatches.into_any_element()]),
            );
        let mut root = div()
            .id("gallery")
            .size_full()
            .overflow_y_scroll()
            .bg(color(t.bg_canvas))
            .text_color(color(t.text_primary))
            .font_family(
                gpui_kit::component::theme::Theme::global(cx)
                    .font_family
                    .clone(),
            )
            .text_size(px(13.))
            .line_height(px(20.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .text_size(px(28.))
                    .line_height(px(36.))
                    .child(format!("Pod / 디자인 시스템 v1 · {:?}", mode)),
            )
            .child("공개 fixture만 사용 · 긴 한국어 레이블 · 모든 상태 · 13px 기준")
            .child(toolbar)
            .child(div().flex().gap(px(24.)).child(left).child(right));
        if self.modal {
            let close = cx.listener(|s: &mut Gallery, _: &ClickEvent, w, cx| {
                s.modal = false;
                s.returns.leave(w, cx);
                cx.notify();
            });
            let frame = Frame::new(
                "modal",
                FrameKind::Dialog,
                "새 작업",
                "Esc로 닫으면 열었던 버튼으로 포커스가 돌아갑니다.",
            )
            .render(
                mode,
                [
                    div()
                        .child("fixture 대화상자 · 입력값은 외부에 저장되지 않습니다")
                        .into_any_element(),
                    Button::new("close")
                        .track_focus(&self.close_focus)
                        .label("닫기")
                        .on_click(close)
                        .into_any_element(),
                ],
            );
            let handle = cx.entity().downgrade();
            let modal = modal_keyboard(
                frame,
                &self.owner,
                vec![self.close_focus.clone()],
                move |w, cx| {
                    let _ = handle.update(cx, |s, cx| {
                        s.modal = false;
                        s.returns.leave(w, cx);
                        cx.notify();
                    });
                },
            );
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(tokens::alpha(t.overlay_scrim))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .child(modal),
            );
        }
        let _ = w;
        root
    }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.iter().any(|a| a == "--light") {
        Mode::Light
    } else {
        Mode::Dark
    };
    let show_dialog = args.iter().any(|a| a == "--dialog");
    let self_check = args.iter().any(|a| a == "--self-check");
    let primary = args.iter().any(|a| a == "--primary");
    let duration = args
        .windows(2)
        .find(|a| a[0] == "--duration")
        .and_then(|a| a[1].parse::<u64>().ok());
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        apply_theme(mode, true, cx);
        let main = cx.primary_display().map(|d| d.id());
        let display_id = if primary {
            main
        } else {
            cx.displays()
                .iter()
                .map(|d| d.id())
                .find(|id| Some(*id) != main)
                .or(main)
        };
        let bounds = Bounds::centered(display_id, size(px(1400.), px(1000.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Maximized(bounds)),
                display_id,
                ..Default::default()
            },
            cx,
            |w, cx| {
                cx.new(|cx| {
                    if self_check {
                        run_live_check(w, cx);
                    }
                    Gallery {
                        mode,
                        query: cx.new(|cx| {
                            InputState::new(w, cx).placeholder("작업, 터미널 또는 명령 검색")
                        }),
                        modal: show_dialog,
                        owner: cx.focus_handle(),
                        close_focus: cx.focus_handle(),
                        open_focus: cx.focus_handle(),
                        returns: FocusReturn::default(),
                        reduced: true,
                    }
                })
            },
        )
        .expect("open gallery");
        if let Some(seconds) = duration {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(seconds))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
        cx.activate(true);
    });
}

/// Fixture-only end-to-end focus check, using GPUI's public event dispatch path.
fn run_live_check(window: &Window, cx: &mut Context<Gallery>) {
    cx.spawn_in(window,async move|view,cx|{
  cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
  let _=view.update_in(cx,|s,w,cx|{s.open_focus.focus(w,cx);s.modal=true;s.returns.enter(&s.close_focus,w,cx);cx.notify();});
  cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
  let opened=view.update_in(cx,|s,w,_|s.modal&&s.close_focus.is_focused(w)).unwrap_or(false);
  let _=cx.update(|w,cx|{w.dispatch_keystroke(Keystroke::parse("tab").expect("Tab"),cx);});
  cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
  let trapped=view.update_in(cx,|s,w,_|s.close_focus.is_focused(w)).unwrap_or(false);
  let _=cx.update(|w,cx|{w.dispatch_keystroke(Keystroke::parse("escape").expect("Escape"),cx);});
  cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
  let returned=view.update_in(cx,|s,w,_|!s.modal&&s.open_focus.is_focused(w)).unwrap_or(false);
  let report=format!("GPUI dispatch: modal initial focus={opened}, Tab trapped={trapped}, Escape closed and restored opener={returned}\nNative Windows IME composition: not verified by this check\n");
  let _=std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"),"/qa/live-check.txt"),report);
 }).detach();
}
