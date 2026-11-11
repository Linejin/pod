//! Runs and items: `orch.run.*` and `orch.item.*`.

use pod_orch::engine::Orch;
use pod_orch::model::run_mailbox;

use super::identity::who;
use super::rpc::oe;
use super::state::{Unconfirmed, cached, remember};
use crate::protocol::*;
use crate::server::{Daemon, log, status};

pub(super) fn run_info(
    o: &Orch,
    run: &str,
    unconfirmed: &Unconfirmed,
) -> Result<RunInfo, RpcError> {
    let st = o.run(run).map_err(oe)?;
    Ok(RunInfo {
        wake_unconfirmed_ms: unconfirmed.since(o, &run_mailbox(run)),
        run: st.rec.run.clone(),
        items: st.rec.items.clone(),
        attempts: st.rec.attempts.iter().map(|a| a.public()).collect(),
        unread: o.unread(&run_mailbox(run)).len(),
        mail: st.mail.len(),
    })
}

impl Daemon {
    pub(super) fn orch_run_create(&self, p: OrchRunCreateParams) -> Result<RunInfo, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchRunCreate::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        let run = o
            .create_run(
                &p.objective,
                w.terminal.as_deref(),
                w.attempt.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        let info = run_info(&o, &run.id, &self.runs.unconfirmed)?;
        remember(&mut o, p.request_id.as_deref(), OrchRunCreate::NAME, &info)?;
        status::publish_orch_changed(&run.id, None, None);
        log(&format!("orch: run {} by {}", run.id, w.from));
        Ok(info)
    }

    pub(super) fn orch_run_list(
        &self,
        p: OrchRunListParams,
    ) -> Result<OrchRunListResult, RpcError> {
        let o = self.orch();
        let mine: Option<Vec<String>> = if p.mine {
            let w = who(&o, &p.caller)?;
            let mut v: Vec<String> = w
                .terminal
                .as_deref()
                .map(|t| o.runs_of_terminal(t))
                .unwrap_or_default();
            v.extend(w.works_in);
            Some(v)
        } else {
            None
        };
        let ids: Vec<String> = o
            .runs()
            .map(|r| r.rec.run.id.clone())
            .filter(|r| mine.as_ref().is_none_or(|m| m.contains(r)))
            .collect();
        Ok(OrchRunListResult {
            runs: ids
                .iter()
                .map(|r| run_info(&o, r, &self.runs.unconfirmed))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn orch_run_show(&self, p: OrchRunRef) -> Result<RunInfo, RpcError> {
        let o = self.orch();
        let run = self.caller_run(&o, p.run, &p.caller)?;
        run_info(&o, &run, &self.runs.unconfirmed)
    }

    pub(super) fn orch_item_create(&self, p: OrchItemCreateParams) -> Result<Item, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchItemCreate::NAME)? {
            return Ok(r);
        }
        let run = self.caller_run(&o, p.run, &p.caller)?;
        let it = o
            .add_item(
                &run,
                p.title.as_deref(),
                &p.spec,
                &p.deps,
                p.parent.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        remember(&mut o, p.request_id.as_deref(), OrchItemCreate::NAME, &it)?;
        status::publish_orch_changed(&run, Some(&it.id), None);
        Ok(it)
    }

    pub(super) fn orch_item_list(
        &self,
        p: OrchItemListParams,
    ) -> Result<OrchItemListResult, RpcError> {
        let o = self.orch();
        let run = self.caller_run(&o, p.run, &p.caller)?;
        let st = o.run(&run).map_err(oe)?;
        Ok(OrchItemListResult {
            items: st
                .rec
                .items
                .iter()
                .filter(|i| !p.ready || (i.state == ItemState::Ready && !i.circuit_open))
                .cloned()
                .collect(),
        })
    }

    pub(super) fn orch_item_update(&self, p: OrchItemUpdateParams) -> Result<Item, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchItemUpdate::NAME)? {
            return Ok(r);
        }
        who(&o, &p.caller)?;
        let it = if p.reopen {
            o.reopen_item(&p.item, crate::now_ms()).map_err(oe)?
        } else {
            o.item(&p.item).map_err(oe)?.clone()
        };
        remember(&mut o, p.request_id.as_deref(), OrchItemUpdate::NAME, &it)?;
        status::publish_orch_changed(&it.run, Some(&it.id), None);
        Ok(it)
    }
}
