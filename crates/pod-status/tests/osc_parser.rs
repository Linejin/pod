//! OSC 7501 parser: raw samples from docs/04 §1.3 replayed byte-for-byte,
//! split at every byte boundary, plus field validation (docs/04 §1.2, §4.4).

use base64::Engine as _;
use pod_status::osc::{InvalidReason, MAX_PAYLOAD, encode_report, parse_payload};
use pod_status::{
    Osc7501Parser, OscEvent, QUERY_REPLY, Report, ReportKind, ReportState, query_reply,
};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn events(bytes: &[u8]) -> Vec<OscEvent> {
    Osc7501Parser::new().feed(bytes)
}

fn reports(bytes: &[u8]) -> Vec<Report> {
    events(bytes)
        .into_iter()
        .filter_map(|e| {
            if let OscEvent::Report(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect()
}

/// Feed `bytes` split at every boundary (and byte-by-byte) and assert the
/// same events come out as from one chunk.
fn assert_split_invariant(bytes: &[u8]) -> Vec<OscEvent> {
    let whole = events(bytes);
    for i in 0..=bytes.len() {
        let mut p = Osc7501Parser::new();
        let mut got = p.feed(&bytes[..i]);
        got.extend(p.feed(&bytes[i..]));
        assert_eq!(got, whole, "split at {i}");
    }
    let mut p = Osc7501Parser::new();
    let mut got = Vec::new();
    for b in bytes {
        got.extend(p.feed(std::slice::from_ref(b)));
    }
    assert_eq!(got, whole, "byte by byte");
    // three-way splits on short inputs
    if bytes.len() <= 120 {
        for i in 0..=bytes.len() {
            for j in i..=bytes.len() {
                let mut p = Osc7501Parser::new();
                let mut got = p.feed(&bytes[..i]);
                got.extend(p.feed(&bytes[i..j]));
                got.extend(p.feed(&bytes[j..]));
                assert_eq!(got, whole, "split at {i},{j}");
            }
        }
    }
    whole
}

// ── §1.3 A: the query reply is exactly the observed reply bytes ──
const SAMPLE_A: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

#[test]
fn sample_a_query_reply_bytes() {
    assert_eq!(QUERY_REPLY, SAMPLE_A);
    assert_eq!(query_reply(), SAMPLE_A);
    // A reply seen in output is neither a query nor a report.
    let ev = assert_split_invariant(SAMPLE_A);
    assert!(
        ev.iter().all(|e| matches!(e, OscEvent::Invalid(_))),
        "{ev:?}"
    );
}

// ── §1.3 B: Claude's startup query (BEL terminated, after XTVERSION + kitty query) ──
#[test]
fn sample_b_claude_query() {
    let stream: &[u8] = b"\x1b[>0q\x1b[?u\x1b]7501;?\x07";
    let ev = assert_split_invariant(stream);
    assert_eq!(ev, vec![OscEvent::Query]);
    // Also ST-terminated.
    assert_eq!(
        assert_split_invariant(b"\x1b]7501;?\x1b\\"),
        vec![OscEvent::Query]
    );
}

// ── §1.3 C: Claude reports in an attach stream (ST replay, BEL originals) ──
const SAMPLE_C: &[u8] = b"...\x1b[=5;1u\x1b]7501;state=idle:app=claude-code\x1b\\\x1b[?1004h...\
\x1b]7501;state=working:app=claude-code\x07\
...\x1b[46;3H\x1b[?25h\x1b]7501;state=done:app=claude-code\x07\x1b[m...";

#[test]
fn sample_c_claude_reports() {
    let ev = assert_split_invariant(SAMPLE_C);
    let states: Vec<_> = ev
        .iter()
        .map(|e| match e {
            OscEvent::Report(r) => {
                assert_eq!(r.app.as_deref(), Some("claude-code"));
                r.state()
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        states,
        vec![ReportState::Idle, ReportState::Working, ReportState::Done]
    );
}

// ── §1.3 D: the observed report sequence as Claude would have sent it ──
pub fn sample_d_stream() -> Vec<u8> {
    let q = "answer: Would you like me to keep my replies short ... (Yes \u{b7} No)";
    let mut s = Vec::new();
    s.extend_from_slice(b"\x1b]7501;state=idle:app=claude-code\x07");
    s.extend_from_slice(b"\x1b]7501;state=working:app=claude-code\x07");
    s.extend_from_slice(b"\x1b]7501;state=done:app=claude-code\x07");
    s.extend_from_slice(b"\x1b]7501;state=working:app=claude-code\x07");
    s.extend_from_slice(
        format!(
            "\x1b]7501;state=blocked:app=claude-code:kind=question:msg={}\x07",
            b64(q)
        )
        .as_bytes(),
    );
    s.extend_from_slice(b"\x1b]7501;state=idle:app=claude-code\x07");
    s
}

#[test]
fn sample_d_question_msg_decoded() {
    let rs: Vec<Report> = assert_split_invariant(&sample_d_stream())
        .into_iter()
        .filter_map(|e| {
            if let OscEvent::Report(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rs.len(), 6);
    assert_eq!(rs[4].state(), ReportState::Blocked);
    assert_eq!(rs[4].kind, Some(ReportKind::Question));
    assert_eq!(
        rs[4].msg.as_deref(),
        Some("answer: Would you like me to keep my replies short ... (Yes \u{b7} No)")
    );
}

// ── §1.3 E: synthetic emitter, all fields ──
pub fn sample_e_stream() -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(
        format!(
            "\x1b]7501;state=working:app=podtest:progress=40:msg={}\x07",
            b64("compiling")
        )
        .as_bytes(),
    );
    s.extend_from_slice(
        format!(
            "\x1b]7501;state=blocked:app=podtest:kind=permission:msg={}\x1b\\",
            b64("Allow rm?")
        )
        .as_bytes(),
    );
    s.extend_from_slice(
        format!(
            "\x1b]7501;state=working:app=podtest:id=sub1:title={}\x07",
            b64("child task")
        )
        .as_bytes(),
    );
    s.extend_from_slice(b"\x1b]7501;state=done:app=podtest\x07");
    s.extend_from_slice(
        format!("\x1b]7501;state=error:app=podtest:msg={}\x07", b64("boom")).as_bytes(),
    );
    s.extend_from_slice(b"\x1b]7501;state=idle:app=podtest\x07");
    s
}

#[test]
fn sample_e_all_fields() {
    let rs: Vec<Report> = assert_split_invariant(&sample_e_stream())
        .into_iter()
        .filter_map(|e| {
            if let OscEvent::Report(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rs.len(), 6);
    assert_eq!(rs[0].progress, Some(40));
    assert_eq!(rs[0].msg.as_deref(), Some("compiling"));
    assert_eq!(rs[1].kind, Some(ReportKind::Permission));
    assert_eq!(rs[1].msg.as_deref(), Some("Allow rm?"));
    assert_eq!(rs[2].id.as_deref(), Some("sub1"));
    assert_eq!(rs[2].title.as_deref(), Some("child task"));
    assert_eq!(rs[4].msg.as_deref(), Some("boom"));
    assert_eq!(rs[5].state(), ReportState::Idle);
}

// ── validation ──

fn one(payload: &str) -> OscEvent {
    parse_payload(payload.as_bytes())
}

fn is_invalid(payload: &str) -> bool {
    matches!(one(payload), OscEvent::Invalid(_))
}

#[test]
fn invalid_field_drops_whole_record() {
    for p in [
        "state=sleeping",
        "state=idle:app=",
        "state=idle:app=bad app",
        "state=idle:app=has:colon", // `colon` becomes a field without '='
        "state=blocked:kind=nope",
        "state=working:progress=101",
        "state=working:progress=1000",
        "state=working:progress=",
        "state=working:progress=-1",
        "state=idle:title=!!!",
        "state=idle:msg=@@",
        "state=idle:Title=YQ==",
        "state=idle:waytoolongkeynamexx=1",
        "state=idle:=x",
        "state=idle:id=a/b/c/d/e/f/g/h/i",
        "state=idle:id=",
        "state=idle:id=a//b",
        "state=idle:id=a b",
        "state=idle:noequals",
        "app=x",
        "",
    ] {
        assert!(is_invalid(p), "{p:?} should be invalid, got {:?}", one(p));
    }
    assert_eq!(one("app=x"), OscEvent::Invalid(InvalidReason::MissingState));
}

#[test]
fn value_charset_and_lengths() {
    // value charset: [A-Za-z0-9_.,+/=-]
    assert!(is_invalid("state=idle:x=a;b"));
    assert!(!is_invalid("state=idle:x=a,b.c+d/e=f-g_h"));
    // app ≤ 32
    assert!(!is_invalid(&format!("state=idle:app={}", "a".repeat(32))));
    assert!(is_invalid(&format!("state=idle:app={}", "a".repeat(33))));
    // id: 8 segments of 32 chars would be 263 > 128, so the total limit applies
    let seg = "s".repeat(15);
    let id8 = [seg.as_str(); 8].join("/"); // 127 chars
    assert!(!is_invalid(&format!("state=idle:id={id8}")));
    assert!(is_invalid(&format!("state=idle:id={}", "a".repeat(33))));
    // title decoded ≤ 192, encoded ≤ 256
    assert!(!is_invalid(&format!(
        "state=idle:title={}",
        b64(&"t".repeat(192))
    )));
    assert!(is_invalid(&format!(
        "state=idle:title={}",
        b64(&"t".repeat(193))
    )));
    // msg decoded ≤ 2048
    assert!(!is_invalid(&format!(
        "state=idle:msg={}",
        b64(&"m".repeat(2048))
    )));
    assert!(is_invalid(&format!(
        "state=idle:msg={}",
        b64(&"m".repeat(2049))
    )));
    // control characters in decoded text are rejected
    assert!(is_invalid(&format!("state=idle:msg={}", b64("a\nb"))));
    assert!(is_invalid(&format!("state=idle:title={}", b64("a\x1bb"))));
    // invalid UTF-8
    let bad = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe]);
    assert!(is_invalid(&format!("state=idle:msg={bad}")));
}

#[test]
fn progress_forms() {
    let p = |s: &str| match one(s) {
        OscEvent::Report(r) => r.progress,
        e => panic!("{s}: {e:?}"),
    };
    assert_eq!(p("state=working:progress=0"), Some(0));
    assert_eq!(p("state=working:progress=0040"), Some(40));
    assert_eq!(p("state=working:progress=100"), Some(100));
    assert_eq!(p("state=blocked:progress=7"), Some(7));
    // progress is dropped (not invalid) for other states
    assert_eq!(p("state=idle:progress=50"), None);
}

#[test]
fn kind_only_kept_for_blocked_and_unknown_keys_ignored() {
    match one("state=working:kind=permission:future=1:xturn=3") {
        OscEvent::Report(r) => {
            assert_eq!(r.state(), ReportState::Working);
            assert_eq!(r.kind, None);
        }
        e => panic!("{e:?}"),
    }
    match one("state=blocked") {
        OscEvent::Report(r) => assert_eq!(r.kind, None),
        e => panic!("{e:?}"),
    }
}

#[test]
fn base64_padding_optional_and_empty_text() {
    // "ok" = b2s= ; unpadded b2s
    for p in ["state=idle:title=b2s=", "state=idle:title=b2s"] {
        match one(p) {
            OscEvent::Report(r) => assert_eq!(r.title.as_deref(), Some("ok")),
            e => panic!("{p}: {e:?}"),
        }
    }
    match one("state=idle:title=") {
        OscEvent::Report(r) => assert_eq!(r.title, None),
        e => panic!("{e:?}"),
    }
}

#[test]
fn clear_with_and_without_id() {
    match one("state=clear") {
        OscEvent::Report(r) => {
            assert_eq!(r.state(), ReportState::Clear);
            assert_eq!(r.id, None)
        }
        e => panic!("{e:?}"),
    }
    match one("state=clear:id=sub1/a") {
        OscEvent::Report(r) => assert_eq!(r.id.as_deref(), Some("sub1/a")),
        e => panic!("{e:?}"),
    }
}

#[test]
fn payload_size_limit() {
    let base = "state=idle:pad=";
    let ok = format!("{base}{}", "a".repeat(MAX_PAYLOAD - base.len()));
    assert_eq!(ok.len(), MAX_PAYLOAD);
    let seq_ok = format!("\x1b]7501;{ok}\x07");
    assert_eq!(reports(seq_ok.as_bytes()).len(), 1);
    let too = format!("\x1b]7501;{ok}a\x07");
    let ev = assert_split_invariant(too.as_bytes());
    assert_eq!(ev, vec![OscEvent::Invalid(InvalidReason::TooLong)]);
    // the parser recovers for the next sequence
    let mut s = too.into_bytes();
    s.extend_from_slice(b"\x1b]7501;state=done\x07");
    assert_eq!(reports(&s).len(), 1);
}

#[test]
fn non_ascii_payload_dropped() {
    let ev = events("\x1b]7501;state=idle:app=caf\u{e9}\x07".as_bytes());
    assert_eq!(ev, vec![OscEvent::Invalid(InvalidReason::NotAscii)]);
}

#[test]
fn stream_robustness() {
    // other OSCs (title), CSI noise, a lookalike number, and ESC-ESC
    let s = b"\x1b]0;\xe2\x9c\xb3 Claude Code\x07\x1b]75011;state=idle\x07\x1b]750;x\x07\x1b\x1b]7501;state=working\x07";
    let ev = assert_split_invariant(s);
    assert_eq!(ev.len(), 1);
    // CAN aborts a sequence
    assert!(events(b"\x1b]7501;state=idle\x18\x07").is_empty());
    // ESC not followed by '\' aborts and may start the next sequence
    let ev = assert_split_invariant(b"\x1b]7501;state=idle\x1b]7501;state=done\x07");
    assert_eq!(ev.len(), 1);
    match &ev[0] {
        OscEvent::Report(r) => assert_eq!(r.state(), ReportState::Done),
        e => panic!("{e:?}"),
    }
    // a sequence left open is pending, not lost
    let mut p = Osc7501Parser::new();
    assert!(p.feed(b"\x1b]7501;state=wor").is_empty());
    assert!(p.is_mid_sequence());
    assert_eq!(p.feed(b"king\x1b").len(), 0);
    assert_eq!(p.feed(b"\\").len(), 1);
    assert!(!p.is_mid_sequence());
}

#[test]
fn encode_round_trip() {
    let r = Report {
        state: Some(ReportState::Blocked),
        app: Some("mytool".into()),
        id: Some("a/b".into()),
        kind: Some(ReportKind::Question),
        progress: Some(30),
        title: Some("작업".into()),
        msg: Some("Deploy to prod?".into()),
    };
    let bytes = encode_report(&r);
    assert_eq!(reports(&bytes), vec![r]);
    // docs/04 §4.4 shell examples
    let ex = format!(
        "\x1b]7501;state=working:app=mytool:progress=30\x07\x1b]7501;state=blocked:app=mytool:kind=question:msg={}\x07\x1b]7501;state=done:app=mytool\x07",
        b64("Deploy to prod?")
    );
    let rs = reports(ex.as_bytes());
    assert_eq!(
        rs.iter().map(Report::state).collect::<Vec<_>>(),
        vec![
            ReportState::Working,
            ReportState::Blocked,
            ReportState::Done
        ]
    );
}
