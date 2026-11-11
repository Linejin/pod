//! The resource queue (docs/19 §5.1): queued attempts in FIFO order and the
//! gate that admits one when the limits and resources allow.

use serde_json::Value;

use super::{Admitted, Orch, Result};
use crate::model::{Attempt, Ms, QueueState, WaitKind};
use crate::resource::{self, ResourceLimits, Sample};

impl Orch {
    /// Put a new attempt into the resource queue with what the daemon needs
    /// to start it later.
    pub fn enqueue(
        &mut self,
        attempt: &str,
        launch: Value,
        disk_path: Option<String>,
        now: Ms,
    ) -> Result<Attempt> {
        self.update_attempt(attempt, |a| {
            a.queue = Some(QueueState {
                since_ms: now,
                kind: WaitKind::Queued,
                reason: "자원 확인을 기다리는 중이에요.".into(),
                position: 0,
                disk_path,
            });
            a.launch = Some(launch);
        })
    }

    /// Queued attempts in FIFO order.
    pub fn queue(&self) -> Vec<&Attempt> {
        resource::queue_order(self.attempts())
    }

    /// Active workers (open, not queued) over all runs.
    pub fn active_workers(&self) -> usize {
        self.attempts().filter(|a| a.is_active()).count()
    }

    /// One pass of the resource gate. Stores the changed verdicts (kind or
    /// position) of waiting attempts and takes the admitted one out of the queue with its new
    /// token hash (the token is made only now, so none was ever stored).
    /// Returns the admitted attempt with its launch data, and every attempt
    /// whose queue state changed.
    pub fn admit(
        &mut self,
        limits: &ResourceLimits,
        sample: &Sample,
        free_disk: impl FnMut(&str) -> Option<u64>,
        token_hash: Option<String>,
        now: Ms,
    ) -> Result<Admitted> {
        let all: Vec<&Attempt> = self.attempts().collect();
        let plan = resource::plan(&all, limits, sample, free_disk, now);
        let mut out = Admitted::default();
        let mut dirty: Vec<String> = Vec::new();
        for v in &plan.waiting {
            let Ok(a) = self.attempt_mut(&v.attempt) else {
                continue;
            };
            let Some(q) = a.queue.as_mut() else { continue };
            // The reason text carries live numbers (CPU %, free GiB): store
            // it only when the kind or place changes, so a waiting queue does
            // not rewrite run.json every pass. `orch.queue` shows fresh text.
            if q.kind != v.kind || q.position != v.position {
                q.kind = v.kind;
                q.reason = v.reason.clone();
                q.position = v.position;
                let a = a.clone();
                if !dirty.contains(&a.run) {
                    dirty.push(a.run.clone());
                }
                out.changed.push(a.public());
            }
        }
        if let (Some(id), Some(hash)) = (&plan.start, token_hash) {
            let a = self.attempt_mut(id)?;
            a.queue = None;
            a.token_hash = hash;
            let launch = a.launch.take().unwrap_or(Value::Null);
            let a = a.clone();
            if !dirty.contains(&a.run) {
                dirty.push(a.run.clone());
            }
            out.start = Some((a.public(), launch));
        }
        for run in dirty {
            self.save_run(&run)?;
        }
        Ok(out)
    }
}
