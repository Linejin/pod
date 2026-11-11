//! Attempts: the limits checked when one begins and the item's new
//! generation, token checks, start failures and the circuit breaker, and the
//! worker's lifetime from attach to release.

use super::{BeginAttempt, ErrorKind, Orch, OrchError, Result, hash_token};
use crate::model::{Attempt, AttemptState, ItemState, Ms, WorkerState};

impl Orch {
    /// Check the limits and record a new attempt (`starting`).
    pub fn begin_attempt(&mut self, b: BeginAttempt, now: Ms) -> Result<Attempt> {
        let limits = self.limits;
        let (depth, item_state, circuit, current) = {
            let st = self.run(&b.run)?;
            let it = st.item(&b.item).ok_or_else(|| {
                OrchError::new(
                    ErrorKind::ItemNotFound,
                    format!("no item {} in run {}", b.item, b.run),
                )
            })?;
            (
                st.rec.run.depth + 1,
                it.state,
                it.circuit_open,
                it.attempt.clone(),
            )
        };
        if depth > limits.max_depth {
            return Err(OrchError::new(
                ErrorKind::DepthExceeded,
                format!(
                    "nesting depth {depth} is over the limit {}: do this work yourself instead of starting a worker",
                    limits.max_depth
                ),
            ));
        }
        if circuit {
            return Err(OrchError::new(
                ErrorKind::CircuitOpen,
                format!(
                    "item {} failed to start {} times in a row; fix the cause, then `pod orch item reopen {}`",
                    b.item, limits.start_strikes, b.item
                ),
            ));
        }
        match item_state {
            ItemState::Pending => {
                return Err(OrchError::new(
                    ErrorKind::DepsNotDone,
                    format!("item {} waits for its dependencies", b.item),
                ));
            }
            ItemState::Completed | ItemState::Failed if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!("item {} is finished; reopen it or pass --retry-of", b.item),
                ));
            }
            ItemState::Blocked if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!(
                        "item {} waits for a decision (gate {}); decide it first",
                        b.item,
                        self.item(&b.item)
                            .map(|i| i.gates.join(", "))
                            .unwrap_or_default()
                    ),
                ));
            }
            ItemState::Assigned if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!(
                        "item {} already has attempt {}; pass --retry-of {} to replace it",
                        b.item,
                        current.as_deref().unwrap_or("?"),
                        current.as_deref().unwrap_or("<attempt>")
                    ),
                ));
            }
            _ => {}
        }
        if let Some(old) = &b.retry_of {
            let a = self.attempt(old)?;
            if a.item != b.item {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("attempt {old} belongs to item {}, not {}", a.item, b.item),
                ));
            }
        }
        // Worker limits (per run and global) and resources are not refused
        // here: the daemon queues the attempt (`enqueue`, `admit`).
        if let Some(old) = &b.retry_of {
            let a = self.attempt_mut(old)?;
            if a.state.is_open() {
                a.state = AttemptState::Abandoned;
                a.ended_ms = Some(now);
                a.error = Some("replaced by a retry".into());
                a.queue = None;
                a.launch = None;
                if a.worker == WorkerState::Active {
                    // The old terminal stays with a person until released.
                    a.worker = WorkerState::Kept;
                }
            }
        }
        let id = self.next_id("a")?;
        let st = self.run_mut(&b.run)?;
        let it = st.rec.items.iter_mut().find(|i| i.id == b.item).unwrap();
        it.generation += 1;
        it.state = if it.gates.is_empty() {
            ItemState::Assigned
        } else {
            ItemState::Blocked
        };
        it.attempt = Some(id.clone());
        it.updated_ms = now;
        let a = Attempt {
            id,
            run: b.run.clone(),
            item: b.item.clone(),
            generation: it.generation,
            state: AttemptState::Starting,
            worker: WorkerState::Active,
            agent: b.agent,
            place: b.place,
            depth,
            token_hash: b.token_hash,
            task: None,
            terminal: None,
            brief_path: None,
            retry_of: b.retry_of,
            created_ms: now,
            ready_ms: None,
            ended_ms: None,
            release_at_ms: None,
            released_ms: None,
            log_path: None,
            body: None,
            files: Vec::new(),
            report: None,
            error: None,
            seen_turn: 0,
            escalated_turn: None,
            stalled_at_output_ms: None,
            reopens: 0,
            queue: None,
            launch: None,
            waiting_on: Vec::new(),
        };
        st.rec.attempts.push(a.clone());
        self.save_run(&b.run)?;
        Ok(a)
    }

    /// The worker's task and terminal are up.
    pub fn attach_worker(
        &mut self,
        attempt: &str,
        task: Option<&str>,
        terminal: &str,
        brief: Option<&str>,
        turn: u64,
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        a.task = task.map(str::to_string);
        a.terminal = Some(terminal.to_string());
        a.brief_path = brief.map(str::to_string);
        a.seen_turn = turn;
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    /// The worker could not be started. Returns whether this opened the
    /// item's circuit breaker (the item is then `failed`).
    pub fn start_failed(&mut self, attempt: &str, error: &str, now: Ms) -> Result<bool> {
        let strikes = self.limits.start_strikes;
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        a.state = AttemptState::StartFailed;
        a.queue = None;
        a.launch = None;
        a.worker = WorkerState::Released;
        a.ended_ms = Some(now);
        a.released_ms = Some(now);
        a.error = Some(error.to_string());
        let (item, gen_) = (a.item.clone(), a.generation);
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        let mut tripped = false;
        if it.generation == gen_ {
            it.start_failures += 1;
            it.updated_ms = now;
            if it.start_failures >= strikes {
                it.state = ItemState::Failed;
                it.circuit_open = true;
                tripped = true;
            } else {
                it.state = ItemState::Ready;
            }
        }
        self.save_run(&run)?;
        Ok(tripped)
    }

    /// The agent was seen working: the start succeeded (resets the breaker).
    pub fn mark_ready(&mut self, attempt: &str, now: Ms) -> Result<bool> {
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        if a.state != AttemptState::Starting {
            return Ok(false);
        }
        a.state = AttemptState::Ready;
        a.ready_ms = Some(now);
        let (item, gen_) = (a.item.clone(), a.generation);
        if let Some(it) = st
            .rec
            .items
            .iter_mut()
            .find(|i| i.id == item && i.generation == gen_)
        {
            it.start_failures = 0;
        }
        self.save_run(&run)?;
        Ok(true)
    }

    /// Check an attempt's token (`ATTEMPT_FENCED` on mismatch).
    pub fn verify_token(&self, attempt: &str, token: Option<&str>) -> Result<&Attempt> {
        let a = self.attempt(attempt)?;
        let ok = token.is_some_and(|t| same(&hash_token(t), &a.token_hash));
        if !ok {
            return Err(OrchError::new(
                ErrorKind::AttemptFenced,
                format!("the token does not belong to attempt {attempt} (POD_ATTEMPT_TOKEN)"),
            ));
        }
        Ok(a)
    }

    /// End an open attempt without a report (stop, lost, unknown,
    /// abandon). The item becomes ready again when this was its current
    /// attempt. Returns false when the attempt was not open.
    pub fn end_attempt(
        &mut self,
        attempt: &str,
        state: AttemptState,
        why: &str,
        worker: Option<WorkerState>,
        release_after_ms: Option<Ms>,
        now: Ms,
    ) -> Result<bool> {
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        if !a.state.is_open() {
            return Ok(false);
        }
        a.state = state;
        a.ended_ms = Some(now);
        a.error = Some(why.to_string());
        a.queue = None;
        a.launch = None;
        match worker {
            Some(w) => a.worker = w,
            None if a.worker == WorkerState::Active => {
                a.worker = WorkerState::Finished;
                a.release_at_ms = release_after_ms.map(|d| now + d);
            }
            None => {}
        }
        let (item, gen_) = (a.item.clone(), a.generation);
        if let Some(it) = st
            .rec
            .items
            .iter_mut()
            .find(|i| i.id == item && i.generation == gen_ && i.state == ItemState::Assigned)
        {
            it.state = ItemState::Ready;
            it.updated_ms = now;
        }
        self.save_run(&run)?;
        Ok(true)
    }

    /// Set the worker's resource state (`keep`, a person typing, release).
    pub fn set_worker(
        &mut self,
        attempt: &str,
        w: WorkerState,
        log_path: Option<&str>,
        now: Ms,
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        a.worker = w;
        match w {
            WorkerState::Released => {
                a.released_ms = Some(now);
                a.release_at_ms = None;
                if let Some(p) = log_path {
                    a.log_path = Some(p.to_string());
                }
            }
            WorkerState::Kept => a.release_at_ms = None,
            _ => {}
        }
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    /// Supervisor bookkeeping (turn seen, escalation marks).
    pub fn update_attempt(
        &mut self,
        attempt: &str,
        f: impl FnOnce(&mut Attempt),
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        f(a);
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    /// Finished workers whose release time has come.
    pub fn due_releases(&self, now: Ms) -> Vec<String> {
        self.attempts()
            .filter(|a| {
                a.worker == WorkerState::Finished && a.release_at_ms.is_some_and(|t| t <= now)
            })
            .map(|a| a.id.clone())
            .collect()
    }
}

/// Constant-time-ish comparison of two hashes.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}
