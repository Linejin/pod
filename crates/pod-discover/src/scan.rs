//! Scan orchestration: every [`Source`]'s files, newest first, within the
//! scan's [`Budget`], reusing the [`Cache`] for unchanged files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Agent, Budget, Cache, CachedFile, Found, Limits, Source, jsonl, last_activity};

#[derive(Debug, Default)]
pub struct ScanOutput {
    /// Newest first (by last activity).
    pub sessions: Vec<Found>,
    /// Ids the stores record for running pids, per agent.
    pub running_ids: HashMap<Agent, HashMap<u32, String>>,
    /// Some files were not read (budget or `max_files`).
    pub truncated: bool,
    pub parsed: usize,
    pub cached: usize,
    pub bytes_read: u64,
}

/// Scan every source, reusing `cache` for unchanged files. The cache keeps
/// only the files seen in this scan (files deleted by the agent disappear).
///
/// Two phases: (1) metadata (head + tail of each file), each source with an
/// equal share of half the budget so one large store cannot starve another;
/// (2) message counts (whole files, newest first) with the other half. A
/// count that did not fit is retried by the next scan.
pub fn scan(sources: &mut [Box<dyn Source>], cache: &mut Cache, limits: &Limits) -> ScanOutput {
    let mut out = ScanOutput::default();
    let parts = (sources.len() as u64).max(1) * 2;
    let share = Limits {
        read_bytes: limits.read_bytes / parts,
        time: limits.time / parts as u32,
        ..limits.clone()
    };
    // (key, source index, entry)
    let mut entries: Vec<(String, usize, CachedFile)> = Vec::new();
    for (si, src) in sources.iter_mut().enumerate() {
        let agent = src.agent();
        let mut budget = Budget::new(&share);
        let mut files = src.list(limits);
        files.sort_by(|a, b| {
            b.mtime_ms
                .cmp(&a.mtime_ms)
                .then_with(|| a.path.cmp(&b.path))
        });
        if files.len() > limits.max_files {
            files.truncate(limits.max_files);
            out.truncated = true;
        }
        src.prepare(&mut budget);
        for f in files {
            let key = f.path.display().to_string();
            let old = cache.files.get(&key).filter(|c| c.agent == agent);
            let hit = old.filter(|c| {
                c.size == f.size && c.mtime_ms == f.mtime_ms && (c.with_preview || !limits.preview)
            });
            let entry = if let Some(c) = hit {
                out.cached += 1;
                c.clone()
            } else {
                let meta = if budget.out_of_time() {
                    None
                } else {
                    src.parse(&f, limits, &mut budget)
                };
                if meta.is_none() && budget.exhausted {
                    // Not read this time: keep what an older scan knew.
                    if let Some(old) = old {
                        entries.push((key, si, old.clone()));
                    }
                    continue;
                }
                out.parsed += 1;
                CachedFile {
                    agent,
                    size: f.size,
                    mtime_ms: f.mtime_ms,
                    with_preview: limits.preview,
                    counted: false,
                    meta,
                }
            };
            entries.push((key, si, entry));
        }
        out.truncated |= budget.exhausted;
        // Small and bounded by the source itself; must not starve behind the files.
        let mut pid_budget = Budget::new(limits);
        out.running_ids
            .insert(agent, src.running_sessions(&mut pid_budget));
        out.bytes_read += pid_budget.bytes_read;
        out.bytes_read += budget.bytes_read;
    }
    // Phase 2: message counts, newest first.
    let mut budget = Budget::new(&Limits {
        read_bytes: limits.read_bytes / 2,
        time: limits.time / 2,
        ..limits.clone()
    });
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|a, b| entries[*b].2.mtime_ms.cmp(&entries[*a].2.mtime_ms));
    for i in order {
        let (key, si, e) = &mut entries[i];
        if e.counted || e.meta.is_none() {
            continue;
        }
        if e.size > limits.count_max_bytes {
            e.counted = true;
            continue;
        }
        let markers = sources[*si].message_markers();
        if let Some(n) = jsonl::count_lines_with(
            Path::new(key.as_str()),
            e.size,
            limits.count_max_bytes,
            markers,
            &mut budget,
        ) {
            e.counted = true;
            if let Some(m) = e.meta.as_mut() {
                m.messages = Some(n);
            }
        }
    }
    out.truncated |= budget.exhausted;
    out.bytes_read += budget.bytes_read;
    let mut keep = HashMap::with_capacity(entries.len());
    for (key, _, mut e) in entries {
        if !limits.preview
            && let Some(m) = e.meta.as_mut()
        {
            m.preview = None;
        }
        if let Some(meta) = e.meta.clone() {
            out.sessions.push(Found {
                agent: e.agent,
                meta,
                path: PathBuf::from(&key),
                mtime_ms: e.mtime_ms,
            });
        }
        keep.insert(key, e);
    }
    cache.files = keep;
    // One entry per (agent, id): a session copied into two files keeps the newest.
    out.sessions
        .sort_by_key(|a| std::cmp::Reverse(last_activity(a)));
    let mut seen = std::collections::HashSet::new();
    out.sessions
        .retain(|s| seen.insert((s.agent, s.meta.id.clone())));
    out
}
