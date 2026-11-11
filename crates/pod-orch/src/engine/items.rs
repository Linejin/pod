//! Runs and work items: creating them, the dependency DAG (pending until
//! every dependency is completed), gates that block an item, reopening, and
//! workers waiting for other items of their run (docs/19 §18.2).

use std::collections::{BTreeMap, HashMap};

use super::{ErrorKind, ItemStates, Orch, OrchError, RUN_VERSION, Result, RunState, spec_title};
use crate::model::{Attempt, Item, ItemState, Ms, Run, RunRecord};

impl Orch {
    /// A new run coordinated by `terminal` (and `attempt`, when the
    /// coordinator is a worker: the run inherits its depth).
    pub fn create_run(
        &mut self,
        objective: &str,
        terminal: Option<&str>,
        attempt: Option<&str>,
        now: Ms,
    ) -> Result<Run> {
        let depth = match attempt {
            Some(a) => self.attempt(a)?.depth,
            None => 0,
        };
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "the objective must not be empty",
            ));
        }
        let id = self.next_id("r")?;
        let run = Run {
            id: id.clone(),
            objective: objective.to_string(),
            coordinator_terminal: terminal.map(str::to_string),
            coordinator_attempt: attempt.map(str::to_string),
            depth,
            created_ms: now,
        };
        self.runs.insert(
            id.clone(),
            RunState {
                rec: RunRecord {
                    version: RUN_VERSION,
                    run: run.clone(),
                    items: Vec::new(),
                    attempts: Vec::new(),
                    wakes: BTreeMap::new(),
                },
                mail: Vec::new(),
                read: HashMap::new(),
            },
        );
        self.save_run(&id)?;
        Ok(run)
    }

    fn deps_state(st: &RunState, deps: &[String]) -> ItemState {
        let done = deps
            .iter()
            .all(|d| st.item(d).is_some_and(|i| i.state == ItemState::Completed));
        if done {
            ItemState::Ready
        } else {
            ItemState::Pending
        }
    }

    pub fn add_item(
        &mut self,
        run: &str,
        title: Option<&str>,
        spec: &str,
        deps: &[String],
        parent: Option<&str>,
        now: Ms,
    ) -> Result<Item> {
        if spec.trim().is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "the spec must not be empty",
            ));
        }
        {
            let st = self.run(run)?;
            for d in deps.iter().map(String::as_str).chain(parent) {
                if st.item(d).is_none() {
                    return Err(OrchError::new(
                        ErrorKind::ItemNotFound,
                        format!("no item {d} in run {run}"),
                    ));
                }
            }
        }
        let id = self.next_id("i")?;
        let st = self.run_mut(run)?;
        let title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| spec_title(spec));
        let item = Item {
            id,
            run: run.to_string(),
            title,
            spec: spec.to_string(),
            deps: deps.to_vec(),
            parent: parent.map(str::to_string),
            state: Self::deps_state(st, deps),
            generation: 0,
            start_failures: 0,
            circuit_open: false,
            attempt: None,
            gates: Vec::new(),
            created_ms: now,
            updated_ms: now,
        };
        st.rec.items.push(item.clone());
        self.save_run(run)?;
        Ok(item)
    }

    /// Open the item again: clears the breaker; completed/failed items become
    /// ready (or pending) for a new attempt.
    pub fn reopen_item(&mut self, item: &str, now: Ms) -> Result<Item> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let deps = st.item(item).map(|i| i.deps.clone()).unwrap_or_default();
        let state = Self::deps_state(st, &deps);
        let open_attempt = st
            .rec
            .attempts
            .iter()
            .any(|a| a.item == item && a.state.is_open());
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        it.start_failures = 0;
        it.circuit_open = false;
        if !open_attempt && it.gates.is_empty() {
            it.state = state;
        }
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(it)
    }

    /// A gate now blocks this item (docs/19 §4.5).
    pub fn block_item(&mut self, item: &str, gate: &str, now: Ms) -> Result<Item> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        if matches!(it.state, ItemState::Completed | ItemState::Failed) {
            let state = if it.state == ItemState::Completed {
                "completed"
            } else {
                "failed"
            };
            return Err(OrchError::new(
                ErrorKind::InvalidState,
                format!("item {item} is already {state}"),
            ));
        }
        if !it.gates.iter().any(|g| g == gate) {
            it.gates.push(gate.to_string());
        }
        it.state = ItemState::Blocked;
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(it)
    }

    /// The gate was decided or expired. When no gate is left the item goes
    /// back to `assigned` (its attempt is open) or `ready`/`pending`.
    /// Idempotent; returns `None` when nothing changed.
    pub fn unblock_item(&mut self, item: &str, gate: &str, now: Ms) -> Result<Option<Item>> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let Some(idx) = st.rec.items.iter().position(|i| i.id == item) else {
            return Ok(None);
        };
        if !st.rec.items[idx].gates.iter().any(|g| g == gate) {
            return Ok(None);
        }
        let deps = st.rec.items[idx].deps.clone();
        let next = Self::deps_state(st, &deps);
        let (cur, gen_) = (
            st.rec.items[idx].attempt.clone(),
            st.rec.items[idx].generation,
        );
        let open = st
            .rec
            .attempts
            .iter()
            .any(|a| Some(&a.id) == cur.as_ref() && a.generation == gen_ && a.state.is_open());
        let it = &mut st.rec.items[idx];
        it.gates.retain(|g| g != gate);
        if it.gates.is_empty() && it.state == ItemState::Blocked {
            it.state = if open { ItemState::Assigned } else { next };
        }
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(Some(it))
    }

    /// Items whose dependencies are now all completed become ready.
    pub(super) fn promote(st: &mut RunState, now: Ms) {
        let completed: Vec<String> = st
            .rec
            .items
            .iter()
            .filter(|i| i.state == ItemState::Completed)
            .map(|i| i.id.clone())
            .collect();
        for it in st.rec.items.iter_mut() {
            if it.state == ItemState::Pending && it.deps.iter().all(|d| completed.contains(d)) {
                it.state = ItemState::Ready;
                it.updated_ms = now;
            }
        }
    }

    // ── waiting for other items (docs/19 §18.2) ─────────────────────

    /// An open worker waits for `items` of its run (not its own). Returns
    /// each item with its state; only unfinished ones are recorded (none
    /// unfinished: nothing to wait for, the record is cleared).
    pub fn wait_for_items(
        &mut self,
        attempt: &str,
        items: &[String],
    ) -> Result<Vec<(String, ItemState)>> {
        let a = self.attempt(attempt)?.clone();
        if !a.state.is_open() {
            return Err(OrchError::new(
                ErrorKind::InvalidState,
                format!("worker {attempt} is {:?}, not running", a.state),
            ));
        }
        if items.is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "name at least one item (--item)",
            ));
        }
        let mut states = Vec::new();
        for id in items {
            if *id == a.item {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{id} is this worker's own item"),
                ));
            }
            let it = self.item(id)?;
            if it.run != a.run {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{id} is not an item of run {}", a.run),
                ));
            }
            if !states.iter().any(|(i, _)| i == id) {
                states.push((id.clone(), it.state));
            }
        }
        let open: Vec<String> = states
            .iter()
            .filter(|(_, s)| !s.is_finished())
            .map(|(i, _)| i.clone())
            .collect();
        self.update_attempt(attempt, |x| x.waiting_on = open)?;
        Ok(states)
    }

    /// Open workers whose awaited items have all finished: their wait is
    /// cleared and they are returned with the items' final states.
    pub fn settle_waits(&mut self) -> Result<Vec<(Attempt, ItemStates)>> {
        let mut due = Vec::new();
        for a in self.attempts().filter(|a| !a.waiting_on.is_empty()) {
            let states: Vec<(String, ItemState)> = a
                .waiting_on
                .iter()
                .map(|i| {
                    (
                        i.clone(),
                        self.item(i).map(|it| it.state).unwrap_or(ItemState::Failed),
                    )
                })
                .collect();
            if !a.state.is_open() || states.iter().all(|(_, s)| s.is_finished()) {
                due.push((a.id.clone(), a.state.is_open(), states));
            }
        }
        let mut out = Vec::new();
        for (id, open, states) in due {
            let a = self.update_attempt(&id, |x| x.waiting_on.clear())?;
            if open {
                out.push((a, states));
            }
        }
        Ok(out)
    }
}
