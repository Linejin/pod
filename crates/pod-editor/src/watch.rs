//! Debounced recursive file watcher for a worktree root.
//!
//! Raw `notify` events are coalesced on a helper thread into batches after
//! [`DEBOUNCE`] of quiet. Ignored paths (every `.gitignore` from the root
//! down, `info/exclude` and the global excludes file, as the index walk
//! applies them; e.g. `target/`, `frontend/dist/`) are dropped so a build
//! does not trigger tree or git refreshes. Changes to the worktree's git
//! `index`/`HEAD` and to branch refs are reported as `git_changed`
//! (stage/commit/checkout done by an agent). For a linked worktree (every
//! Pod task: `.git` is a file) those live outside the root, in
//! `<repo>/.git/worktrees/<name>` and `<repo>/.git/refs`, and are watched
//! there.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{RecursiveMode, Watcher as _};

use crate::git::{GitDirs, git_dirs};

pub const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    /// Changed paths outside `.git`, de-duplicated.
    pub paths: Vec<PathBuf>,
    pub git_changed: bool,
}

pub struct RootWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Ignore rules for watcher events, the same the index walk applies:
/// nested `.gitignore` files (deepest first; a `!` rule re-includes),
/// then `info/exclude`, then the global excludes file.
pub struct IgnoreRules {
    root: PathBuf,
    /// Per directory: its `.gitignore` (None: none there). Loaded on demand.
    per_dir: HashMap<PathBuf, Option<Gitignore>>,
    extra: Vec<Gitignore>,
}

impl IgnoreRules {
    pub fn new(root: &Path, git: Option<&GitDirs>) -> Self {
        let mut extra = Vec::new();
        if let Some(g) = git {
            let mut b = GitignoreBuilder::new(root);
            // A linked worktree reads `info/exclude` from the common dir.
            for dir in [&g.git_dir, &g.common_dir] {
                let f = dir.join("info").join("exclude");
                if f.is_file() {
                    b.add(f);
                }
            }
            if let Ok(gi) = b.build() {
                extra.push(gi);
            }
        }
        extra.push(GitignoreBuilder::new(root).build_global().0);
        IgnoreRules {
            root: root.to_path_buf(),
            per_dir: HashMap::new(),
            extra,
        }
    }

    /// A `.gitignore` changed: read it again next time.
    pub fn forget(&mut self, gitignore: &Path) {
        if let Some(dir) = gitignore.parent() {
            self.per_dir.remove(dir);
        }
    }

    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        if !path.starts_with(&self.root) || path == self.root {
            return false;
        }
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(&self.root) {
                break;
            }
            let gi = self.per_dir.entry(d.to_path_buf()).or_insert_with(|| {
                let f = d.join(".gitignore");
                f.is_file().then(|| {
                    let mut b = GitignoreBuilder::new(d);
                    b.add(f);
                    b.build().unwrap_or_else(|_| Gitignore::empty())
                })
            });
            if let Some(gi) = gi {
                let m = gi.matched_path_or_any_parents(path, is_dir);
                if m.is_ignore() {
                    return true;
                }
                if m.is_whitelist() {
                    return false;
                }
            }
            if d == self.root {
                break;
            }
            dir = d.parent();
        }
        self.extra
            .iter()
            .any(|gi| gi.matched_path_or_any_parents(path, is_dir).is_ignore())
    }
}

/// Classifies one raw path. Returns `None` to drop it, `Err(())` for a git
/// metadata change.
pub fn classify(
    root: &Path,
    git: Option<&GitDirs>,
    rules: &mut IgnoreRules,
    path: &Path,
) -> Option<Result<PathBuf, ()>> {
    if let Some(g) = git
        && !path.starts_with(root)
    {
        return is_git_meta(g, path).then_some(Err(()));
    }
    let rel = path.strip_prefix(root).ok()?;
    let mut comps = rel.components();
    if let Some(first) = comps.next()
        && first.as_os_str().eq_ignore_ascii_case(".git")
    {
        let rest = comps
            .as_path()
            .to_string_lossy()
            .to_lowercase()
            .replace('\\', "/");
        return (rest == "index"
            || rest == "head"
            || rest == "packed-refs"
            || rest.starts_with("refs"))
        .then_some(Err(()));
    }
    if path.file_name().is_some_and(|n| n == ".gitignore") {
        rules.forget(path);
    }
    let is_dir = path.is_dir();
    if rules.is_ignored(path, is_dir) {
        return None;
    }
    Some(Ok(path.to_path_buf()))
}

/// The linked worktree's own `index`/`HEAD`, or a branch ref of its repo.
fn is_git_meta(g: &GitDirs, path: &Path) -> bool {
    let rel_lower = |base: &Path| {
        path.strip_prefix(base)
            .ok()
            .map(|r| r.to_string_lossy().to_lowercase().replace('\\', "/"))
    };
    if let Some(r) = rel_lower(&g.git_dir)
        && (r == "index" || r == "head")
    {
        return true;
    }
    rel_lower(&g.common_dir).is_some_and(|r| r == "packed-refs" || r.starts_with("refs/"))
}

/// The git dirs of `root` when they are outside it (linked worktree).
fn outside_git_dirs(root: &Path) -> Option<GitDirs> {
    git_dirs(root).filter(|g| !g.git_dir.starts_with(root))
}

impl RootWatcher {
    pub fn start(root: &Path) -> notify::Result<(RootWatcher, UnboundedReceiver<Batch>)> {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |e| {
            let _ = raw_tx.send(e);
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        let linked = outside_git_dirs(root);
        if let Some(g) = &linked {
            // Best effort: a missing refs folder just means no ref events.
            let _ = watcher.watch(&g.git_dir, RecursiveMode::NonRecursive);
            let _ = watcher.watch(&g.common_dir, RecursiveMode::NonRecursive);
            let _ = watcher.watch(&g.common_dir.join("refs"), RecursiveMode::Recursive);
        }
        let (tx, rx) = unbounded();
        let root = root.to_path_buf();
        std::thread::Builder::new()
            .name("pod-editor-watch".into())
            .spawn(move || debounce_loop(root, linked, raw_rx, tx))
            .ok();
        Ok((RootWatcher { _watcher: watcher }, rx))
    }
}

fn debounce_loop(
    root: PathBuf,
    linked: Option<GitDirs>,
    raw: mpsc::Receiver<notify::Result<notify::Event>>,
    tx: UnboundedSender<Batch>,
) {
    let dirs = linked.clone().or_else(|| git_dirs(&root));
    let mut rules = IgnoreRules::new(&root, dirs.as_ref());
    let mut batch = Batch::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let msg = match deadline {
            Some(d) => raw.recv_timeout(d.saturating_duration_since(Instant::now())),
            None => raw.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Ok(ev)) => {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    continue;
                }
                for p in ev.paths {
                    match classify(&root, linked.as_ref(), &mut rules, &p) {
                        Some(Ok(p)) => {
                            if seen.insert(p.clone()) {
                                batch.paths.push(p);
                            }
                        }
                        Some(Err(())) => batch.git_changed = true,
                        None => continue,
                    }
                    deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                }
            }
            Ok(Err(_)) => {
                // Overflow or watcher error: ask for a full refresh.
                if seen.insert(root.clone()) {
                    batch.paths.push(root.clone());
                }
                batch.git_changed = true;
                deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                deadline = None;
                seen.clear();
                let b = std::mem::take(&mut batch);
                if (!b.paths.is_empty() || b.git_changed) && tx.unbounded_send(b).is_err() {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_filters_ignored_and_git() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        let mut rules = IgnoreRules::new(r, None);
        assert_eq!(
            classify(r, None, &mut rules, &r.join("src/a.rs")),
            Some(Ok(r.join("src/a.rs")))
        );
        assert_eq!(
            classify(r, None, &mut rules, &r.join("target/debug/x.o")),
            None
        );
        assert_eq!(
            classify(r, None, &mut rules, &r.join(".git/index")),
            Some(Err(()))
        );
        assert_eq!(
            classify(r, None, &mut rules, &r.join(".git/objects/ab/cd")),
            None
        );
    }

    /// editor-10: a nested `.gitignore` (monorepo `frontend/dist`) and
    /// `info/exclude` drop events like the index walk does; `!` re-includes.
    #[test]
    fn nested_gitignore_and_exclude() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("frontend/dist")).unwrap();
        std::fs::create_dir_all(r.join(".git/info")).unwrap();
        std::fs::write(r.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(r.join("frontend/.gitignore"), "dist/\n!keep.log\n").unwrap();
        std::fs::write(r.join(".git/info/exclude"), "scratch/\n").unwrap();
        let g = git_dirs(r);
        let mut rules = IgnoreRules::new(r, g.as_ref());
        let mut c = |p: &str| classify(r, None, &mut rules, &r.join(p)).is_some();
        assert!(!c("frontend/dist/app.js"));
        assert!(!c("a.log"));
        assert!(c("frontend/keep.log"), "re-included by the nested file");
        assert!(!c("scratch/x"));
        assert!(c("frontend/src/app.ts"));
        // The nested file changes: read again.
        std::fs::write(r.join("frontend/.gitignore"), "").unwrap();
        assert!(c("frontend/.gitignore"));
        assert!(c("frontend/dist/app.js"));
    }

    /// editor-07: a linked worktree's index/HEAD and the repo's refs are
    /// outside the root; their changes are git changes.
    #[test]
    fn linked_worktree_git_meta() {
        let g = GitDirs {
            git_dir: PathBuf::from("C:/repo/.git/worktrees/t"),
            common_dir: PathBuf::from("C:/repo/.git"),
        };
        let root = Path::new("C:/wt/t");
        let mut rules = IgnoreRules {
            root: root.to_path_buf(),
            per_dir: HashMap::new(),
            extra: vec![],
        };
        let mut c = |p: &str| classify(root, Some(&g), &mut rules, Path::new(p));
        assert_eq!(c("C:/repo/.git/worktrees/t/index"), Some(Err(())));
        assert_eq!(c("C:/repo/.git/worktrees/t/HEAD"), Some(Err(())));
        assert_eq!(c("C:/repo/.git/refs/heads/main"), Some(Err(())));
        assert_eq!(c("C:/repo/.git/packed-refs"), Some(Err(())));
        assert_eq!(
            c("C:/repo/.git/index"),
            None,
            "the main checkout's index is not ours"
        );
        assert_eq!(c("C:/repo/.git/worktrees/t/logs/HEAD"), None);
    }

    /// A real `git worktree add` checkout reports a commit made in it.
    #[test]
    fn linked_worktree_commit_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let base = crate::uri::simplify(&d.path().canonicalize().unwrap());
        let main = base.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&main, &["init", "-q"]);
        git(
            &main,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        let wt = base.join("wt");
        git(
            &main,
            &["worktree", "add", "-q", "-b", "task", wt.to_str().unwrap()],
        );
        let wt = crate::uri::simplify(&wt.canonicalize().unwrap());
        let (_w, mut rx) = RootWatcher::start(&wt).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        git(
            &wt,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "agent",
            ],
        );
        let mut got = false;
        for _ in 0..50 {
            if let Ok(b) = rx.try_recv()
                && b.git_changed
            {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(got, "a commit in the linked worktree is a git change");
    }

    #[test]
    fn batches_real_changes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let (_w, mut rx) = RootWatcher::start(&root).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(root.join("a.txt"), "1").unwrap();
        std::fs::write(root.join("b.txt"), "2").unwrap();
        let mut batch = None;
        for _ in 0..50 {
            if let Ok(b) = rx.try_recv() {
                batch = Some(b);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let batch = batch.expect("a batch within 5s");
        let names: Vec<_> = batch
            .paths
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"a.txt".to_string()), "{names:?}");
    }
}
