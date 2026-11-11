//! Pod UI: fixture-independent presentation, tokens and GPUI components.
pub mod agent;
pub mod components;
pub mod controls;
pub mod status;
pub mod terminal;
pub mod theme;
pub mod themes;
pub mod tokens;
pub use agent::{AgentMark, agent_mark, agent_mark_el, agent_stack};
pub use components::{
    BUSY_LABEL, ControlState, FocusReturn, FocusStack, Frame, FrameKind, Icon, IconButton,
    KEEP_ALL_MAX, ListSelection, ModalClose, ModalNext, ModalPrevious, TreeRow, banner, choice,
    count_badge, focus_style, hit_area, icon, icon_sized, icon_text, install_key_bindings,
    keep_all, keep_all_start, keep_all_unbreakable, keep_all_words, kit_icon, modal_keyboard,
    next_focus, radio_group, segment, segmented, semantic_button, status_dot, toast_timeout,
};
pub use controls::{
    A11y, CATEGORY_WIDTH, CONTROL_WIDTH, SelectEvent, category_item, category_list, select, switch,
};
pub use status::*;
pub use theme::apply_theme;
pub use tokens::{Mode, Tokens};

#[cfg(test)]
mod tests {
    use super::*;
    use pod_status::{AgentState as S, BlockedKind as B};
    #[test]
    fn every_status_has_unique_shape_and_label() {
        let states = [
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
        ];
        let mut shapes = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for state in states {
            let v = status_visual(state);
            assert!(shapes.insert(v.icon));
            assert!(labels.insert(v.label));
            assert!(!v.icon.bytes().is_empty());
        }
    }
    #[test]
    fn status_metadata_does_not_destroy_state() {
        let mut b = StatusBadge::fixture(S::Done);
        b.running_children = 3;
        assert!(b.label().contains("3"));
        b.stale_since = Some("2분 전".into());
        assert_eq!(b.state, S::Done);
        assert!(b.label().contains("연결 끊김"));
        b.state = S::Exited;
        b.exit_code = Some(7);
        assert!(b.label().contains("실패 종료 (7)"));
        assert!(!b.label().contains("하위"));
    }
    #[test]
    fn summary_deduplicates_and_preserves_mixed_lifecycle() {
        let done = StatusBadge::fixture(S::Done);
        let exited = StatusBadge::fixture(S::Exited);
        let summary = StatusSummary::from_terminals([("a", &done), ("a", &done), ("b", &exited)]);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.priority(), None);
        assert!(summary.label().contains("완료 1 · 종료 1"));
        let error = StatusBadge::fixture(S::Error);
        let blocked = StatusBadge::fixture(S::Blocked(B::Auth));
        assert_eq!(
            StatusSummary::from_terminals([("a", &error), ("b", &blocked)]).priority(),
            Some(S::Error)
        );
    }
    #[test]
    fn summary_exit_code_requires_agreement_and_known_codes() {
        let mut zero = StatusBadge::fixture(S::Exited);
        zero.exit_code = Some(0);
        let mut failed = zero.clone();
        failed.exit_code = Some(7);
        let mut unknown = zero.clone();
        unknown.exit_code = None;
        assert_eq!(
            StatusSummary::from_terminals([("a", &failed), ("b", &failed)]).exit_code,
            Some(7)
        );
        assert_eq!(
            StatusSummary::from_terminals([("a", &zero), ("b", &failed), ("c", &zero)]).exit_code,
            None
        );
        assert_eq!(
            StatusSummary::from_terminals([("a", &unknown), ("b", &zero)]).exit_code,
            None
        );
        assert_eq!(
            StatusSummary::from_terminals([("a", &zero), ("a", &failed)]).exit_code,
            Some(0)
        );
    }

    #[test]
    fn contrast_protects_readable_roles() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = Tokens::for_mode(mode);
            let surfaces = [t.bg_canvas, t.bg_chrome, t.bg_surface, t.bg_raised];
            let minimum = |fg| {
                surfaces
                    .iter()
                    .map(|bg| tokens::contrast(fg, *bg))
                    .fold(f64::INFINITY, f64::min)
            };
            assert!(minimum(t.text_muted) >= 4.5);
            assert!(tokens::contrast(t.text_muted, t.bg_hover) >= 4.5);
            assert!(tokens::contrast(t.text_muted, t.bg_selected) >= 4.5);
            assert!(minimum(t.border_control) >= 3.);
            assert!(minimum(t.accent_fg) >= 3.);
            let primary = [
                t.action_primary_bg,
                t.action_primary_hover,
                t.action_primary_pressed,
            ]
            .into_iter()
            .map(|bg| tokens::contrast(t.action_primary_fg, bg))
            .fold(f64::INFINITY, f64::min);
            assert!(primary >= 4.5);
            let semantic = [Tone::Success, Tone::Warning, Tone::Danger, Tone::Info]
                .into_iter()
                .map(|tone| {
                    let (fg, bg) = tone.colors(t);
                    tokens::contrast(fg, bg)
                })
                .fold(f64::INFINITY, f64::min);
            assert!(semantic >= 4.5);
            println!(
                "{mode:?}: muted {:.3}, control {:.3}, focus {:.3}, primary {:.3}, semantic {:.3}",
                minimum(t.text_muted),
                minimum(t.border_control),
                minimum(t.accent_fg),
                primary,
                semantic
            );
        }
    }
    #[test]
    fn terminal_indexed_and_truecolor_boundary() {
        let dark = terminal::TerminalPalette::for_mode(Mode::Dark);
        let light = terminal::TerminalPalette::for_mode(Mode::Light);
        assert_eq!(dark.indexed(16), 0);
        assert_eq!(dark.indexed(231), 0xffffff);
        for i in 16..=255 {
            assert_eq!(dark.indexed(i), light.indexed(i));
        }
    }

    #[test]
    fn child_blocked_overrides_done_but_not_exited() {
        let mut snapshot = pod_status::TerminalStatus::new(0).snapshot();
        snapshot.state = S::Done;
        snapshot.source = Some(pod_status::Source::Osc);
        snapshot.confidence = 0.9;
        snapshot.running_children = 2;
        snapshot.records.push(pod_status::SubRecord {
            id: "child".into(),
            state: S::Blocked(B::Question),
            source: pod_status::Source::Osc,
            title: None,
            msg: None,
            progress: None,
            since: 0,
        });
        let badge = StatusBadge::from_snapshot(&snapshot);
        assert_eq!(badge.state, S::Blocked(B::Question));
        assert_eq!(badge.root_state, S::Done);
        assert_eq!(badge.source, Some(pod_status::Source::Osc));
        snapshot.state = S::Exited;
        assert_eq!(StatusBadge::from_snapshot(&snapshot).state, S::Exited);
    }
}
