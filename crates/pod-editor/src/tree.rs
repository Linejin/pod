//! Lazily loaded, `.gitignore`-aware file tree model (GUI-free).
//!
//! Only expanded directories are listed; the view renders [`FileTree::rows`]
//! with a virtual list, so huge repositories cost only what is visible.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::git::{GitMark, GitStatus};
use crate::uri::path_key;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub mark: Option<GitMark>,
}

/// What a key press or click asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAction {
    None,
    Open(PathBuf),
}

/// Lists one directory: ignore rules from `.gitignore`, `.ignore`,
/// `.git/info/exclude` and parent directories apply; dotfiles are shown;
/// `.git` itself is hidden. Directories first, then case-insensitive names.
pub fn list_dir(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for e in walker.flatten() {
        if e.depth() == 0 {
            continue;
        }
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().to_path_buf(),
            is_dir,
        });
    }
    sort_entries(&mut out);
    out
}

pub fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub struct FileTree {
    root: PathBuf,
    children: HashMap<String, Vec<Entry>>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    git: GitStatus,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        let mut t = FileTree {
            root,
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            git: GitStatus::default(),
        };
        let key = path_key(&t.root);
        t.expanded.insert(key);
        t.load(&t.root.clone());
        t.rebuild();
        t
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }
    pub fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.path == sel)
    }
    pub fn git(&self) -> &GitStatus {
        &self.git
    }

    pub fn set_git(&mut self, git: GitStatus) {
        self.git = git;
        self.rebuild();
    }

    fn load(&mut self, dir: &Path) {
        let list = list_dir(dir);
        self.children.insert(path_key(dir), list);
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(&path_key(dir))
    }

    /// Directories whose listing is cached (the root plus every directory
    /// expanded at least once): the watcher re-lists these.
    pub fn loaded_dirs(&self) -> Vec<PathBuf> {
        let mut v = vec![self.root.clone()];
        for r in &self.rows {
            if r.is_dir && r.expanded {
                v.push(r.path.clone());
            }
        }
        v
    }

    pub fn expand(&mut self, dir: &Path) {
        let k = path_key(dir);
        if !self.children.contains_key(&k) {
            self.load(dir);
        }
        self.expanded.insert(k);
        self.rebuild();
    }

    pub fn collapse(&mut self, dir: &Path) {
        if dir == self.root {
            return;
        }
        self.expanded.remove(&path_key(dir));
        self.rebuild();
    }

    pub fn toggle(&mut self, dir: &Path) {
        if self.is_expanded(dir) {
            self.collapse(dir)
        } else {
            self.expand(dir)
        }
    }

    /// Expands every ancestor of `path` (inside the root) and selects it.
    pub fn reveal(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return;
        };
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in comps.iter().take(comps.len().saturating_sub(1)) {
            cur.push(c);
            let k = path_key(&cur);
            if !self.children.contains_key(&k) {
                self.load(&cur);
            }
            self.expanded.insert(k);
        }
        self.selected = Some(path.to_path_buf());
        self.rebuild();
    }

    /// Re-lists cached directories affected by changed paths (their parents,
    /// or themselves). Unrelated expanded dirs keep their cached listing.
    pub fn refresh_paths(&mut self, changed: &[PathBuf]) -> bool {
        let mut dirs: HashSet<String> = HashSet::new();
        for p in changed {
            for cand in [Some(p.as_path()), p.parent()].into_iter().flatten() {
                let k = path_key(cand);
                if self.children.contains_key(&k) {
                    dirs.insert(k);
                }
            }
        }
        if dirs.is_empty() {
            return false;
        }
        let paths: Vec<PathBuf> = self
            .loaded_dirs()
            .into_iter()
            .chain(self.children.keys().map(PathBuf::from))
            .collect();
        let mut done = HashSet::new();
        for d in paths {
            let k = path_key(&d);
            if dirs.contains(&k) && done.insert(k.clone()) {
                // Keys are lowercase on Windows; list via a real path.
                let real = self.real_dir(&k).unwrap_or(d);
                if real.is_dir() {
                    self.load(&real);
                } else {
                    self.children.remove(&k);
                    self.expanded.remove(&k);
                }
            }
        }
        self.rebuild();
        true
    }

    /// Re-lists every cached directory.
    pub fn refresh_all(&mut self) {
        let dirs: Vec<(String, PathBuf)> = self
            .children
            .keys()
            .filter_map(|k| self.real_dir(k).map(|p| (k.clone(), p)))
            .collect();
        for (k, d) in dirs {
            if d.is_dir() {
                self.load(&d);
            } else {
                self.children.remove(&k);
            }
        }
        self.rebuild();
    }

    fn real_dir(&self, key: &str) -> Option<PathBuf> {
        if path_key(&self.root) == key {
            return Some(self.root.clone());
        }
        self.children
            .values()
            .flatten()
            .find(|e| e.is_dir && path_key(&e.path) == key)
            .map(|e| e.path.clone())
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.push_children(&self.root.clone(), 0, &mut rows);
        self.rows = rows;
        if let Some(sel) = self.selected.clone()
            && !self.rows.iter().any(|r| r.path == sel)
        {
            // Keep selection on the nearest visible ancestor.
            let mut p = sel.parent();
            self.selected = None;
            while let Some(a) = p {
                if self.rows.iter().any(|r| r.path == a) {
                    self.selected = Some(a.to_path_buf());
                    break;
                }
                p = a.parent();
            }
        }
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let Some(list) = self.children.get(&path_key(dir)) else {
            return;
        };
        for e in list {
            let expanded = e.is_dir && self.expanded.contains(&path_key(&e.path));
            rows.push(Row {
                path: e.path.clone(),
                name: e.name.clone(),
                depth,
                is_dir: e.is_dir,
                expanded,
                mark: self.git.mark(&e.path, e.is_dir),
            });
            if expanded {
                self.push_children(&e.path, depth + 1, rows);
            }
        }
    }

    pub fn select(&mut self, path: &Path) {
        self.selected = Some(path.to_path_buf());
    }

    pub fn select_index(&mut self, ix: usize) {
        if let Some(r) = self.rows.get(ix) {
            self.selected = Some(r.path.clone());
        }
    }

    /// Click / Enter on a row: toggles directories, opens files.
    pub fn activate(&mut self, ix: usize) -> TreeAction {
        let Some(r) = self.rows.get(ix).cloned() else {
            return TreeAction::None;
        };
        self.selected = Some(r.path.clone());
        if r.is_dir {
            self.toggle(&r.path);
            TreeAction::None
        } else {
            TreeAction::Open(r.path)
        }
    }

    pub fn move_up(&mut self) {
        let ix = self
            .selected_index()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.select_index(ix);
    }
    pub fn move_down(&mut self) {
        let ix = self
            .selected_index()
            .map(|i| (i + 1).min(self.rows.len().saturating_sub(1)))
            .unwrap_or(0);
        self.select_index(ix);
    }
    pub fn move_home(&mut self) {
        self.select_index(0);
    }
    pub fn move_end(&mut self) {
        self.select_index(self.rows.len().saturating_sub(1));
    }
    /// Right arrow: expand a collapsed dir, else step into its first child.
    pub fn move_right(&mut self) {
        let Some(ix) = self.selected_index() else {
            return self.select_index(0);
        };
        let r = self.rows[ix].clone();
        if r.is_dir && !r.expanded {
            self.expand(&r.path);
        } else if r.is_dir && self.rows.get(ix + 1).is_some_and(|n| n.depth > r.depth) {
            self.select_index(ix + 1);
        }
    }
    /// Left arrow: collapse an expanded dir, else go to the parent row.
    pub fn move_left(&mut self) {
        let Some(ix) = self.selected_index() else {
            return;
        };
        let r = self.rows[ix].clone();
        if r.is_dir && r.expanded {
            self.collapse(&r.path);
        } else if let Some(parent) = r.path.parent()
            && self.rows.iter().any(|x| x.path == parent)
        {
            self.selected = Some(parent.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/nested")).unwrap();
        std::fs::create_dir_all(r.join("target/debug")).unwrap();
        std::fs::create_dir_all(r.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::write(r.join(".gitignore"), "target/\nnode_modules\n*.log\n").unwrap();
        std::fs::write(r.join("src/.gitignore"), "secret.txt\n").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(r.join("src/secret.txt"), "x").unwrap();
        std::fs::write(r.join("src/nested/deep.rs"), "").unwrap();
        std::fs::write(r.join("README.md"), "").unwrap();
        std::fs::write(r.join("b.txt"), "").unwrap();
        std::fs::write(r.join("debug.log"), "").unwrap();
        std::fs::write(r.join(".env.example"), "").unwrap();
        d
    }

    fn names(t: &FileTree) -> Vec<String> {
        t.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn gitignore_respected_and_dirs_first() {
        let d = fixture();
        let t = FileTree::new(d.path().to_path_buf());
        assert_eq!(
            names(&t),
            vec!["src", ".env.example", ".gitignore", "b.txt", "README.md"]
        );
    }

    #[test]
    fn nested_gitignore_and_lazy_expand() {
        let d = fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        assert!(
            !t.rows().iter().any(|r| r.name == "main.rs"),
            "not loaded before expand"
        );
        t.expand(&d.path().join("src"));
        assert_eq!(
            names(&t)[..4],
            ["src", "  nested", "  .gitignore", "  main.rs"]
        );
        assert!(!names(&t).iter().any(|n| n.contains("secret")));
        t.collapse(&d.path().join("src"));
        assert_eq!(t.rows().len(), 5);
    }

    #[test]
    fn keyboard_navigation() {
        let d = fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        t.move_down();
        assert_eq!(t.selected_index(), Some(0));
        t.move_right(); // expand src
        assert!(t.rows()[0].expanded);
        t.move_right(); // into first child
        assert_eq!(t.rows()[t.selected_index().unwrap()].name, "nested");
        t.move_left(); // nested is collapsed -> parent
        assert_eq!(t.selected_index(), Some(0));
        t.move_left(); // collapse src
        assert!(!t.rows()[0].expanded);
        t.move_end();
        assert_eq!(t.rows()[t.selected_index().unwrap()].name, "README.md");
        let ix = t.selected_index().unwrap();
        assert_eq!(t.activate(ix), TreeAction::Open(d.path().join("README.md")));
    }

    #[test]
    fn reveal_and_refresh() {
        let d = fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        let deep = d.path().join("src/nested/deep.rs");
        t.reveal(&deep);
        assert_eq!(t.selected(), Some(deep.as_path()));
        assert!(t.rows().iter().any(|r| r.path == deep));
        std::fs::write(d.path().join("src/nested/new.rs"), "").unwrap();
        assert!(t.refresh_paths(&[d.path().join("src/nested/new.rs")]));
        assert!(t.rows().iter().any(|r| r.name == "new.rs"));
        std::fs::remove_dir_all(d.path().join("src/nested")).unwrap();
        t.refresh_paths(&[d.path().join("src/nested")]);
        assert!(!t.rows().iter().any(|r| r.name == "deep.rs"));
        assert_eq!(
            t.selected(),
            Some(d.path().join("src").as_path()),
            "selection falls back to ancestor"
        );
    }
}
