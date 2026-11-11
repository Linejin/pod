//! `host(op, json)` for one mod: the single path every host call takes.
//! Permission, scope and quota checks come first (permission and scope
//! refusals and the timer quota are also logged as `denied: …`), then the
//! [`Host`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::core::Core;
use super::handle::program_name;
use super::{EVENTS, ORCH_EVENTS, trunc};
use crate::host::{BLOCKED_KINDS, HINT_STATES, Host, Notice, StatusHint};
use crate::manifest::{Permission, valid_id};
use crate::runtime::{HostFn, Limits};
use crate::types::CommandInfo;

const MAX_SEND: usize = 4096;
/// Longest `setTimeout`/`setInterval` delay (24 h).
const MAX_TIMER_MS: f64 = 86_400_000.0;

fn denied(core: &mut Core, msg: String) -> String {
    core.log(&format!("denied: {msg}"));
    msg
}

fn arg<T: for<'de> Deserialize<'de>>(args: &str) -> Result<T, String> {
    serde_json::from_str(args).map_err(|e| format!("InvalidArgument: {e}"))
}

/// The host call behind `host(op, json)` for one mod: permission and scope
/// checks, limits, then the [`Host`].
pub(super) fn host_fn(core: Rc<RefCell<Core>>, host: Arc<dyn Host>, limits: Limits) -> HostFn {
    Rc::new(
        move |op: &str, args: &str| -> Result<Option<String>, String> {
            let mut c = core.borrow_mut();
            let need = |c: &mut Core, p: Permission| -> Result<(), String> {
                if c.has(p) {
                    Ok(())
                } else {
                    Err(denied(
                        c,
                        format!("PermissionDenied: {op} needs the {:?} permission", p.name()),
                    ))
                }
            };
            // Is `terminal` inside the callback's scope?
            let in_scope = |c: &mut Core,
                            host: &Arc<dyn Host>,
                            terminal: &str,
                            allow_scope_terminal: bool|
             -> Result<(), String> {
                if allow_scope_terminal && c.scope.terminal.as_deref() == Some(terminal) {
                    return Ok(());
                }
                let Some(task) = c.scope.task.clone() else {
                    return Err(denied(
                        c,
                        format!("PermissionDenied: {op}: this callback has no task scope"),
                    ));
                };
                let t = host.terminals().into_iter().find(|t| t.id == terminal);
                match t {
                    Some(t) if t.task.as_deref() == Some(task.as_str()) => Ok(()),
                    Some(_) => Err(denied(
                        c,
                        format!("PermissionDenied: terminal {terminal} is not in task {task}"),
                    )),
                    None => Err(denied(
                        c,
                        format!("InvalidArgument: no terminal {terminal}"),
                    )),
                }
            };
            match op {
                "log" => {
                    #[derive(Deserialize)]
                    struct A {
                        msg: String,
                    }
                    let a: A = arg(args)?;
                    c.log(&trunc(&a.msg, 2000));
                    Ok(None)
                }
                "register.event" => {
                    #[derive(Deserialize)]
                    struct A {
                        event: String,
                    }
                    let a: A = arg(args)?;
                    if a.event != "*" && !EVENTS.contains(&a.event.as_str()) {
                        return Err(format!(
                            "InvalidArgument: unknown event {:?} (known: {})",
                            a.event,
                            EVENTS.join(", ")
                        ));
                    }
                    // Registering needs the request in mod.json; delivery needs
                    // the grant (so a mod still loads before the user grants it).
                    if ORCH_EVENTS.contains(&a.event.as_str())
                        && !c
                            .manifest
                            .as_ref()
                            .is_ok_and(|m| m.permissions.contains(&Permission::Orch))
                    {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: {} needs the \"orch\" permission in mod.json",
                                a.event
                            ),
                        ));
                    }
                    c.events.insert(a.event);
                    Ok(None)
                }
                "register.command" => {
                    #[derive(Deserialize)]
                    struct A {
                        id: String,
                        title: String,
                        detail: Option<String>,
                    }
                    need(&mut c, Permission::Commands)?;
                    if !c.loading {
                        return Err(
                            "InvalidArgument: pod.command may only be called while the mod loads"
                                .into(),
                        );
                    }
                    let a: A = arg(args)?;
                    if !valid_id(&a.id) {
                        return Err(format!(
                            "InvalidArgument: command id {:?} must match [a-z0-9-]{{1,32}}",
                            a.id
                        ));
                    }
                    if a.title.trim().is_empty() {
                        return Err("InvalidArgument: a command needs a title".into());
                    }
                    if c.commands.iter().any(|x| x.id == a.id) || c.commands.len() >= 32 {
                        return Err(format!(
                            "InvalidArgument: command {:?} is defined twice (or more than 32 commands)",
                            a.id
                        ));
                    }
                    c.commands.push(CommandInfo {
                        id: a.id,
                        title: trunc(&a.title, 60),
                        detail: a.detail.map(|d| trunc(&d, 120)),
                    });
                    Ok(None)
                }
                "register.detector" => {
                    #[derive(Deserialize)]
                    struct A {
                        #[serde(rename = "match")]
                        names: Vec<String>,
                    }
                    need(&mut c, Permission::Status)?;
                    if !c.loading {
                        return Err(
                            "InvalidArgument: pod.detector may only be called while the mod loads"
                                .into(),
                        );
                    }
                    let a: A = arg(args)?;
                    let names: Vec<String> = a
                        .names
                        .iter()
                        .map(|n| program_name(n.trim()))
                        .filter(|n| !n.is_empty() && n.len() <= 64)
                        .collect();
                    if names.is_empty() || names.len() > 8 || c.detectors.len() >= 8 {
                        return Err("InvalidArgument: a detector matches 1-8 program names (at most 8 detectors)".into());
                    }
                    c.detectors.push(names);
                    Ok(None)
                }
                "notify" => {
                    #[derive(Deserialize)]
                    struct A {
                        title: String,
                        body: String,
                        level: String,
                    }
                    need(&mut c, Permission::Notify)?;
                    let a: A = arg(args)?;
                    if !["info", "success", "warning", "error"].contains(&a.level.as_str()) {
                        return Err(format!(
                            "InvalidArgument: level {:?} must be info, success, warning or error",
                            a.level
                        ));
                    }
                    c.spend(&limits, true)?;
                    let notice = Notice {
                        mod_name: c.name.clone(),
                        title: trunc(&a.title, 100),
                        body: trunc(&a.body, 500),
                        level: a.level,
                        task: c.scope.task.clone(),
                        terminal: c.scope.terminal.clone(),
                    };
                    c.log(&format!("notify: {} — {}", notice.title, notice.body));
                    drop(c);
                    host.notify(notice);
                    Ok(None)
                }
                "query.terminals" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.terminals()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "query.tasks" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.tasks()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "query.groups" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.groups()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "terminal.send" => {
                    #[derive(Deserialize)]
                    struct A {
                        terminal: String,
                        text: String,
                        enter: bool,
                    }
                    need(&mut c, Permission::TerminalSend)?;
                    let a: A = arg(args)?;
                    if a.text.len() > MAX_SEND {
                        return Err(format!(
                            "InvalidArgument: at most {MAX_SEND} bytes per send"
                        ));
                    }
                    in_scope(&mut c, &host, &a.terminal, false)?;
                    c.spend(&limits, true)?;
                    c.log(&format!(
                        "send to {}: {} bytes{}",
                        a.terminal,
                        a.text.len(),
                        if a.enter { " + Enter" } else { "" }
                    ));
                    drop(c);
                    host.send_text(&a.terminal, &a.text, a.enter)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "terminal.status" => {
                    #[derive(Deserialize)]
                    struct A {
                        terminal: String,
                        state: String,
                        kind: Option<String>,
                        msg: Option<String>,
                    }
                    need(&mut c, Permission::Status)?;
                    let a: A = arg(args)?;
                    let hint = parse_hint(&a.state, a.kind, a.msg)?;
                    in_scope(&mut c, &host, &a.terminal, true)?;
                    c.spend(&limits, false)?;
                    let owner = c.name.clone();
                    drop(c);
                    host.set_status(&owner, &a.terminal, hint)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "task.check" => {
                    #[derive(Deserialize)]
                    struct A {
                        task: String,
                        check: String,
                    }
                    need(&mut c, Permission::Check)?;
                    let a: A = arg(args)?;
                    let argv = c
                        .manifest
                        .as_ref()
                        .ok()
                        .and_then(|m| m.checks.get(&a.check).cloned());
                    let Some(argv) = argv else {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: check {:?} is not declared in mod.json",
                                a.check
                            ),
                        ));
                    };
                    if c.scope.task.as_deref() != Some(a.task.as_str()) {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: task {} is not this callback's task",
                                a.task
                            ),
                        ));
                    }
                    c.spend(&limits, true)?;
                    c.log(&format!(
                        "check {} in {}: {}",
                        a.check,
                        a.task,
                        argv.join(" ")
                    ));
                    let owner = c.name.clone();
                    drop(c);
                    host.run_check(&owner, &a.task, &a.check, argv)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "gate.request" => {
                    #[derive(Deserialize)]
                    struct A {
                        task: String,
                        title: String,
                        message: String,
                    }
                    need(&mut c, Permission::Gate)?;
                    let a: A = arg(args)?;
                    if c.scope.task.as_deref() != Some(a.task.as_str()) {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: task {} is not this callback's task",
                                a.task
                            ),
                        ));
                    }
                    if a.message.trim().is_empty() {
                        return Err("InvalidArgument: a gate needs a message".into());
                    }
                    c.spend(&limits, true)?;
                    let owner = c.name.clone();
                    c.log(&format!(
                        "gate request in {}: {}",
                        a.task,
                        trunc(&a.title, 100)
                    ));
                    drop(c);
                    let id = host
                        .request_gate(
                            &owner,
                            &a.task,
                            &trunc(&a.title, 100),
                            &trunc(&a.message, 2000),
                        )
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(Some(serde_json::to_string(&id).unwrap_or_default()))
                }
                "timer.set" => {
                    #[derive(Deserialize)]
                    struct A {
                        delay: f64,
                        repeat: bool,
                    }
                    let a: A = arg(args)?;
                    if !a.delay.is_finite() || a.delay > MAX_TIMER_MS {
                        return Err(format!(
                            "InvalidArgument: a timer delay is at most {MAX_TIMER_MS} ms"
                        ));
                    }
                    if c.timers.len() as u32 >= limits.max_timers {
                        return Err(denied(
                            &mut c,
                            format!(
                                "RateLimited: more than {} pending timers",
                                limits.max_timers
                            ),
                        ));
                    }
                    let min = if a.repeat {
                        limits.min_interval_ms
                    } else {
                        limits.min_timeout_ms
                    };
                    let delay = Duration::from_millis((a.delay.max(0.0) as u64).max(min));
                    let id = c.add_timer(delay, a.repeat);
                    Ok(Some(id.to_string()))
                }
                "timer.clear" => {
                    #[derive(Deserialize)]
                    struct A {
                        id: u32,
                    }
                    let a: A = arg(args)?;
                    c.timers.remove(&a.id);
                    Ok(None)
                }
                other => Err(format!("InvalidArgument: unknown host call {other:?}")),
            }
        },
    )
}

/// `None` = clear (`unknown`).
pub(super) fn parse_hint(
    state: &str,
    kind: Option<String>,
    msg: Option<String>,
) -> Result<Option<StatusHint>, String> {
    if state == "unknown" {
        return Ok(None);
    }
    if !HINT_STATES.contains(&state) {
        return Err(format!(
            "InvalidArgument: state {state:?} must be one of {} or unknown",
            HINT_STATES.join(", ")
        ));
    }
    let kind = match (state, kind) {
        ("blocked", Some(k)) if BLOCKED_KINDS.contains(&k.as_str()) => Some(k),
        ("blocked", Some(k)) => {
            return Err(format!(
                "InvalidArgument: blocked kind {k:?} must be one of {}",
                BLOCKED_KINDS.join(", ")
            ));
        }
        ("blocked", None) => Some("other".to_string()),
        _ => None,
    };
    Ok(Some(StatusHint {
        state: state.to_string(),
        kind,
        msg: msg.map(|m| trunc(&m, 200)),
    }))
}
