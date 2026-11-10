//! Pressing Enter after typed or pasted text so that it is submitted, and
//! checking that the agent took it (docs/12 B1).
//!
//! Codex on Windows reads console key events (no bracketed paste: the
//! console host strips the paste markers), so it detects pastes by timing:
//! characters arriving within 8 ms of each other are a "paste burst", shown
//! only after 60 ms without characters, and an Enter while a burst is
//! active or within 120 ms of its last character inserts a newline instead
//! of submitting. "Within" is measured when Codex *processes* the key, so a
//! fixed pause after writing (30 ms for `terminal.send`, 250 ms for review
//! deliveries) is not enough when Codex lags behind a long paste.
//!
//! So Enter is written only once the program has taken the text in: it drew
//! something after the write and has then been quiet for [`QUIET_MS`] (a
//! burst is drawn only when it ends), bounded by [`max_settle_ms`]. Under CPU
//! load the program is descheduled for long stretches in the middle of a
//! burst: it draws what it has read so far, and the rest follows after a
//! pause longer than [`QUIET_MS`]. So the quiet period grows with the longest
//! pause seen since the write (write to first draw, and between draws:
//! [`quiet_needed_ms`]), and such a lagging program gets up to
//! [`MAX_SETTLE_MS`]. Then, for
//! terminals whose agent reports a status, the agent must start a turn
//! (working/blocked, or a completed turn) within [`CONFIRM_MS`]; if it does
//! not, Enter is pressed once more (an Enter that became a newline leaves
//! the text in the input box; a second one submits it, an extra one on an
//! empty box does nothing). The outcome is a [`SubmitCheck`].

use std::time::Duration;

use super::terminal::Terminal;
use crate::protocol::{StatusInfo, SubmitCheck, SubmitState};

/// Never press Enter sooner than this after the text was written.
pub const MIN_ENTER_DELAY_MS: u64 = 150;
/// The program's output must have been quiet this long (Codex: 60 ms flush +
/// 120 ms Enter window after the last character, with margin).
pub const QUIET_MS: u64 = 300;
/// Upper bound of any wait before Enter.
pub const MAX_SETTLE_MS: u64 = 5_000;
/// Programs that draw nothing for the text: Enter after this long anyway.
pub const NO_ECHO_MS: u64 = 1_000;
/// How long the agent has to react to an Enter before it is retried once.
pub const CONFIRM_MS: u64 = 4_000;
const POLL_MS: u64 = 25;

/// Upper bound of the wait before Enter (output that never settles, e.g. a
/// spinner): 1 s plus 0.5 ms per character, at most 5 s.
pub fn max_settle_ms(chars: usize) -> u64 {
    (1_000 + chars as u64 / 2).min(MAX_SETTLE_MS)
}

/// Quiet period before Enter when the program's output since the write
/// paused up to `max_gap_ms`: a program that pauses that long may still have
/// characters to read after a shorter pause.
pub fn quiet_needed_ms(max_gap_ms: u64) -> u64 {
    QUIET_MS.max(max_gap_ms)
}

/// What has been seen of the program's output since the text was written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Echo {
    /// Time of the latest output (any time, also before the write).
    pub last_output_ms: u64,
    /// Longest pause since the write: from the write to the first output
    /// after it, and between outputs after it.
    pub max_gap_ms: u64,
}

impl Echo {
    /// Record the output time read at a poll. `written_ms` 0: no text was
    /// written (a bare Enter), so there is no pause to measure.
    pub fn observe(&mut self, written_ms: u64, last_output_ms: u64) {
        if last_output_ms > self.last_output_ms {
            if written_ms > 0 && last_output_ms >= written_ms {
                let from = self.last_output_ms.max(written_ms);
                self.max_gap_ms = self.max_gap_ms.max(last_output_ms - from);
            }
            self.last_output_ms = last_output_ms;
        }
    }

    /// The program paused longer than the quiet period: it lags behind its
    /// input (CPU load), so it gets the longer bound.
    pub fn lagging(&self) -> bool {
        self.max_gap_ms > QUIET_MS
    }
}

/// Whether Enter can be pressed now for text written at `written_ms`.
pub fn settled(now: u64, written_ms: u64, echo: Echo, chars: usize) -> bool {
    let elapsed = now.saturating_sub(written_ms);
    let bound = if echo.lagging() {
        MAX_SETTLE_MS
    } else {
        max_settle_ms(chars)
    };
    if elapsed >= bound {
        return true;
    }
    let echoed = echo.last_output_ms >= written_ms;
    let quiet = now.saturating_sub(echo.last_output_ms.max(written_ms));
    elapsed >= MIN_ENTER_DELAY_MS
        && quiet >= quiet_needed_ms(echo.max_gap_ms)
        && (echoed || elapsed >= NO_ECHO_MS)
}

/// Status before Enter, to recognise the agent's reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Baseline {
    pub turn: u64,
    /// An agent reports a status here and is not busy: a reaction is visible.
    pub verifiable: bool,
}

pub fn baseline(s: &StatusInfo) -> Baseline {
    let reports = s.agent.is_some() || matches!(s.source.as_deref(), Some("hook" | "osc"));
    let busy = matches!(s.state.as_str(), "working" | "blocked" | "exited");
    Baseline {
        turn: s.turn,
        verifiable: reports && !busy,
    }
}

/// The agent's reaction to an Enter pressed at `entered_ms`, if any.
pub fn reaction(b: Baseline, s: &StatusInfo, entered_ms: u64) -> Option<&'static str> {
    if s.turn > b.turn {
        return Some("turn");
    }
    if s.since + 50 < entered_ms {
        return None; // the state is older than the Enter
    }
    match s.state.as_str() {
        "working" => Some("working"),
        "blocked" => Some("blocked"),
        _ => None,
    }
}

async fn wait_reaction(t: &Terminal, b: Baseline, entered_ms: u64) -> Option<&'static str> {
    let deadline = entered_ms + CONFIRM_MS;
    loop {
        if let Some(r) = reaction(b, &t.status.info(), entered_ms) {
            return Some(r);
        }
        if crate::now_ms() >= deadline || !t.is_running() {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

/// Wait until text written at `written_ms` has been taken in.
pub async fn settle(t: &Terminal, written_ms: u64, chars: usize) {
    let mut echo = Echo::default();
    loop {
        echo.observe(written_ms, t.last_output_ms());
        let now = crate::now_ms();
        if settled(now, written_ms, echo, chars) {
            if echo.lagging() {
                super::log(&format!(
                    "{}: lagging program (output paused up to {} ms): Enter {} ms after the text",
                    t.id,
                    echo.max_gap_ms,
                    now.saturating_sub(written_ms)
                ));
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

/// Press Enter for text written at `written_ms` and (with `confirm`) check
/// that the agent took it, pressing Enter a second time if not.
/// `on_pending` is called right after the first Enter of a check that will
/// wait for confirmation. `None`: the holder is gone.
pub async fn press_enter(
    t: &Terminal,
    written_ms: u64,
    chars: usize,
    confirm: bool,
    mut on_pending: impl FnMut(&SubmitCheck),
) -> Option<SubmitCheck> {
    settle(t, written_ms, chars).await;
    let b = baseline(&t.status.info());
    if !t.input(b"\r".to_vec()) {
        return None;
    }
    let entered = crate::now_ms();
    let mut check = SubmitCheck {
        state: SubmitState::Unverifiable,
        enters: 1,
        signal: None,
        entered_ms: Some(entered),
        settled_ms: None,
    };
    if !confirm || !b.verifiable {
        return Some(check);
    }
    check.state = SubmitState::Pending;
    on_pending(&check);
    let mut signal = wait_reaction(t, b, entered).await;
    if signal.is_none() && t.is_running() {
        // Probably a newline in the input box: press Enter again once, out of
        // any paste window (the output has been quiet since).
        settle(t, crate::now_ms(), 0).await;
        if !t.input(b"\r".to_vec()) {
            return None;
        }
        check.enters = 2;
        super::log(&format!(
            "{}: no reaction {CONFIRM_MS} ms after Enter; pressed Enter again",
            t.id
        ));
        signal = wait_reaction(t, b, crate::now_ms()).await;
    }
    check.state = if signal.is_some() {
        SubmitState::Confirmed
    } else {
        SubmitState::Unconfirmed
    };
    check.signal = signal.map(str::to_string);
    check.settled_ms = Some(crate::now_ms());
    Some(check)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(
        state: &str,
        source: Option<&str>,
        agent: Option<&str>,
        turn: u64,
        since: u64,
    ) -> StatusInfo {
        serde_json::from_value(serde_json::json!({
            "state": state, "source": source, "agent": agent, "confidence": 0.9,
            "since": since, "turn": turn, "badge": state,
        }))
        .unwrap()
    }

    /// Output seen at these times (polled), for text written at `w`.
    fn drawn_at(w: u64, times: &[u64]) -> Echo {
        let mut e = Echo::default();
        for &at in times {
            e.observe(w, at);
        }
        e
    }

    fn drawn(at: u64) -> Echo {
        Echo {
            last_output_ms: at,
            max_gap_ms: 0,
        }
    }

    /// Enter waits for the echo and then for quiet output; never sooner
    /// than the minimum, never later than the bound.
    #[test]
    fn settle_rules() {
        let w = 10_000;
        // Nothing drawn yet: wait (up to NO_ECHO_MS).
        assert!(!settled(w + 400, w, drawn(0), 100));
        assert!(settled(w + NO_ECHO_MS, w, drawn(0), 100));
        // Drawn at +100 (a burst shown when it ended): quiet 300 ms later.
        let e = drawn_at(w, &[w + 100]);
        assert_eq!(e.max_gap_ms, 100);
        assert!(!settled(w + 399, w, e, 100));
        assert!(settled(w + 400, w, e, 100));
        // A lagging paste: drawn only at +250 -> Enter at +550, not +300.
        let e = drawn_at(w, &[w + 250]);
        assert!(!settled(w + 549, w, e, 2_000));
        assert!(settled(w + 550, w, e, 2_000));
        // Output that never settles: the bound.
        assert!(!settled(w + 1_999, w, drawn(w + 1_990), 2_000));
        assert!(settled(w + 2_000, w, drawn(w + 1_990), 2_000));
        assert_eq!(max_settle_ms(1_000_000), MAX_SETTLE_MS);
        // Minimum delay even for an instant echo.
        assert!(!settled(w + 100, w, drawn(w - 5_000), 10));
    }

    /// Under CPU load the program is descheduled in the middle of a burst
    /// and draws it in pieces: the quiet period grows with the longest pause,
    /// and the bound with it.
    #[test]
    fn a_program_that_pauses_gets_a_longer_quiet_period() {
        let w = 10_000;
        // Output from before the text is no pause; the same output polled
        // twice neither.
        let e = drawn_at(w, &[w - 3_000, w + 200, w + 300, w + 750, w + 900, w + 900]);
        assert_eq!((e.last_output_ms, e.max_gap_ms), (w + 900, 450));
        assert!(e.lagging());
        assert_eq!(quiet_needed_ms(e.max_gap_ms), 450);
        // 300 ms of quiet is not enough after a pause of 450 ms ...
        assert!(!settled(w + 1_200, w, e, 1_600));
        assert!(!settled(w + 1_349, w, e, 1_600));
        assert!(settled(w + 1_350, w, e, 1_600));
        // ... nor after a first draw 800 ms after the write.
        let e = drawn_at(w, &[w + 800]);
        assert!(!settled(w + 1_100, w, e, 600));
        assert!(settled(w + 1_600, w, e, 600));
        // The bound (1.8 s for 1600 chars) becomes MAX_SETTLE_MS.
        let busy = drawn_at(w, &(1..=9).map(|i| w + i * 530).collect::<Vec<_>>());
        assert!(!settled(w + 3_000, w, busy, 1_600));
        assert!(!settled(w + MAX_SETTLE_MS - 1, w, busy, 1_600));
        assert!(settled(w + MAX_SETTLE_MS, w, busy, 1_600));
        // A bare Enter (no text written): output is never a pause.
        let bare = drawn_at(0, &[w - 3_000, w, w + 500]);
        assert_eq!(bare.max_gap_ms, 0);
        assert!(settled(w + 800, 0, bare, 0));
        // A spinner (draws ~100 ms apart) keeps the normal rules.
        let spin = drawn_at(w, &(1..=18).map(|i| w + i * 100).collect::<Vec<_>>());
        assert!(!spin.lagging());
        assert_eq!(quiet_needed_ms(spin.max_gap_ms), QUIET_MS);
        assert!(!settled(w + max_settle_ms(1_600) - 1, w, spin, 1_600));
        assert!(settled(w + max_settle_ms(1_600), w, spin, 1_600));
    }

    #[test]
    fn baseline_and_reaction() {
        // A shell: nothing to confirm.
        assert!(!baseline(&st("unknown", None, None, 0, 0)).verifiable);
        // An agent already working: an Enter queues a message, no reaction.
        assert!(!baseline(&st("working", Some("hook"), Some("codex"), 2, 0)).verifiable);
        let b = baseline(&st("idle", Some("hook"), Some("codex"), 2, 1_000));
        assert!(b.verifiable);
        let entered = 5_000;
        assert_eq!(
            reaction(
                b,
                &st("idle", Some("hook"), Some("codex"), 2, 1_000),
                entered
            ),
            None
        );
        assert_eq!(
            reaction(
                b,
                &st("working", Some("hook"), Some("codex"), 2, 5_200),
                entered
            ),
            Some("working")
        );
        assert_eq!(
            reaction(
                b,
                &st("blocked", Some("hook"), Some("codex"), 2, 5_200),
                entered
            ),
            Some("blocked")
        );
        // A whole (short) turn already ended.
        assert_eq!(
            reaction(
                b,
                &st("done", Some("hook"), Some("codex"), 3, 5_900),
                entered
            ),
            Some("turn")
        );
        // A stale working state from before the Enter does not count.
        assert_eq!(
            reaction(b, &st("working", Some("osc"), None, 2, 4_000), entered),
            None
        );
        // Heuristic-only Codex.
        assert!(baseline(&st("idle", Some("heuristic"), Some("codex"), 0, 0)).verifiable);
    }
}
