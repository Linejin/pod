//! The `orch.*` dispatch and the engine's error codes. Mutating calls
//! take `ops` (here or in their handler) so a `request_id` retry sees the
//! first result.

use std::sync::Arc;

use pod_orch::engine::{ErrorKind, OrchError};
use serde_json::Value;

use crate::protocol::*;
use crate::server::{Daemon, params, to_value};

/// O1 (docs/19 §19): the agent trust policy says a person must act (a
/// gate a coordinator may not decide, limits changed by an agent). The
/// O1 card owns -32110..-32114; the constant moves to `pod-protocol`'s
/// `run_codes` with the next protocol change.
pub(super) const NEEDS_PERSON: i64 = -32110;

pub(super) fn oe(e: OrchError) -> RpcError {
    use run_codes::*;
    let code = match e.kind {
        ErrorKind::InvalidParams => codes::INVALID_PARAMS,
        ErrorKind::InvalidState => orch_codes::INVALID_STATE,
        ErrorKind::RunNotFound => RUN_NOT_FOUND,
        ErrorKind::ItemNotFound => ITEM_NOT_FOUND,
        ErrorKind::AttemptNotFound => ATTEMPT_NOT_FOUND,
        ErrorKind::MailNotFound => MAIL_NOT_FOUND,
        ErrorKind::AttemptFenced => ATTEMPT_FENCED,
        ErrorKind::DepthExceeded => DEPTH_EXCEEDED,
        ErrorKind::CircuitOpen => CIRCUIT_OPEN,
        ErrorKind::DepsNotDone => DEPS_NOT_DONE,
        ErrorKind::WorkerLimit => WORKER_LIMIT,
        ErrorKind::Io => codes::INTERNAL_ERROR,
    };
    RpcError::new(code, e.message)
}

pub(in crate::server) fn handles(method: &str) -> bool {
    method.starts_with("orch.")
}

pub(in crate::server) async fn dispatch(
    d: Arc<Daemon>,
    method: &str,
    p: Value,
) -> Result<Value, RpcError> {
    match method {
        OrchRunCreate::NAME => {
            let p: OrchRunCreateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_run_create(p)?)
        }
        OrchRunList::NAME => to_value(d.orch_run_list(params(p)?)?),
        OrchRunShow::NAME => to_value(d.orch_run_show(params(p)?)?),
        OrchItemCreate::NAME => {
            let p: OrchItemCreateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_item_create(p)?)
        }
        OrchItemList::NAME => to_value(d.orch_item_list(params(p)?)?),
        OrchItemUpdate::NAME => {
            let p: OrchItemUpdateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_item_update(p)?)
        }
        OrchWorkerStart::NAME => to_value(d.orch_worker_start(params(p)?).await?),
        OrchWorkerList::NAME => to_value(d.orch_worker_list(params(p)?)?),
        OrchWorkerShow::NAME => to_value(d.orch_worker_show(params(p)?)?),
        OrchWorkerRead::NAME => to_value(d.orch_worker_read(params(p)?)?),
        OrchWorkerStop::NAME => to_value(d.orch_worker_stop(params(p)?).await?),
        OrchWorkerAbandon::NAME | OrchWorkerKeep::NAME | OrchWorkerRelease::NAME => {
            let m = match method {
                OrchWorkerAbandon::NAME => OrchWorkerAbandon::NAME,
                OrchWorkerKeep::NAME => OrchWorkerKeep::NAME,
                _ => OrchWorkerRelease::NAME,
            };
            to_value(d.orch_worker_simple(m, params(p)?).await?)
        }
        OrchMailSend::NAME => to_value(d.orch_mail_send(params(p)?).await?),
        OrchMailCheck::NAME => to_value(d.orch_mail_check(params(p)?).await?),
        OrchMailReply::NAME => to_value(d.orch_mail_reply(params(p)?).await?),
        OrchMailAsk::NAME => to_value(d.orch_mail_ask(params(p)?).await?),
        OrchDone::NAME => to_value(d.orch_done(params(p)?).await?),
        OrchWait::NAME => to_value(d.orch_wait(params(p)?).await?),
        OrchCommit::NAME => to_value(d.orch_commit(params(p)?).await?),
        OrchMailList::NAME => to_value(d.orch_mail_list(params(p)?)?),
        OrchConfigGet::NAME => to_value(d.orch_config().settings()),
        OrchConfigSet::NAME => {
            let p: OrchSettings = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_config_set(p)?)
        }
        OrchQueue::NAME => to_value(d.orch_queue(params(p)?)?),
        OrchGateRequest::NAME => {
            let p: OrchGateRequestParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_gate_request(p)?)
        }
        OrchGateDecide::NAME => to_value(d.orch_gate_decide(params(p)?)?),
        OrchLimitsGet::NAME => {
            let _: OrchLimitsGetParams = params(p)?;
            to_value(d.orch_limits_result())
        }
        OrchLimitsSet::NAME => {
            let p: OrchLimitsSetParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_limits_set(p)?)
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_map() {
        let e = oe(OrchError::new(ErrorKind::AttemptFenced, "x"));
        assert_eq!(e.code, run_codes::ATTEMPT_FENCED);
        assert_eq!(
            oe(OrchError::new(ErrorKind::InvalidState, "x")).code,
            orch_codes::INVALID_STATE
        );
        assert_eq!(
            oe(OrchError::new(ErrorKind::DepthExceeded, "x")).code,
            run_codes::DEPTH_EXCEEDED
        );
    }
}
