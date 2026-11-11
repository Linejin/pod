//! Evidence slots and how one of them wins: source priority (hook > OSC >
//! mod > heuristic), hook staleness (and its early end when the agent's own
//! OSC report ends the turn), and `done` held until [`TerminalStatus::ack`].

use super::TerminalStatus;
use crate::types::{AgentState, Millis, Source};

pub const CONF_HOOK: f32 = 0.95;
pub const CONF_OSC: f32 = 0.9;
pub const CONF_PROCESS: f32 = 0.2;
/// A Pod extension's status detector or hint (docs/16 §7.1).
pub const CONF_MOD: f32 = 0.75;
/// `working` inferred from the user answering a hook permission/question
/// prompt (no hook reports the answer; docs/12 B2). Reported as `heuristic`.
pub const CONF_ANSWERED: f32 = 0.6;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Evidence {
    pub(super) state: AgentState,
    /// Last time this source confirmed or changed its state.
    pub(super) at: Millis,
    pub(super) confidence: f32,
    pub(super) msg: Option<String>,
    pub(super) progress: Option<u8>,
    pub(super) title: Option<String>,
}

impl TerminalStatus {
    pub(super) fn chosen_evidence(&self) -> Option<&Evidence> {
        match self.source {
            Some(Source::Hook) => self.hook.as_ref(),
            Some(Source::Osc) => self.osc.as_ref(),
            Some(Source::Mod) => self.modev.as_ref(),
            _ => None,
        }
    }

    /// How long hook evidence beats lower sources. Normally `hook_stale_ms`,
    /// but a hook `blocked` (permission/question) is over at once when the
    /// agent's own OSC report later *changes* to the end of a turn
    /// (idle/done/error): the user answered with Esc/interrupt, which sends no
    /// hook (docs/07 §7). `hook_stale_ms` stays the fallback for agents
    /// without OSC reports. A later OSC `working` does not clear it (the
    /// prompt may still be up while the report lags).
    fn hook_stale_after(&self) -> Millis {
        let superseded = match (&self.hook, &self.osc) {
            (Some(h), Some(o)) => {
                h.state.is_blocked()
                    && self.osc_changed_at > self.hook_set_at
                    && matches!(
                        o.state,
                        AgentState::Idle | AgentState::Done | AgentState::Error
                    )
            }
            _ => false,
        };
        if superseded {
            0
        } else {
            self.cfg.hook_stale_ms
        }
    }

    /// Choose the effective state from the evidence slots.
    pub(super) fn resolve(&self, now: Millis) -> (AgentState, Option<Source>, f32) {
        if self.exited.is_some() {
            return (AgentState::Exited, Some(Source::Process), 1.0);
        }
        let heur = self.heur.verdict().map(|v| Evidence {
            state: v.state,
            at: v.at,
            confidence: v.confidence,
            msg: v.msg,
            progress: None,
            title: None,
        });
        let slots: [(Source, Option<&Evidence>, Millis); 4] = [
            (Source::Hook, self.hook.as_ref(), self.hook_stale_after()),
            (Source::Osc, self.osc.as_ref(), Millis::MAX), // fresh until process exit
            (Source::Mod, self.modev.as_ref(), Millis::MAX), // until cleared or exit
            (Source::Heuristic, heur.as_ref(), Millis::MAX),
        ];
        let mut stale_best: Option<(Source, &Evidence)> = None;
        let mut chosen: Option<(Source, &Evidence, f32)> = None;
        for (src, ev, stale_after) in slots {
            let Some(ev) = ev else { continue };
            if let Some((_, sb)) = stale_best {
                // A stale higher source only yields to newer lower evidence.
                if ev.at <= sb.at {
                    continue;
                }
            }
            if now.saturating_sub(ev.at) < stale_after {
                chosen = Some((src, ev, ev.confidence));
                break;
            }
            if stale_best.is_none() {
                stale_best = Some((src, ev));
            }
        }
        let (src, ev, conf) = match (chosen, stale_best) {
            (Some(c), _) => c,
            (None, Some((s, e))) => (s, e, e.confidence * 0.5),
            (None, None) => {
                let src = self.agent_from_process.then_some(Source::Process);
                let conf = if src.is_some() { CONF_PROCESS } else { 0.0 };
                return (AgentState::Unknown, src, conf);
            }
        };
        // A lower source saying "idle" must not silently acknowledge a "done"
        // from a stale higher source: done → idle only via ack().
        if ev.state == AgentState::Idle
            && let Some((s, e)) = stale_best
            && s > src
            && e.state == AgentState::Done
        {
            return (AgentState::Done, Some(s), e.confidence * 0.5);
        }
        (ev.state, Some(src), conf)
    }
}
