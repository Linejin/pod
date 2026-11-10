//! Background file index for quick open.
//!
//! The walk honours the same ignore rules as the tree and runs on a
//! background thread; queries run on a background executor too, so a large
//! worktree never blocks typing. Paths are stored relative to the root with
//! `/` separators.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fuzzy::{self, Query};

/// Hard cap on indexed files; beyond this quick open says the list is partial.
pub const MAX_FILES: usize = 500_000;

#[derive(Clone, Debug, Default)]
pub struct IndexSnapshot {
    pub files: Arc<Vec<String>>,
    pub truncated: bool,
    /// The same files and every folder holding one, for O(1) lookups when
    /// watcher batches are classified.
    pub lookup: Arc<Lookup>,
}

#[derive(Debug, Default)]
pub struct Lookup {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

impl Lookup {
    pub fn new(files: &[String]) -> Self {
        let mut dirs = HashSet::new();
        for f in files {
            let mut s = f.as_str();
            while let Some(i) = s.rfind('/') {
                s = &s[..i];
                if !dirs.insert(s.to_string()) {
                    break;
                }
            }
        }
        Lookup {
            files: files.iter().cloned().collect(),
            dirs,
        }
    }
}

/// Whether a watcher batch adds or removes files (the index must be
/// rebuilt), as opposed to only changing file contents. Blocking (stats the
/// paths): run off the UI thread. `rel` maps a path to its index key.
pub fn is_structural(paths: &[PathBuf], lookup: &Lookup, rel: impl Fn(&Path) -> String) -> bool {
    paths.iter().any(|p| {
        let r = rel(p);
        let known = lookup.files.contains(&r);
        if known {
            !p.exists()
        } else {
            p.is_file() || (lookup.dirs.contains(&r) && !p.exists())
        }
    })
}

/// Walks `root` (blocking). Stops early when `cancel` is set.
pub fn build(root: &Path, cancel: &AtomicBool) -> IndexSnapshot {
    let mut files = Vec::new();
    let mut truncated = false;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for e in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = e.path().strip_prefix(root) {
            let s = rel.to_string_lossy();
            files.push(if cfg!(windows) {
                s.replace('\\', "/")
            } else {
                s.into_owned()
            });
        }
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
    }
    files.sort_unstable_by_key(|a| a.to_lowercase());
    let lookup = Arc::new(Lookup::new(&files));
    IndexSnapshot {
        files: Arc::new(files),
        truncated,
        lookup,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rel: String,
    /// Char indices of matched characters in `rel`.
    pub indices: Vec<usize>,
    pub score: i32,
    pub recent: bool,
}

/// Bonus for recently opened files, decreasing with age.
pub fn recent_bonus(pos: usize) -> i32 {
    (40 - pos as i32 * 4).max(8)
}

/// Ranks files for `query`. An empty query lists recents first, then files
/// in name order. `recents` is most-recent-first, relative `/` paths.
pub fn search(files: &[String], query: &str, recents: &[String], limit: usize) -> Vec<Hit> {
    let q = Query::new(query);
    let recent_pos = |rel: &str| recents.iter().position(|r| r == rel);
    if q.is_empty() {
        let mut out: Vec<Hit> = recents
            .iter()
            .filter(|r| files.iter().any(|f| f == *r))
            .take(limit)
            .map(|r| Hit {
                rel: r.clone(),
                indices: vec![],
                score: 0,
                recent: true,
            })
            .collect();
        for f in files.iter() {
            if out.len() >= limit {
                break;
            }
            if recent_pos(f).is_none() {
                out.push(Hit {
                    rel: f.clone(),
                    indices: vec![],
                    score: 0,
                    recent: false,
                });
            }
        }
        return out;
    }
    let mut hits: Vec<Hit> = files
        .iter()
        .filter_map(|f| {
            let m = fuzzy::score(&q, f)?;
            let rp = recent_pos(f);
            Some(Hit {
                rel: f.clone(),
                score: m.score + rp.map(recent_bonus).unwrap_or(0),
                indices: m.indices,
                recent: rp.is_some(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.rel.len().cmp(&b.rel.len()))
            .then_with(|| a.rel.cmp(&b.rel))
    });
    hits.truncate(limit);
    hits
}

/// Most-recently-used list (front = newest), bounded.
#[derive(Clone, Debug, Default)]
pub struct Recents {
    items: Vec<String>,
}

impl Recents {
    pub const MAX: usize = 50;
    pub fn touch(&mut self, rel: &str) {
        self.items.retain(|r| r != rel);
        self.items.insert(0, rel.to_string());
        self.items.truncate(Self::MAX);
    }
    pub fn items(&self) -> &[String] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// editor-10: batch classification is lookups, not scans of the index.
    #[test]
    fn structural_batches() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/deep")).unwrap();
        std::fs::write(r.join("src/a.rs"), "").unwrap();
        std::fs::write(r.join("src/new.rs"), "").unwrap();
        let files = vec![
            "src/a.rs".to_string(),
            "src/gone.rs".to_string(),
            "old/x/y.rs".to_string(),
        ];
        let lookup = Lookup::new(&files);
        let rel = |p: &Path| {
            p.strip_prefix(r)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        };
        let check = |p: &str| is_structural(&[r.join(p)], &lookup, rel);
        assert!(!check("src/a.rs"), "content change of a known file");
        assert!(check("src/new.rs"), "new file");
        assert!(check("src/gone.rs"), "known file deleted");
        assert!(check("old"), "folder of known files deleted");
        assert!(check("old/x"));
        assert!(!check("src/deep"), "new empty folder");
        assert!(!check("nothing"), "unknown path that does not exist");
    }

    #[test]
    fn index_respects_gitignore() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::write(r.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(r.join("src/a.rs"), "").unwrap();
        std::fs::write(r.join("target/x.o"), "").unwrap();
        let s = build(r, &AtomicBool::new(false));
        assert_eq!(
            *s.files,
            vec![".gitignore".to_string(), "src/a.rs".to_string()]
        );
    }

    #[test]
    fn recents_boost_and_empty_query() {
        let files: Vec<String> = ["src/main.rs", "src/menu.rs", "docs/manual.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut rec = Recents::default();
        rec.touch("docs/manual.md");
        let hits = search(&files, "", rec.items(), 10);
        assert_eq!(hits[0].rel, "docs/manual.md");
        assert!(hits[0].recent);
        assert_eq!(hits.len(), 3);
        let plain = search(&files, "man", &[], 10);
        let boosted = search(&files, "man", rec.items(), 10);
        let pos = |v: &[Hit]| v.iter().position(|h| h.rel == "docs/manual.md").unwrap();
        assert!(pos(&boosted) <= pos(&plain));
        assert_eq!(boosted[0].rel, "docs/manual.md");
        rec.touch("src/main.rs");
        assert_eq!(rec.items()[0], "src/main.rs");
    }

    /// Generates 50k paths and checks a query over all of them stays fast.
    /// (Quick, single-threaded; the only "perf" check in this crate.)
    #[test]
    fn fifty_thousand_files_query_is_fast() {
        let mut files = Vec::with_capacity(50_000);
        for i in 0..50_000 {
            files.push(format!("pkg{}/module_{}/src/file_{i}.rs", i % 97, i % 1013));
        }
        files.push("crates/pod-editor/src/quick_open.rs".into());
        let t = std::time::Instant::now();
        let hits = search(&files, "quickopen", &[], 50);
        let el = t.elapsed();
        assert_eq!(hits[0].rel, "crates/pod-editor/src/quick_open.rs");
        let t2 = std::time::Instant::now();
        let many = search(&files, "file12", &[], 50);
        let el2 = t2.elapsed();
        assert!(!many.is_empty());
        eprintln!("50k query: {el:?} (rare), {el2:?} (common)");
        // Generous bound for debug builds on a loaded machine.
        assert!(el2 < std::time::Duration::from_secs(3), "{el2:?}");
    }
}
