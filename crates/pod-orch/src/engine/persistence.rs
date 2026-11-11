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
        let mut index: Index =
            store::read_json(&store::index_json(root), now, &mut notes).unwrap_or_default();
        let mut runs = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let path = e.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let Some(rec) =
                    store::read_json::<RunRecord>(&store::run_json(root, &name), now, &mut notes)
                else {
                    continue;
                };
                let load = match store::load_mail(&store::mail_jsonl(root, &name)) {
                    Ok(l) => l,
                    Err(e) => {
                        notes.push(format!("run {name}: cannot read mail: {e}"));
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
                    if let Err(e) = store::write_json(&store::run_json(root, &name), &st.rec) {
                        notes.push(format!("run {name}: cannot save the replayed done: {e}"));
                    }
                }
                runs.insert(st.rec.run.id.clone(), st);
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
        }
    }

    pub(super) fn save_index(&self) -> Result<()> {
        store::write_json(&store::index_json(&self.root), &self.index).map_err(io)
    }

    pub(super) fn next_id(&mut self, prefix: &str) -> Result<String> {
        let c = self.index.counters.entry(prefix.to_string()).or_insert(0);
        *c += 1;
        let id = format!("{prefix}{c}");
        self.save_index()?;
        Ok(id)
    }

    pub(super) fn save_run(&self, run: &str) -> Result<()> {
        let st = self
            .runs
            .get(run)
            .ok_or_else(|| not_found(ErrorKind::RunNotFound, "run", run))?;
        store::write_json(&store::run_json(&self.root, run), &st.rec).map_err(io)
    }
}
