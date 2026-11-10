//! Root view: file list | gpui-component `Diff` | comment list.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffFile, DiffFileStatus, DiffHoverHighlight, DiffLinePosition, DiffMode,
    DiffSide, DiffState,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::Serialize;

use crate::Args;
use crate::stats;
use pod_diff::SourceFile;

const CONTEXT_LINES: usize = 3;

/// One review comment, in the shape that will later be sent to an agent.
#[derive(Clone, Debug, Serialize)]
pub struct Comment {
    pub id: u64,
    pub path: String,
    /// "old" (original / left) or "new" (modified / right).
    pub side: &'static str,
    /// 1-based, inclusive.
    pub line: usize,
    pub end_line: usize,
    pub body: String,
    /// The commented source lines, so the receiver needs no checkout.
    pub code: String,
}

#[derive(Serialize)]
struct Export<'a> {
    version: u32,
    source: &'a str,
    comments: &'a [Comment],
}

struct Draft {
    path: String,
    side: DiffSide,
    line: usize,
    end_line: usize,
    input: Entity<InputState>,
}

#[derive(Clone)]
enum AnnotationView {
    Comment(Comment),
    Draft(Entity<InputState>, SharedString),
}

struct FileRow {
    path: SharedString,
    status: DiffFileStatus,
    additions: usize,
    deletions: usize,
}

pub struct Review {
    state: Entity<DiffState>,
    sources: Rc<Vec<SourceFile>>,
    files: Rc<Vec<FileRow>>,
    source_label: String,
    split: bool,
    collapsed: bool,
    /// Soft wrap: split mode needs it when the window is narrower than both columns.
    wrap: bool,
    comments: Vec<Comment>,
    next_id: u64,
    draft: Option<Draft>,
    export: PathBuf,
    status: SharedString,
    perf: SharedString,
    _subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

fn side_name(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Original => "old",
        DiffSide::Modified => "new",
    }
}

impl Review {
    pub fn new(
        args: &Args,
        sources: Vec<SourceFile>,
        patch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = Instant::now();
        let parsed = match DiffFile::parse(&patch) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[open] patch parse failed: {e}");
                Vec::new()
            }
        };
        let parse_ms = t.elapsed().as_secs_f64() * 1e3;
        let files: Vec<FileRow> = parsed
            .iter()
            .map(|f| FileRow {
                path: f.path().clone(),
                status: f.status(),
                additions: f.additions(),
                deletions: f.deletions(),
            })
            .collect();
        let t = Instant::now();
        let split = args.split;
        let collapsed = !args.expand_all;
        let state = cx.new(|cx| {
            DiffState::new(parsed, cx)
                .with_mode(if split {
                    DiffMode::Split
                } else {
                    DiffMode::Unified
                })
                .with_context_lines(collapsed.then_some(CONTEXT_LINES))
        });
        let state_ms = t.elapsed().as_secs_f64() * 1e3;
        let (adds, dels) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        println!("[open] parse {parse_ms:.1}ms, DiffState build {state_ms:.1}ms, +{adds} −{dels}");

        let source_label = match (&args.git, &args.old, &args.new) {
            (Some(repo), _, _) => format!("git {} (working tree vs {})", repo.display(), args.rev),
            (_, Some(o), Some(n)) => format!("{} → {}", o.display(), n.display()),
            _ => String::new(),
        };

        let files_len = files.len();
        let mut this = Self {
            state,
            sources: Rc::new(sources),
            files: Rc::new(files),
            source_label,
            split,
            collapsed,
            wrap: args.wrap.unwrap_or(split),
            comments: Vec::new(),
            next_id: 1,
            draft: None,
            export: args.export.clone(),
            status: format!(
                "{} files · +{adds} −{dels} · click a line to comment",
                files_len
            )
            .into(),
            perf: "".into(),
            _subs: Vec::new(),
            _tasks: Vec::new(),
        };
        if args.demo_comments {
            this.add_demo_comments(window, cx);
        }
        this._tasks.push(this.stats_task(args, window, cx));
        this
    }

    /// Pre-populate for screenshots: one saved comment and one open draft on
    /// the first changed lines of the first file.
    fn add_demo_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sources = self.sources.clone();
        let Some(file) = sources.first() else { return };
        let old: Vec<&str> = file.text(false).split_inclusive('\n').collect();
        let new: Vec<&str> = file.text(true).split_inclusive('\n').collect();
        let diff = similar::TextDiff::from_slices(&old, &new);
        let mut changed_new = diff.ops().iter().filter_map(|op| {
            let r = op.new_range();
            (op.tag() != similar::DiffTag::Equal && !r.is_empty()).then_some(r.start + 1)
        });
        let path = self
            .files
            .first()
            .map(|f| f.path.to_string())
            .unwrap_or_default();
        if let Some(line) = changed_new.next() {
            self.push_comment(
                path.clone(),
                DiffSide::Modified,
                line,
                line,
                "Why the change here? Please add a test that covers the overflow case.".into(),
            );
        }
        if let Some(line) = changed_new.next() {
            self.open_draft(path, DiffSide::Modified, line, line, window, cx);
            if let Some(d) = &self.draft {
                d.input.update(cx, |i, cx| {
                    i.set_value("This debug probe should not be committed", window, cx)
                });
            }
        }
        if let Some(c) = self.comments.first() {
            let pos = DiffLinePosition::new(c.path.clone(), DiffSide::Modified, c.line);
            self.state.update(cx, |s, cx| s.scroll_to_line(pos, cx));
        }
    }

    fn push_comment(
        &mut self,
        path: String,
        side: DiffSide,
        line: usize,
        end_line: usize,
        body: String,
    ) {
        let code = self
            .sources
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.lines(side == DiffSide::Modified, line, end_line))
            .unwrap_or_default();
        self.comments.push(Comment {
            id: self.next_id,
            path,
            side: side_name(side),
            line,
            end_line,
            body,
            code,
        });
        self.next_id += 1;
    }

    fn open_draft(
        &mut self,
        path: String,
        side: DiffSide,
        line: usize,
        end_line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = format!("Comment on {}:{line} — Enter to save", side_name(side));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.save_draft(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self._subs.push(sub);
        self.draft = Some(Draft {
            path,
            side,
            line,
            end_line,
            input,
        });
        cx.notify();
    }

    fn save_draft(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.draft.take() else { return };
        let body = d.input.read(cx).value().to_string();
        if !body.trim().is_empty() {
            self.push_comment(d.path, d.side, d.line, d.end_line, body);
            self.status = format!("{} comments", self.comments.len()).into();
        }
        cx.notify();
    }

    fn export_comments(&mut self, cx: &mut Context<Self>) {
        let json = serde_json::to_string_pretty(&Export {
            version: 1,
            source: &self.source_label,
            comments: &self.comments,
        })
        .unwrap_or_default();
        cx.write_to_clipboard(ClipboardItem::new_string(json.clone()));
        self.status = match std::fs::write(&self.export, &json) {
            Ok(()) => format!(
                "exported {} comments → {} (and clipboard)",
                self.comments.len(),
                self.export.display()
            ),
            Err(e) => format!("export failed: {e} (copied to clipboard)"),
        }
        .into();
        println!("[export] {}", self.status);
        cx.notify();
    }

    fn stats_task(&self, args: &Args, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        let duration = args.duration;
        let bench = args.bench_scroll;
        let bench_delay = args.bench_delay;
        let export_at_end = args.demo_comments;
        cx.spawn_in(window, async move |this, cx| {
            let start = Instant::now();
            let mut last = Instant::now();
            let mut samples = Vec::new();
            let mut first_reported = false;
            let mut bench_started = false;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let s = stats::take(last.elapsed());
                last = Instant::now();
                if !first_reported && let Some(fp) = stats::first_paint() {
                    println!(
                        "[open] first frame painted {:.0}ms after process start",
                        fp.as_secs_f64() * 1e3
                    );
                    first_reported = true;
                }
                println!(
                    "[stats] t={:>3}s fps={:>5.1} frame cpu avg={:.2}ms max={:.2}ms slow={}",
                    start.elapsed().as_secs(),
                    s.fps,
                    s.avg_frame_cpu_ms,
                    s.max_frame_cpu_ms,
                    s.slow_frames
                );
                if bench_started {
                    samples.push(s);
                }
                if let Some(px_per_frame) = bench
                    && !bench_started
                    && start.elapsed() >= Duration::from_secs(bench_delay)
                {
                    bench_started = true;
                    println!("[bench] scrolling {px_per_frame}px per frame");
                    let _ = cx.update(|window, cx| bench_tick(px_per_frame, 0, window, cx));
                }
                let ok = this
                    .update(cx, |w, cx| {
                        w.perf = format!(
                            "{:.0} fps · frame cpu {:.2}ms (max {:.2})",
                            s.fps, s.avg_frame_cpu_ms, s.max_frame_cpu_ms
                        )
                        .into();
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
                if let Some(d) = duration
                    && start.elapsed() >= Duration::from_secs(d)
                {
                    if export_at_end {
                        let _ = this.update(cx, |w, cx| w.export_comments(cx));
                    }
                    stats::summarize(
                        if bench.is_some() {
                            "scroll bench"
                        } else {
                            "idle"
                        },
                        &samples,
                    );
                    let _ = cx.update(|_, cx| cx.quit());
                    break;
                }
            }
        })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let split = self.split;
        let collapsed = self.collapsed;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .bg(rgb(0x202227))
            .border_b_1()
            .border_color(rgb(0x2c2f36))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Review"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8b9099))
                    .child(self.source_label.clone()),
            )
            .child(div().flex_1())
            .child(
                Button::new("mode-split")
                    .label("Side-by-side")
                    .small()
                    .when(split, |b| b.primary())
                    .when(!split, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| this.set_split(true, cx))),
            )
            .child(
                Button::new("mode-unified")
                    .label("Inline")
                    .small()
                    .when(!split, |b| b.primary())
                    .when(split, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| this.set_split(false, cx))),
            )
            .child(
                Button::new("collapse")
                    .label(if collapsed {
                        "Show all lines"
                    } else {
                        "Collapse unchanged"
                    })
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_collapse(cx))),
            )
            .child(
                Button::new("wrap")
                    .label(if self.wrap { "Wrap: on" } else { "Wrap: off" })
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wrap = !this.wrap;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("prev")
                    .label("↑ Prev change")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.previous_change(cx))
                    })),
            )
            .child(
                Button::new("next")
                    .label("↓ Next change")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.next_change(cx))
                    })),
            )
            .child(
                Button::new("export")
                    .label(format!("Export {} comments (JSON)", self.comments.len()))
                    .small()
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.export_comments(cx))),
            )
    }

    fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.split = split;
        self.state.update(cx, |s, cx| {
            s.set_mode(
                if split {
                    DiffMode::Split
                } else {
                    DiffMode::Unified
                },
                cx,
            )
        });
        cx.notify();
    }

    fn toggle_collapse(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        let lines = self.collapsed.then_some(CONTEXT_LINES);
        self.state
            .update(cx, |s, cx| s.set_context_lines(lines, cx));
        cx.notify();
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files.clone();
        div()
            .w(px(240.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1b1c20))
            .border_r_1()
            .border_color(rgb(0x2c2f36))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x8b9099))
                    .child(format!("FILES ({})", files.len())),
            )
            .child(
                // Virtualized: only visible rows are built, so thousands of files are fine.
                uniform_list(
                    "files",
                    files.len(),
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let f = &this.files[ix];
                                let path = f.path.clone();
                                let (letter, color) = match f.status {
                                    DiffFileStatus::Added => ("A", 0x3fb950),
                                    DiffFileStatus::Deleted => ("D", 0xf85149),
                                    _ => ("M", 0xd29922),
                                };
                                div()
                                    .id(("file", ix))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .h(px(24.))
                                    .text_xs()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0x2a2d34)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let p = path.clone();
                                        this.state.update(cx, |s, cx| s.scroll_to_file(&p, cx));
                                    }))
                                    .child(div().flex_none().text_color(rgb(color)).child(letter))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(f.path.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(rgb(0x3fb950))
                                            .child(format!("+{}", f.additions)),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(rgb(0xf85149))
                                            .child(format!("−{}", f.deletions)),
                                    )
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }

    fn render_comment_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("comments")
            .w(px(280.))
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgb(0x1b1c20))
            .border_l_1()
            .border_color(rgb(0x2c2f36))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x8b9099))
                    .child(format!("COMMENTS ({})", self.comments.len())),
            )
            .children(self.comments.iter().map(|c| {
                let id = c.id;
                let pos = DiffLinePosition::new(
                    c.path.clone(),
                    if c.side == "old" {
                        DiffSide::Original
                    } else {
                        DiffSide::Modified
                    },
                    c.line,
                );
                div()
                    .id(("comment", id as usize))
                    .mx_2()
                    .my_1()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(0x23262c))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x2a2d34)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let pos = pos.clone();
                        this.state.update(cx, |s, cx| s.scroll_to_line(pos, cx));
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .text_xs()
                            .text_color(rgb(0x8b9099))
                            .child(
                                div()
                                    .flex_1()
                                    .child(format!("{} · {}:{}", c.path, c.side, c.line)),
                            )
                            .child(
                                div()
                                    .id(("del", id as usize))
                                    .px_1()
                                    .hover(|s| s.text_color(rgb(0xf85149)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.comments.retain(|c| c.id != id);
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    .child(div().text_sm().child(c.body.clone()))
            }))
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> Diff {
        let mut views: HashMap<ElementId, AnnotationView> = HashMap::new();
        let mut annotations = Vec::new();
        for c in &self.comments {
            let id = ElementId::from(SharedString::from(format!("comment-{}", c.id)));
            let side = if c.side == "old" {
                DiffSide::Original
            } else {
                DiffSide::Modified
            };
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(c.path.clone(), side, c.end_line),
            ));
            views.insert(id, AnnotationView::Comment(c.clone()));
        }
        if let Some(d) = &self.draft {
            let id = ElementId::from(SharedString::from("draft"));
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(d.path.clone(), d.side, d.end_line),
            ));
            let label = if d.line == d.end_line {
                format!("{}:{}", side_name(d.side), d.line)
            } else {
                format!("{}:{}–{}", side_name(d.side), d.line, d.end_line)
            };
            views.insert(id, AnnotationView::Draft(d.input.clone(), label.into()));
        }
        let views = Rc::new(views);
        let weak = cx.entity().downgrade();
        let weak_click = weak.clone();
        let weak_add = weak.clone();

        Diff::new(&self.state)
            .size_full()
            .soft_wrap(self.wrap)
            .hover_highlight(DiffHoverHighlight::Both)
            .annotations(annotations)
            .render_annotation(move |ann, _window, _cx| {
                let card = div()
                    .mx_2()
                    .my_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x3b6fd6))
                    .bg(rgb(0x1e2533))
                    .text_sm();
                match views.get(ann.id()) {
                    Some(AnnotationView::Comment(c)) => card
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8b9099))
                                .child(format!("💬 #{} · {}:{}", c.id, c.side, c.line)),
                        )
                        .child(c.body.clone())
                        .into_any_element(),
                    Some(AnnotationView::Draft(input, label)) => {
                        let (w1, w2) = (weak.clone(), weak.clone());
                        card.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8b9099))
                                .child(format!("New comment on {label}")),
                        )
                        .child(div().my_1().child(Input::new(input)))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    Button::new("draft-save")
                                        .label("Comment")
                                        .small()
                                        .primary()
                                        .on_click(move |_, window, cx| {
                                            let _ = w1
                                                .update(cx, |this, cx| this.save_draft(window, cx));
                                        }),
                                )
                                .child(
                                    Button::new("draft-cancel")
                                        .label("Cancel")
                                        .small()
                                        .ghost()
                                        .on_click(move |_, _, cx| {
                                            let _ = w2.update(cx, |this, cx| {
                                                this.draft = None;
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                        .into_any_element()
                    }
                    None => div().into_any_element(),
                }
            })
            .on_line_click(move |pos, _ev, window, cx| {
                let (path, side, line) = (pos.path().to_string(), pos.side(), pos.line());
                let _ = weak_click.update(cx, |this, cx| {
                    this.open_draft(path, side, line, line, window, cx)
                });
            })
            .on_add_annotation(move |range, window, cx| {
                let (path, side, start, end) = (
                    range.path().to_string(),
                    range.side(),
                    range.start(),
                    range.end(),
                );
                let _ = weak_add.update(cx, |this, cx| {
                    this.open_draft(path, side, start, end, window, cx)
                });
            })
    }
}

/// Scroll benchmark driver: one synthetic wheel event per frame over the diff,
/// reversing direction every 600 frames. Goes through GPUI's normal hit-test
/// and list-scroll path, exactly like a real wheel.
fn bench_tick(px_per_frame: f32, n: u64, window: &mut Window, cx: &mut App) {
    let vp = window.viewport_size();
    let down = (n / 600).is_multiple_of(2);
    let dy = if down { -px_per_frame } else { px_per_frame };
    let event = ScrollEvent {
        position: point(vp.width * 0.5, vp.height * 0.5),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    };
    window.dispatch_event(PlatformInput::Scroll(event), cx);
    window.on_next_frame(move |window, cx| bench_tick(px_per_frame, n + 1, window, cx));
}

impl Render for Review {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        stats::record_frame();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x16171a))
            .text_color(rgb(0xd6d8db))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.render_file_list(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_diff(cx)),
                    )
                    .child(self.render_comment_list(cx)),
            )
            .child(
                div()
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .text_xs()
                    .bg(rgb(0x202227))
                    .child(div().flex_1().child(self.status.clone()))
                    .child(self.perf.clone()),
            )
            .child(stats::FrameEndMarker)
    }
}
