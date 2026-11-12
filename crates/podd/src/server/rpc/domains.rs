//! Every method the daemon serves, registered by domain: system, terminal,
//! status, repo/task, PR, review/group/gate, orch, discovery, mods, mobile
//! (and the debug-only `mobile.test.*`). The direct handlers are the former
//! dispatch match arms, moved verbatim; the status, orch, discovery, mods and
//! mobile methods go to their domain's own dispatch, which also answers the
//! unregistered names it claims ([`CLAIMS`]).

use std::sync::Arc;

use serde_json::Value;

use super::registry::{
    Claim, MethodRegistration, Mode, RpcFuture, RpcHandler, duplicate, unknown_method,
};
use super::{params, to_value};
use crate::protocol::*;
use crate::server::terminal_ops::id_number;
use crate::server::{Daemon, discovery, mobile, mods, orch_runs, status};

const fn unary<M: Method>(handler: RpcHandler) -> MethodRegistration {
    MethodRegistration::unary::<M>(handler)
}

/// All registered methods: the public ones (the schema's `methods`) and
/// [`MOBILE_TEST_HOOKS`].
pub(super) const REGISTERED: &[&[MethodRegistration]] = &[
    SYSTEM,
    TERMINAL,
    STATUS,
    REPO_TASK,
    PR,
    REVIEW_GROUP_GATE,
    ORCH,
    DISCOVERY,
    MODS,
    MOBILE,
    MOBILE_TEST_HOOKS,
];

// A name registered twice fails the build; the message is that name.
const _: () = if let Some(name) = duplicate(REGISTERED) {
    panic!("{}", name)
};

/// The domain dispatches, in their former order, for names nobody
/// registered. No two `handles` claim the same unregistered name (`tests`
/// below), so the order never decides.
pub(super) const CLAIMS: &[Claim] = &[
    Claim {
        handles: status::handles,
        handler: dispatch_status,
    },
    Claim {
        handles: mods::handles,
        handler: dispatch_mods,
    },
    Claim {
        handles: orch_runs::handles,
        handler: dispatch_orch,
    },
    Claim {
        handles: discovery::handles,
        handler: dispatch_discovery,
    },
    Claim {
        handles: mobile::handles,
        handler: dispatch_mobile,
    },
];

// ───────────────────────────── system ─────────────────────────────

const SYSTEM: &[MethodRegistration] = &[
    unary::<ServerStatus>(|d, _, p| {
        Box::pin(async move {
            let _: Empty = params(p)?;
            to_value(d.status())
        })
    }),
    unary::<ServerStop>(|d, _, p| Box::pin(async move { to_value(d.stop(params(p)?)) })),
    unary::<ConnectionHello>(|d, _, p| Box::pin(async move { d.hello(params(p)?) })),
];

// ───────────────────────────── terminal ─────────────────────────────

const TERMINAL: &[MethodRegistration] = &[
    unary::<TerminalRun>(|d, _, p| {
        Box::pin(async move { to_value(d.run_in_task(params(p)?).await?) })
    }),
    unary::<TerminalList>(|d, _, p| {
        Box::pin(async move {
            let _: Empty = params(p)?;
            let mut terminals = d.list();
            terminals.extend(d.unreachable_infos());
            terminals.sort_by_key(|t| id_number(&t.id));
            let retention = terminals
                .iter()
                .map(|t| d.retention_of(t))
                .filter(|r| r.kept || r.expires_ms.is_some())
                .collect();
            to_value(ListResult {
                terminals,
                retention_ms: d.config.exited_retention.map(|r| r.as_millis() as u64),
                retention,
            })
        })
    }),
    unary::<TerminalSend>(|d, _, p| Box::pin(async move { to_value(d.send(params(p)?).await?) })),
    unary::<TerminalKeys>(|d, _, p| Box::pin(async move { to_value(d.keys(params(p)?)?) })),
    unary::<TerminalInput>(|d, _, p| Box::pin(async move { to_value(d.input(params(p)?)?) })),
    // v20 (docs/17 R4): input with an identity, applied at most once.
    unary::<TerminalInputOnce>(|d, _, p| {
        Box::pin(async move { to_value(d.input_once(params(p)?)?) })
    }),
    unary::<TerminalRead>(|d, _, p| Box::pin(async move { to_value(d.read(params(p)?)?) })),
    unary::<TerminalWait>(|d, _, p| Box::pin(async move { to_value(d.wait(params(p)?).await?) })),
    unary::<TerminalResize>(|d, _, p| {
        Box::pin(async move { to_value(d.resize(params(p)?).await?) })
    }),
    unary::<TerminalKill>(|d, _, p| Box::pin(async move { to_value(d.kill(params(p)?).await?) })),
    unary::<TerminalRm>(|d, _, p| Box::pin(async move { to_value(d.rm(params(p)?).await?) })),
    unary::<TerminalKeep>(|d, _, p| Box::pin(async move { to_value(d.keep(params(p)?)?) })),
    // A request with an id becomes the output stream (connection.rs).
    MethodRegistration::stream::<TerminalAttach>(Mode::Attach, |_, _, _| {
        Box::pin(async {
            Err(RpcError::new(
                codes::INVALID_REQUEST,
                "terminal.attach must be a request with an id",
            ))
        })
    }),
];

// ───────────────────────────── status ─────────────────────────────

const STATUS: &[MethodRegistration] = &[
    unary::<TerminalShow>(dispatch_status),
    unary::<TerminalAck>(dispatch_status),
    unary::<StatusWait>(dispatch_status),
    // A request with an id becomes the event stream (connection.rs); the
    // status dispatch never served it as one request.
    MethodRegistration::stream::<StatusSubscribe>(Mode::Subscribe, |_, m, _| {
        Box::pin(async move { Err(unknown_method(m)) })
    }),
];

fn dispatch_status(d: Arc<Daemon>, m: &str, p: Value) -> RpcFuture<'_> {
    Box::pin(status::dispatch(d, m, p))
}

// ───────────────────────────── repo and task ─────────────────────────────

const REPO_TASK: &[MethodRegistration] = &[
    unary::<RepoAdd>(|d, _, p| Box::pin(async move { to_value(d.repo_add(params(p)?).await?) })),
    unary::<RepoList>(|d, _, p| {
        Box::pin(async move {
            let _: Empty = params(p)?;
            to_value(d.repo_list().await)
        })
    }),
    unary::<RepoRemove>(|d, _, p| Box::pin(async move { to_value(d.repo_remove(params(p)?)?) })),
    unary::<TaskCreate>(|d, _, p| {
        Box::pin(async move { to_value(d.task_create(params(p)?).await?) })
    }),
    unary::<TaskList>(|d, _, p| Box::pin(async move { to_value(d.task_list(params(p)?).await?) })),
    unary::<TaskShow>(|d, _, p| Box::pin(async move { to_value(d.task_show(params(p)?).await?) })),
    unary::<TaskDiff>(|d, _, p| Box::pin(async move { to_value(d.task_diff(params(p)?).await?) })),
    unary::<TaskRemove>(|d, _, p| {
        Box::pin(async move { to_value(d.task_remove(params(p)?).await?) })
    }),
    unary::<TaskTouch>(|d, _, p| {
        Box::pin(async move { to_value(d.task_touch(params(p)?).await?) })
    }),
    unary::<TaskArchive>(|d, _, p| {
        Box::pin(async move { to_value(d.task_archive(params(p)?).await?) })
    }),
    unary::<TaskUnarchive>(|d, _, p| {
        Box::pin(async move { to_value(d.task_unarchive(params(p)?).await?) })
    }),
    unary::<TaskResume>(|d, _, p| {
        Box::pin(async move { to_value(d.task_resume(params(p)?).await?) })
    }),
    unary::<TaskCleanupCandidates>(|d, _, p| {
        Box::pin(async move { to_value(d.task_cleanup_candidates(params(p)?).await?) })
    }),
    unary::<TaskSuggestName>(|d, _, p| {
        Box::pin(async move { to_value(d.task_suggest_name(params(p)?).await?) })
    }),
    unary::<RepoConfigMethod>(|d, _, p| {
        Box::pin(async move { to_value(d.repo_config(params(p)?).await?) })
    }),
    unary::<RepoSet>(|d, _, p| Box::pin(async move { to_value(d.repo_set(params(p)?).await?) })),
];

// ───────────────────────────── PR ─────────────────────────────

const PR: &[MethodRegistration] = &[
    unary::<TaskPrStatusMethod>(|d, _, p| {
        Box::pin(async move { to_value(d.task_pr_status(params(p)?).await?) })
    }),
    unary::<TaskPrCreate>(|d, _, p| {
        Box::pin(async move { to_value(d.task_pr_create(params(p)?).await?) })
    }),
    unary::<TaskPrMerge>(|d, _, p| {
        Box::pin(async move { to_value(d.task_pr_merge(params(p)?).await?) })
    }),
];

// ───────────────────────────── review, group, gate ─────────────────────────────

const REVIEW_GROUP_GATE: &[MethodRegistration] = &[
    unary::<ReviewCommentsAdd>(|d, _, p| {
        Box::pin(async move { to_value(d.review_add(params(p)?).await?) })
    }),
    unary::<ReviewCommentsList>(|d, _, p| {
        Box::pin(async move { to_value(d.review_list(params(p)?)?) })
    }),
    unary::<ReviewCommentsResolve>(|d, _, p| {
        Box::pin(async move { to_value(d.review_resolve(params(p)?)?) })
    }),
    unary::<ReviewCommentsRemove>(|d, _, p| {
        Box::pin(async move { to_value(d.review_remove(params(p)?)?) })
    }),
    unary::<ReviewSend>(|d, _, p| {
        Box::pin(async move { to_value(d.review_send(params(p)?).await?) })
    }),
    unary::<ReviewDeliveries>(|d, _, p| {
        Box::pin(async move { to_value(d.review_deliveries(params(p)?)?) })
    }),
    unary::<ReviewCancel>(|d, _, p| {
        Box::pin(async move { to_value(d.review_cancel(params(p)?)?) })
    }),
    unary::<GroupCreate>(|d, _, p| {
        Box::pin(async move { to_value(d.group_create(params(p)?).await?) })
    }),
    unary::<GroupList>(|d, _, p| {
        Box::pin(async move { to_value(d.group_list(params(p)?).await?) })
    }),
    unary::<GroupShow>(|d, _, p| {
        Box::pin(async move { to_value(d.group_show(params(p)?).await?) })
    }),
    unary::<GroupCompare>(|d, _, p| {
        Box::pin(async move { to_value(d.group_compare(params(p)?).await?) })
    }),
    unary::<GroupCheck>(|d, _, p| {
        Box::pin(async move { to_value(d.group_check(params(p)?).await?) })
    }),
    unary::<GroupCheckCancel>(|d, _, p| {
        Box::pin(async move { to_value(d.group_check_cancel(params(p)?).await?) })
    }),
    unary::<GroupPick>(|d, _, p| {
        Box::pin(async move { to_value(d.group_pick(params(p)?).await?) })
    }),
    unary::<GroupRemove>(|d, _, p| Box::pin(async move { to_value(d.group_remove(params(p)?)?) })),
    unary::<GateCreate>(|d, _, p| Box::pin(async move { to_value(d.gate_create(params(p)?)?) })),
    unary::<GateResolve>(|d, _, p| Box::pin(async move { to_value(d.gate_resolve(params(p)?)?) })),
    unary::<GateList>(|d, _, p| Box::pin(async move { to_value(d.gate_list(params(p)?)?) })),
    unary::<GateShow>(|d, _, p| Box::pin(async move { to_value(d.gate_show(params(p)?)?) })),
    unary::<GateWait>(|d, _, p| Box::pin(async move { to_value(d.gate_wait(params(p)?).await?) })),
];

// ───────────────────────────── orch ─────────────────────────────

const ORCH: &[MethodRegistration] = &[
    unary::<OrchRunCreate>(dispatch_orch),
    unary::<OrchRunList>(dispatch_orch),
    unary::<OrchRunShow>(dispatch_orch),
    unary::<OrchItemCreate>(dispatch_orch),
    unary::<OrchItemList>(dispatch_orch),
    unary::<OrchItemUpdate>(dispatch_orch),
    unary::<OrchWorkerStart>(dispatch_orch),
    unary::<OrchWorkerList>(dispatch_orch),
    unary::<OrchWorkerShow>(dispatch_orch),
    unary::<OrchWorkerRead>(dispatch_orch),
    unary::<OrchWorkerStop>(dispatch_orch),
    unary::<OrchWorkerAbandon>(dispatch_orch),
    unary::<OrchWorkerKeep>(dispatch_orch),
    unary::<OrchWorkerRelease>(dispatch_orch),
    unary::<OrchMailSend>(dispatch_orch),
    unary::<OrchMailCheck>(dispatch_orch),
    unary::<OrchMailReply>(dispatch_orch),
    unary::<OrchMailAsk>(dispatch_orch),
    unary::<OrchDone>(dispatch_orch),
    unary::<OrchMailList>(dispatch_orch),
    unary::<OrchConfigGet>(dispatch_orch),
    unary::<OrchConfigSet>(dispatch_orch),
    unary::<OrchQueue>(dispatch_orch),
    unary::<OrchGateRequest>(dispatch_orch),
    unary::<OrchGateDecide>(dispatch_orch),
    unary::<OrchLimitsGet>(dispatch_orch),
    unary::<OrchLimitsSet>(dispatch_orch),
    unary::<OrchWait>(dispatch_orch),
    unary::<OrchCommit>(dispatch_orch),
];

fn dispatch_orch(d: Arc<Daemon>, m: &str, p: Value) -> RpcFuture<'_> {
    Box::pin(orch_runs::dispatch(d, m, p))
}

// ───────────────────────────── discovery ─────────────────────────────

const DISCOVERY: &[MethodRegistration] = &[
    unary::<SessionsListMethod>(dispatch_discovery),
    unary::<SessionsScan>(dispatch_discovery),
    unary::<SessionsSettings>(dispatch_discovery),
    unary::<SessionsRegister>(dispatch_discovery),
    unary::<SessionsResume>(dispatch_discovery),
    unary::<SessionsHide>(dispatch_discovery),
];

fn dispatch_discovery(d: Arc<Daemon>, m: &str, p: Value) -> RpcFuture<'_> {
    Box::pin(discovery::dispatch(d, m, p))
}

// ───────────────────────────── mods ─────────────────────────────

const MODS: &[MethodRegistration] = &[
    unary::<ModListMethod>(dispatch_mods),
    unary::<ModSetEnabled>(dispatch_mods),
    unary::<ModGrant>(dispatch_mods),
    unary::<ModLog>(dispatch_mods),
    unary::<ModCommand>(dispatch_mods),
    unary::<ModReload>(dispatch_mods),
];

fn dispatch_mods(_: Arc<Daemon>, m: &str, p: Value) -> RpcFuture<'_> {
    Box::pin(mods::dispatch(m, p))
}

// ───────────────────────────── mobile ─────────────────────────────

const MOBILE: &[MethodRegistration] = &[
    unary::<MobileDevices>(dispatch_mobile),
    unary::<MobileRevoke>(dispatch_mobile),
    unary::<MobilePolicy>(dispatch_mobile),
    unary::<MobileAudit>(dispatch_mobile),
    unary::<MobileSettingsMethod>(dispatch_mobile),
];

/// Not public (not in the schema). The mobile dispatch keeps their gate: a
/// debug build with `POD_MOBILE_TEST_HOOKS=1`, else an unknown method.
const MOBILE_TEST_HOOKS: &[MethodRegistration] = &[
    MethodRegistration::new(mobile::TEST_PAIR, Mode::Unary, dispatch_mobile),
    MethodRegistration::new(mobile::TEST_SESSION, Mode::Unary, dispatch_mobile),
    MethodRegistration::new(mobile::TEST_CALL, Mode::Unary, dispatch_mobile),
];

fn dispatch_mobile(d: Arc<Daemon>, m: &str, p: Value) -> RpcFuture<'_> {
    Box::pin(mobile::dispatch(d, m, p))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::registry::RpcRegistry;
    use super::*;

    fn names(tables: &[&[MethodRegistration]]) -> BTreeSet<&'static str> {
        tables
            .iter()
            .flat_map(|t| t.iter().map(|r| r.name))
            .collect()
    }

    /// The domains whose `handles` claims `method`.
    fn claimed_by(method: &str) -> Vec<&'static str> {
        [
            ("status", status::handles(method)),
            ("mods", mods::handles(method)),
            ("orch", orch_runs::handles(method)),
            ("discovery", discovery::handles(method)),
            ("mobile", mobile::handles(method)),
        ]
        .into_iter()
        .filter(|(_, claimed)| *claimed)
        .map(|(domain, _)| domain)
        .collect()
    }

    #[test]
    fn the_public_methods_are_the_schema_methods() {
        let schema = crate::protocol::schema();
        let schema: BTreeSet<&str> = schema["methods"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let hooks = names(&[MOBILE_TEST_HOOKS]);
        let public: BTreeSet<&str> = names(REGISTERED).difference(&hooks).copied().collect();
        // 105 since v20 added terminal.input_once (docs/17 R4).
        assert_eq!(public.len(), 105);
        assert_eq!(public, schema);
        assert!(hooks.iter().all(|m| m.starts_with("mobile.test.")));
        assert_eq!(hooks.len(), 3);
    }

    #[test]
    fn only_attach_and_subscribe_are_streams() {
        for r in REGISTERED.iter().flat_map(|t| t.iter()) {
            let want = match r.name {
                TerminalAttach::NAME => Mode::Attach,
                StatusSubscribe::NAME => Mode::Subscribe,
                _ => Mode::Unary,
            };
            assert_eq!(r.mode, want, "{}", r.name);
        }
    }

    /// Each name goes to the dispatch the former ordered match chose: the
    /// domain whose `handles` claims it, or a direct handler when none does.
    #[test]
    fn each_method_keeps_its_former_dispatch() {
        let tables: [(&[MethodRegistration], Option<&str>); 11] = [
            (SYSTEM, None),
            (TERMINAL, None),
            (STATUS, Some("status")),
            (REPO_TASK, None),
            (PR, None),
            (REVIEW_GROUP_GATE, None),
            (ORCH, Some("orch")),
            (DISCOVERY, Some("discovery")),
            (MODS, Some("mods")),
            (MOBILE, Some("mobile")),
            (MOBILE_TEST_HOOKS, Some("mobile")),
        ];
        assert_eq!(tables.len(), REGISTERED.len());
        for (table, domain) in tables {
            for r in table {
                // Streams were never in a domain dispatch.
                let want = if r.mode == Mode::Unary { domain } else { None };
                assert_eq!(claimed_by(r.name), Vec::from_iter(want), "{}", r.name);
            }
        }
    }

    #[test]
    fn an_unregistered_name_reaches_at_most_one_claim() {
        let r = RpcRegistry::new(REGISTERED, CLAIMS);
        for (name, domain) in [
            ("orch.nope", Some("orch")),
            ("orch.", Some("orch")),
            ("mod.nope", Some("mods")),
            ("sessions.nope", Some("discovery")),
            ("mobile.test.nope", Some("mobile")),
            ("mobile.nope", None),
            ("status.nope", None),
            ("terminal.nope", None),
            ("orch", None),
            ("", None),
        ] {
            assert!(r.get(name).is_none(), "{name}");
            assert_eq!(claimed_by(name), Vec::from_iter(domain), "{name}");
            assert_eq!(r.handler(name).is_some(), domain.is_some(), "{name}");
        }
    }

    #[test]
    fn the_index_holds_every_registration() {
        let r = RpcRegistry::new(REGISTERED, CLAIMS);
        assert_eq!(r.names().collect::<BTreeSet<_>>(), names(REGISTERED));
        assert_eq!(r.names().count(), 105 + 3);
        assert_eq!(
            r.get(TerminalAttach::NAME).map(|m| m.mode),
            Some(Mode::Attach)
        );
    }
}
