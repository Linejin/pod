//! Ctrl+P quick open: fuzzy file search over the background index.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Mode, Tokens, tokens::color as tc};

use crate::index::{self, Hit};

pub const RESULT_LIMIT: usize = 200;
const ROW_H: f32 = 32.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickOpenEvent {
    Open(String),
    Dismiss,
}

pub struct QuickOpen {
    pub input: Entity<InputState>,
    files: Arc<Vec<String>>,
    recents: Vec<String>,
    hits: Vec<Hit>,
    selected: usize,
    indexing: bool,
    truncated: bool,
    mode: Mode,
    scroll: UniformListScrollHandle,
    generation: u64,
    _search: Task<()>,
    _sub: Subscription,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

impl QuickOpen {
    pub fn new(
        files: Arc<Vec<String>>,
        recents: Vec<String>,
        indexing: bool,
        truncated: bool,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("파일 이름으로 이동 (퍼지 검색)"));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => this.search(cx),
                InputEvent::PressEnter { .. } => this.confirm(cx),
                _ => {}
            },
        );
        let mut q = QuickOpen {
            input,
            files,
            recents,
            hits: vec![],
            selected: 0,
            indexing,
            truncated,
            mode,
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            _search: Task::ready(()),
            _sub: sub,
        };
        q.search(cx);
        q
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// New index snapshot arrived while open (background indexing finished).
    pub fn set_files(
        &mut self,
        files: Arc<Vec<String>>,
        indexing: bool,
        truncated: bool,
        cx: &mut Context<Self>,
    ) {
        self.files = files;
        self.indexing = indexing;
        self.truncated = truncated;
        self.search(cx);
    }

    pub fn set_query(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(q.to_string(), window, cx));
        self.search(cx);
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().to_string();
        let files = self.files.clone();
        let recents = self.recents.clone();
        self.generation += 1;
        let generation = self.generation;
        let job =
            cx.background_spawn(async move { index::search(&files, &q, &recents, RESULT_LIMIT) });
        self._search = cx.spawn(async move |this, cx| {
            let hits = job.await;
            let _ = this.update(cx, |s, cx| {
                if s.generation == generation {
                    s.hits = hits;
                    s.selected = 0;
                    s.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
            });
        });
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = self.hits.get(self.selected) {
            cx.emit(QuickOpenEvent::Open(h.rel.clone()));
        }
    }

    fn move_sel(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(n)) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "up" => self.move_sel(-1, cx),
            "down" => self.move_sel(1, cx),
            "pageup" => self.move_sel(-10, cx),
            "pagedown" => self.move_sel(10, cx),
            "escape" => cx.emit(QuickOpenEvent::Dismiss),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// Name/dir split with matched char ranges for highlighting.
fn split_highlights(
    rel: &str,
    indices: &[usize],
) -> (String, Vec<Range<usize>>, String, Vec<Range<usize>>) {
    let chars: Vec<char> = rel.chars().collect();
    let name_start = chars
        .iter()
        .rposition(|&c| c == '/')
        .map(|i| i + 1)
        .unwrap_or(0);
    let name: String = chars[name_start..].iter().collect();
    let dir: String = chars[..name_start.saturating_sub(1)].iter().collect();
    let to_ranges = |s: &str, offset: usize, len: usize| -> Vec<Range<usize>> {
        let byte_of: Vec<usize> = s
            .char_indices()
            .map(|(b, _)| b)
            .chain(std::iter::once(s.len()))
            .collect();
        indices
            .iter()
            .filter(|&&i| i >= offset && i < offset + len)
            .map(|&i| {
                let ci = i - offset;
                byte_of[ci]..byte_of[ci + 1]
            })
            .collect()
    };
    let nr = to_ranges(&name, name_start, chars.len() - name_start);
    let dr = to_ranges(&dir, 0, name_start.saturating_sub(1));
    (name, nr, dir, dr)
}

impl Render for QuickOpen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::for_mode(self.mode);
        let count = self.hits.len();
        let status = if self.indexing {
            format!("색인 중… 파일 {}개", self.files.len())
        } else if self.truncated {
            format!("파일 {}개 (일부만 색인됨)", self.files.len())
        } else {
            format!("파일 {}개", self.files.len())
        };
        let hl = HighlightStyle {
            color: Some(tc(t.accent_fg)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        div()
            .id("pod-quick-open")
            .w(px(640.))
            .max_h(px(480.))
            .flex()
            .flex_col()
            .bg(tc(t.bg_raised))
            .border_1()
            .border_color(tc(t.border_subtle))
            .rounded(px(12.))
            .shadow(pod_ui::tokens::elevation(2, self.mode))
            .overflow_hidden()
            .capture_key_down(cx.listener(Self::on_key))
            .child(div().p(px(8.)).child(Input::new(&self.input)))
            .child(
                div()
                    .px(px(12.))
                    .pb(px(4.))
                    .flex()
                    .justify_between()
                    .text_size(px(12.))
                    .text_color(tc(t.text_muted))
                    .child(if count == 0 && !self.input.read(cx).value().is_empty() {
                        "일치하는 파일 없음".to_string()
                    } else {
                        format!("결과 {count}개")
                    })
                    .child(status),
            )
            .child(
                uniform_list(
                    "quick-open-results",
                    count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let t = Tokens::for_mode(this.mode);
                        range
                            .map(|ix| {
                                let h = &this.hits[ix];
                                let (name, nr, dir, dr) = split_highlights(&h.rel, &h.indices);
                                let sel = ix == this.selected;
                                let rel = h.rel.clone();
                                let name_text = StyledText::new(name)
                                    .with_highlights(nr.into_iter().map(|r| (r, hl)));
                                let dir_text = StyledText::new(dir)
                                    .with_highlights(dr.into_iter().map(|r| (r, hl)));
                                div()
                                    .id(("qo", ix))
                                    .w_full()
                                    .h(px(ROW_H))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .when(sel, |d| d.bg(tc(t.bg_selected)))
                                    .hover(|d| d.bg(tc(t.bg_hover)))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(QuickOpenEvent::Open(rel.clone()))
                                    }))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(tc(t.text_primary))
                                            .child(name_text),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(12.))
                                            .text_color(tc(t.text_muted))
                                            .child(dir_text),
                                    )
                                    .when(h.recent, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_size(px(12.))
                                                .text_color(tc(t.text_muted))
                                                .child("최근"),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .h(px((count.min(12) as f32) * ROW_H)),
            )
            .child(div().h(px(8.)))
    }
}

#[cfg(test)]
mod tests {
    use super::split_highlights;

    #[test]
    fn highlight_ranges_are_byte_ranges_per_part() {
        let (name, nr, dir, dr) = split_highlights("docs/한글.md", &[0, 5, 6]);
        assert_eq!(name, "한글.md");
        assert_eq!(nr, vec![0..3, 3..6]);
        assert_eq!(dir, "docs");
        assert_eq!(dr, vec![0..1]);
    }
}
