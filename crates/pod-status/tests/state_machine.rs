//! Engine transitions: every arrow of docs/04 §4.2, priority, staleness, ack,
//! turn counter, sub-records and aggregation.

use base64::Engine as _;
use pod_status::{
    AgentKind, AgentState, BlockedKind, HookAgent, HookMessage, Outcome, ProcessInfo, QUERY_REPLY,
    Snapshot, Source, StatusConfig, TerminalStatus, aggregate, aggregate_snapshots,
};
use serde_json::{Value, json};

use AgentState::*;

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn osc(fields: &str) -> Vec<u8> {
    format!("\x1b]7501;{fields}\x07").into_bytes()
}

fn hook(agent: HookAgent, event: &str, payload: Value) -> HookMessage {
    HookMessage::new("t1", agent, event, payload)
}

fn claude(event: &str, payload: Value) -> HookMessage {
    hook(HookAgent::Claude, event, payload)
}

fn codex(event: &str, payload: Value) -> HookMessage {
    hook(HookAgent::Codex, event, payload)
}

fn claude_proc() -> ProcessInfo {
    ProcessInfo::new("claude.exe", &["claude.exe"])
}

fn codex_proc() -> ProcessInfo {
    ProcessInfo::new("codex.exe", &["codex.exe", "--no-daemon"])
}

const PERM: AgentState = Blocked(BlockedKind::Permission);
const QUESTION: AgentState = Blocked(BlockedKind::Question);

// ───────────── arrows of §4.2 ─────────────

#[test]
fn unknown_to_idle_by_osc() {
    let mut t = TerminalStatus::new(0);
    assert_eq!(t.state(), Unknown);
    let r = t.feed_output(&osc("state=idle:app=claude-code"), 10);
    let tr = r.transition.unwrap();
    assert_eq!(
        (tr.from, tr.to, tr.source),
        (Unknown, Idle, Some(Source::Osc))
    );
    let s = t.snapshot();
    assert_eq!(s.agent, Some(AgentKind::Claude));
    assert_eq!(s.app.as_deref(), Some("claude-code"));
    assert_eq!(s.since, 10);
    assert!(s.confidence > 0.8);
}

#[test]
fn unknown_to_idle_by_hook_session_start() {
    let mut t = TerminalStatus::new(0);
    let tr = t
        .on_hook(
            &claude(
                "SessionStart",
                json!({"session_id":"s1","source":"startup"}),
            ),
            5,
        )
        .unwrap();
    assert_eq!(
        (tr.from, tr.to, tr.source),
        (Unknown, Idle, Some(Source::Hook))
    );
    assert_eq!(t.snapshot().session_id.as_deref(), Some("s1"));
}

#[test]
fn unknown_to_working_by_heuristic_spinner() {
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&claude_proc()), 1);
    assert_eq!(t.snapshot().source, Some(Source::Process));
    let tr = t.on_title("\u{2802} Fix the bug", 2).unwrap();
    assert_eq!(
        (tr.from, tr.to, tr.source),
        (Unknown, Working, Some(Source::Heuristic))
    );
}

#[test]
fn idle_to_working_by_osc_and_hook() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=idle:app=claude-code"), 1);
    assert_eq!(
        t.feed_output(&osc("state=working:app=claude-code"), 2)
            .transition
            .unwrap()
            .to,
        Working
    );

    let mut t = TerminalStatus::new(0);
    t.on_hook(&codex("SessionStart", json!({})), 1);
    assert_eq!(
        t.on_hook(&codex("UserPromptSubmit", json!({"prompt":"hi"})), 2)
            .unwrap()
            .to,
        Working
    );
}

#[test]
fn working_to_blocked_kinds() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working:app=claude-code"), 1);
    t.feed_output(
        &osc(&format!(
            "state=blocked:app=claude-code:kind=permission:msg={}",
            b64("Allow rm?")
        )),
        2,
    );
    assert_eq!(t.state(), PERM);
    assert_eq!(t.snapshot().msg.as_deref(), Some("Allow rm?"));
    t.feed_output(&osc("state=blocked:kind=auth"), 3);
    assert_eq!(t.state(), Blocked(BlockedKind::Auth));
    t.feed_output(&osc("state=blocked"), 4);
    assert_eq!(t.state(), Blocked(BlockedKind::Other));

    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(&claude("PermissionRequest", json!({"tool_name":"Bash"})), 2);
    assert_eq!(t.state(), PERM);
    assert_eq!(t.snapshot().msg.as_deref(), Some("Bash"));
    assert_eq!(t.snapshot().tool_name.as_deref(), Some("Bash"));

    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(
        &claude("PreToolUse", json!({"tool_name":"AskUserQuestion"})),
        2,
    );
    assert_eq!(t.state(), QUESTION);

    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(
        &claude("Elicitation", json!({"message":"Pick a server"})),
        2,
    );
    assert_eq!(t.state(), QUESTION);
    assert_eq!(t.snapshot().msg.as_deref(), Some("Pick a server"));
}

#[test]
fn blocked_to_working_on_approve_or_answer() {
    // PostToolUse after a permission prompt
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(&claude("PermissionRequest", json!({"tool_name":"Bash"})), 2);
    let tr = t
        .on_hook(&claude("PostToolUse", json!({"tool_name":"Bash"})), 3)
        .unwrap();
    assert_eq!((tr.from, tr.to), (PERM, Working));
    // ElicitationResult restores the pre-wait state
    t.on_hook(&claude("Elicitation", json!({})), 4);
    assert_eq!(t.state(), QUESTION);
    assert_eq!(
        t.on_hook(&claude("ElicitationResult", json!({})), 5)
            .unwrap()
            .to,
        Working
    );
    // osc
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=blocked:kind=question"), 1);
    assert_eq!(
        t.feed_output(&osc("state=working"), 2)
            .transition
            .unwrap()
            .to,
        Working
    );
}

#[test]
fn blocked_to_idle_on_interrupt() {
    // osc: Claude reports idle after Esc (sample D, last line)
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=blocked:kind=question"), 1);
    assert_eq!(
        t.feed_output(&osc("state=idle"), 2).transition.unwrap().to,
        Idle
    );
    // Codex Interrupt hook
    let mut t = TerminalStatus::new(0);
    t.on_hook(&codex("UserPromptSubmit", json!({})), 1);
    t.on_hook(&codex("PermissionRequest", json!({"tool_name":"shell"})), 2);
    assert_eq!(t.state(), PERM);
    let tr = t.on_hook(&codex("Interrupt", json!({})), 3).unwrap();
    assert_eq!((tr.from, tr.to), (PERM, Idle));
    assert_eq!(t.snapshot().outcome, Some(Outcome::Cancelled));
}

#[test]
fn working_to_done_and_turn_counter() {
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    assert_eq!(t.turn(), 0);
    let tr = t.on_hook(&claude("Stop", json!({})), 2).unwrap();
    assert_eq!((tr.from, tr.to, tr.turn), (Working, Done, 1));
    assert_eq!(t.snapshot().outcome, Some(Outcome::Completed));

    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working"), 1);
    let tr = t.feed_output(&osc("state=done"), 2).transition.unwrap();
    assert_eq!((tr.to, tr.turn), (Done, 1));
}

#[test]
fn session_start_is_not_the_end_of_a_turn() {
    // docs/19 §17: Codex looked busy while loading MCP servers, then its
    // SessionStart hook said idle; that counted turn 1 before the prompt ran.
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working"), 1);
    assert_eq!(t.state(), Working);
    let tr = t
        .on_hook(&codex("SessionStart", json!({"session_id":"s1"})), 2)
        .unwrap();
    assert_eq!((tr.from, tr.to, tr.turn), (Working, Idle, 0));
    assert_eq!(t.turn(), 0);
    // The real turn still counts, once.
    t.on_hook(&codex("UserPromptSubmit", json!({})), 3);
    let tr = t.on_hook(&codex("Stop", json!({})), 4).unwrap();
    assert_eq!((tr.to, tr.turn), (Done, 1));
    // A boundary only spares the transition it caused.
    t.on_hook(&claude("SessionStart", json!({"source":"startup"})), 5);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 6);
    t.on_hook(&claude("Stop", json!({})), 7);
    assert_eq!(t.turn(), 2);
}

#[test]
fn done_to_idle_on_ack_and_done_to_working_on_new_prompt() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working"), 1);
    t.feed_output(&osc("state=done"), 2);
    // ack only applies to done
    let tr = t.ack(3).unwrap();
    assert_eq!((tr.from, tr.to), (Done, Idle));
    assert_eq!(t.ack(4), None);

    t.feed_output(&osc("state=working"), 5);
    t.feed_output(&osc("state=done"), 6);
    let tr = t.feed_output(&osc("state=working"), 7).transition.unwrap();
    assert_eq!((tr.from, tr.to), (Done, Working));
    assert_eq!(t.turn(), 2);

    // hook path: ack converts the hook's done as well
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(&claude("Stop", json!({})), 2);
    assert_eq!(t.ack(3).unwrap().to, Idle);
    assert_eq!(
        t.on_hook(&claude("UserPromptSubmit", json!({})), 4)
            .unwrap()
            .to,
        Working
    );
}

#[test]
fn working_to_error_and_back() {
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    let tr = t
        .on_hook(&claude("StopFailure", json!({"error":"rate_limit"})), 2)
        .unwrap();
    assert_eq!((tr.from, tr.to, tr.turn), (Working, Error, 1));
    let s = t.snapshot();
    assert_eq!(s.outcome, Some(Outcome::Failed));
    assert_eq!(s.msg.as_deref(), Some("rate_limit"));
    let tr = t
        .on_hook(&claude("UserPromptSubmit", json!({})), 3)
        .unwrap();
    assert_eq!((tr.from, tr.to), (Error, Working));
    assert_eq!(t.snapshot().outcome, None);

    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working"), 1);
    t.feed_output(&osc(&format!("state=error:msg={}", b64("boom"))), 2);
    assert_eq!(t.state(), Error);
    assert_eq!(t.snapshot().msg.as_deref(), Some("boom"));
    assert_eq!(
        t.feed_output(&osc("state=working"), 3)
            .transition
            .unwrap()
            .to,
        Working
    );
}

#[test]
fn working_interrupt_to_idle_cancelled() {
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    let tr = t
        .on_hook(&claude("Stop", json!({"is_interrupt": true})), 2)
        .unwrap();
    assert_eq!((tr.from, tr.to, tr.turn), (Working, Idle, 1));
    assert_eq!(t.snapshot().outcome, Some(Outcome::Cancelled));

    let mut t = TerminalStatus::new(0);
    t.on_hook(&codex("UserPromptSubmit", json!({})), 1);
    assert_eq!(
        t.on_hook(&codex("Interrupt", json!({})), 2).unwrap().to,
        Idle
    );
}

#[test]
fn any_state_to_exited_keeps_final_state_and_restart_resets() {
    for st in [
        "working",
        "blocked:kind=permission",
        "done",
        "idle",
        "error",
    ] {
        let mut t = TerminalStatus::new(0);
        t.on_foreground(Some(&claude_proc()), 0);
        t.feed_output(&osc(&format!("state={st}")), 1);
        let before = t.state();
        let tr = t.on_process_exit(2).unwrap();
        assert_eq!((tr.from, tr.to), (before, Exited));
        let s = t.snapshot();
        assert_eq!(s.state, Exited);
        assert_eq!(s.final_state, Some(before));
        assert_eq!(s.display_state(), Exited);
        // a second exit is a no-op
        assert_eq!(t.on_process_exit(3), None);
    }
    // foreground back to the shell = agent exited
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&claude_proc()), 0);
    t.feed_output(&osc("state=working"), 1);
    let tr = t
        .on_foreground(Some(&ProcessInfo::new("pwsh.exe", &["pwsh.exe"])), 2)
        .unwrap();
    assert_eq!(tr.to, Exited);
    assert_eq!(t.turn(), 1, "leaving working counts a turn");
    // restart: a new agent process resets the run (turn kept)
    t.on_foreground(Some(&claude_proc()), 3);
    assert_eq!(t.state(), Unknown);
    assert_eq!(t.snapshot().final_state, None);
    assert_eq!(t.turn(), 1);
    // OSC after exit (no foreground info) also starts a new run
    t.on_process_exit(4);
    assert_eq!(
        t.feed_output(&osc("state=idle"), 5).transition.unwrap().to,
        Idle
    );
}

#[test]
fn osc_clear_demotes_to_lower_source() {
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&claude_proc()), 0);
    t.on_title("\u{2733} Claude Code", 1);
    assert_eq!(
        (t.state(), t.snapshot().source),
        (Idle, Some(Source::Heuristic))
    );
    t.feed_output(&osc("state=working"), 2);
    assert_eq!(
        (t.state(), t.snapshot().source),
        (Working, Some(Source::Osc))
    );
    // the title still says idle; clearing OSC demotes back to it
    let tr = t
        .feed_output(b"\x1b]7501;state=clear\x07", 3)
        .transition
        .unwrap();
    assert_eq!((tr.to, tr.source), (Idle, Some(Source::Heuristic)));
    // without anything lower, clear goes to unknown (process source)
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working"), 1);
    assert_eq!(
        t.feed_output(&osc("state=clear"), 2).transition.unwrap().to,
        Unknown
    );
}

// ───────────── priority & staleness ─────────────

#[test]
fn hook_outranks_osc_while_fresh() {
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1_000);
    t.on_hook(
        &claude("PermissionRequest", json!({"tool_name":"Bash"})),
        2_000,
    );
    // OSC says working (e.g. delayed); hook (fresh) keeps blocked
    t.feed_output(&osc("state=working"), 3_000);
    assert_eq!((t.state(), t.snapshot().source), (PERM, Some(Source::Hook)));
}

#[test]
fn stale_hook_yields_to_newer_osc_only() {
    let cfg = StatusConfig {
        hook_stale_ms: 120_000,
        ..Default::default()
    };
    let mut t = TerminalStatus::with_config(cfg, 0);
    t.feed_output(&osc("state=idle"), 500); // older than the hook
    t.on_hook(
        &claude("PermissionRequest", json!({"tool_name":"Bash"})),
        1_000,
    );
    // stale, but OSC is older → hook state stays, lower confidence
    t.tick(1_000 + 120_000);
    let s = t.snapshot();
    assert_eq!((s.state, s.source), (PERM, Some(Source::Hook)));
    assert!(s.confidence < 0.6);
    // newer OSC after staleness wins
    let tr = t
        .feed_output(&osc("state=working"), 130_000)
        .transition
        .unwrap();
    assert_eq!((tr.to, tr.source), (Working, Some(Source::Osc)));
    // but a fresh hook takes over again
    t.on_hook(&claude("Stop", json!({})), 131_000);
    assert_eq!((t.state(), t.snapshot().source), (Done, Some(Source::Hook)));
}

/// docs/07 §7: Esc on a permission/question prompt sends no hook. The agent's
/// own OSC report moving to the end of a turn clears the hook `blocked` at
/// once instead of after `hook_stale_ms` (kept only as the fallback).
#[test]
fn hook_blocked_clears_on_osc_turn_end_after_interrupt() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working:app=claude-code"), 500);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1_000);
    t.on_hook(
        &claude("PermissionRequest", json!({"tool_name":"Bash"})),
        2_000,
    );
    // The OSC side reports the prompt too, then re-confirms it: still blocked.
    t.feed_output(&osc("state=blocked:kind=permission"), 2_100);
    t.feed_output(&osc("state=blocked:kind=permission"), 2_500);
    assert_eq!((t.state(), t.snapshot().source), (PERM, Some(Source::Hook)));
    // A lagging OSC `working` does not clear it either.
    t.feed_output(&osc("state=working"), 2_600);
    assert_eq!(t.state(), PERM);
    // Esc: no hook, the OSC report goes idle -> immediately idle (not 120 s).
    let tr = t.feed_output(&osc("state=idle"), 3_000).transition.unwrap();
    assert_eq!((tr.from, tr.to, tr.source), (PERM, Idle, Some(Source::Osc)));
    // A new prompt (fresh hook) blocks again and is not cleared by the old
    // idle being re-confirmed.
    t.on_hook(
        &claude("PermissionRequest", json!({"tool_name":"Edit"})),
        4_000,
    );
    assert_eq!(t.state(), PERM);
    t.feed_output(&osc("state=idle"), 4_100);
    assert_eq!((t.state(), t.snapshot().source), (PERM, Some(Source::Hook)));
    // Turn end reported as done/error clears it as well.
    t.feed_output(&osc("state=working"), 4_200);
    t.feed_output(&osc("state=done"), 4_300);
    assert_eq!((t.state(), t.snapshot().source), (Done, Some(Source::Osc)));

    // Codex's Interrupt hook already clears it (no OSC needed).
    let mut c = TerminalStatus::new(0);
    c.on_hook(
        &codex("PermissionRequest", json!({"tool_name":"shell"})),
        1_000,
    );
    assert_eq!(c.state(), PERM);
    c.on_hook(&codex("Interrupt", json!({})), 1_500);
    assert_eq!(c.state(), Idle);
    // Without any OSC/hook signal the 120 s rule is the fallback.
    let mut f = TerminalStatus::new(0);
    f.on_hook(
        &claude("PermissionRequest", json!({"tool_name":"Bash"})),
        1_000,
    );
    assert_eq!(f.tick(60_000), None);
    assert_eq!(f.state(), PERM);
}

#[test]
fn osc_never_goes_stale_and_outranks_heuristics() {
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&claude_proc()), 0);
    t.feed_output(&osc("state=working"), 1);
    t.on_title("\u{2733} Claude Code", 2); // glyph says idle
    assert_eq!(
        (t.state(), t.snapshot().source),
        (Working, Some(Source::Osc))
    );
    t.tick(10_000_000);
    assert_eq!(
        (t.state(), t.snapshot().source),
        (Working, Some(Source::Osc))
    );
}

#[test]
fn heuristic_idle_does_not_ack_stale_hook_done() {
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&codex_proc()), 0);
    t.on_hook(&codex("UserPromptSubmit", json!({})), 1_000);
    t.on_hook(&codex("Stop", json!({})), 2_000);
    // long after, the title shows no spinner and output is quiet → heuristic idle
    t.on_title("Reply ok | work", 200_000);
    t.tick(210_000);
    assert_eq!(t.state(), Done, "done → idle only through ack");
    assert_eq!(t.ack(211_000).unwrap().to, Idle);
}

#[test]
fn heuristic_working_overrides_stale_hook_done() {
    // A Codex turn without hooks firing (e.g. hooks not trusted yet) after an old Stop
    let mut t = TerminalStatus::new(0);
    t.on_foreground(Some(&codex_proc()), 0);
    t.on_hook(&codex("Stop", json!({})), 1_000);
    t.on_title("\u{280b} work", 200_000);
    assert_eq!(
        (t.state(), t.snapshot().source),
        (Working, Some(Source::Heuristic))
    );
}

// ───────────── sub-records ─────────────

#[test]
fn osc_sub_records_and_held_open_done() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=working:app=claude-code"), 1);
    t.feed_output(
        &osc(&format!(
            "state=working:id=task1:title={}",
            b64("child task")
        )),
        2,
    );
    t.feed_output(&osc("state=working:id=task2"), 2);
    // root unaffected by records
    assert_eq!(t.state(), Working);
    t.feed_output(&osc("state=done:app=claude-code"), 3);
    let s = t.snapshot();
    assert_eq!(s.state, Done);
    assert_eq!(s.running_children, 2);
    assert_eq!(s.records[0].title.as_deref(), Some("child task"));
    assert_eq!(s.badge(), "done (+2 running)");
    // child blocked → display blocked
    t.feed_output(&osc("state=blocked:id=task2:kind=question"), 4);
    let s = t.snapshot();
    assert_eq!(s.display_state(), QUESTION);
    assert_eq!(s.blocked_children, 1);
    // clear:id removes the record
    t.feed_output(&osc("state=clear:id=task2"), 5);
    t.feed_output(&osc("state=clear:id=task1"), 5);
    let s = t.snapshot();
    assert!(s.records.is_empty());
    assert_eq!(s.badge(), "done");
}

#[test]
fn hook_subagents() {
    let mut t = TerminalStatus::new(0);
    t.on_hook(&claude("UserPromptSubmit", json!({})), 1);
    t.on_hook(
        &claude(
            "SubagentStart",
            json!({"agent_id":"a1","agent_type":"Explore"}),
        ),
        2,
    );
    // subagent tool activity does not touch the lead
    t.on_hook(
        &claude("PreToolUse", json!({"agent_id":"a1","tool_name":"Grep"})),
        3,
    );
    t.on_hook(&claude("Stop", json!({})), 4);
    let s = t.snapshot();
    assert_eq!(s.state, Done);
    assert_eq!(s.running_children, 1);
    assert_eq!(s.records[0].title.as_deref(), Some("Explore"));
    assert_eq!(s.records[0].source, Source::Hook);
    t.on_hook(&claude("SubagentStop", json!({"agent_id":"a1"})), 5);
    assert_eq!(t.snapshot().running_children, 0);
    // SessionStart clears the hook roster
    t.on_hook(&claude("SubagentStart", json!({"agent_id":"a2"})), 6);
    t.on_hook(&claude("SessionStart", json!({"source":"clear"})), 7);
    assert!(t.snapshot().records.is_empty());
}

// ───────────── replays of docs/04 §1.3 ─────────────

#[test]
fn replay_sample_d_turns_like_hc() {
    let q = "answer: Would you like me to keep my replies short ... (Yes \u{b7} No)";
    let stream = [
        osc("state=idle:app=claude-code"),
        osc("state=working:app=claude-code"),
        osc("state=done:app=claude-code"),
        osc("state=working:app=claude-code"),
        osc(&format!(
            "state=blocked:app=claude-code:kind=question:msg={}",
            b64(q)
        )),
        osc("state=idle:app=claude-code"),
    ];
    let mut t = TerminalStatus::new(0);
    let mut seen = Vec::new();
    for (i, chunk) in stream.iter().enumerate() {
        if let Some(tr) = t.feed_output(chunk, i as u64).transition {
            seen.push((tr.to, tr.turn));
        }
    }
    assert_eq!(
        seen,
        vec![
            (Idle, 0),
            (Working, 0),
            (Done, 1),
            (Working, 1),
            (QUESTION, 2),
            (Idle, 2)
        ]
    );
}

#[test]
fn replay_sample_e_and_query_count() {
    let mut t = TerminalStatus::new(0);
    let r = t.feed_output(b"\x1b[>0q\x1b[?u\x1b]7501;?\x07", 0);
    assert_eq!(r.queries, 1);
    assert!(!r.changed);
    assert_eq!(QUERY_REPLY.len(), 80);
    t.feed_output(
        &osc(&format!(
            "state=working:app=podtest:progress=40:msg={}",
            b64("compiling")
        )),
        1,
    );
    let s = t.snapshot();
    assert_eq!(
        (s.state, s.progress, s.msg.as_deref()),
        (Working, Some(40), Some("compiling"))
    );
    assert_eq!(s.agent, Some(AgentKind::Other("podtest".into())));
    let tr = t
        .feed_output(
            &osc(&format!(
                "state=blocked:app=podtest:kind=permission:msg={}",
                b64("Allow rm?")
            )),
            2,
        )
        .transition
        .unwrap();
    assert_eq!((tr.to, tr.turn), (PERM, 1));
    let r = t.feed_output(
        &osc(&format!(
            "state=working:app=podtest:id=sub1:title={}",
            b64("child task")
        )),
        3,
    );
    assert!(
        r.changed && r.transition.is_none(),
        "records do not move the root"
    );
    assert_eq!(t.state(), PERM);
    t.feed_output(&osc("state=done:app=podtest"), 4);
    t.feed_output(
        &osc(&format!("state=error:app=podtest:msg={}", b64("boom"))),
        5,
    );
    assert_eq!(t.state(), Error);
    t.feed_output(&osc("state=idle:app=podtest"), 6);
    assert_eq!(t.state(), Idle);
    // invalid sequences are counted, not applied
    let r = t.feed_output(&osc("state=working:progress=999"), 7);
    assert_eq!((r.invalid, t.state()), (1, Idle));
}

// ───────────── aggregation ─────────────

#[test]
fn aggregate_priority() {
    assert_eq!(aggregate([Idle, Working, Done]), Done);
    assert_eq!(aggregate([Done, Error]), Error);
    assert_eq!(aggregate([Error, PERM, Working]), PERM);
    assert_eq!(aggregate([Unknown, Idle]), Idle);
    assert_eq!(aggregate([Unknown, Exited]), Unknown);
    assert_eq!(aggregate([Working, Idle]), Working);
    assert_eq!(aggregate(std::iter::empty()), Unknown);
    let order = [PERM, Error, Done, Working, Idle, Unknown, Exited];
    for w in order.windows(2) {
        assert!(w[0].priority() > w[1].priority(), "{:?} > {:?}", w[0], w[1]);
    }
}

#[test]
fn aggregate_snapshots_uses_display_state() {
    let mut a = TerminalStatus::new(0);
    a.feed_output(&osc("state=done"), 1);
    a.feed_output(&osc("state=blocked:id=x:kind=permission"), 1);
    let mut b = TerminalStatus::new(0);
    b.feed_output(&osc("state=error"), 1);
    let snaps: Vec<Snapshot> = vec![a.snapshot(), b.snapshot()];
    assert_eq!(aggregate_snapshots(&snaps), PERM);
}

#[test]
fn snapshot_serializes_to_json() {
    let mut t = TerminalStatus::new(0);
    t.feed_output(&osc("state=blocked:kind=question:app=claude-code"), 1);
    let v = serde_json::to_value(t.snapshot()).unwrap();
    assert_eq!(v["state"], json!({"state":"blocked","kind":"question"}));
    assert_eq!(v["source"], json!("osc"));
    assert_eq!(v["agent"], json!("claude"));
}

/// docs/12 B2: after the user approves a hook permission request, no hook
/// arrives until the approved tool finishes (PostToolUse), so the state
/// stayed `blocked:permission` while the command ran. An answer key typed
/// into the terminal, followed by output, now means `working` (source
/// `heuristic`) until the next hook.
#[test]
fn answered_permission_prompt_is_working_until_the_next_hook() {
    for agent in [HookAgent::Codex, HookAgent::Claude] {
        let mut st = TerminalStatus::new(0);
        st.on_hook(&hook(agent, "UserPromptSubmit", json!({})), 100);
        st.on_hook(
            &hook(agent, "PreToolUse", json!({"tool_name": "Bash"})),
            200,
        );
        st.on_hook(
            &hook(agent, "PermissionRequest", json!({"tool_name": "Bash"})),
            300,
        );
        assert_eq!(st.state(), PERM);
        // Navigating the prompt and query replies are not answers.
        st.on_input(b"\x1b[B", 1_000);
        st.on_input(b"\x1b]11;rgb:1c1c/1d1d/2121\x1b\\", 1_010);
        let _ = st.feed_output(b"redraw", 1_100);
        assert_eq!(st.state(), PERM, "{agent:?}: arrows do not answer");
        // Enter (Claude: "1", Codex: "y" would do the same).
        st.on_input(b"\r", 2_000);
        assert_eq!(st.state(), PERM, "nothing drawn yet");
        let t = st
            .feed_output(b"Running command...", 2_050)
            .transition
            .expect("transition");
        assert_eq!((t.from, t.to), (PERM, Working));
        let s = st.snapshot();
        assert_eq!(s.source, Some(Source::Heuristic));
        assert!(s.confidence < 0.9);
        // Output alone keeps it working; PostToolUse confirms via hook.
        let _ = st.feed_output(b"...", 30_000);
        assert_eq!(st.state(), Working);
        st.on_hook(
            &hook(agent, "PostToolUse", json!({"tool_name": "Bash"})),
            40_000,
        );
        assert_eq!(st.state(), Working);
        assert_eq!(st.snapshot().source, Some(Source::Hook));
        assert_eq!(st.answer_inferred_at(), None);
    }
}

/// Esc still cancels through the agent's own report (no false `working`);
/// an answer without any output afterwards is ignored; answers do nothing
/// outside a hook `blocked` state.
#[test]
fn esc_and_unanswered_prompts_keep_their_behaviour() {
    let mut st = TerminalStatus::new(0);
    st.on_hook(&codex("UserPromptSubmit", json!({})), 100);
    st.on_hook(
        &codex("PermissionRequest", json!({"tool_name": "shell"})),
        300,
    );
    st.on_input(b"\x1b", 1_000);
    let _ = st.feed_output(b"redraw", 1_050);
    assert_eq!(st.state(), PERM, "Esc is not an approval");
    st.on_hook(&codex("Interrupt", json!({})), 1_200);
    assert_eq!(st.state(), Idle);

    // Answer typed but the agent draws nothing for > 3 s: stays blocked.
    st.on_hook(&codex("UserPromptSubmit", json!({})), 2_000);
    st.on_hook(
        &codex("PermissionRequest", json!({"tool_name": "shell"})),
        2_100,
    );
    st.on_input(b"y", 3_000);
    let _ = st.feed_output(b"late", 6_500);
    assert_eq!(st.state(), PERM);

    // Typing while working is not an answer to anything.
    st.on_hook(&codex("PostToolUse", json!({})), 7_000);
    st.on_input(b"\r", 7_100);
    let _ = st.feed_output(b"x", 7_200);
    assert_eq!(st.snapshot().source, Some(Source::Hook));

    // A daemon restart replays the blocked hook, then the recorded answer.
    let mut st = TerminalStatus::new(0);
    st.on_hook(
        &codex("PermissionRequest", json!({"tool_name": "shell"})),
        300,
    );
    let t = st.restore_answer(400, 500).expect("transition");
    assert_eq!(t.to, Working);
    assert_eq!(st.snapshot().source, Some(Source::Heuristic));
}

#[test]
fn answer_keys() {
    use pod_status::is_answer_input as a;
    for yes in [
        &b"\r"[..],
        b"y",
        b"a",
        b"n",
        b"p",
        b"1",
        b"2",
        b"\x1b[B\r",
        b"y\r",
    ] {
        assert!(a(yes), "{yes:?}");
    }
    for no in [
        &b""[..],
        b"\x1b",
        b"\x1b[A",
        b"\x1bOB",
        b"\x1b[I",
        b"\x1b]10;rgb:d6d6/d8d8/dbdb\x1b\\",
        b"\x1b[200~hello\x1b[201~",
        b"x",
        b"hello world\r",
        b"\x03",
    ] {
        assert!(!a(no), "{no:?}");
    }
}
