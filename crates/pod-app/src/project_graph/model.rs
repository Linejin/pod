//! The project agent graph as data: which agent terminals belong to one
//! registered repository, which recorded parent/child links join them, and
//! where each node sits. No drawing here (the panel renders [`ProjectGraph`]).
//!
//! Membership is explicit podd data only: a terminal belongs to a project when
//! its owning task (`TaskVm.terminals`, else `TerminalVm.task_id`) has
//! `TaskVm.repo == repo`, or, for a loose `pod run` terminal, when its recorded
//! `parent_task` / `parent_terminal` leads there. Names, ids, cwd and labels are
//! never matched by prefix.
//!
//! Edges are only the parent/child records podd keeps (one parent per node):
//! - [`EdgeKind::StartedTask`]: the task of this agent was created from that
//!   terminal (`TaskVm.parent_terminal`, the task's first terminal only);
//! - [`EdgeKind::RanTerminal`]: this terminal was started with `pod run` inside
//!   that terminal (`TerminalVm.parent_terminal`, protocol v7).
//!
//! Group membership and orchestration Runs are other notions and draw no edge
//! (a node names its group in its text instead).

use std::collections::{HashMap, HashSet};

use crate::agent_kind::AgentKind;
use crate::vm::{TaskVm, TerminalVm};

/// Which recorded relation an edge stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// `pod task new` / `pod group new` inside the parent terminal.
    StartedTask,
    /// `pod run` inside the parent terminal.
    RanTerminal,
}

/// Why a node's recorded parent draws no edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentNote {
    /// The parent id is not in podd's terminal list (removed, never seen).
    Missing(String),
    /// The parent is a plain shell terminal of this project (not a node).
    Shell(String),
    /// The parent belongs to another repository (or to none).
    OtherProject(String),
    /// The record names the node itself.
    SelfParent,
    /// Following parents leads back here; this link was dropped.
    Cycle(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    /// podd terminal id (unique in the graph).
    pub id: String,
    pub kind: AgentKind,
    /// The terminal's display name (`Workspace::label_of`).
    pub name: String,
    pub task: Option<String>,
    pub task_title: Option<String>,
    pub group: Option<String>,
    /// The terminal is in podd's list: it can be opened.
    pub present: bool,
    /// Index of the parent node (an edge joins them).
    pub parent: Option<usize>,
    /// Kind of the edge to [`Self::parent`].
    pub via: Option<EdgeKind>,
    pub note: Option<ParentNote>,
    pub children: Vec<usize>,
    /// Layer (0 = root).
    pub depth: usize,
    /// Horizontal slot in node widths (a parent is centred over its children).
    pub col: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
}

/// One repository's graph. `nodes` are in reading order: layer by layer, left
/// to right. The order depends on ids, creation times and parent records only,
/// never on agent status.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectGraph {
    pub repo: String,
    /// The repo is registered (`repo.list`) or named by a task.
    pub known: bool,
    /// Tasks of this repo.
    pub tasks: usize,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Terminal ids listed more than once (podd list or two tasks); the first wins.
    pub duplicates: Vec<String>,
    /// Width in node slots and depth in layers.
    pub cols: usize,
    pub rows: usize,
}

impl ProjectGraph {
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }
}

/// Where a terminal belongs: the first task listing it, else its `task_id`.
fn owner<'a>(t: &str, task_id: Option<&str>, tasks: &'a [TaskVm]) -> Option<&'a TaskVm> {
    tasks
        .iter()
        .find(|k| k.terminals.iter().any(|x| x == t))
        .or_else(|| task_id.and_then(|id| tasks.iter().find(|k| k.id == id)))
}

/// The repository a terminal belongs to, from explicit records only.
fn project_of<'a>(
    id: &str,
    tasks: &'a [TaskVm],
    terminals: &'a [TerminalVm],
    seen: &mut HashSet<String>,
) -> Option<&'a str> {
    if !seen.insert(id.to_string()) {
        return None; // parent cycle
    }
    let term = terminals.iter().find(|t| t.id == id);
    if let Some(k) = owner(id, term.and_then(|t| t.task_id.as_deref()), tasks) {
        return k.repo.as_deref();
    }
    let term = term?;
    if let Some(k) = term
        .parent_task
        .as_deref()
        .and_then(|p| tasks.iter().find(|k| k.id == p))
    {
        return k.repo.as_deref();
    }
    term.parent_terminal
        .as_deref()
        .filter(|p| *p != id)
        .and_then(|p| project_of(p, tasks, terminals, seen))
}

/// Build the graph of `repo` from the workspace snapshot.
pub fn build(
    repo: &str,
    registered: bool,
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    label: &dyn Fn(&TerminalVm) -> String,
) -> ProjectGraph {
    let in_repo = |id: &str| project_of(id, tasks, terminals, &mut HashSet::new()) == Some(repo);
    let project_tasks: Vec<&TaskVm> = tasks
        .iter()
        .filter(|k| k.repo.as_deref() == Some(repo))
        .collect();

    // Duplicate ids: in podd's list, or claimed by two tasks.
    let mut duplicates = Vec::new();
    {
        let mut seen = HashSet::new();
        for t in terminals {
            if !seen.insert(t.id.as_str()) && !duplicates.contains(&t.id) {
                duplicates.push(t.id.clone());
            }
        }
        let mut claimed: HashMap<&str, &str> = HashMap::new();
        for k in tasks {
            let mut own = HashSet::new();
            for x in &k.terminals {
                if !own.insert(x.as_str()) {
                    if !duplicates.contains(x) {
                        duplicates.push(x.clone());
                    }
                    continue;
                }
                match claimed.get(x.as_str()) {
                    Some(other) if *other != k.id && !duplicates.contains(x) => {
                        duplicates.push(x.clone())
                    }
                    Some(_) => {}
                    None => {
                        claimed.insert(x.as_str(), k.id.as_str());
                    }
                }
            }
        }
    }

    // Candidate nodes: agent terminals of this project (first record of an id).
    struct Cand {
        id: String,
        kind: AgentKind,
        name: String,
        task: Option<String>,
        task_title: Option<String>,
        group: Option<String>,
        present: bool,
        created: u64,
        /// Recorded parent terminal and the relation.
        parent: Option<(String, EdgeKind)>,
    }
    let mut cands: Vec<Cand> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    for t in terminals {
        if taken.contains(&t.id) || !in_repo(&t.id) {
            continue;
        }
        let k = owner(&t.id, t.task_id.as_deref(), tasks);
        let kind = crate::agent_kind::identify(t, k).kind;
        if !kind.is_agent() {
            continue;
        }
        taken.insert(t.id.clone());
        let first_of_task = k.is_some_and(|k| k.terminals.first() == Some(&t.id));
        let parent = k
            .filter(|_| first_of_task)
            .and_then(|k| k.parent_terminal.clone())
            .map(|p| (p, EdgeKind::StartedTask))
            .or_else(|| {
                t.parent_terminal
                    .clone()
                    .map(|p| (p, EdgeKind::RanTerminal))
            });
        cands.push(Cand {
            id: t.id.clone(),
            kind,
            name: label(t),
            task: k.map(|k| k.id.clone()),
            task_title: k.map(|k| k.title.clone()),
            group: k.and_then(|k| k.group.clone()),
            present: true,
            created: t.created_ms,
            parent,
        });
    }
    // A task's agent terminal podd no longer lists: shown, not openable.
    for k in &project_tasks {
        let Some(first) = k.terminals.first() else {
            continue;
        };
        if taken.contains(first) || terminals.iter().any(|t| &t.id == first) {
            continue;
        }
        let Some(kind) = k
            .agent
            .as_deref()
            .and_then(crate::agent_kind::from_task_agent)
        else {
            continue;
        };
        if owner(first, None, tasks).map(|o| o.id.as_str()) != Some(k.id.as_str()) {
            continue; // listed by an earlier task: that one owns it
        }
        taken.insert(first.clone());
        cands.push(Cand {
            id: first.clone(),
            kind,
            name: first.clone(),
            task: Some(k.id.clone()),
            task_title: Some(k.title.clone()),
            group: k.group.clone(),
            present: false,
            created: u64::MAX,
            parent: k
                .parent_terminal
                .clone()
                .map(|p| (p, EdgeKind::StartedTask)),
        });
    }
    // Stable order: creation time, then id (status never takes part).
    cands.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));

    let index: HashMap<String, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.clone(), i))
        .collect();
    let known_terminal = |id: &str| {
        terminals.iter().any(|t| t.id == id)
            || tasks.iter().any(|k| k.terminals.iter().any(|x| x == id))
    };
    let mut parent: Vec<Option<(usize, EdgeKind)>> = vec![None; cands.len()];
    let mut notes: Vec<Option<ParentNote>> = vec![None; cands.len()];
    for (i, c) in cands.iter().enumerate() {
        let Some((p, via)) = &c.parent else { continue };
        if *p == c.id {
            notes[i] = Some(ParentNote::SelfParent);
        } else if let Some(&j) = index.get(p) {
            parent[i] = Some((j, *via));
        } else if !terminals.iter().any(|t| &t.id == p) {
            notes[i] = Some(if known_terminal(p) && in_repo(p) {
                ParentNote::Missing(p.clone())
            } else if known_terminal(p) {
                ParentNote::OtherProject(p.clone())
            } else {
                ParentNote::Missing(p.clone())
            });
        } else if in_repo(p) {
            notes[i] = Some(ParentNote::Shell(p.clone()));
        } else {
            notes[i] = Some(ParentNote::OtherProject(p.clone()));
        }
    }
    // Cycles (one parent each, so each is a ring): drop the link of the
    // ring's first node in the stable order.
    {
        let mut state = vec![0u8; cands.len()]; // 0 new, 1 on the walk, 2 done
        for start in 0..cands.len() {
            let mut path = Vec::new();
            let mut at = Some(start);
            while let Some(i) = at {
                match state[i] {
                    2 => break,
                    1 => {
                        let ring: Vec<usize> =
                            path.iter().skip_while(|&&x| x != i).copied().collect();
                        let cut = *ring.iter().min().expect("ring");
                        let to = parent[cut].map(|(j, _)| cands[j].id.clone());
                        parent[cut] = None;
                        notes[cut] = to.map(ParentNote::Cycle);
                        break;
                    }
                    _ => {
                        state[i] = 1;
                        path.push(i);
                        at = parent[i].map(|(j, _)| j);
                    }
                }
            }
            for i in path {
                state[i] = 2;
            }
        }
    }

    // Layout: a forest, roots in stable order; leaves take the next column,
    // parents sit centred over their children.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); cands.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some((j, _)) = p {
            children[*j].push(i);
        }
    }
    let mut depth = vec![0usize; cands.len()];
    let mut col = vec![0f32; cands.len()];
    fn place(
        i: usize,
        d: usize,
        children: &[Vec<usize>],
        depth: &mut [usize],
        col: &mut [f32],
        next: &mut f32,
    ) {
        depth[i] = d;
        if children[i].is_empty() {
            col[i] = *next;
            *next += 1.;
            return;
        }
        for &c in &children[i] {
            place(c, d + 1, children, depth, col, next);
        }
        let (a, b) = (children[i][0], *children[i].last().expect("child"));
        col[i] = (col[a] + col[b]) / 2.;
    }
    let mut next = 0f32;
    for (i, p) in parent.iter().enumerate() {
        if p.is_none() {
            place(i, 0, &children, &mut depth, &mut col, &mut next);
        }
    }

    // Reading order: layer, then column (then the stable order).
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| {
        (depth[a], col[a])
            .partial_cmp(&(depth[b], col[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut at = vec![0usize; cands.len()];
    for (pos, &i) in order.iter().enumerate() {
        at[i] = pos;
    }
    let mut nodes: Vec<GraphNode> = Vec::with_capacity(cands.len());
    let mut cands: Vec<Option<Cand>> = cands.into_iter().map(Some).collect();
    for &i in &order {
        let c = cands[i].take().expect("each once");
        nodes.push(GraphNode {
            id: c.id,
            kind: c.kind,
            name: c.name,
            task: c.task,
            task_title: c.task_title,
            group: c.group,
            present: c.present,
            parent: parent[i].map(|(j, _)| at[j]),
            via: parent[i].map(|(_, k)| k),
            note: notes[i].take(),
            children: children[i].iter().map(|&c| at[c]).collect(),
            depth: depth[i],
            col: col[i],
        });
    }
    let edges = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            Some(GraphEdge {
                from: n.parent?,
                to: i,
                kind: n.via?,
            })
        })
        .collect();
    ProjectGraph {
        repo: repo.to_string(),
        known: registered || !project_tasks.is_empty(),
        tasks: project_tasks.len(),
        cols: next as usize,
        rows: nodes.iter().map(|n| n.depth + 1).max().unwrap_or(0),
        nodes,
        edges,
        duplicates,
    }
}

/// Keyboard movement over the graph (one tab stop, like the agent tree):
/// ↑ parent, ↓ first child, ←/→ neighbour in the same layer, Home/End.
/// With nothing selected any movement key picks the first node.
pub fn graph_key(g: &ProjectGraph, selected: Option<usize>, key: &str) -> Option<usize> {
    let last = g.nodes.len().checked_sub(1)?;
    let Some(i) = selected.filter(|i| *i <= last) else {
        return matches!(key, "down" | "up" | "left" | "right" | "home")
            .then_some(0)
            .or((key == "end").then_some(last));
    };
    let n = &g.nodes[i];
    let same_layer = |step: isize| {
        let mut j = i as isize + step;
        while (0..=last as isize).contains(&j) {
            if g.nodes[j as usize].depth == n.depth {
                return Some(j as usize);
            }
            j += step;
        }
        Some(i)
    };
    match key {
        "up" => Some(n.parent.unwrap_or(i)),
        "down" => Some(n.children.first().copied().unwrap_or(i)),
        "left" => same_layer(-1),
        "right" => same_layer(1),
        "home" => Some(0),
        "end" => Some(last),
        _ => None,
    }
}
