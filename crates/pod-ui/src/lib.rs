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
pub use components::*;
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
    fn icons_parse_and_follow_the_style_stroke() {
        for i in Icon::ALL {
            let svg = std::str::from_utf8(i.source()).unwrap();
            assert!(
                svg.contains("<svg") && svg.contains("stroke-width=\"2\""),
                "{}",
                i.name()
            );
            assert!(
                !svg.contains("<script") && !svg.contains("href"),
                "{}",
                i.name()
            );
        }
        let thin = std::str::from_utf8(components::stroked_for_test(Icon::X, 1.5)).unwrap();
        assert!(thin.contains("stroke-width=\"1.5\""));
        assert!(
            std::ptr::eq(
                components::stroked_for_test(Icon::X, 1.5),
                components::stroked_for_test(Icon::X, 1.5)
            ),
            "cached"
        );
        assert!(
            std::ptr::eq(components::stroked_for_test(Icon::X, 2.), Icon::X.source()),
            "2px is the source"
        );
    }
    #[test]
    fn nested_focus_round_trip_and_keyboard_wrap() {
        let mut stack = FocusStack::default();
        stack.enter("terminal");
        stack.enter("palette-query");
        assert_eq!(stack.leave(), Some("palette-query"));
        assert_eq!(stack.leave(), Some("terminal"));
        assert_eq!(stack.leave(), None);
        assert_eq!(next_focus(Some(2), 3, false), 0);
        assert_eq!(next_focus(Some(0), 3, true), 2);
        assert_eq!(next_focus(None, 3, false), 0);
        assert_eq!(next_focus(None, 3, true), 2);
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
    fn toast_pause_and_persistent_attention() {
        assert!(toast_timeout(Tone::Warning, false, false).is_none());
        assert!(toast_timeout(Tone::Danger, false, false).is_none());
        assert!(toast_timeout(Tone::Success, true, false).is_none());
        assert!(toast_timeout(Tone::Info, false, true).is_none());
        assert_eq!(
            toast_timeout(Tone::Success, false, false)
                .unwrap()
                .as_secs(),
            5
        );
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
    #[test]
    fn list_selection_survives_refresh_and_ime_never_executes() {
        let mut list = ListSelection::default();
        list.replace(vec!["a".into(), "b".into()]);
        list.navigate(false);
        assert_eq!(list.selected.as_deref(), Some("b"));
        list.replace(vec!["b".into(), "c".into()]);
        assert_eq!(list.selected.as_deref(), Some("b"));
        assert_eq!(list.activation(true, &ControlState::Ready), None);
        assert_eq!(list.activation(false, &ControlState::Loading), None);
        assert_eq!(list.activation(false, &ControlState::Ready), Some("b"));
    }
}
