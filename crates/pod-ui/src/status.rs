//! Domain-preserving status presentation. UI staleness is never an AgentState.
use crate::components::{Icon, icon};
use crate::tokens::color;
use crate::{Mode, Tokens};
use gpui_kit::{Animation, AnimationExt, IntoElement, Transformation, radians};
use gpui_kit::{
    Div, InteractiveElement, ParentElement, SharedString, Stateful, StatefulInteractiveElement,
    Styled, div, px,
};
use pod_status::{AgentState, BlockedKind, Snapshot, Source};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Secondary,
    Info,
    Warning,
    Success,
    Danger,
}
impl Tone {
    pub fn colors(self, t: Tokens) -> (u32, u32) {
        match self {
            Self::Muted => (t.text_muted, t.bg_chrome),
            Self::Secondary => (t.text_secondary, t.bg_chrome),
            Self::Info => (t.info_fg, t.info_bg),
            Self::Warning => (t.warning_fg, t.warning_bg),
            Self::Success => (t.success_fg, t.success_bg),
            Self::Danger => (t.danger_fg, t.danger_bg),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusVisual {
    pub icon: Icon,
    pub label: &'static str,
    pub tone: Tone,
}
pub fn status_visual(state: AgentState) -> StatusVisual {
    let (icon, label, tone) = match state {
        AgentState::Idle => (Icon::Circle, "대기", Tone::Muted),
        AgentState::Working => (Icon::LoaderCircle, "작업 중", Tone::Info),
        AgentState::Blocked(BlockedKind::Permission) => {
            (Icon::ShieldQuestion, "승인 필요", Tone::Warning)
        }
        AgentState::Blocked(BlockedKind::Question) => {
            (Icon::MessageCircleQuestion, "답변 필요", Tone::Warning)
        }
        AgentState::Blocked(BlockedKind::Auth) => (Icon::LockKeyhole, "로그인 필요", Tone::Warning),
        AgentState::Blocked(BlockedKind::Other) => {
            (Icon::TriangleAlert, "확인 필요", Tone::Warning)
        }
        AgentState::Done => (Icon::CircleCheck, "완료", Tone::Success),
        AgentState::Error => (Icon::OctagonX, "오류", Tone::Danger),
        AgentState::Unknown => (Icon::CircleHelp, "상태 미확인", Tone::Secondary),
        AgentState::Exited => (Icon::Square, "종료", Tone::Muted),
    };
    StatusVisual { icon, label, tone }
}
#[derive(Clone, Debug)]
pub struct StatusBadge {
    pub state: AgentState,
    pub root_state: AgentState,
    pub source: Option<Source>,
    pub confidence: f32,
    pub running_children: usize,
    pub exit_code: Option<i32>,
    pub final_state: Option<AgentState>,
    pub stale_since: Option<String>,
}
impl StatusBadge {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            state: s.display_state(),
            root_state: s.state,
            source: s.source,
            confidence: s.confidence,
            running_children: s.running_children,
            exit_code: None,
            final_state: s.final_state,
            stale_since: None,
        }
    }
    pub fn fixture(state: AgentState) -> Self {
        Self {
            state,
            root_state: state,
            source: Some(Source::Hook),
            confidence: 0.95,
            running_children: 0,
            exit_code: None,
            final_state: None,
            stale_since: None,
        }
    }
    pub fn label(&self) -> String {
        let mut label = if self.state == AgentState::Exited {
            match self.exit_code {
                Some(c) if c != 0 => format!("실패 종료 ({c})"),
                Some(c) => format!("종료 ({c})"),
                None => "종료 중…".into(),
            }
        } else {
            status_visual(self.state).label.into()
        };
        if self.running_children > 0
            && self.state != AgentState::Working
            && self.state != AgentState::Exited
        {
            label.push_str(&format!(" · 하위 {} 작업 중", self.running_children));
        }
        if self.confidence < 0.8 && self.state != AgentState::Exited {
            label.push_str(" · 추정");
        }
        if let Some(last) = &self.stale_since {
            label.push_str(&format!(" · 연결 끊김 · 마지막 확인 {last}"));
        }
        label
    }
    pub fn detail(&self) -> String {
        format!(
            "{} · 출처: {} · 신뢰도: {:.0}% · 상위: {} · 마지막: {}",
            self.label(),
            match self.source {
                Some(Source::Hook) => "hook",
                Some(Source::Osc) => "osc",
                Some(Source::Heuristic) => "heuristic",
                Some(Source::Mod) => "mod",
                Some(Source::Process) => "process",
                None => "unknown",
            },
            self.confidence * 100.,
            status_visual(self.root_state).label,
            self.final_state
                .map(|s| status_visual(s).label)
                .unwrap_or("—")
        )
    }
    pub fn render(&self, mode: Mode) -> Stateful<Div> {
        self.render_with_motion(mode, true)
    }
    /// Colour and icon of this state (worker rows draw the icon on the left, docs/10 §D4.6).
    pub fn glyph(&self, mode: Mode) -> (Icon, u32) {
        let t = Tokens::for_mode(mode);
        let visual = status_visual(self.state);
        let fg = if self.state == AgentState::Exited && self.exit_code.is_some_and(|c| c != 0) {
            t.danger_fg
        } else {
            visual.tone.colors(t).0
        };
        (visual.icon, fg)
    }
    /// [`Self::render_compact`] without the leading icon (the row shows it on the left).
    pub fn render_compact_text(&self, mode: Mode) -> Stateful<Div> {
        self.compact(mode, false)
    }
    /// Dense rows retain the full provenance in their accessible name and tooltip.
    pub fn render_compact(&self, mode: Mode) -> Stateful<Div> {
        self.compact(mode, true)
    }
    fn compact(&self, mode: Mode, with_icon: bool) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let visual = status_visual(self.state);
        let fg = if self.state == AgentState::Exited && self.exit_code.is_some_and(|c| c != 0) {
            t.danger_fg
        } else {
            visual.tone.colors(t).0
        };
        let text = if self.state == AgentState::Unknown {
            "미확인".to_string()
        } else if self.state == AgentState::Exited {
            self.label()
        } else {
            format!(
                "{}{}",
                visual.label,
                if self.confidence < 0.8 { " ≈" } else { "" }
            )
        };
        let detail = self.detail();
        div()
            .id(SharedString::from(detail.clone()))
            .role(gpui_kit::Role::Status)
            .aria_label(detail.clone())
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(4.))
            .text_color(color(fg))
            .text_size(px(12.))
            .children(with_icon.then(|| icon(visual.icon, fg)))
            .child(text)
            .children(
                self.stale_since
                    .as_ref()
                    .map(|_| icon(Icon::WifiOff, t.text_secondary)),
            )
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                    .mt(px(28.))
                    .build(w, cx)
            })
    }
    /// Animate only visible working state; stale/reduced-motion stays a static arc.
    pub fn render_with_motion(&self, mode: Mode, reduced: bool) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let v = status_visual(self.state);
        let tone = if self.state == AgentState::Exited && self.exit_code.is_some_and(|c| c != 0) {
            Tone::Danger
        } else {
            v.tone
        };
        let (fg, bg) = tone.colors(t);
        let glyph = icon(v.icon, fg);
        let glyph = if self.state == AgentState::Working && self.stale_since.is_none() && !reduced {
            glyph
                .with_animation(
                    "working",
                    Animation::new(std::time::Duration::from_millis(crate::tokens::ACTIVITY_MS))
                        .repeat(),
                    |svg, phase| {
                        svg.with_transformation(Transformation::rotate(radians(
                            phase * std::f32::consts::TAU,
                        )))
                    },
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };
        let mut row = div()
            .id(SharedString::from(self.detail()))
            .role(gpui_kit::Role::Status)
            .aria_label(self.detail())
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(color(bg))
            .text_color(color(fg))
            .text_size(px(13.))
            .child(glyph)
            .child(self.label());
        if self.stale_since.is_some() {
            row = row.child(icon(Icon::WifiOff, t.text_secondary));
        }
        row
    }
}
/// Counts unique terminal IDs; does not also count their embedded subrecords.
#[derive(Default, Clone, Debug)]
pub struct StatusSummary {
    pub total: usize,
    pub working: usize,
    pub blocked: usize,
    pub error: usize,
    pub unknown: usize,
    pub idle: usize,
    pub done: usize,
    pub exited: usize,
    /// Common code only when all exited terminals have the same known code.
    pub exit_code: Option<i32>,
}
impl StatusSummary {
    pub fn from_terminals<'a>(rows: impl IntoIterator<Item = (&'a str, &'a StatusBadge)>) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut s = Self::default();
        for (id, b) in rows {
            if !seen.insert(id) {
                continue;
            }
            s.total += 1;
            match b.state {
                AgentState::Working => s.working += 1,
                AgentState::Blocked(_) => s.blocked += 1,
                AgentState::Error => s.error += 1,
                AgentState::Unknown => s.unknown += 1,
                AgentState::Idle => s.idle += 1,
                AgentState::Done => s.done += 1,
                AgentState::Exited => {
                    s.exit_code = if s.exited == 0 || s.exit_code == b.exit_code {
                        b.exit_code
                    } else {
                        None
                    };
                    s.exited += 1;
                }
            }
        }
        s
    }
    /// Presentation priority from design v1, intentionally not engine aggregate().
    pub fn priority(&self) -> Option<AgentState> {
        if self.total == 0 {
            return None;
        }
        for (count, state) in [
            (self.error, AgentState::Error),
            (self.blocked, AgentState::Blocked(BlockedKind::Other)),
            (self.working, AgentState::Working),
            (self.unknown, AgentState::Unknown),
            (self.idle, AgentState::Idle),
        ] {
            if count > 0 {
                return Some(state);
            }
        }
        if self.done == self.total {
            Some(AgentState::Done)
        } else if self.exited == self.total {
            Some(AgentState::Exited)
        } else {
            None
        }
    }
    pub fn label(&self) -> String {
        format!(
            "작업 중 {} · 응답 {} · 오류 {} · 미확인 {} · 대기 {} · 완료 {} · 종료 {}",
            self.working, self.blocked, self.error, self.unknown, self.idle, self.done, self.exited
        )
    }
    pub fn response_count(&self) -> String {
        if self.blocked > 99 {
            "99+".into()
        } else {
            self.blocked.to_string()
        }
    }
    pub fn render(&self, mode: Mode) -> Div {
        div()
            .text_size(px(12.))
            .text_color(color(Tokens::for_mode(mode).text_secondary))
            .child(self.label())
    }
}
