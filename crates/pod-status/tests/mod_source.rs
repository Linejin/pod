//! Extension evidence (`Source::Mod`, docs/16 §7.1): below hook and OSC,
//! above the heuristics; cleared by `None`, process exit and `ack` rules.

use pod_status::*;

fn osc(state: &str) -> Vec<u8> {
    format!("\x1b]7501;state={state}\x07").into_bytes()
}

#[test]
fn mod_beats_heuristics_but_not_osc_or_hooks() {
    let mut st = TerminalStatus::new(0);
    // Heuristic evidence first: a spinner title says working.
    st.on_title("\u{280b} thinking", 10);
    let t = st
        .on_mod(Some(AgentState::Idle), Some("prompt".into()), 20)
        .expect("transition");
    assert_eq!((t.to, t.source), (AgentState::Idle, Some(Source::Mod)));
    let snap = st.snapshot();
    assert_eq!(snap.msg.as_deref(), Some("prompt"));
    assert!((snap.confidence - 0.75).abs() < 1e-6);

    // OSC outranks the mod.
    st.feed_output(&osc("working"), 30);
    assert_eq!(
        (st.state(), st.snapshot().source),
        (AgentState::Working, Some(Source::Osc))
    );
    st.on_mod(Some(AgentState::Blocked(BlockedKind::Question)), None, 40);
    assert_eq!(st.state(), AgentState::Working, "osc still wins");

    // Hooks outrank both.
    let hook = HookMessage::from_line(
        r#"{"v":1,"terminal_id":"t","agent":"claude","event":"Stop","payload":{}}"#,
    )
    .unwrap();
    st.on_hook(&hook, 50);
    assert_eq!(
        (st.state(), st.snapshot().source),
        (AgentState::Done, Some(Source::Hook))
    );
}

#[test]
fn clearing_and_exit_drop_mod_evidence() {
    let mut st = TerminalStatus::new(0);
    st.on_mod(Some(AgentState::Working), None, 10);
    assert_eq!(st.state(), AgentState::Working);
    let t = st.on_mod(None, None, 20).unwrap();
    assert_eq!(t.to, AgentState::Unknown);
    assert_eq!(st.turn(), 1, "leaving working ends a turn");

    st.on_mod(Some(AgentState::Done), None, 30);
    assert_eq!(st.state(), AgentState::Done);
    st.ack(40);
    assert_eq!(st.state(), AgentState::Idle, "ack turns mod done into idle");

    st.on_mod(Some(AgentState::Working), None, 50);
    st.on_process_exit(60);
    assert_eq!(st.state(), AgentState::Exited);
    assert_eq!(st.snapshot().final_state, Some(AgentState::Working));
}

#[test]
fn source_order() {
    assert!(
        Source::Hook > Source::Osc
            && Source::Osc > Source::Mod
            && Source::Mod > Source::Heuristic
            && Source::Heuristic > Source::Process
    );
    assert_eq!(serde_json::to_string(&Source::Mod).unwrap(), "\"mod\"");
}
