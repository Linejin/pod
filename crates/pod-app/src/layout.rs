//! App-side persistence: dock layout (which terminal is in which dock slot)
//! and app-local terminal labels, in `%APPDATA%\pod\pod-app\layout.json`.
//!
//! The dock layout is gpui-component's own `DockAreaState`. Panels persist as
//! `PodTerminal {"terminal": id}`, `PodReview {"task": id}`, `PodGroup
//! {"group": id}`, `PodGates {}`, `PodAgentTree {}` and `PodEditor {"task":
//! id, "session": {tabs: [{path, line, col}], active}}`. On restore, panels
//! whose target no longer exists (or whose daemon feature is missing) are
//! pruned, and emptied containers with them, before the dock loads the tree
//! (center and the left/right/bottom docks).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use gpui_kit::base::dock::{DockAreaState, DockState, PanelInfo, PanelState};
use serde::{Deserialize, Serialize};

use crate::editor_panel::EDITOR_PANEL_NAME;
use crate::gates::GATES_PANEL_NAME;
use crate::groups::GROUP_PANEL_NAME;
use crate::panel::PANEL_NAME;
#[cfg(test)]
use crate::panel::TerminalPanel;
use crate::review::REVIEW_PANEL_NAME;
use crate::tree::TREE_PANEL_NAME;

/// Bump when the stored shape changes incompatibly (old files are ignored).
pub const LAYOUT_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock: Option<DockAreaState>,
    /// App-local names (used while podd has no `terminal.rename`), keyed by
    /// [`label_key`] so a reused id after `rm` does not inherit a label.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

pub fn label_key(id: &str, created_ms: u64) -> String {
    format!("{id}@{created_ms}")
}

#[cfg(test)]
thread_local! {
    /// A test's own config dir (settings, layout.json): tests that persist
    /// and restart run in parallel with others in one process.
    pub(crate) static TEST_CONFIG_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn test_config_dir_set() -> bool {
    TEST_CONFIG_DIR.with(|d| d.borrow().is_some())
}
#[cfg(test)]
pub(crate) fn set_test_config_dir(dir: Option<PathBuf>) {
    TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = dir);
}

/// `%APPDATA%\pod\pod-app` (or `POD_APP_CONFIG_DIR`).
pub fn config_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(d) = TEST_CONFIG_DIR.with(|d| d.borrow().clone()) {
        return d;
    }
    if let Some(d) = std::env::var_os("POD_APP_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("pod").join("pod-app")
}

pub fn layout_path() -> PathBuf {
    config_dir().join("layout.json")
}

pub fn load() -> Saved {
    let Ok(text) = std::fs::read_to_string(layout_path()) else {
        return Saved::default();
    };
    match serde_json::from_str::<Saved>(&text) {
        Ok(s) if s.version == LAYOUT_VERSION => s,
        _ => Saved::default(),
    }
}

pub fn save(saved: &Saved) -> std::io::Result<()> {
    let path = layout_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(saved).map_err(std::io::Error::other)?;
    // Write-then-rename so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

/// Terminal ids referenced by a layout tree, in layout order.
#[cfg(test)]
pub fn terminals_in(state: &PanelState) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(s: &PanelState, out: &mut Vec<String>) {
        if s.panel_name == PANEL_NAME {
            out.extend(TerminalPanel::terminal_of_state(s));
        }
        for c in &s.children {
            walk(c, out);
        }
    }
    walk(state, &mut out);
    out
}

/// What exists when a saved layout is restored.
#[derive(Clone, Debug, Default)]
pub struct Alive {
    pub terminals: HashSet<String>,
    pub tasks: HashSet<String>,
    pub groups: HashSet<String>,
    /// Daemon has `review.*` (protocol v5).
    pub review: bool,
    /// Daemon has groups/gates (protocol v6).
    pub orch: bool,
    /// Tasks whose worktree exists on disk (editor panels need one).
    pub worktrees: HashSet<String>,
}

/// String field of a leaf's `PanelInfo::Panel` JSON.
pub fn leaf_field<'a>(state: &'a PanelState, key: &str) -> Option<&'a str> {
    match &state.info {
        PanelInfo::Panel(v) => v.get(key).and_then(|t| t.as_str()),
        _ => None,
    }
}

/// Whether a persisted leaf can be rebuilt: its terminal/task/group still
/// exists and the daemon has the feature. Unknown panel names are dropped.
fn keep_leaf(state: &PanelState, alive: &Alive) -> bool {
    let has =
        |set: &HashSet<String>, key: &str| leaf_field(state, key).is_some_and(|v| set.contains(v));
    match state.panel_name.as_str() {
        PANEL_NAME => has(&alive.terminals, "terminal"),
        REVIEW_PANEL_NAME => alive.review && has(&alive.tasks, "task"),
        GROUP_PANEL_NAME => alive.orch && has(&alive.groups, "group"),
        GATES_PANEL_NAME => alive.orch,
        EDITOR_PANEL_NAME => has(&alive.worktrees, "task"),
        TREE_PANEL_NAME | "PodSettings" | "PodInspector" => true,
        _ => false,
    }
}

/// Drop leaves that cannot be rebuilt ([`keep_leaf`]), then containers left
/// empty. `None` when nothing is left.
pub fn prune(state: &PanelState, alive: &Alive) -> Option<PanelState> {
    match &state.info {
        PanelInfo::Panel(_) => keep_leaf(state, alive).then(|| state.clone()),
        PanelInfo::Tabs { active_index } => {
            let kept: Vec<(usize, PanelState)> = state
                .children
                .iter()
                .enumerate()
                .filter_map(|(i, c)| prune(c, alive).map(|c| (i, c)))
                .collect();
            if kept.is_empty() {
                return None;
            }
            // Keep the same panel active when it survived, else the first.
            let active = kept
                .iter()
                .position(|(i, _)| i == active_index)
                .unwrap_or(0);
            Some(PanelState {
                panel_name: state.panel_name.clone(),
                children: kept.into_iter().map(|(_, c)| c).collect(),
                info: PanelInfo::Tabs {
                    active_index: active,
                },
            })
        }
        PanelInfo::Stack { sizes, axis } => {
            let mut children = Vec::new();
            let mut new_sizes = Vec::new();
            for (i, c) in state.children.iter().enumerate() {
                if let Some(c) = prune(c, alive) {
                    children.push(c);
                    if let Some(s) = sizes.get(i) {
                        new_sizes.push(*s);
                    }
                }
            }
            if children.is_empty() {
                return None;
            }
            if new_sizes.len() != children.len() {
                new_sizes.clear();
            }
            Some(PanelState {
                panel_name: state.panel_name.clone(),
                children,
                info: PanelInfo::Stack {
                    sizes: new_sizes,
                    axis: *axis,
                },
            })
        }
    }
}

fn prune_side(dock: &Option<DockState>, alive: &Alive) -> Option<DockState> {
    let d = dock.as_ref()?;
    let panel = prune(d.panel(), alive)?;
    Some(DockState::new(panel, d.placement(), d.size(), d.open()))
}

/// Prune the whole dock state: center and side docks. `None` when nothing is
/// left anywhere (the app then starts with its default layout).
pub fn prune_dock(state: &DockAreaState, alive: &Alive) -> Option<DockAreaState> {
    let center = prune(&state.center, alive);
    let (left_dock, right_dock, bottom_dock) = (
        prune_side(&state.left_dock, alive),
        prune_side(&state.right_dock, alive),
        prune_side(&state.bottom_dock, alive),
    );
    if center.is_none() && left_dock.is_none() && right_dock.is_none() && bottom_dock.is_none() {
        return None;
    }
    // Only docks survived: an empty center split, like a fresh dock area.
    let center = center.unwrap_or_else(|| PanelState {
        panel_name: "StackPanel".into(),
        children: Vec::new(),
        info: PanelInfo::stack(Vec::new(), gpui_kit::Axis::Horizontal),
    });
    Some(DockAreaState {
        version: state.version,
        center,
        left_dock,
        right_dock,
        bottom_dock,
    })
}

/// Rows x cols for `n` panels arranged as a near-square grid (2x2 for 4, 3x2 for 5 …).
pub fn grid_shape(n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    (rows, cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Axis, px};
    use serde_json::json;

    fn leaf(id: &str) -> PanelState {
        TerminalPanel::state_for(id)
    }

    fn tabs(active: usize, children: Vec<PanelState>) -> PanelState {
        PanelState {
            panel_name: "TabPanel".into(),
            children,
            info: PanelInfo::tabs(active),
        }
    }

    fn stack(axis: Axis, children: Vec<PanelState>) -> PanelState {
        let sizes = children.iter().map(|_| px(100.)).collect();
        PanelState {
            panel_name: "StackPanel".into(),
            children,
            info: PanelInfo::stack(sizes, axis),
        }
    }

    fn alive(ids: &[&str]) -> Alive {
        Alive {
            terminals: ids.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn named(name: &str, info: serde_json::Value) -> PanelState {
        let mut s = PanelState::new(name);
        s.info = PanelInfo::panel(info);
        s
    }

    #[test]
    fn prune_drops_dead_terminals_and_empty_containers() {
        let tree = stack(
            Axis::Vertical,
            vec![
                stack(
                    Axis::Horizontal,
                    vec![
                        tabs(1, vec![leaf("t1"), leaf("t2")]),
                        tabs(0, vec![leaf("t3")]),
                    ],
                ),
                stack(Axis::Horizontal, vec![tabs(0, vec![leaf("t4")])]),
            ],
        );
        assert_eq!(terminals_in(&tree), ["t1", "t2", "t3", "t4"]);

        let p = prune(&tree, &alive(&["t2", "t4"])).unwrap();
        assert_eq!(terminals_in(&p), ["t2", "t4"]);
        // First row lost its second group; sizes follow the children.
        let row = &p.children[0];
        assert_eq!(row.children.len(), 1);
        assert_eq!(row.info.sizes().unwrap().len(), 1);
        // t2 was active and stays active (now index 0).
        assert_eq!(row.children[0].info.active_index(), Some(0));

        assert!(prune(&tree, &alive(&[])).is_none());
        assert_eq!(
            prune(&tree, &alive(&["t1", "t2", "t3", "t4"])).unwrap(),
            tree
        );
    }

    #[test]
    fn active_tab_falls_back_to_first() {
        let t = tabs(1, vec![leaf("t1"), leaf("t2")]);
        let p = prune(&t, &alive(&["t1"])).unwrap();
        assert_eq!(p.info.active_index(), Some(0));
        assert_eq!(terminals_in(&p), ["t1"]);
    }

    #[test]
    fn unknown_leaves_are_dropped() {
        let t = tabs(0, vec![PanelState::new("Other"), leaf("t1")]);
        assert_eq!(terminals_in(&prune(&t, &alive(&["t1"])).unwrap()), ["t1"]);
    }

    /// E2E B7: the layout saved during the E2E run (docs/e2e/layout-before.json):
    /// t1, t2, Review, Group tabs in the center; Gates + Agents in the right dock.
    #[test]
    fn restores_review_group_gates_agents_and_right_dock() {
        let saved: Saved = serde_json::from_str(
            include_str!("../../../docs/e2e/layout-before.json").trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let dock = saved.dock.unwrap();
        let mut a = alive(&["t1", "t2"]);
        a.tasks = ["sample/cli-fix".to_string()].into();
        a.groups = ["sample/duel".to_string()].into();
        (a.review, a.orch) = (true, true);
        let p = prune_dock(&dock, &a).unwrap();
        assert_eq!(
            p, dock,
            "everything still exists: layout restored unchanged"
        );
        let names = |s: &PanelState| {
            s.children
                .iter()
                .map(|c| c.panel_name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&p.center.children[0]),
            ["PodTerminal", "PodTerminal", "PodReview", "PodGroup"]
        );
        let right = p.right_dock.as_ref().unwrap();
        assert_eq!(names(right.panel()), ["PodGates", "PodAgentTree"]);
        assert!(right.open());

        // Task and group deleted meanwhile: their panels are dropped, the rest stays.
        let mut gone = a.clone();
        gone.tasks.clear();
        gone.groups.clear();
        let p = prune_dock(&dock, &gone).unwrap();
        assert_eq!(names(&p.center.children[0]), ["PodTerminal", "PodTerminal"]);
        // Group tab (index 3) was active; falls back to the first.
        assert_eq!(p.center.children[0].info.active_index(), Some(0));
        assert_eq!(
            names(p.right_dock.as_ref().unwrap().panel()),
            ["PodGates", "PodAgentTree"]
        );

        // Older daemon without groups/gates: Gates and Group go, Agents stays.
        let mut old = a.clone();
        old.orch = false;
        let p = prune_dock(&dock, &old).unwrap();
        assert_eq!(
            names(&p.center.children[0]),
            ["PodTerminal", "PodTerminal", "PodReview"]
        );
        assert_eq!(
            names(p.right_dock.as_ref().unwrap().panel()),
            ["PodAgentTree"]
        );
        assert_eq!(
            p.right_dock.as_ref().unwrap().panel().info.active_index(),
            Some(0)
        );

        // No terminals left: the right dock still comes back around an empty center.
        let p = prune_dock(
            &dock,
            &Alive {
                orch: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(p.center.children.is_empty());
        assert!(p.right_dock.is_some());
    }

    #[test]
    fn review_and_group_need_their_target() {
        let t = tabs(
            0,
            vec![
                named("PodReview", json!({"task": "k1"})),
                named("PodGroup", json!({"group": "g1"})),
                named("PodReview", json!({})),
            ],
        );
        let mut a = alive(&[]);
        (a.review, a.orch) = (true, true);
        assert!(prune(&t, &a).is_none());
        a.tasks.insert("k1".into());
        a.groups.insert("g1".into());
        let p = prune(&t, &a).unwrap();
        assert_eq!(
            p.children.len(),
            2,
            "the review without a task id is dropped"
        );
        assert!(
            prune_dock(
                &DockAreaState {
                    center: t,
                    ..Default::default()
                },
                &alive(&[])
            )
            .is_none()
        );
    }

    #[test]
    fn saved_roundtrip() {
        let mut s = Saved {
            version: LAYOUT_VERSION,
            ..Default::default()
        };
        s.dock = Some(DockAreaState {
            center: stack(Axis::Horizontal, vec![tabs(0, vec![leaf("t1")])]),
            ..Default::default()
        });
        s.labels.insert(label_key("t1", 42), "build".into());
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), s);
    }

    #[test]
    fn chrome_panels_survive_without_daemon_targets() {
        let t = tabs(
            1,
            vec![
                named("PodSettings", json!({})),
                named("PodInspector", json!({})),
            ],
        );
        let restored = prune(&t, &Alive::default()).expect("local panels survive");
        assert_eq!(restored, t);
    }

    #[test]
    fn editor_panels_need_an_existing_worktree() {
        let ed = |task: &str| crate::editor_panel::EditorPanel::state_for_test(task);
        let t = tabs(1, vec![leaf("t1"), ed("k1"), ed("k2")]);
        let mut a = alive(&["t1"]);
        a.tasks = ["k1".to_string(), "k2".to_string()].into();
        a.worktrees = ["k2".to_string()].into();
        let p = prune(&t, &a).unwrap();
        let names: Vec<_> = p
            .children
            .iter()
            .map(|c| {
                (
                    c.panel_name.clone(),
                    leaf_field(c, "task").map(str::to_owned),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("PodTerminal".to_string(), None),
                ("PodEditor".to_string(), Some("k2".to_string()))
            ]
        );
        // k1 (active, index 1) was dropped: the first panel becomes active.
        assert_eq!(p.info.active_index(), Some(0));
    }

    #[test]
    fn grid() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(5), (2, 3));
        assert_eq!(grid_shape(16), (4, 4));
    }
}
