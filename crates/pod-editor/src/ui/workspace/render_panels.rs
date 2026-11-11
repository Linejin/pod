//! Rendering of the banner above the editor, the problems/references panel
//! and the status bar.

use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types as lt;
use pod_ui::{Icon, Tokens, icon, tokens::color as tc};

use crate::lsp::ServerState;
use crate::text;

use super::{DiskState, EditorWorkspace, PANEL_H, PanelTab, rel_of};

impl EditorWorkspace {
    pub(super) fn render_banner(
        &mut self,
        t: Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = self.mode;
        let b = self.buffers.get(self.tabs.active()?)?;
        let bar = |bg: u32, fg: u32| {
            div()
                .flex_none()
                .min_h(px(32.))
                .px(px(12.))
                .py(px(4.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .bg(tc(bg))
                .text_size(px(13.))
                .text_color(tc(fg))
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |st| pod_ui::focus_style(st, mode))
                .flex_none()
                .px(px(8.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(tc(t.border_control))
                .text_color(tc(t.text_primary))
                .bg(tc(t.bg_surface))
                .cursor_pointer()
                .hover(|d| d.bg(tc(t.bg_hover)))
                .child(label)
        };
        if let (Some(f), DiskState::Same) = (&b.save_failed, &b.disk) {
            let replace = f.replace;
            let mut bar = bar(t.danger_bg, t.danger_fg)
                .child(icon(Icon::OctagonX, t.danger_fg))
                .child(div().flex_1().min_w(px(220.)).child(format!(
                    "저장하지 못했습니다. 디스크의 파일은 그대로입니다: {}",
                    f.message
                )))
                .child(button("save-retry", "다시 저장").on_click(
                    cx.listener(|this, _, w, cx| this.answer_save_failure(false, w, cx)),
                ));
            if replace {
                bar =
                    bar.child(button("save-overwrite", "그대로 덮어쓰기").on_click(
                        cx.listener(|this, _, w, cx| this.answer_save_failure(true, w, cx)),
                    ));
            }
            return Some(
                bar.child(
                    button("save-dismiss", "닫기")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_save_failure(cx))),
                )
                .into_any_element(),
            );
        }
        match &b.disk {
            DiskState::Changed(_) => {
                let (msg, keep) = if b.save_blocked {
                    (
                        "저장하지 않았습니다: 열어 둔 뒤 디스크의 파일이 바뀌었습니다 (에이전트 또는 다른 프로그램).",
                        "덮어써서 저장",
                    )
                } else {
                    (
                        "디스크의 파일이 바뀌었습니다 (에이전트 또는 다른 프로그램). 저장하지 않은 변경이 있습니다.",
                        "내 변경 유지",
                    )
                };
                return Some(
                    bar(t.warning_bg, t.warning_fg)
                        .child(icon(Icon::TriangleAlert, t.warning_fg))
                        .child(div().flex_1().min_w(px(220.)).child(msg))
                        .child(button("disk-reload", "디스크 내용 불러오기").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(true, w, cx)),
                        ))
                        .child(
                            button("disk-diff", "차이 보기")
                                .on_click(cx.listener(|this, _, w, cx| this.show_disk_diff(w, cx))),
                        )
                        .child(button("disk-keep", keep).on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(false, w, cx)),
                        ))
                        .into_any_element(),
                );
            }
            DiskState::Deleted => {
                return Some(
                    bar(t.danger_bg, t.danger_fg)
                        .child(icon(Icon::OctagonX, t.danger_fg))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.))
                                .child("디스크에서 파일이 삭제되었습니다."),
                        )
                        .child(button("del-close", "탭 닫기").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(true, w, cx)),
                        ))
                        .child(button("del-keep", "내 변경 유지").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(false, w, cx)),
                        ))
                        .into_any_element(),
                );
            }
            DiskState::Same => {}
        }
        if let Some(r) = self
            .lsp
            .read(cx)
            .untrusted_repo_config()
            .filter(|_| !self.repo_lsp_dismissed)
        {
            let hash = r.hash.clone();
            let mut text = div().flex_1().min_w(px(220.)).flex().flex_col().child("이 저장소의 .pod/lsp.json이 언어 서버 실행 명령을 정합니다. 승인하기 전에는 이 파일의 명령을 쓰지 않습니다:");
            for c in &r.commands {
                text = text.child(
                    div()
                        .font_family("Cascadia Mono")
                        .text_size(px(12.))
                        .child(c.clone()),
                );
            }
            return Some(
                bar(t.warning_bg, t.warning_fg)
                    .child(icon(Icon::TriangleAlert, t.warning_fg))
                    .child(text)
                    .child(button("repo-lsp-trust", "이 내용 신뢰하고 사용").on_click(
                        cx.listener(move |this, _, _, cx| this.trust_repo_lsp(&hash, cx)),
                    ))
                    .child(button("repo-lsp-ignore", "무시").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.repo_lsp_dismissed = true;
                            cx.notify();
                        },
                    )))
                    .into_any_element(),
            );
        }
        if b.large {
            return Some(
                bar(t.info_bg, t.info_fg)
                    .child(icon(Icon::CircleHelp, t.info_fg))
                    .child("큰 파일입니다: 읽기 전용으로 열었고 구문 강조와 언어 서버를 껐습니다.")
                    .into_any_element(),
            );
        }
        if b.undecodable {
            let msg = if b.format.encoding == text::utf8() {
                "UTF-8이 아닌 파일이라 읽기 전용으로 열었습니다 (그대로 저장하면 글자가 깨집니다). 인코딩을 골라 다시 열 수 있습니다.".to_string()
            } else {
                format!(
                    "{}(으)로 손실 없이 읽을 수 없어 읽기 전용으로 열었습니다. 다른 인코딩을 골라 보세요.",
                    b.format.encoding.name()
                )
            };
            let ix = self.tabs.active()?;
            let mut row = bar(t.warning_bg, t.warning_fg)
                .child(icon(Icon::TriangleAlert, t.warning_fg))
                .child(div().flex_1().min_w(px(220.)).child(msg));
            for (n, (label, enc)) in text::REOPEN_ENCODINGS.iter().enumerate() {
                let enc: &'static text::Encoding = enc;
                row = row.child(
                    button(["reopen-enc-0", "reopen-enc-1", "reopen-enc-2"][n], label).on_click(
                        cx.listener(move |this, _, w, cx| {
                            this.reopen_with_encoding(ix, enc, w, cx)
                        }),
                    ),
                );
            }
            return Some(row.into_any_element());
        }
        let path = b.path.clone();
        let status = self.lsp.read(cx).status_for(&path)?;
        match &status.state {
            ServerState::NotInstalled(reason, hint) => Some(
                bar(t.info_bg, t.info_fg)
                    .child(icon(Icon::CircleHelp, t.info_fg))
                    .child(div().flex_1().min_w(px(220.)).child(format!(
                        "언어 서버 {}가 설치되어 있지 않습니다 ({reason}). 설치: {hint}",
                        status.id
                    )))
                    .into_any_element(),
            ),
            ServerState::Failed(reason) => Some(
                bar(t.danger_bg, t.danger_fg)
                    .child(icon(Icon::OctagonX, t.danger_fg))
                    .child(div().flex_1().min_w(px(220.)).child(format!(
                        "{}가 반복해서 종료되어 멈췄습니다: {}",
                        status.id,
                        reason.lines().next().unwrap_or("")
                    )))
                    .child(button("lsp-restart", "다시 시작").on_click(cx.listener(
                        move |this, _, _, cx| {
                            let p = path.clone();
                            this.lsp.update(cx, |s, cx| s.restart(&p, cx));
                        },
                    )))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    pub(super) fn render_panel(&mut self, t: Tokens, cx: &mut Context<Self>) -> AnyElement {
        let store = self.lsp.read(cx);
        let mut problems: Vec<(PathBuf, lt::Diagnostic)> = store
            .diagnostics()
            .flat_map(|f| f.items.iter().map(move |d| (f.path.clone(), d.clone())))
            .collect();
        problems.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(
                    a.1.severity
                        .map(|s| format!("{s:?}"))
                        .cmp(&b.1.severity.map(|s| format!("{s:?}"))),
                )
                .then(a.1.range.start.line.cmp(&b.1.range.start.line))
        });
        let (errs, warns) = store.diagnostic_counts();
        let tab = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(12.))
                .when(on, |d| {
                    d.text_color(tc(t.text_primary))
                        .border_b_2()
                        .border_color(tc(t.accent_fg))
                })
                .when(!on, |d| d.text_color(tc(t.text_secondary)))
                .child(label)
        };
        let root = self.root.clone();
        let rows: Vec<AnyElement> = match self.panel_tab {
            PanelTab::Problems => problems
                .iter()
                .enumerate()
                .map(|(i, (p, d))| {
                    let (ic, col) = match d.severity {
                        Some(lt::DiagnosticSeverity::ERROR) => (Icon::OctagonX, t.danger_fg),
                        Some(lt::DiagnosticSeverity::WARNING) => {
                            (Icon::TriangleAlert, t.warning_fg)
                        }
                        _ => (Icon::CircleHelp, t.info_fg),
                    };
                    let (p2, pos) = (p.clone(), d.range.start);
                    panel_row(("prob", i), t)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            let _ = this.open_at(&p2, Some(pos), w, cx);
                        }))
                        .child(icon(ic, col).size(px(14.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(tc(t.text_primary))
                                .child(d.message.lines().next().unwrap_or("").to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(tc(t.text_muted))
                                .child(format!("{}:{}", rel_of(&root, p), d.range.start.line + 1)),
                        )
                        .into_any_element()
                })
                .collect(),
            PanelTab::References => self
                .references
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let (p2, pos) = (l.path.clone(), l.range.start);
                    let line = std::fs::read_to_string(&l.path)
                        .ok()
                        .and_then(|s| {
                            s.lines()
                                .nth(l.range.start.line as usize)
                                .map(|x| x.trim().to_string())
                        })
                        .unwrap_or_default();
                    panel_row(("ref", i), t)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            let _ = this.open_at(&p2, Some(pos), w, cx);
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_color(tc(t.text_muted))
                                .child(format!(
                                    "{}:{}",
                                    rel_of(&root, &l.path),
                                    l.range.start.line + 1
                                )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family("Cascadia Mono")
                                .text_color(tc(t.text_primary))
                                .child(line),
                        )
                        .into_any_element()
                })
                .collect(),
        };
        let empty = rows.is_empty();
        let empty_text = match self.panel_tab {
            PanelTab::Problems => "문제가 없습니다",
            PanelTab::References => "Shift+F12로 참조를 찾으세요",
        };
        div()
            .h(px(PANEL_H))
            .flex_none()
            .flex()
            .flex_col()
            .bg(tc(t.bg_surface))
            .border_t_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .bg(tc(t.bg_chrome))
                    .child(
                        tab(
                            "panel-problems",
                            format!("문제 · 오류 {errs} · 경고 {warns}"),
                            self.panel_tab == PanelTab::Problems,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel_tab = PanelTab::Problems;
                            cx.notify();
                        })),
                    )
                    .child(
                        tab(
                            "panel-refs",
                            if self.references_title.is_empty() {
                                "참조".to_string()
                            } else {
                                self.references_title.clone()
                            },
                            self.panel_tab == PanelTab::References,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel_tab = PanelTab::References;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("panel-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(px(12.))
                                .text_size(px(13.))
                                .text_color(tc(t.text_muted))
                                .child(empty_text),
                        )
                    })
                    .children(rows),
            )
            .into_any_element()
    }

    pub(super) fn render_status(&mut self, t: Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mut left: Vec<String> = vec![];
        if let Some(b) = &self.branch {
            left.push(format!("⎇ {b}"));
        }
        let (e, w) = self.lsp.read(cx).diagnostic_counts();
        left.push(format!("오류 {e} · 경고 {w}"));
        if self.indexing {
            left.push("파일 색인 중…".into());
        }
        let mut right: Vec<String> = vec![];
        let mut lsp_label: Option<(String, u32)> = None;
        if let Some(b) = self.tabs.active().and_then(|i| self.buffers.get(i)) {
            let pos = b.state.read(cx).cursor_position();
            right.push(format!("줄 {}, 열 {}", pos.line + 1, pos.character + 1));
            right.push(b.format.ending_label());
            let enc = b.format.encoding.name();
            right.push(if b.format.bom {
                format!("{enc} BOM")
            } else {
                enc.to_string()
            });
            if b.undecodable {
                right.push("읽기 전용".into());
            }
            right.push(b.language.map(|l| l.label).unwrap_or("일반 텍스트").into());
            if let Some(s) = self.lsp.read(cx).status_for(&b.path) {
                let col = match s.state {
                    ServerState::Running => t.success_fg,
                    ServerState::NotInstalled(..)
                    | ServerState::Checking
                    | ServerState::Starting => t.text_muted,
                    ServerState::Restarting { .. } => t.warning_fg,
                    _ => t.danger_fg,
                };
                let mut label = s.state.label(&s.id);
                if let Some(p) = s.progress {
                    let p: String = if p.chars().count() > 60 {
                        p.chars().take(59).chain(std::iter::once('…')).collect()
                    } else {
                        p
                    };
                    label = format!("{} · {p}", s.id);
                }
                lsp_label = Some((label, col));
            }
        }
        div()
            .h(px(24.))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(16.))
            .bg(tc(t.bg_chrome))
            .border_t_1()
            .border_color(tc(t.border_subtle))
            .text_size(px(12.))
            .text_color(tc(t.text_secondary))
            .children(left.into_iter().map(|s| div().flex_none().child(s)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when_some(self.message.clone(), |d, (m, err)| {
                        d.text_color(tc(if err { t.danger_fg } else { t.text_primary }))
                            .child(m)
                    }),
            )
            .children(right.into_iter().map(|s| div().flex_none().child(s)))
            .when_some(lsp_label, |d, (l, c)| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(tc(c)))
                        .child(l),
                )
            })
            .into_any_element()
    }
}

fn panel_row(id: (&'static str, usize), t: Tokens) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|d| d.bg(tc(t.bg_hover)))
}
