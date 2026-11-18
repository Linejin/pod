//! Loading and saving the engine state: each run's record with its mail and
//! read receipts, the index (id counters, remembered requests), and the id
//! recovery that never hands out an id already on disk.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::{ErrorKind, Index, Limits, Orch, Result, RunState, id_number, io, not_found};
use crate::model::{MailLine, Ms, RunRecord};
use crate::store;

impl Orch {
    /// Load everything under `root` (`<state>\orch\runs`).
    pub fn load(root: &Path, limits: Limits, now: Ms) -> Orch {
        let mut notes = Vec::new();
        let mut recovery_incomplete = false;
        let mut index: Index =
            match store::read_json_checked(&store::index_json(root), now, &mut notes) {
                Ok(value) => value.unwrap_or_default(),
                Err(e) => {
                    notes.push(format!(
                        "cannot read {}: {e}",
                        store::index_json(root).display()
                    ));
                    recovery_incomplete = true;
                    Index::default()
                }
            };
        let mut runs = BTreeMap::new();
        let mut replayed = Vec::new();
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                notes.push(format!("cannot enumerate {}: {e}", root.display()));
                recovery_incomplete = true;
                None
            }
        };
        if let Some(entries) = entries {
            for entry in entries {
                let e = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        notes.push(format!("cannot enumerate entry in {}: {e}", root.display()));
                        recovery_incomplete = true;
                        continue;
                    }
                };
                let path = e.path();
                match std::fs::metadata(&path) {
                    Ok(kind) if kind.is_dir() => {}
                    Ok(_) => continue,
                    Err(e) => {
                        notes.push(format!("cannot inspect {}: {e}", path.display()));
                        recovery_incomplete = true;
                        continue;
                    }
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                // A directory reserves its run ID even without a snapshot.
                if let Some(n) = name.strip_prefix('r').and_then(|s| s.parse::<u64>().ok()) {
                    let counter = index.counters.entry("r".into()).or_default();
                    *counter = (*counter).max(n);
                }
                let rec = match store::read_json_checked::<RunRecord>(
                    &store::run_json(root, &name),
                    now,
                    &mut notes,
                ) {
                    Ok(Some(rec)) => rec,
                    Ok(None) => continue,
                    Err(e) => {
                        notes.push(format!("run {name}: cannot read snapshot: {e}"));
                        recovery_incomplete = true;
                        continue;
                    }
                };
                let load = match store::load_mail(&store::mail_jsonl(root, &name)) {
                    Ok(l) => l,
                    Err(e) => {
                        notes.push(format!("run {name}: cannot read mail: {e}"));
                        recovery_incomplete = true;
                        Default::default()
                    }
                };
                if load.truncated_bytes > 0 {
                    notes.push(format!(
                        "run {name}: dropped a torn last mail line ({} bytes)",
                        load.truncated_bytes
                    ));
                }
                if load.skipped_lines > 0 {
                    notes.push(format!(
                        "run {name}: skipped {} unreadable mail line(s)",
                        load.skipped_lines
                    ));
                }
                let mut st = RunState {
                    rec,
                    mail: Vec::new(),
                    read: HashMap::new(),
                };
                for line in load.lines {
                    match line {
                        MailLine::Mail { mail } => st.mail.push(*mail),
                        MailLine::Read { mailbox, ids, ms } => {
                            let r = st.read.entry(mailbox).or_default();
                            for id in ids {
                                r.entry(id).or_insert(ms);
                            }
                        }
                    }
                }
                let closed = Self::replay_done(&mut st);
                if !closed.is_empty() {
                    notes.push(format!(
                        "run {name}: applied the recorded done of {}",
                        closed.join(", ")
                    ));
                    replayed.push(name);
                }
                runs.insert(st.rec.run.id.clone(), st);
            }
        }
        // Do not publish replayed snapshots until the entire store was read.
        // The mail journal remains authoritative if a replay save fails.
        if !recovery_incomplete {
            for name in replayed {
                if let Some(st) = runs.get(&name)
                    && let Err(e) = store::write_json(&store::run_json(root, &name), &st.rec)
                {
                    notes.push(format!("run {name}: cannot save the replayed done: {e}"));
                }
            }
        }
        // Never hand out an id that exists, whatever the index says.
        let mut bump = |prefix: &str, n: u64| {
            let c = index.counters.entry(prefix.to_string()).or_insert(0);
            *c = (*c).max(n);
        };
        for st in runs.values() {
            bump("r", id_number(&st.rec.run.id));
            for i in &st.rec.items {
                bump("i", id_number(&i.id));
            }
            for a in &st.rec.attempts {
                bump("a", id_number(&a.id));
            }
            for m in &st.mail {
                bump("m", id_number(&m.id));
            }
        }
        Orch {
            root: root.to_path_buf(),
            index,
            runs,
            limits,
            notes,
            recovery_incomplete,
        }
    }

    pub(super) fn save_index(&self) -> Result<()> {
        self.ensure_recovered()?;
        store::write_json(&store::index_json(&self.root), &self.index).map_err(io)
    }

    pub(super) fn next_id(&mut self, prefix: &str) -> Result<String> {
        self.ensure_recovered()?;
        let mut index = self.index.clone();
        let c = index.counters.entry(prefix.to_string()).or_insert(0);
        *c += 1;
        let id = format!("{prefix}{c}");
        store::write_json(&store::index_json(&self.root), &index).map_err(io)?;
        self.index = index;
        Ok(id)
    }

    /// Publish a staged transition only after its snapshot is saved. Mail
    /// and read receipts stay in place; completion keeps its own mail journal.
    pub(super) fn commit_run(&mut self, rec: RunRecord) -> Result<()> {
        let run = rec.run.id.clone();
        let path = store::run_json(&self.root, &run);
        let st = self.run_mut(&run)?;
        store::write_json(&path, &rec).map_err(io)?;
        st.rec = rec;
        Ok(())
    }

    pub(super) fn save_run(&self, run: &str) -> Result<()> {
        self.ensure_recovered()?;
        let st = self
            .runs
            .get(run)
            .ok_or_else(|| not_found(ErrorKind::RunNotFound, "run", run))?;
        store::write_json(&store::run_json(&self.root, run), &st.rec).map_err(io)
    }

    pub(super) fn ensure_recovered(&self) -> Result<()> {
        if self.recovery_incomplete {
            return Err(io(std::io::Error::other(
                "orchestration recovery is incomplete; resolve the reported read errors and reload before writing",
            )));
        }
        Ok(())
    }
}
