//! Title glyph / screen pattern / quiet-time / process adapters with the
//! Claude and Codex samples from docs/04 (§1.4, §2, §3.4, §4.3).

use pod_status::heuristic::{
    HeuristicTracker, SCREEN_PATTERNS, TITLE_RULES, TitleSignal, classify_title, match_screen,
};
use pod_status::{
    AgentKind, AgentState, BlockedKind, ProcessInfo, Source, StatusConfig, TerminalStatus,
    detect_agent,
};

const CLAUDE: Option<&AgentKind> = Some(&AgentKind::Claude);
const CODEX: Option<&AgentKind> = Some(&AgentKind::Codex);

#[test]
fn claude_title_glyphs() {
    assert_eq!(
        classify_title(CLAUDE, "\u{2733} Claude Code"),
        Some(TitleSignal::Idle)
    );
    assert_eq!(
        classify_title(CLAUDE, "  \u{2733} Fix login bug"),
        Some(TitleSignal::Idle)
    );
    assert_eq!(
        classify_title(CLAUDE, "\u{2802} Fix login bug"),
        Some(TitleSignal::Working)
    );
    assert_eq!(
        classify_title(CLAUDE, "\u{2810} Fix login bug"),
        Some(TitleSignal::Working)
    );
    assert_eq!(
        classify_title(CLAUDE, "\u{25d0} thinking"),
        Some(TitleSignal::Working)
    );
    assert_eq!(classify_title(CLAUDE, "Claude Code"), None);
    // no agent known → titles say nothing (a shell may show anything)
    assert_eq!(classify_title(None, "\u{280b} npm install"), None);
}

#[test]
fn codex_title_samples() {
    // docs/04 §4.3: `⠋ work` → `⠋ Reply ok | work` → (end) `Reply ok | work`
    assert_eq!(
        classify_title(CODEX, "\u{280b} work"),
        Some(TitleSignal::Working)
    );
    assert_eq!(
        classify_title(CODEX, "\u{280b} Reply ok | work"),
        Some(TitleSignal::Working)
    );
    assert_eq!(
        classify_title(CODEX, "Reply ok | work"),
        Some(TitleSignal::NoIndicator)
    );
    // the whole Braille block counts
    assert_eq!(
        classify_title(CODEX, "\u{2800} x"),
        Some(TitleSignal::Working)
    );
    assert_eq!(
        classify_title(CODEX, "\u{28ff} x"),
        Some(TitleSignal::Working)
    );
    // Claude's idle mark is not a Codex rule
    assert_eq!(
        classify_title(CODEX, "\u{2733} x"),
        Some(TitleSignal::NoIndicator)
    );
}

#[test]
fn tables_are_well_formed() {
    assert!(
        TITLE_RULES
            .iter()
            .all(|r| r.first <= r.last && !r.note.is_empty())
    );
    assert!(
        SCREEN_PATTERNS
            .iter()
            .all(|p| p.needle == p.needle.to_lowercase() && !p.needle.is_empty())
    );
    assert!(
        SCREEN_PATTERNS.iter().any(|p| p.verified),
        "at least the observed Claude trust dialog"
    );
}

#[test]
fn claude_tracker_working_then_idle_mark_is_done() {
    let mut h = HeuristicTracker::new();
    h.on_title(CLAUDE, "\u{2733} Claude Code", 0);
    assert_eq!(h.verdict().unwrap().state, AgentState::Idle);
    h.on_title(CLAUDE, "\u{2802} Reply ok", 1);
    assert_eq!(h.verdict().unwrap().state, AgentState::Working);
    h.on_title(CLAUDE, "\u{2810} Reply ok", 2);
    h.on_title(CLAUDE, "\u{2733} Reply ok", 3);
    assert_eq!(h.verdict().unwrap().state, AgentState::Done);
    h.ack();
    assert_eq!(h.verdict().unwrap().state, AgentState::Idle);
}

#[test]
fn codex_quiet_time_idle() {
    let cfg = StatusConfig {
        quiet_idle_ms: 3_000,
        ..Default::default()
    };
    let mut t = TerminalStatus::with_config(cfg, 0);
    t.on_foreground(
        Some(&ProcessInfo::new(
            "codex.exe",
            &["codex.exe", "--no-daemon"],
        )),
        0,
    );
    // initial quiet with no spinner → idle
    t.feed_output(b"welcome", 100);
    assert_eq!(t.tick(2_000), None);
    let tr = t.tick(3_200).unwrap();
    assert_eq!(
        (tr.to, tr.source),
        (AgentState::Idle, Some(Source::Heuristic))
    );
    // spinner → working
    assert_eq!(
        t.on_title("\u{280b} work", 4_000).unwrap().to,
        AgentState::Working
    );
    t.on_title("\u{280b} Reply ok | work", 4_100);
    // spinner gone, but output keeps flowing → still working
    t.on_title("Reply ok | work", 5_000);
    t.feed_output(b"tokens", 7_500);
    assert_eq!(t.tick(8_000), None);
    assert_eq!(t.state(), AgentState::Working);
    // quiet for 3 s after the last output → done (a turn finished)
    let tr = t.tick(10_600).unwrap();
    assert_eq!(
        (tr.from, tr.to, tr.turn),
        (AgentState::Working, AgentState::Done, 1)
    );
    assert!(t.snapshot().confidence < 0.5);
    assert_eq!(t.ack(11_000).unwrap().to, AgentState::Idle);
}

#[test]
fn screen_patterns() {
    // observed: Claude trust dialog shows no OSC 7501 (docs/04 §1.4)
    let screen = " Accessing workspace:\n C:\\work\\x\n Is this a project you trust? (Claude Code may read files)\n > 1. Yes, proceed";
    let p = match_screen(CLAUDE, screen).unwrap();
    assert_eq!(
        (p.kind, p.label, p.verified),
        (BlockedKind::Permission, "trust", true)
    );
    assert!(match_screen(CLAUDE, "Quick safety check: trust this folder?").is_some());
    assert!(match_screen(CLAUDE, "Do you want to proceed?\n 1. Yes").is_some());
    assert!(match_screen(CODEX, "> You are in C:\\w\n  Trust this folder?").is_some());
    assert!(match_screen(CODEX, "Would you like to run the following command?").is_some());
    assert!(match_screen(CLAUDE, "Would you like to run the following command?").is_none());
    assert!(match_screen(None, "Is this a project you trust?").is_none());
    assert!(match_screen(CLAUDE, "all good").is_none());
}

#[test]
fn screen_blocked_overrides_title_and_clears() {
    let mut t = TerminalStatus::new(0);
    t.on_foreground(
        Some(&ProcessInfo::new(
            "cmd.exe",
            &["cmd.exe", "/c", "claude.CMD"],
        )),
        0,
    );
    let tr = t.on_screen("Is this a project you trust?", 1).unwrap();
    assert_eq!(
        (tr.to, tr.source),
        (
            AgentState::Blocked(BlockedKind::Permission),
            Some(Source::Heuristic)
        )
    );
    assert_eq!(
        t.snapshot().msg,
        None,
        "heuristic msg is not exposed as agent msg"
    );
    // dialog accepted; first OSC report arrives (idle) and outranks the screen anyway
    t.on_screen("Welcome to Claude Code", 2);
    assert_eq!(t.state(), AgentState::Unknown);
    t.feed_output(b"\x1b]7501;state=idle:app=claude-code\x07", 3);
    assert_eq!(
        (t.state(), t.snapshot().source),
        (AgentState::Idle, Some(Source::Osc))
    );
}

#[test]
fn agent_from_title_when_process_unknown() {
    let mut t = TerminalStatus::new(0);
    t.on_title("\u{2733} Claude Code", 1);
    assert_eq!(t.agent(), Some(&AgentKind::Claude));
    assert_eq!(t.state(), AgentState::Idle);
}

#[test]
fn process_adapter() {
    let d = |name: &str, argv: &[&str]| detect_agent(&ProcessInfo::new(name, argv));
    assert_eq!(d("claude.exe", &["claude.exe"]), Some(AgentKind::Claude));
    assert_eq!(d("CLAUDE.EXE", &[]), Some(AgentKind::Claude));
    assert_eq!(
        d("codex.EXE", &["codex", "--no-daemon"]),
        Some(AgentKind::Codex)
    );
    // npm shims (docs/04 §2: `cmd.exe /c claude.CMD …`)
    assert_eq!(
        d("cmd.exe", &["cmd.exe", "/c", "claude.CMD", "reply ok"]),
        Some(AgentKind::Claude)
    );
    assert_eq!(
        d(
            "cmd.exe",
            &[
                "C:\\Windows\\system32\\cmd.exe",
                "/d",
                "/s",
                "/c",
                "C:\\Users\\u\\AppData\\Roaming\\npm\\codex.cmd"
            ]
        ),
        Some(AgentKind::Codex)
    );
    // interpreters running the package
    assert_eq!(
        d(
            "node.exe",
            &[
                "node",
                "C:\\Users\\u\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js"
            ]
        ),
        Some(AgentKind::Claude)
    );
    assert_eq!(
        d(
            "node",
            &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"]
        ),
        Some(AgentKind::Codex)
    );
    // not agents
    assert_eq!(d("pwsh.exe", &["pwsh.exe", "-NoLogo"]), None);
    assert_eq!(d("cmd.exe", &["cmd.exe", "/c", "dir"]), None);
    assert_eq!(d("python.exe", &["python", "claude.py"]), None);
    assert_eq!(d("claude-helper.exe", &[]), None);
}
