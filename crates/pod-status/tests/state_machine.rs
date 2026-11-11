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

/// Every public `engine::` path and tunable (refactor D-P7 keeps them on the
/// facade).
#[test]
fn engine_public_paths_and_tunables() {
    use pod_status::engine::{
        ANSWER_OUTPUT_WINDOW_MS, CONF_ANSWERED, CONF_HOOK, CONF_MOD, CONF_OSC, CONF_PROCESS,
        OutputResult, Snapshot, StatusConfig as Cfg, SubRecord, TerminalStatus as Engine,
        Transition, aggregate_snapshots, is_answer_input,
    };
    assert_eq!(
        [CONF_HOOK, CONF_OSC, CONF_MOD, CONF_ANSWERED, CONF_PROCESS],
        [0.95, 0.9, 0.75, 0.6, 0.2]
    );
    assert_eq!(ANSWER_OUTPUT_WINDOW_MS, 3_000);
    assert_eq!(
        Cfg::default(),
        Cfg {
            hook_stale_ms: 120_000,
            quiet_idle_ms: 3_000
        }
    );
    let _: fn(&[u8]) -> bool = is_answer_input;
    let _: Option<(SubRecord, Transition)> = None;
    assert_eq!(OutputResult::default().queries, 0);
    let s: Snapshot = Engine::new(0).snapshot();
    assert_eq!(aggregate_snapshots([&s]), Unknown);
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

// ───────────── pinned input traces (refactor D-P7) ─────────────
//
// Each trace drives one engine through a fixed list of timed inputs and
// records, per step, the call's result (serialized `Transition`, or the
// `OutputResult` fields) and the snapshot keys whose serialized value
// changed. The golden lines were taken from the engine before it was split
// into `engine/{evidence,input}.rs`; any difference is a behaviour change.

enum Step {
    Hook(HookAgent, &'static str, Value),
    Out(Vec<u8>),
    Input(&'static [u8]),
    Title(&'static str),
    Screen(&'static str),
    Fg(Option<ProcessInfo>),
    Exit,
    Mod(Option<AgentState>, Option<&'static str>),
    Ack,
    Tick,
    SetAgent(AgentKind),
    RestoreTurn(u64),
    RestoreAnswer(u64),
    ResetRun,
}

fn label(step: &Step) -> String {
    match step {
        Step::Hook(a, ev, p) => format!("hook {}/{ev} {p}", a.as_str()),
        Step::Out(b) => format!("out {:?}", String::from_utf8_lossy(b)),
        Step::Input(b) => format!("input {:?}", String::from_utf8_lossy(b)),
        Step::Title(t) => format!("title {t:?}"),
        Step::Screen(s) => format!("screen {s:?}"),
        Step::Fg(p) => format!("fg {:?}", p.as_ref().map(|p| p.name.clone())),
        Step::Exit => "exit".into(),
        Step::Mod(s, m) => format!("mod {} {m:?}", serde_json::to_string(s).unwrap()),
        Step::Ack => "ack".into(),
        Step::Tick => "tick".into(),
        Step::SetAgent(a) => format!("set_agent {}", serde_json::to_string(a).unwrap()),
        Step::RestoreTurn(n) => format!("restore_turn {n}"),
        Step::RestoreAnswer(at) => format!("restore_answer {at}"),
        Step::ResetRun => "reset_run".into(),
    }
}

fn tr_json(tr: Option<pod_status::Transition>) -> String {
    serde_json::to_string(&tr).unwrap()
}

/// Snapshot as a JSON object plus the derived values a UI reads.
fn view(t: &TerminalStatus) -> serde_json::Map<String, Value> {
    let s = t.snapshot();
    let Value::Object(mut m) = serde_json::to_value(&s).unwrap() else {
        unreachable!()
    };
    m.insert("badge".into(), json!(s.badge()));
    m.insert(
        "display_state".into(),
        serde_json::to_value(s.display_state()).unwrap(),
    );
    m.insert("answer_inferred_at".into(), json!(t.answer_inferred_at()));
    m
}

fn run_trace(steps: Vec<(u64, Step)>) -> Vec<String> {
    let mut t = TerminalStatus::new(0);
    let mut prev = view(&t);
    let mut lines = vec![format!("start {}", Value::Object(prev.clone()))];
    for (now, step) in steps {
        let name = label(&step);
        let res = match step {
            Step::Hook(agent, ev, p) => tr_json(t.on_hook(&hook(agent, ev, p), now)),
            Step::Out(bytes) => {
                let r = t.feed_output(&bytes, now);
                format!(
                    "q={} inv={} changed={} tr={}",
                    r.queries,
                    r.invalid,
                    r.changed,
                    tr_json(r.transition)
                )
            }
            Step::Input(bytes) => {
                t.on_input(bytes, now);
                "()".into()
            }
            Step::Title(s) => tr_json(t.on_title(s, now)),
            Step::Screen(s) => tr_json(t.on_screen(s, now)),
            Step::Fg(p) => tr_json(t.on_foreground(p.as_ref(), now)),
            Step::Exit => tr_json(t.on_process_exit(now)),
            Step::Mod(s, m) => tr_json(t.on_mod(s, m.map(str::to_string), now)),
            Step::Ack => tr_json(t.ack(now)),
            Step::Tick => tr_json(t.tick(now)),
            Step::SetAgent(a) => tr_json(t.set_agent(a, now)),
            Step::RestoreTurn(n) => {
                t.restore_turn(n);
                "()".into()
            }
            Step::RestoreAnswer(at) => tr_json(t.restore_answer(at, now)),
            Step::ResetRun => {
                t.reset_run(now);
                "()".into()
            }
        };
        let cur = view(&t);
        let delta: serde_json::Map<String, Value> = cur
            .iter()
            .filter(|(k, v)| prev.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        lines.push(format!("{now} {name} => {res} | {}", Value::Object(delta)));
        prev = cur;
    }
    lines
}

fn assert_trace(name: &str, actual: Vec<String>, golden: &str) {
    let expected: Vec<&str> = golden
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if actual != expected {
        let first = actual
            .iter()
            .zip(&expected)
            .position(|(a, e)| a != e)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "trace {name} differs at line {first}\n expected: {:?}\n   actual: {:?}\n\nactual trace:\n{}",
            expected.get(first),
            actual.get(first),
            actual.join("\n")
        );
    }
}

fn claude_hook_trace() -> Vec<(u64, Step)> {
    use HookAgent::Claude as C;
    vec![
        (0, Step::Fg(Some(claude_proc()))),
        (
            10,
            Step::Hook(
                C,
                "SessionStart",
                json!({"session_id":"s1","source":"startup"}),
            ),
        ),
        (20, Step::Title("\u{2733} Claude Code")),
        (
            30,
            Step::Hook(
                C,
                "UserPromptSubmit",
                json!({"session_id":"s1","prompt":"fix it"}),
            ),
        ),
        (40, Step::Out(b"thinking".to_vec())),
        (
            50,
            Step::Hook(
                C,
                "PreToolUse",
                json!({"session_id":"s1","tool_name":"Bash","tool_use_id":"tu1"}),
            ),
        ),
        (
            60,
            Step::Hook(
                C,
                "SubagentStart",
                json!({"session_id":"s1","agent_id":"a 1","agent_type":"Explore"}),
            ),
        ),
        (
            70,
            Step::Hook(
                C,
                "PreToolUse",
                json!({"session_id":"s1","agent_id":"a 1","tool_name":"Read"}),
            ),
        ),
        (
            80,
            Step::Hook(
                C,
                "PermissionRequest",
                json!({"session_id":"s1","tool_name":"Bash"}),
            ),
        ),
        (90, Step::Input(b"\x1b[I")),
        (100, Step::Out(b"x".to_vec())),
        (110, Step::Input(b"1")),
        (120, Step::Out(b"running".to_vec())),
        (
            130,
            Step::Hook(
                C,
                "PostToolUse",
                json!({"session_id":"s1","tool_name":"Bash"}),
            ),
        ),
        (
            140,
            Step::Hook(
                C,
                "PreToolUse",
                json!({"session_id":"s1","tool_name":"AskUserQuestion"}),
            ),
        ),
        (150, Step::Input(b"\x1b")),
        (160, Step::Input(b"2")),
        (4000, Step::Out(b"late".to_vec())),
        (
            4010,
            Step::Hook(
                C,
                "Elicitation",
                json!({"session_id":"s1","message":"Pick a server"}),
            ),
        ),
        (
            4020,
            Step::Hook(C, "ElicitationResult", json!({"session_id":"s1"})),
        ),
        (
            4030,
            Step::Hook(
                C,
                "SubagentStop",
                json!({"session_id":"s1","agent_id":"a 1"}),
            ),
        ),
        (4040, Step::Hook(C, "Stop", json!({"session_id":"s1"}))),
        (4050, Step::Ack),
        (
            4060,
            Step::Hook(C, "UserPromptSubmit", json!({"session_id":"s1"})),
        ),
        (
            4070,
            Step::Hook(
                C,
                "StopFailure",
                json!({"session_id":"s1","error":"overloaded"}),
            ),
        ),
        (
            4080,
            Step::Hook(C, "UserPromptSubmit", json!({"session_id":"s1"})),
        ),
        (
            4090,
            Step::Hook(C, "Stop", json!({"session_id":"s1","is_interrupt":true})),
        ),
        (
            4100,
            Step::Hook(
                C,
                "SessionStart",
                json!({"session_id":"s1","source":"compact"}),
            ),
        ),
        (4110, Step::Title("\u{2802} Working")),
        (130_000, Step::Tick),
        (
            130_010,
            Step::Hook(C, "Notification", json!({"session_id":"s1"})),
        ),
        (
            130_020,
            Step::Hook(C, "SessionEnd", json!({"session_id":"s1","reason":"exit"})),
        ),
        (130_030, Step::Exit),
        (
            130_040,
            Step::Hook(
                C,
                "SessionStart",
                json!({"session_id":"s2","source":"startup"}),
            ),
        ),
        (130_050, Step::RestoreTurn(7)),
        (
            130_060,
            Step::Hook(C, "UserPromptSubmit", json!({"session_id":"s2"})),
        ),
        (
            130_070,
            Step::Hook(
                C,
                "PermissionRequest",
                json!({"session_id":"s2","tool_name":"Write"}),
            ),
        ),
        (130_080, Step::RestoreAnswer(130_075)),
        (130_090, Step::Hook(C, "Stop", json!({"session_id":"s2"}))),
        (130_100, Step::Title("\u{2733} Claude Code")),
        (260_000, Step::Tick),
        (260_010, Step::Ack),
    ]
}

fn codex_hook_trace() -> Vec<(u64, Step)> {
    use HookAgent::Codex as X;
    vec![
        (0, Step::Fg(Some(codex_proc()))),
        (100, Step::Title("\u{280b} codex")),
        (
            200,
            Step::Hook(X, "SessionStart", json!({"session_id":"c1"})),
        ),
        (
            300,
            Step::Hook(
                X,
                "UserPromptSubmit",
                json!({"session_id":"c1","turn_id":"t1"}),
            ),
        ),
        (
            400,
            Step::Hook(
                X,
                "PreToolUse",
                json!({"session_id":"c1","tool_name":"shell"}),
            ),
        ),
        (
            500,
            Step::Hook(
                X,
                "PermissionRequest",
                json!({"session_id":"c1","tool_name":"shell"}),
            ),
        ),
        (
            600,
            Step::Screen("Would you like to run the following command?\n  cargo test"),
        ),
        (700, Step::Input(b"y")),
        (800, Step::Out(b"ok".to_vec())),
        (900, Step::Screen("")),
        (
            1000,
            Step::Hook(
                X,
                "PreToolUse",
                json!({"session_id":"c1","tool_name":"request_user_input"}),
            ),
        ),
        (1100, Step::Hook(X, "Interrupt", json!({"session_id":"c1"}))),
        (
            1200,
            Step::Hook(
                X,
                "UserPromptSubmit",
                json!({"session_id":"c1","turn_id":"t2"}),
            ),
        ),
        (
            1300,
            Step::Hook(
                X,
                "SubagentStart",
                json!({"session_id":"c1","agent_id":"w1","agent_type":"worker"}),
            ),
        ),
        (
            1350,
            Step::Hook(
                X,
                "PostToolUse",
                json!({"session_id":"c1","tool_name":"shell"}),
            ),
        ),
        (
            1400,
            Step::Hook(
                X,
                "SubagentStop",
                json!({"session_id":"c1","agent_id":"w1"}),
            ),
        ),
        (1500, Step::Hook(X, "Stop", json!({"session_id":"c1"}))),
        (1600, Step::Title("codex")),
        (5000, Step::Tick),
        (200_000, Step::Tick),
        (200_100, Step::Ack),
        (200_200, Step::Out(osc("state=working:app=codex"))),
        (200_300, Step::Fg(None)),
        (200_400, Step::Fg(Some(codex_proc()))),
        (200_500, Step::Tick),
        (204_000, Step::Tick),
    ]
}

fn osc_mod_trace() -> Vec<(u64, Step)> {
    let working = format!(
        "state=working:app=claude-code:progress=10:msg={}:title={}",
        b64("compiling"),
        b64("Build")
    );
    let split = osc("state=done:app=claude-code");
    let (head, tail) = split.split_at(9);
    vec![
        (0, Step::Out(b"\x1b[>0q\x1b]7501;?\x07".to_vec())),
        (10, Step::Out(osc("state=idle:app=claude-code"))),
        (20, Step::Out(osc(&working))),
        (30, Step::Out(head.to_vec())),
        (31, Step::Out(tail.to_vec())),
        (35, Step::Out(osc("state=working:app=claude-code"))),
        (
            40,
            Step::Out(osc(&format!(
                "state=working:id=sub1:title={}",
                b64("child")
            ))),
        ),
        (50, Step::Out(osc("state=blocked:id=sub1:kind=question"))),
        (60, Step::Mod(Some(Done), Some("mod says done"))),
        (
            70,
            Step::Hook(
                HookAgent::Claude,
                "PermissionRequest",
                json!({"tool_name":"Bash"}),
            ),
        ),
        (80, Step::Out(osc("state=working"))),
        (90, Step::Out(osc("state=idle"))),
        (100, Step::Out(osc("state=clear:id=sub1"))),
        (110, Step::Out(osc("state=clear"))),
        (115, Step::Hook(HookAgent::Claude, "SessionEnd", json!({}))),
        (120, Step::Ack),
        (130, Step::Mod(None, None)),
        (140, Step::Out(osc("state=done:app=podtest"))),
        (150, Step::Out(osc("state=working:progress=999"))),
        (160, Step::SetAgent(AgentKind::Codex)),
        (
            170,
            Step::Out(osc(&format!("state=error:msg={}", b64("boom")))),
        ),
        (180, Step::Exit),
        (190, Step::Out(osc("state=idle:app=codex"))),
        (200, Step::Title("\u{2802} x")),
        (210, Step::Mod(Some(PERM), None)),
        (220, Step::ResetRun),
    ]
}

#[test]
fn pinned_trace_claude_hooks() {
    assert_trace("claude", run_trace(claude_hook_trace()), CLAUDE_TRACE);
}

#[test]
fn pinned_trace_codex_hooks_and_screen() {
    assert_trace("codex", run_trace(codex_hook_trace()), CODEX_TRACE);
}

#[test]
fn pinned_trace_osc_and_mod() {
    assert_trace("osc", run_trace(osc_mod_trace()), OSC_TRACE);
}

const CLAUDE_TRACE: &str = r##"
start {"state":{"state":"unknown"},"final_state":null,"source":null,"confidence":0.0,"since":0,"turn":0,"agent":null,"app":null,"msg":null,"title":null,"progress":null,"outcome":null,"session_id":null,"tool_name":null,"records":[],"running_children":0,"blocked_children":0,"badge":"unknown","display_state":{"state":"unknown"},"answer_inferred_at":null}
0 fg Some("claude.exe") => null | {"source":"process","confidence":0.20000000298023224,"agent":"claude"}
10 hook claude/SessionStart {"session_id":"s1","source":"startup"} => {"from":{"state":"unknown"},"to":{"state":"idle"},"source":"hook","turn":0,"at":10} | {"state":{"state":"idle"},"source":"hook","confidence":0.949999988079071,"since":10,"session_id":"s1","badge":"idle","display_state":{"state":"idle"}}
20 title "✳ Claude Code" => null | {}
30 hook claude/UserPromptSubmit {"session_id":"s1","prompt":"fix it"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"hook","turn":0,"at":30} | {"state":{"state":"working"},"since":30,"badge":"working","display_state":{"state":"working"}}
40 out "thinking" => q=0 inv=0 changed=false tr=null | {}
50 hook claude/PreToolUse {"session_id":"s1","tool_name":"Bash","tool_use_id":"tu1"} => null | {"tool_name":"Bash"}
60 hook claude/SubagentStart {"session_id":"s1","agent_id":"a 1","agent_type":"Explore"} => null | {"records":[{"id":"hook/a-1","state":{"state":"working"},"source":"hook","title":"Explore","msg":null,"progress":null,"since":60}],"running_children":1}
70 hook claude/PreToolUse {"session_id":"s1","agent_id":"a 1","tool_name":"Read"} => null | {}
80 hook claude/PermissionRequest {"session_id":"s1","tool_name":"Bash"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"permission"},"source":"hook","turn":1,"at":80} | {"state":{"state":"blocked","kind":"permission"},"since":80,"turn":1,"msg":"Bash","badge":"blocked:permission (+1 running)","display_state":{"state":"blocked","kind":"permission"}}
90 input "\u{1b}[I" => () | {}
100 out "x" => q=0 inv=0 changed=false tr=null | {}
110 input "1" => () | {}
120 out "running" => q=0 inv=0 changed=true tr={"from":{"state":"blocked","kind":"permission"},"to":{"state":"working"},"source":"heuristic","turn":1,"at":120} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":120,"msg":null,"badge":"working","display_state":{"state":"working"},"answer_inferred_at":120}
130 hook claude/PostToolUse {"session_id":"s1","tool_name":"Bash"} => null | {"source":"hook","confidence":0.949999988079071,"answer_inferred_at":null}
140 hook claude/PreToolUse {"session_id":"s1","tool_name":"AskUserQuestion"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"question"},"source":"hook","turn":2,"at":140} | {"state":{"state":"blocked","kind":"question"},"since":140,"turn":2,"msg":"AskUserQuestion","tool_name":"AskUserQuestion","badge":"blocked:question (+1 running)","display_state":{"state":"blocked","kind":"question"}}
150 input "\u{1b}" => () | {}
160 input "2" => () | {}
4000 out "late" => q=0 inv=0 changed=false tr=null | {}
4010 hook claude/Elicitation {"session_id":"s1","message":"Pick a server"} => null | {"msg":"Pick a server"}
4020 hook claude/ElicitationResult {"session_id":"s1"} => {"from":{"state":"blocked","kind":"question"},"to":{"state":"working"},"source":"hook","turn":2,"at":4020} | {"state":{"state":"working"},"since":4020,"msg":null,"badge":"working","display_state":{"state":"working"}}
4030 hook claude/SubagentStop {"session_id":"s1","agent_id":"a 1"} => null | {"records":[],"running_children":0}
4040 hook claude/Stop {"session_id":"s1"} => {"from":{"state":"working"},"to":{"state":"done"},"source":"hook","turn":3,"at":4040} | {"state":{"state":"done"},"since":4040,"turn":3,"outcome":"completed","badge":"done","display_state":{"state":"done"}}
4050 ack => {"from":{"state":"done"},"to":{"state":"idle"},"source":"hook","turn":3,"at":4050} | {"state":{"state":"idle"},"since":4050,"badge":"idle","display_state":{"state":"idle"}}
4060 hook claude/UserPromptSubmit {"session_id":"s1"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"hook","turn":3,"at":4060} | {"state":{"state":"working"},"since":4060,"outcome":null,"badge":"working","display_state":{"state":"working"}}
4070 hook claude/StopFailure {"session_id":"s1","error":"overloaded"} => {"from":{"state":"working"},"to":{"state":"error"},"source":"hook","turn":4,"at":4070} | {"state":{"state":"error"},"since":4070,"turn":4,"msg":"overloaded","outcome":"failed","badge":"error","display_state":{"state":"error"}}
4080 hook claude/UserPromptSubmit {"session_id":"s1"} => {"from":{"state":"error"},"to":{"state":"working"},"source":"hook","turn":4,"at":4080} | {"state":{"state":"working"},"since":4080,"msg":null,"outcome":null,"badge":"working","display_state":{"state":"working"}}
4090 hook claude/Stop {"session_id":"s1","is_interrupt":true} => {"from":{"state":"working"},"to":{"state":"idle"},"source":"hook","turn":5,"at":4090} | {"state":{"state":"idle"},"since":4090,"turn":5,"outcome":"cancelled","badge":"idle","display_state":{"state":"idle"}}
4100 hook claude/SessionStart {"session_id":"s1","source":"compact"} => null | {}
4110 title "⠂ Working" => null | {}
130000 tick => {"from":{"state":"idle"},"to":{"state":"working"},"source":"heuristic","turn":5,"at":130000} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":130000,"outcome":null,"badge":"working","display_state":{"state":"working"}}
130010 hook claude/Notification {"session_id":"s1"} => {"from":{"state":"working"},"to":{"state":"idle"},"source":"hook","turn":6,"at":130010} | {"state":{"state":"idle"},"source":"hook","confidence":0.949999988079071,"since":130010,"turn":6,"badge":"idle","display_state":{"state":"idle"}}
130020 hook claude/SessionEnd {"session_id":"s1","reason":"exit"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"heuristic","turn":6,"at":130020} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":130020,"badge":"working","display_state":{"state":"working"}}
130030 exit => {"from":{"state":"working"},"to":{"state":"exited"},"source":"process","turn":7,"at":130030} | {"state":{"state":"exited"},"final_state":{"state":"working"},"source":"process","confidence":1.0,"since":130030,"turn":7,"badge":"exited","display_state":{"state":"exited"}}
130040 hook claude/SessionStart {"session_id":"s2","source":"startup"} => {"from":{"state":"unknown"},"to":{"state":"idle"},"source":"hook","turn":7,"at":130040} | {"state":{"state":"idle"},"final_state":null,"source":"hook","confidence":0.949999988079071,"since":130040,"session_id":"s2","tool_name":null,"badge":"idle","display_state":{"state":"idle"}}
130050 restore_turn 7 => () | {}
130060 hook claude/UserPromptSubmit {"session_id":"s2"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"hook","turn":7,"at":130060} | {"state":{"state":"working"},"since":130060,"badge":"working","display_state":{"state":"working"}}
130070 hook claude/PermissionRequest {"session_id":"s2","tool_name":"Write"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"permission"},"source":"hook","turn":8,"at":130070} | {"state":{"state":"blocked","kind":"permission"},"since":130070,"turn":8,"msg":"Write","tool_name":"Write","badge":"blocked:permission","display_state":{"state":"blocked","kind":"permission"}}
130080 restore_answer 130075 => {"from":{"state":"blocked","kind":"permission"},"to":{"state":"working"},"source":"heuristic","turn":8,"at":130080} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":130080,"msg":null,"badge":"working","display_state":{"state":"working"},"answer_inferred_at":130075}
130090 hook claude/Stop {"session_id":"s2"} => {"from":{"state":"working"},"to":{"state":"done"},"source":"hook","turn":9,"at":130090} | {"state":{"state":"done"},"source":"hook","confidence":0.949999988079071,"since":130090,"turn":9,"outcome":"completed","badge":"done","display_state":{"state":"done"},"answer_inferred_at":null}
130100 title "✳ Claude Code" => null | {}
260000 tick => null | {"confidence":0.4749999940395355}
260010 ack => {"from":{"state":"done"},"to":{"state":"idle"},"source":"heuristic","turn":9,"at":260010} | {"state":{"state":"idle"},"source":"heuristic","confidence":0.6000000238418579,"since":260010,"badge":"idle","display_state":{"state":"idle"}}
"##;

const CODEX_TRACE: &str = r##"
start {"state":{"state":"unknown"},"final_state":null,"source":null,"confidence":0.0,"since":0,"turn":0,"agent":null,"app":null,"msg":null,"title":null,"progress":null,"outcome":null,"session_id":null,"tool_name":null,"records":[],"running_children":0,"blocked_children":0,"badge":"unknown","display_state":{"state":"unknown"},"answer_inferred_at":null}
0 fg Some("codex.exe") => null | {"source":"process","confidence":0.20000000298023224,"agent":"codex"}
100 title "⠋ codex" => {"from":{"state":"unknown"},"to":{"state":"working"},"source":"heuristic","turn":0,"at":100} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":100,"badge":"working","display_state":{"state":"working"}}
200 hook codex/SessionStart {"session_id":"c1"} => {"from":{"state":"working"},"to":{"state":"idle"},"source":"hook","turn":0,"at":200} | {"state":{"state":"idle"},"source":"hook","confidence":0.949999988079071,"since":200,"session_id":"c1","badge":"idle","display_state":{"state":"idle"}}
300 hook codex/UserPromptSubmit {"session_id":"c1","turn_id":"t1"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"hook","turn":0,"at":300} | {"state":{"state":"working"},"since":300,"badge":"working","display_state":{"state":"working"}}
400 hook codex/PreToolUse {"session_id":"c1","tool_name":"shell"} => null | {"tool_name":"shell"}
500 hook codex/PermissionRequest {"session_id":"c1","tool_name":"shell"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"permission"},"source":"hook","turn":1,"at":500} | {"state":{"state":"blocked","kind":"permission"},"since":500,"turn":1,"msg":"shell","badge":"blocked:permission","display_state":{"state":"blocked","kind":"permission"}}
600 screen "Would you like to run the following command?\n  cargo test" => null | {}
700 input "y" => () | {}
800 out "ok" => q=0 inv=0 changed=true tr={"from":{"state":"blocked","kind":"permission"},"to":{"state":"working"},"source":"heuristic","turn":1,"at":800} | {"state":{"state":"working"},"source":"heuristic","confidence":0.6000000238418579,"since":800,"msg":null,"badge":"working","display_state":{"state":"working"},"answer_inferred_at":800}
900 screen "" => null | {}
1000 hook codex/PreToolUse {"session_id":"c1","tool_name":"request_user_input"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"question"},"source":"hook","turn":2,"at":1000} | {"state":{"state":"blocked","kind":"question"},"source":"hook","confidence":0.949999988079071,"since":1000,"turn":2,"msg":"request_user_input","tool_name":"request_user_input","badge":"blocked:question","display_state":{"state":"blocked","kind":"question"},"answer_inferred_at":null}
1100 hook codex/Interrupt {"session_id":"c1"} => {"from":{"state":"blocked","kind":"question"},"to":{"state":"idle"},"source":"hook","turn":2,"at":1100} | {"state":{"state":"idle"},"since":1100,"msg":null,"outcome":"cancelled","badge":"idle","display_state":{"state":"idle"}}
1200 hook codex/UserPromptSubmit {"session_id":"c1","turn_id":"t2"} => {"from":{"state":"idle"},"to":{"state":"working"},"source":"hook","turn":2,"at":1200} | {"state":{"state":"working"},"since":1200,"outcome":null,"badge":"working","display_state":{"state":"working"}}
1300 hook codex/SubagentStart {"session_id":"c1","agent_id":"w1","agent_type":"worker"} => null | {"records":[{"id":"hook/w1","state":{"state":"working"},"source":"hook","title":"worker","msg":null,"progress":null,"since":1300}],"running_children":1}
1350 hook codex/PostToolUse {"session_id":"c1","tool_name":"shell"} => null | {"tool_name":"shell"}
1400 hook codex/SubagentStop {"session_id":"c1","agent_id":"w1"} => null | {"records":[],"running_children":0}
1500 hook codex/Stop {"session_id":"c1"} => {"from":{"state":"working"},"to":{"state":"done"},"source":"hook","turn":3,"at":1500} | {"state":{"state":"done"},"since":1500,"turn":3,"outcome":"completed","badge":"done","display_state":{"state":"done"}}
1600 title "codex" => null | {}
5000 tick => null | {}
200000 tick => null | {"source":"heuristic","confidence":0.4000000059604645}
200100 ack => {"from":{"state":"done"},"to":{"state":"idle"},"source":"heuristic","turn":3,"at":200100} | {"state":{"state":"idle"},"since":200100,"badge":"idle","display_state":{"state":"idle"}}
200200 out "\u{1b}]7501;state=working:app=codex\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"idle"},"to":{"state":"working"},"source":"osc","turn":3,"at":200200} | {"state":{"state":"working"},"source":"osc","confidence":0.8999999761581421,"since":200200,"app":"codex","outcome":null,"badge":"working","display_state":{"state":"working"}}
200300 fg None => {"from":{"state":"working"},"to":{"state":"exited"},"source":"process","turn":4,"at":200300} | {"state":{"state":"exited"},"final_state":{"state":"working"},"source":"process","confidence":1.0,"since":200300,"turn":4,"badge":"exited","display_state":{"state":"exited"}}
200400 fg Some("codex.exe") => null | {"state":{"state":"unknown"},"final_state":null,"confidence":0.20000000298023224,"since":200400,"app":null,"session_id":null,"tool_name":null,"badge":"unknown","display_state":{"state":"unknown"}}
200500 tick => null | {}
204000 tick => {"from":{"state":"unknown"},"to":{"state":"idle"},"source":"heuristic","turn":4,"at":204000} | {"state":{"state":"idle"},"source":"heuristic","confidence":0.4000000059604645,"since":204000,"badge":"idle","display_state":{"state":"idle"}}
"##;

const OSC_TRACE: &str = r##"
start {"state":{"state":"unknown"},"final_state":null,"source":null,"confidence":0.0,"since":0,"turn":0,"agent":null,"app":null,"msg":null,"title":null,"progress":null,"outcome":null,"session_id":null,"tool_name":null,"records":[],"running_children":0,"blocked_children":0,"badge":"unknown","display_state":{"state":"unknown"},"answer_inferred_at":null}
0 out "\u{1b}[>0q\u{1b}]7501;?\u{7}" => q=1 inv=0 changed=false tr=null | {}
10 out "\u{1b}]7501;state=idle:app=claude-code\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"unknown"},"to":{"state":"idle"},"source":"osc","turn":0,"at":10} | {"state":{"state":"idle"},"source":"osc","confidence":0.8999999761581421,"since":10,"agent":"claude","app":"claude-code","badge":"idle","display_state":{"state":"idle"}}
20 out "\u{1b}]7501;state=working:app=claude-code:progress=10:msg=Y29tcGlsaW5n:title=QnVpbGQ=\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"idle"},"to":{"state":"working"},"source":"osc","turn":0,"at":20} | {"state":{"state":"working"},"since":20,"msg":"compiling","title":"Build","progress":10,"badge":"working","display_state":{"state":"working"}}
30 out "\u{1b}]7501;st" => q=0 inv=0 changed=false tr=null | {}
31 out "ate=done:app=claude-code\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"working"},"to":{"state":"done"},"source":"osc","turn":1,"at":31} | {"state":{"state":"done"},"since":31,"turn":1,"msg":null,"progress":null,"badge":"done","display_state":{"state":"done"}}
35 out "\u{1b}]7501;state=working:app=claude-code\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"done"},"to":{"state":"working"},"source":"osc","turn":1,"at":35} | {"state":{"state":"working"},"since":35,"badge":"working","display_state":{"state":"working"}}
40 out "\u{1b}]7501;state=working:id=sub1:title=Y2hpbGQ=\u{7}" => q=0 inv=0 changed=true tr=null | {"records":[{"id":"sub1","state":{"state":"working"},"source":"osc","title":"child","msg":null,"progress":null,"since":40}],"running_children":1}
50 out "\u{1b}]7501;state=blocked:id=sub1:kind=question\u{7}" => q=0 inv=0 changed=true tr=null | {"records":[{"id":"sub1","state":{"state":"blocked","kind":"question"},"source":"osc","title":"child","msg":null,"progress":null,"since":50}],"running_children":0,"blocked_children":1,"badge":"blocked:question","display_state":{"state":"blocked","kind":"question"}}
60 mod {"state":"done"} Some("mod says done") => null | {}
70 hook claude/PermissionRequest {"tool_name":"Bash"} => {"from":{"state":"working"},"to":{"state":"blocked","kind":"permission"},"source":"hook","turn":2,"at":70} | {"state":{"state":"blocked","kind":"permission"},"source":"hook","confidence":0.949999988079071,"since":70,"turn":2,"msg":"Bash","tool_name":"Bash","badge":"blocked:permission","display_state":{"state":"blocked","kind":"permission"}}
80 out "\u{1b}]7501;state=working\u{7}" => q=0 inv=0 changed=true tr=null | {}
90 out "\u{1b}]7501;state=idle\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"blocked","kind":"permission"},"to":{"state":"idle"},"source":"osc","turn":2,"at":90} | {"state":{"state":"idle"},"source":"osc","confidence":0.8999999761581421,"since":90,"msg":null,"badge":"blocked:question","display_state":{"state":"blocked","kind":"question"}}
100 out "\u{1b}]7501;state=clear:id=sub1\u{7}" => q=0 inv=0 changed=true tr=null | {"records":[],"blocked_children":0,"badge":"idle","display_state":{"state":"idle"}}
110 out "\u{1b}]7501;state=clear\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"idle"},"to":{"state":"blocked","kind":"permission"},"source":"hook","turn":2,"at":110} | {"state":{"state":"blocked","kind":"permission"},"source":"hook","confidence":0.949999988079071,"since":110,"msg":"Bash","badge":"blocked:permission","display_state":{"state":"blocked","kind":"permission"}}
115 hook claude/SessionEnd {} => {"from":{"state":"blocked","kind":"permission"},"to":{"state":"done"},"source":"mod","turn":2,"at":115} | {"state":{"state":"done"},"source":"mod","confidence":0.75,"since":115,"msg":"mod says done","badge":"done","display_state":{"state":"done"}}
120 ack => {"from":{"state":"done"},"to":{"state":"idle"},"source":"mod","turn":2,"at":120} | {"state":{"state":"idle"},"since":120,"badge":"idle","display_state":{"state":"idle"}}
130 mod null None => {"from":{"state":"idle"},"to":{"state":"unknown"},"source":null,"turn":2,"at":130} | {"state":{"state":"unknown"},"source":null,"confidence":0.0,"since":130,"msg":null,"badge":"unknown","display_state":{"state":"unknown"}}
140 out "\u{1b}]7501;state=done:app=podtest\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"unknown"},"to":{"state":"done"},"source":"osc","turn":2,"at":140} | {"state":{"state":"done"},"source":"osc","confidence":0.8999999761581421,"since":140,"app":"podtest","badge":"done","display_state":{"state":"done"}}
150 out "\u{1b}]7501;state=working:progress=999\u{7}" => q=0 inv=1 changed=false tr=null | {}
160 set_agent "codex" => null | {"agent":"codex"}
170 out "\u{1b}]7501;state=error:msg=Ym9vbQ==\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"done"},"to":{"state":"error"},"source":"osc","turn":2,"at":170} | {"state":{"state":"error"},"since":170,"msg":"boom","badge":"error","display_state":{"state":"error"}}
180 exit => {"from":{"state":"error"},"to":{"state":"exited"},"source":"process","turn":2,"at":180} | {"state":{"state":"exited"},"final_state":{"state":"error"},"source":"process","confidence":1.0,"since":180,"msg":null,"badge":"exited","display_state":{"state":"exited"}}
190 out "\u{1b}]7501;state=idle:app=codex\u{7}" => q=0 inv=0 changed=true tr={"from":{"state":"unknown"},"to":{"state":"idle"},"source":"osc","turn":2,"at":190} | {"state":{"state":"idle"},"final_state":null,"source":"osc","confidence":0.8999999761581421,"since":190,"app":"codex","title":null,"tool_name":null,"badge":"idle","display_state":{"state":"idle"}}
200 title "⠂ x" => null | {}
210 mod {"state":"blocked","kind":"permission"} None => null | {}
220 reset_run => () | {"state":{"state":"unknown"},"source":null,"confidence":0.0,"since":220,"agent":null,"app":null,"badge":"unknown","display_state":{"state":"unknown"}}
"##;
