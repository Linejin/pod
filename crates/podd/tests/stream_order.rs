//! The order of the live notifications for one terminal's lifecycle
//! (rescore step 4, docs/20-refactor/rescore.md §5; daemon-validation.md
//! named it as the gap of the D-T3 acceptance): one isolated daemon, one
//! `status.subscribe` stream with lifecycle events, one fake agent driven by
//! files, one `terminal.attach` stream. Steps: run -> attach (snapshot) ->
//! working -> blocked -> done -> `terminal.ack` -> exit -> `terminal.rm`.
//! Each step records, in arrival order, what the attach stream brought (OSC
//! 7501 reports and the agent's new text) and what the status stream
//! brought (`status.changed`, `terminal.changed`), and the whole log is
//! compared with [`GOLDEN`].
//!
//! Stable on a slow runner: no fixed sleeps. The agent moves on only when
//! the test creates its next `go-<n>` file, and a step ends when both
//! streams delivered that step's last notification (a state, a text marker,
//! `terminal.exited`, `removed`), so every notification belongs to exactly
//! one step. Masked: times, process ids, byte offsets (their continuity is
//! asserted instead), and the one race the daemon has here: the agent's first
//! report is published as soon as its holder streams, `terminal.changed
//! created` once the terminal is listed, so either comes first and an
//! `unknown` status may precede the report. That step waits for both and
//! records `created` (live status masked, as in the `terminal.run` result)
//! before the status, without `unknown`. Every later step has one order;
//! only `attached` in `terminal.changed exited` is masked (the attach stream
//! closes right after `terminal.exited`, before or after that info is built).
//!
//! Run in an isolated profile: `tools/test-daemon-refactor.ps1 -Test
//! stream_order`. With `POD_STREAM_ORDER_OUT=<file>` the log is written
//! there instead of compared (re-recording is a behaviour change: say why in
//! the commit).

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{Daemon, cwd, fake_agent_exe, profile_dir, unique};
use podd::client::{AttachEvent, StatusStreamEvent, SubscribeEvent};
use podd::protocol::*;
use serde_json::{Map, Value, json};

const OUT_ENV: &str = "POD_STREAM_ORDER_OUT";
/// How long one step may take on a slow runner.
const STEP: Duration = Duration::from_secs(60);
/// The text the agent prints, one per step; recorded the first time seen.
const MARKERS: [&str; 4] = ["READY", "WORKING", "ASKING", "DONE"];

// ───────────────────────────── normalizing ─────────────────────────────

/// Path spellings to replace, longest first.
struct Paths(Vec<(String, &'static str)>);

impl Paths {
    fn new(dirs: &[(&Path, &'static str)]) -> Self {
        let mut pairs = Vec::new();
        for (p, mark) in dirs {
            let mut spellings = vec![p.display().to_string()];
            if let Ok(long) = std::fs::canonicalize(p) {
                let s = long.display().to_string();
                spellings.push(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string());
            }
            for s in spellings {
                let s = s.trim_end_matches('\\').to_string();
                for v in [s.clone(), s.replace('\\', "/")] {
                    pairs.push((v.to_lowercase(), *mark));
                    pairs.push((v, *mark));
                }
            }
        }
        pairs.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        pairs.dedup();
        Paths(pairs)
    }

    fn text(&self, s: &str) -> String {
        let mut s = s.to_string();
        for (from, to) in &self.0 {
            s = s.replace(from.as_str(), to);
        }
        s
    }

    /// Sorted keys, paths replaced, run-dependent numbers masked.
    fn value(&self, key: Option<&str>, v: &Value) -> Value {
        if let Some(k) = key
            && volatile(k)
            && !v.is_null()
        {
            return json!("<n>");
        }
        match v {
            Value::String(s) => json!(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(None, x)).collect()),
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), self.value(Some(k), &m[k]));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
}

/// Times, process ids and byte offsets.
fn volatile(k: &str) -> bool {
    k.ends_with("_ms") || k.ends_with("_pid") || ["pid", "since", "seq"].contains(&k)
}

/// A terminal's info with its live status masked (see the module doc).
fn without_status(mut v: Value) -> Value {
    if v.get("status").is_some_and(|s| !s.is_null()) {
        v["status"] = json!("<live>");
    }
    v
}

// ───────────────────────────── output ─────────────────────────────

/// The OSC 7501 reports (`osc:<payload>`) and the printed text (`text:<..>`)
/// of raw terminal output, in order, and how many bytes that took: an escape
/// sequence cut off at the end (split across notifications) is left for the
/// caller to prepend to the next bytes. Other escape sequences are dropped.
fn tokens(data: &[u8]) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut text = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0x1b {
            text.push(data[i]);
            i += 1;
            continue;
        }
        let next = match data.get(i + 1) {
            None => None,
            // OSC: the body up to BEL or ST.
            Some(b']') => {
                let body = i + 2;
                let mut j = body;
                let end = loop {
                    match (data.get(j), data.get(j + 1)) {
                        (None, _) | (Some(0x1b), None) => break None,
                        (Some(0x07), _) => break Some((j, j + 1)),
                        (Some(0x1b), Some(b'\\')) => break Some((j, j + 2)),
                        _ => j += 1,
                    }
                };
                if let Some((stop, _)) = end
                    && let Some(report) = data[body..stop].strip_prefix(b"7501;")
                {
                    out.push(format!("text:{}", String::from_utf8_lossy(&text)));
                    out.push(format!("osc:{}", String::from_utf8_lossy(report)));
                    text.clear();
                }
                end.map(|(_, next)| next)
            }
            // CSI: parameters, then one final byte.
            Some(b'[') => (i + 2..data.len())
                .find(|&j| (0x40..=0x7e).contains(&data[j]))
                .map(|j| j + 1),
            Some(_) => Some(i + 2),
        };
        match next {
            Some(n) => i = n,
            None => break,
        }
    }
    out.push(format!("text:{}", String::from_utf8_lossy(&text)));
    (out, i)
}

/// The reports and text of `data` delivered in two notifications cut at
/// `at`, the way [`Log::attach_until`] reads them.
fn tokens_split(data: &[u8], at: usize) -> Vec<String> {
    let mut cut = data[..at].to_vec();
    let (mut got, used) = tokens(&cut);
    cut.drain(..used);
    cut.extend_from_slice(&data[at..]);
    got.extend(tokens(&cut).0);
    got.retain(|t| t != "text:");
    got
}

#[test]
fn tokens_survive_any_split() {
    let data = b"A\x1b[2J\x1b]0;title\x07\x1b]7501;state=working:app=fake\x1b\\WORK\x1b]7501;state=idle\x07B";
    let want = [
        "text:A",
        "osc:state=working:app=fake",
        "text:WORK",
        "osc:state=idle",
        "text:B",
    ];
    for at in 0..=data.len() {
        let mut got = tokens_split(data, at);
        // Text is cut where the bytes are: join neighbouring text.
        got.dedup_by(|b, a| {
            let join = a.starts_with("text:") && b.starts_with("text:");
            if join {
                a.push_str(&b["text:".len()..]);
            }
            join
        });
        assert_eq!(got, want, "cut at {at}");
    }
}

// ───────────────────────────── the streams ─────────────────────────────

enum Attach {
    Event(AttachEvent),
    End,
}

struct Log {
    id: String,
    paths: Paths,
    status: Receiver<SubscribeEvent>,
    status_seq: u64,
    attach: Option<Receiver<Attach>>,
    /// The next output offset the attach stream must bring.
    offset: u64,
    /// An escape sequence cut off at the end of the last output.
    cut: Vec<u8>,
    seen: BTreeSet<&'static str>,
    /// The argv of `terminal.run`, printed once: later infos say `<as run>`.
    argv: Value,
    lines: Vec<String>,
}

impl Log {
    fn rec(&mut self, line: String) {
        self.lines.push(line);
    }

    fn step(&mut self, name: &str) {
        self.rec(format!("== {name}"));
    }

    fn json(&self, v: &Value) -> String {
        fn same_argv(v: &mut Value, argv: &Value) {
            match v {
                Value::Object(m) => {
                    for (k, x) in m.iter_mut() {
                        if k == "argv" && x == argv {
                            *x = json!("<as run>");
                        } else {
                            same_argv(x, argv);
                        }
                    }
                }
                Value::Array(a) => a.iter_mut().for_each(|x| same_argv(x, argv)),
                _ => {}
            }
        }
        let mut v = self.paths.value(None, v);
        same_argv(&mut v, &self.argv);
        serde_json::to_string(&v).unwrap()
    }

    /// Status-stream notifications of this terminal, in order, up to and
    /// including the one that makes every check in `all` seen at least once.
    fn status_collect(&mut self, what: &str, all: &[&dyn Fn(&Value) -> bool]) -> Vec<Value> {
        let deadline = Instant::now() + STEP;
        let mut seen = vec![false; all.len()];
        let mut got = Vec::new();
        while seen.contains(&false) {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self
                .status
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("status stream: {what} not seen ({e}); got {got:?}"));
            let (seq, kind, mut v) = match ev {
                SubscribeEvent::Status(StatusStreamEvent::Changed(e)) => {
                    (e.seq, "status.changed", serde_json::to_value(&*e).unwrap())
                }
                SubscribeEvent::Status(StatusStreamEvent::Lagged(n)) => {
                    panic!("status stream lagged ({n} missed)")
                }
                SubscribeEvent::Terminal(e) => (
                    e.seq,
                    "terminal.changed",
                    serde_json::to_value(&*e).unwrap(),
                ),
                _ => continue,
            };
            assert!(
                seq > self.status_seq,
                "status seq {seq} after {}",
                self.status_seq
            );
            self.status_seq = seq;
            if v["id"] != json!(self.id) {
                continue;
            }
            for (s, check) in seen.iter_mut().zip(all) {
                *s |= check(&v);
            }
            v["notification"] = json!(kind);
            got.push(v);
        }
        got
    }

    fn rec_status(&mut self, v: &Value) {
        let mut v = v.clone();
        let kind = v["notification"].take();
        v.as_object_mut().unwrap().remove("notification");
        let line = format!("status {} {}", kind.as_str().unwrap(), self.json(&v));
        self.rec(line);
    }

    /// Status-stream notifications of this terminal up to and including the
    /// first for which `last` holds.
    fn status_until(&mut self, what: &str, last: impl Fn(&Value) -> bool) {
        for v in self.status_collect(what, &[&last]) {
            self.rec_status(&v);
        }
    }

    /// Attach-stream notifications until every marker in `want` was seen
    /// (`None`: until the stream ends). Output is recorded as its OSC 7501
    /// reports and the markers not seen in an earlier step, so a repaint or
    /// how the bytes are split into notifications does not show.
    fn attach_until(&mut self, want: Option<&[&'static str]>) {
        let rx = self.attach.take().expect("attached");
        let deadline = Instant::now() + STEP;
        let mut text = String::new();
        let mut reports = Vec::new();
        let flush = |log: &mut Log, text: &mut String, reports: &mut Vec<String>| {
            let mut found: Vec<(usize, &'static str)> = MARKERS
                .iter()
                .filter(|m| !log.seen.contains(*m))
                .filter_map(|m| text.find(m).map(|i| (i, *m)))
                .collect();
            found.sort();
            let mut parts = std::mem::take(reports);
            for (_, m) in found {
                log.seen.insert(m);
                parts.push(format!("text {m}"));
            }
            text.clear();
            if !parts.is_empty() {
                log.rec(format!("attach output {}", parts.join(", ")));
            }
        };
        loop {
            if let Some(want) = want {
                // Markers are matched over the step's whole text: one can be
                // split across notifications.
                if want
                    .iter()
                    .all(|m| self.seen.contains(m) || text.contains(m))
                {
                    flush(self, &mut text, &mut reports);
                    break;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = rx.recv_timeout(left).unwrap_or_else(|e| {
                panic!("attach stream: {want:?} not seen ({e}); text {text:?}")
            });
            match ev {
                Attach::Event(AttachEvent::Output { seq, data }) => {
                    assert_eq!(seq, self.offset, "output continues where the last ended");
                    self.offset += data.len() as u64;
                    self.cut.extend_from_slice(&data);
                    let (toks, used) = tokens(&self.cut);
                    self.cut.drain(..used);
                    for t in toks {
                        if let Some(r) = t.strip_prefix("osc:") {
                            // Text before a report belongs before it.
                            flush(self, &mut text, &mut reports);
                            reports.push(format!("osc {r}"));
                        } else if let Some(x) = t.strip_prefix("text:") {
                            text.push_str(x);
                        }
                    }
                }
                Attach::Event(AttachEvent::Exited(e)) => {
                    flush(self, &mut text, &mut reports);
                    assert_eq!(e.seq, self.offset, "exited at the end of the output");
                    let v = serde_json::to_value(&e).unwrap();
                    let line = format!("attach terminal.exited {}", self.json(&v));
                    self.rec(line);
                }
                Attach::Event(other) => {
                    flush(self, &mut text, &mut reports);
                    self.rec(format!("attach {other:?}"));
                }
                Attach::End => {
                    flush(self, &mut text, &mut reports);
                    self.rec("attach end".into());
                    assert!(want.is_none(), "attach stream ended early");
                    return;
                }
            }
        }
        self.attach = Some(rx);
    }
}

fn state_is(label: &'static str) -> impl Fn(&Value) -> bool {
    move |v| {
        v.get("status").is_some_and(|s| {
            let st: StatusInfo = serde_json::from_value(s.clone()).unwrap();
            st.state_label() == label
        }) && v["removed"] != json!(true)
    }
}

fn change_is(change: &'static str) -> impl Fn(&Value) -> bool {
    move |v| v["change"] == json!(change)
}

fn go(dir: &Path, n: u32) {
    std::fs::write(dir.join(format!("go-{n}")), b"").unwrap();
}

fn check(got: &[String]) {
    let text = got.join("\n") + "\n";
    if let Some(p) = std::env::var_os(OUT_ENV) {
        std::fs::write(&p, &text).unwrap();
        eprintln!("recorded {}", PathBuf::from(p).display());
        return;
    }
    // The raw string starts with a line break (CRLF in an autocrlf checkout).
    let want: Vec<&str> = GOLDEN.trim_start_matches(['\r', '\n']).lines().collect();
    let have: Vec<&str> = text.lines().collect();
    if want == have {
        return;
    }
    let first = want
        .iter()
        .zip(&have)
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(have.len()));
    let dump = std::env::temp_dir().join(format!("{}.txt", unique("stream-order-got")));
    let _ = std::fs::write(&dump, &text);
    panic!(
        "stream order differs at line {} ({} vs {} lines; got written to {})\n\
         want: {}\n got: {}",
        first + 1,
        want.len(),
        have.len(),
        dump.display(),
        want.get(first).unwrap_or(&"<end>"),
        have.get(first).unwrap_or(&"<end>"),
    );
}

// ───────────────────────────── the test ─────────────────────────────

#[test]
fn one_terminal_lifecycle_streams_in_order() {
    let d = Daemon::start("stream-order");
    let dir = std::env::temp_dir().join(unique("stream-order-go"));
    std::fs::create_dir_all(&dir).unwrap();
    let paths = Paths::new(&[
        (dir.as_path(), "<GO>"),
        (d.state_dir.as_path(), "<STATE>"),
        (profile_dir().as_path(), "<BIN>"),
        (std::env::temp_dir().as_path(), "<TMP>"),
    ]);

    let (snap, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams {
            lifecycle: true,
            ..Default::default()
        })
        .expect("subscribe");
    let (tx, status) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });
    let mut log = Log {
        id: String::new(),
        paths,
        status,
        status_seq: snap.seq,
        attach: None,
        offset: 0,
        cut: Vec::new(),
        seen: BTreeSet::new(),
        argv: Value::Null,
        lines: Vec::new(),
    };
    log.step("status.subscribe");
    log.rec(format!(
        "terminals {} gates {}",
        snap.terminals.len(),
        snap.gates.len()
    ));

    // The agent: reports and text, each step after its go file.
    let go_file = |n: u32| format!("wait-file:{}", dir.join(format!("go-{n}")).display());
    let mut argv = vec![fake_agent_exe().display().to_string()];
    argv.extend([
        "osc:state=idle:app=fake".to_string(),
        "print:READY".into(),
        go_file(1),
        "osc:state=working:app=fake".into(),
        "print:WORKING".into(),
        go_file(2),
        // base64("Proceed?") = UHJvY2VlZD8=
        "osc:state=blocked:app=fake:kind=question:msg=UHJvY2VlZD8=".into(),
        "print:ASKING".into(),
        go_file(3),
        "osc:state=done:app=fake".into(),
        "print:DONE".into(),
        go_file(4),
        "exit:3".into(),
    ]);

    let mut c = d.client();
    log.step("terminal.run");
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv,
            cwd: cwd(),
            name: Some("streams".into()),
            ..Default::default()
        })
        .expect("run");
    log.id = t.id.clone();
    let v = without_status(serde_json::to_value(&t).unwrap());
    let line = format!("rpc {}", log.json(&v));
    log.rec(line);
    log.argv = log.paths.value(None, &json!(t.argv));
    // `terminal.changed created` is published once the terminal is listed,
    // the agent's first report as soon as its holder streams: either can be
    // first, and an `unknown` status may come before the report. So: wait
    // for both, leave out `unknown`, `created` first, its live status masked.
    let mut got = log.status_collect(
        "created and idle",
        &[&change_is("created"), &state_is("idle")],
    );
    got.retain(|v| v["status"]["state"] != json!("unknown"));
    got.sort_by_key(|v| v["notification"] != json!("terminal.changed"));
    for mut v in got {
        if let Some(t) = v.get_mut("terminal") {
            *t = without_status(t.take());
        }
        if v["from"] == json!("unknown") {
            v.as_object_mut().unwrap().remove("from");
        }
        log.rec_status(&v);
    }

    // Attach once the agent's first text is on the screen.
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::id(&t.id),
            until: WaitUntil::OutputContains("READY".into()),
            timeout_ms: Some(STEP.as_millis() as u64),
        })
        .expect("wait");
    assert_eq!(w.outcome, WaitOutcome::Matched, "READY not printed");
    log.step("terminal.attach");
    let (a, screen, mut stream) = d
        .client()
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .expect("attach");
    log.offset = a.seq;
    let text: String = tokens(&screen)
        .0
        .iter()
        .filter_map(|t| t.strip_prefix("text:"))
        .collect();
    let shown: Vec<&str> = text
        .split(['\r', '\n'])
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    for m in MARKERS {
        if text.contains(m) {
            log.seen.insert(m);
        }
    }
    let v = serde_json::to_value(&a.terminal).unwrap();
    let line = format!("rpc terminal {} screen {shown:?}", log.json(&v));
    log.rec(line);
    let (tx, attach) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let ev = match stream.next_event() {
                Ok(Some(ev)) => Attach::Event(ev),
                _ => Attach::End,
            };
            let end = matches!(ev, Attach::End);
            if tx.send(ev).is_err() || end {
                break;
            }
        }
    });
    log.attach = Some(attach);

    log.step("go 1: working");
    go(&dir, 1);
    log.attach_until(Some(&["WORKING"]));
    log.status_until("working", state_is("working"));

    log.step("go 2: blocked");
    go(&dir, 2);
    log.attach_until(Some(&["ASKING"]));
    log.status_until("blocked", state_is("blocked:question"));

    log.step("go 3: done");
    go(&dir, 3);
    log.attach_until(Some(&["DONE"]));
    log.status_until("done", state_is("done"));

    log.step("terminal.ack");
    let acked = c.call::<TerminalAck>(&TerminalRef::id(&t.id)).expect("ack");
    let v = serde_json::to_value(&acked).unwrap();
    let line = format!("rpc {}", log.json(&v));
    log.rec(line);
    log.status_until("idle after ack", state_is("idle"));

    log.step("go 4: exit 3");
    go(&dir, 4);
    log.attach_until(None);
    // The attach stream ends right after `terminal.exited`, racing the info
    // `terminal.changed exited` carries: its `attached` is 1 or 0.
    for mut v in log.status_collect("exited", &[&change_is("exited")]) {
        if let Some(n) = v.pointer_mut("/terminal/attached") {
            *n = json!("<live>");
        }
        log.rec_status(&v);
    }

    log.step("terminal.rm");
    let rm = c
        .call::<TerminalRm>(&RmParams {
            target: TerminalRef::id(&t.id),
            force: false,
        })
        .expect("rm");
    let v = serde_json::to_value(&rm).unwrap();
    let line = format!("rpc {}", log.json(&v));
    log.rec(line);
    log.status_until("removed", change_is("removed"));

    let _ = std::fs::remove_dir_all(&dir);
    check(&log.lines);
}

const GOLDEN: &str = r##"
== status.subscribe
terminals 0 gates 0
== terminal.run
rpc {"argv":["<BIN>\\examples/fake-agent.exe","osc:state=idle:app=fake","print:READY","wait-file:<GO>\\go-1","osc:state=working:app=fake","print:WORKING","wait-file:<GO>\\go-2","osc:state=blocked:app=fake:kind=question:msg=UHJvY2VlZD8=","print:ASKING","wait-file:<GO>\\go-3","osc:state=done:app=fake","print:DONE","wait-file:<GO>\\go-4","exit:3"],"attached":0,"cols":120,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<TMP>\\","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"streams","pid":"<n>","rows":30,"state":"running","status":"<live>"}
status terminal.changed {"change":"created","id":"t1","seq":"<n>","terminal":{"argv":"<as run>","attached":0,"cols":120,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<TMP>\\","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"streams","pid":"<n>","rows":30,"state":"running","status":"<live>"}}
status status.changed {"id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"idle","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":0}}
== terminal.attach
rpc terminal {"argv":"<as run>","attached":1,"cols":120,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<TMP>\\","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"streams","pid":"<n>","rows":30,"state":"running","status":{"agent":"fake","app":"fake","badge":"idle","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":0}} screen ["READY"]
== go 1: working
attach output osc state=working:app=fake, text WORKING
status status.changed {"from":"idle","id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"working","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"working","turn":0}}
== go 2: blocked
attach output osc state=blocked:app=fake:kind=question:msg=UHJvY2VlZD8=, text ASKING
status status.changed {"from":"working","id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"blocked:question","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"kind":"question","msg":"Proceed?","records":0,"running_children":0,"since":"<n>","source":"osc","state":"blocked","turn":1}}
== go 3: done
attach output osc state=done:app=fake, text DONE
status status.changed {"from":"blocked:question","id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"done","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"done","turn":1}}
== terminal.ack
rpc {"argv":"<as run>","attached":1,"cols":120,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<TMP>\\","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"streams","pid":"<n>","rows":30,"state":"running","status":{"agent":"fake","app":"fake","badge":"idle","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":1}}
status status.changed {"from":"done","id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"idle","blocked_children":0,"confidence":0.8999999761581421,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":1}}
== go 4: exit 3
attach terminal.exited {"exit_code":3,"id":"t1","killed":false,"seq":"<n>"}
attach end
status status.changed {"from":"idle","id":"t1","removed":false,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"exited","blocked_children":0,"confidence":1.0,"final_state":"idle","hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"process","state":"exited","turn":1}}
status terminal.changed {"change":"exited","id":"t1","seq":"<n>","terminal":{"argv":"<as run>","attached":"<live>","cols":120,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<TMP>\\","exit_code":3,"exited_ms":"<n>","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"streams","pid":"<n>","rows":30,"state":"exited","status":{"agent":"fake","app":"fake","badge":"exited","blocked_children":0,"confidence":1.0,"final_state":"idle","hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"process","state":"exited","turn":1}}}
== terminal.rm
rpc {"id":"t1"}
status status.changed {"id":"t1","removed":true,"seq":"<n>","status":{"agent":"fake","app":"fake","badge":"exited","blocked_children":0,"confidence":1.0,"final_state":"idle","hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"process","state":"exited","turn":1}}
status terminal.changed {"change":"removed","id":"t1","seq":"<n>"}
"##;
