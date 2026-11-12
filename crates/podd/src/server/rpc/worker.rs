//! The restricted worker endpoint's dispatch (docs/19 §20, docs/22 TM-20).
//!
//! The full pipe is the OS user: any process that can open it may call
//! every method, and caller fields there are claims, not authentication.
//! The worker endpoint (`<pipe>-worker`, same-user DACL) is where Pod points
//! its workers (`POD_SOCKET`). A connection there first says
//! `connection.hello` with its attempt and token; the daemon checks the
//! token and from then on **derives** the caller of every request from it
//! (checked again per request, so a token that stopped matching stops
//! working). A request's own `caller` may only repeat that identity: a
//! missing caller never becomes `human`, another attempt or terminal is
//! refused ([`worker_codes::CALLER_MISMATCH`]).
//!
//! What a worker may call ([`allowed`]): `server.status`; reads of its own
//! terminal; the `orch.*` methods of its own run (reports, questions, mail,
//! commit, gate requests) and of the runs it coordinates itself (its
//! helpers). Never: gate decisions, merges, input to any terminal, other
//! terminals, limits, config, tasks, repos, mods, mobile or streams. A name
//! outside the list gets [`worker_codes::NOT_ALLOWED`] with a fixed
//! message; a target outside the worker's runs or terminal gets
//! [`worker_codes::OUT_OF_SCOPE`]. Then the request goes through the same
//! dispatch as on the full pipe.

use std::sync::Arc;

use pod_orch::engine::Orch;
use serde_json::{Value, json};

use crate::protocol::*;
use crate::server::connection::{err_response, ok_response};
use crate::server::{Daemon, log, params};

/// The fixed message of [`worker_codes::NOT_ALLOWED`].
pub(in crate::server) const NOT_ALLOWED_MSG: &str = "not available on the worker endpoint: a Pod worker reaches its own run and terminal only (docs/19 §20); a person uses the full pipe";
/// The fixed message of [`worker_codes::NO_WORKER`].
pub(in crate::server) const NO_WORKER_MSG: &str = "the worker endpoint serves Pod workers only: say connection.hello with this worker's POD_ATTEMPT_ID and POD_ATTEMPT_TOKEN first";
/// The fixed message of [`worker_codes::OUT_OF_SCOPE`].
pub(in crate::server) const OUT_OF_SCOPE_MSG: &str =
    "outside this worker's run and terminal (worker endpoint, docs/19 §20)";
/// The fixed message of [`worker_codes::CALLER_MISMATCH`].
pub(in crate::server) const CALLER_MISMATCH_MSG: &str = "the caller names another attempt or terminal than this worker endpoint connection; the daemon derives the caller from the attempt token";

/// The worker a connection said it speaks for (checked at hello).
#[derive(Debug, Clone)]
pub(in crate::server) struct WorkerConn {
    pub(in crate::server) attempt: String,
    token: String,
}

fn refusal(code: i64, message: &str, method: &str) -> RpcError {
    let mut e = RpcError::new(code, message);
    e.data = Some(json!({ "method": method }));
    e
}

/// How a method is checked before it reaches the dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// No target (`server.status`).
    Plain,
    /// `id`/`name` must be the worker's own terminal.
    OwnTerminal,
    /// `orch.*` with a `caller` (rewritten to the worker) and the targets
    /// named in [`Targets`].
    Orch(Targets),
    /// `orch.*` without a `caller` field.
    OrchNoCaller(Targets),
}

/// Which params name a target, and whether it must be a run the worker
/// coordinates (`coord`) or any of its runs (its own and those).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Targets {
    run: Option<bool>,
    item: Option<bool>,
    attempt: Option<bool>,
    mail: bool,
    to: bool,
}

const OWN: Option<bool> = Some(false);
const COORD: Option<bool> = Some(true);
const NONE: Targets = Targets {
    run: None,
    item: None,
    attempt: None,
    mail: false,
    to: false,
};

/// The allowlist: every method a worker endpoint connection may call
/// (besides `connection.hello`), and how it is checked.
fn allowed(method: &str) -> Option<Rule> {
    use Rule::*;
    let t = |run, item, attempt| Targets {
        run,
        item,
        attempt,
        ..NONE
    };
    Some(match method {
        ServerStatus::NAME => Plain,
        TerminalRead::NAME | TerminalShow::NAME | TerminalWait::NAME | StatusWait::NAME => {
            OwnTerminal
        }
        // Its own report, question, wait and commit: the caller is enough.
        OrchMailAsk::NAME | OrchDone::NAME | OrchWait::NAME | OrchCommit::NAME => Orch(NONE),
        OrchRunList::NAME => Orch(NONE),
        OrchRunShow::NAME | OrchItemList::NAME => Orch(t(OWN, None, None)),
        OrchWorkerList::NAME | OrchQueue::NAME => Orch(t(OWN, None, None)),
        // A run's mailbox is its coordinator's: only runs it coordinates.
        OrchMailCheck::NAME => Orch(t(COORD, None, None)),
        OrchMailList::NAME => OrchNoCaller(t(COORD, None, None)),
        OrchMailSend::NAME => Orch(Targets {
            run: OWN,
            to: true,
            ..NONE
        }),
        OrchMailReply::NAME => Orch(Targets { mail: true, ..NONE }),
        OrchGateRequest::NAME => Orch(t(OWN, OWN, None)),
        // Coordinating its own helpers.
        OrchWorkerStart::NAME => Orch(t(COORD, COORD, None)),
        OrchItemCreate::NAME => Orch(t(COORD, None, None)),
        OrchItemUpdate::NAME => Orch(t(None, COORD, None)),
        OrchWorkerShow::NAME => Orch(t(None, None, OWN)),
        OrchWorkerRead::NAME => OrchNoCaller(t(None, None, OWN)),
        OrchWorkerStop::NAME
        | OrchWorkerAbandon::NAME
        | OrchWorkerKeep::NAME
        | OrchWorkerRelease::NAME => Orch(t(None, None, COORD)),
        _ => return None,
    })
}

/// The worker's runs, from the daemon's records.
struct Scope {
    attempt: Attempt,
    /// Runs it coordinates (its helpers' runs).
    coord: Vec<String>,
}

impl Scope {
    /// `None` when the token is not (or no longer) the attempt's.
    fn of(o: &Orch, w: &WorkerConn) -> Option<Scope> {
        let attempt = o.verify_token(&w.attempt, Some(&w.token)).ok()?.clone();
        let mut coord: Vec<String> = o
            .runs()
            .filter(|r| r.rec.run.coordinator_attempt.as_deref() == Some(w.attempt.as_str()))
            .map(|r| r.rec.run.id.clone())
            .collect();
        if let Some(t) = &attempt.terminal {
            coord.extend(o.runs_of_terminal(t));
        }
        coord.retain(|r| r != &attempt.run);
        Some(Scope { attempt, coord })
    }

    fn reaches(&self, run: &str, coord: bool) -> bool {
        self.coord.iter().any(|r| r == run) || (!coord && run == self.attempt.run)
    }

    fn caller(&self, token: &str) -> OrchCaller {
        OrchCaller {
            terminal: self.attempt.terminal.clone(),
            attempt: Some(self.attempt.id.clone()),
            token: Some(token.to_string()),
        }
    }
}

fn str_field<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}

/// Check `rule`'s targets in `p` and put the worker's caller in it.
fn check_orch(
    o: &Orch,
    s: &Scope,
    w: &WorkerConn,
    method: &str,
    p: &mut Value,
    targets: Targets,
    caller: bool,
) -> Result<(), RpcError> {
    let out = || refusal(worker_codes::OUT_OF_SCOPE, OUT_OF_SCOPE_MSG, method);
    if let Some(coord) = targets.run
        && let Some(r) = str_field(p, "run")
        && !s.reaches(r, coord)
    {
        return Err(out());
    }
    if let Some(coord) = targets.item
        && let Some(i) = str_field(p, "item")
        && !o.item(i).is_ok_and(|it| s.reaches(&it.run, coord))
    {
        return Err(out());
    }
    if let Some(coord) = targets.attempt
        && let Some(a) = str_field(p, "attempt")
        && a != s.attempt.id
        && !o.attempt(a).is_ok_and(|at| s.reaches(&at.run, coord))
    {
        return Err(out());
    }
    if let Some(coord) = targets.attempt
        && coord
        && str_field(p, "attempt") == Some(s.attempt.id.as_str())
    {
        // Stopping, abandoning, keeping or releasing itself is its
        // coordinator's (or a person's) call.
        return Err(out());
    }
    if targets.mail
        && let Some(id) = str_field(p, "id")
        && !o.find_mail(id).is_some_and(|m| s.reaches(&m.run, false))
    {
        return Err(out());
    }
    if targets.to
        && let Some(to) = str_field(p, "to")
    {
        for addr in to.split(',').map(str::trim) {
            let ok = if let Some(r) = addr.strip_prefix("run:") {
                s.reaches(r, false)
            } else if let Some(a) = addr.strip_prefix("attempt:") {
                o.attempt(a).is_ok_and(|at| s.reaches(&at.run, false))
            } else {
                // Groups are resolved in the run the daemon picks for this
                // caller (one of its own), or the `run` checked above.
                addr.starts_with('@')
            };
            if !ok {
                return Err(out());
            }
        }
    }
    if caller {
        let given: OrchCaller = match p.get("caller") {
            Some(c) => serde_json::from_value(c.clone()).map_err(|e| {
                RpcError::new(codes::INVALID_PARAMS, format!("invalid params: {e}"))
            })?,
            None => OrchCaller::default(),
        };
        if given.attempt.as_deref().is_some_and(|a| a != w.attempt)
            || given
                .terminal
                .as_ref()
                .is_some_and(|t| Some(t) != s.attempt.terminal.as_ref())
        {
            return Err(refusal(
                worker_codes::CALLER_MISMATCH,
                CALLER_MISMATCH_MSG,
                method,
            ));
        }
        if let Value::Object(m) = p {
            m.insert(
                "caller".into(),
                serde_json::to_value(s.caller(&w.token)).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

impl Daemon {
    /// The worker's own terminal is the one `p` names.
    fn check_own_terminal(&self, s: &Scope, method: &str, p: &Value) -> Result<(), RpcError> {
        let r = TerminalRef {
            id: str_field(p, "id").map(str::to_string),
            name: str_field(p, "name").map(str::to_string),
        };
        let t = self.resolve(&r)?;
        if s.attempt.terminal.as_deref() == Some(t.id.as_str()) {
            Ok(())
        } else {
            Err(refusal(
                worker_codes::OUT_OF_SCOPE,
                OUT_OF_SCOPE_MSG,
                method,
            ))
        }
    }

    /// `connection.hello` on the worker endpoint: the token must be the
    /// attempt's; the connection then speaks for that worker.
    fn worker_hello(
        &self,
        conn: &mut Option<WorkerConn>,
        p: Value,
    ) -> Result<(Value, WorkerConn), RpcError> {
        let hello: HelloParams = params(p)?;
        let refuse = || {
            refusal(
                worker_codes::NO_WORKER,
                NO_WORKER_MSG,
                ConnectionHello::NAME,
            )
        };
        let auth = hello.worker.clone().ok_or_else(refuse)?;
        let w = WorkerConn {
            attempt: auth.attempt,
            token: auth.token,
        };
        if conn.as_ref().is_some_and(|c| c.attempt != w.attempt) {
            return Err(refusal(
                worker_codes::CALLER_MISMATCH,
                CALLER_MISMATCH_MSG,
                ConnectionHello::NAME,
            ));
        }
        if Scope::of(&self.runs.engine.lock().unwrap(), &w).is_none() {
            log(&format!(
                "worker endpoint: hello refused for attempt {:?} (token)",
                w.attempt
            ));
            return Err(refuse());
        }
        let v = self.hello(hello)?;
        Ok((v, w))
    }

    /// One request on a worker endpoint connection; `None` for a
    /// notification. `conn` is the worker this connection speaks for.
    pub(in crate::server) async fn handle_worker_request(
        self: Arc<Self>,
        conn: &mut Option<WorkerConn>,
        req: Request,
    ) -> Option<Response> {
        let id = req.id.clone();
        if req.jsonrpc != JSONRPC_VERSION {
            return Some(err_response(
                id.unwrap_or(Value::Null),
                RpcError::new(codes::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
            ));
        }
        let result = if req.method == ConnectionHello::NAME {
            match self.worker_hello(conn, req.params) {
                Ok((v, w)) => {
                    *conn = Some(w);
                    Ok(v)
                }
                Err(e) => Err(e),
            }
        } else {
            self.worker_dispatch(conn.as_ref(), &req.method, req.params)
                .await
        };
        let id = id?;
        Some(match result {
            Ok(v) => ok_response(id, v),
            Err(e) => err_response(id, e),
        })
    }

    async fn worker_dispatch(
        self: Arc<Self>,
        conn: Option<&WorkerConn>,
        method: &str,
        mut p: Value,
    ) -> Result<Value, RpcError> {
        let Some(w) = conn else {
            return Err(refusal(worker_codes::NO_WORKER, NO_WORKER_MSG, method));
        };
        let Some(rule) = allowed(method) else {
            log(&format!(
                "worker endpoint: {method} refused for attempt {}",
                w.attempt
            ));
            return Err(refusal(worker_codes::NOT_ALLOWED, NOT_ALLOWED_MSG, method));
        };
        if p.is_null() {
            p = Value::Object(Default::default());
        }
        let scope = {
            let o = self.runs.engine.lock().unwrap();
            let s = Scope::of(&o, w)
                .ok_or_else(|| refusal(worker_codes::NO_WORKER, NO_WORKER_MSG, method))?;
            match rule {
                Rule::Orch(t) => check_orch(&o, &s, w, method, &mut p, t, true)?,
                Rule::OrchNoCaller(t) => check_orch(&o, &s, w, method, &mut p, t, false)?,
                Rule::Plain | Rule::OwnTerminal => {}
            }
            s
        };
        if rule == Rule::OwnTerminal {
            self.check_own_terminal(&scope, method, &p)?;
        }
        let mut v = self.clone().dispatch(method, p).await?;
        if method == OrchRunList::NAME
            && let Some(Value::Array(runs)) = v.get_mut("runs")
        {
            runs.retain(|r| {
                r.pointer("/run/id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| scope.reaches(id, false))
            });
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/19 §20: the allowlist names no method that decides, merges,
    /// types, configures or reaches another terminal; every name it does
    /// list is a real method.
    #[test]
    fn the_allowlist_is_narrow() {
        for m in [
            OrchGateDecide::NAME,
            GateResolve::NAME,
            GateCreate::NAME,
            TaskPrMerge::NAME,
            TaskPrCreate::NAME,
            GroupPick::NAME,
            TerminalSend::NAME,
            TerminalKeys::NAME,
            TerminalInput::NAME,
            TerminalInputOnce::NAME,
            TerminalRun::NAME,
            TerminalKill::NAME,
            TerminalRm::NAME,
            TerminalList::NAME,
            TerminalAttach::NAME,
            StatusSubscribe::NAME,
            TerminalAck::NAME,
            OrchLimitsSet::NAME,
            OrchLimitsGet::NAME,
            OrchConfigSet::NAME,
            OrchConfigGet::NAME,
            OrchRunCreate::NAME,
            ServerStop::NAME,
            RepoSet::NAME,
            TaskCreate::NAME,
            TaskRemove::NAME,
            ReviewSend::NAME,
            ModCommand::NAME,
            MobileRevoke::NAME,
            SessionsResume::NAME,
            "mobile.test.call",
            "orch.nope",
            "",
        ] {
            assert_eq!(allowed(m), None, "{m}");
        }
        let schema = crate::protocol::schema();
        let names: Vec<&str> = schema["methods"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|m| allowed(m).is_some())
            .collect();
        assert_eq!(names.len(), 28, "{names:?}");
        assert!(
            names.iter().all(|m| m.starts_with("orch.")
                || m.starts_with("terminal.")
                || m.starts_with("status.")
                || *m == "server.status"),
            "{names:?}"
        );
        // Mutating its helpers needs a run it coordinates.
        for m in [
            OrchWorkerStop::NAME,
            OrchWorkerAbandon::NAME,
            OrchItemCreate::NAME,
            OrchWorkerStart::NAME,
            OrchMailCheck::NAME,
        ] {
            let coord = match allowed(m) {
                Some(Rule::Orch(t) | Rule::OrchNoCaller(t)) => {
                    t.run == COORD || t.attempt == COORD || t.item == COORD
                }
                _ => false,
            };
            assert!(coord, "{m}");
        }
    }
}
