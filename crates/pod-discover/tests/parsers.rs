//! Parser and scan tests over synthetic fixtures (tests/fixtures) that mirror
//! the structure of the agents' stores. No real session content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use pod_discover::claude::ClaudeSource;
use pod_discover::codex::CodexSource;
use pod_discover::time::parse_rfc3339_ms;
use pod_discover::{Agent, Budget, Cache, FileStamp, Found, Limits, SessionMeta, Source, scan};

const C1: &str = "11111111-2222-4333-8444-555555555555";
const C3: &str = "33333333-2222-4333-8444-555555555555";
const X1: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const X2: &str = "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const X4: &str = "dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn sources(root: &Path) -> Vec<Box<dyn Source>> {
    vec![
        Box::new(ClaudeSource::new(root.join("claude"))),
        Box::new(CodexSource::new(root.join("codex"))),
    ]
}

fn find<'a>(all: &'a [Found], agent: Agent, id: &str) -> &'a Found {
    all.iter()
        .find(|f| f.agent == agent && f.meta.id == id)
        .unwrap_or_else(|| panic!("{id} not found"))
}

fn ms(s: &str) -> Option<u64> {
    parse_rfc3339_ms(s)
}

/// Copy the fixtures to a scratch directory (tests that change files).
fn copy_fixtures(tag: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), t).unwrap();
            }
        }
    }
    let dir = std::env::temp_dir().join(
        format!(
            "pod-discover-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        )
        .replace([':', ' '], "_"),
    );
    copy(&fixtures(), &dir);
    dir
}

#[test]
fn claude_fixture_metadata() {
    let mut src = sources(&fixtures());
    let out = scan(&mut src, &mut Cache::default(), &Limits::default());
    let claude: Vec<&Found> = out
        .sessions
        .iter()
        .filter(|f| f.agent == Agent::Claude)
        .collect();
    // Metadata-only session, non-uuid file and sub-agent transcript are not sessions.
    assert_eq!(claude.len(), 2, "{claude:#?}");
    let s = &find(&out.sessions, Agent::Claude, C1).meta;
    assert_eq!(s.cwd, r"C:\src\demo");
    assert_eq!(s.title.as_deref(), Some("Fix login bug"));
    assert_eq!(
        s.preview.as_deref(),
        Some("Fix the login bug in auth.rs please")
    );
    assert_eq!(s.model.as_deref(), Some("claude-test-model"));
    assert_eq!(s.branch.as_deref(), Some("feature/login"));
    assert_eq!(s.version.as_deref(), Some("2.1.0"));
    assert_eq!(s.origin.as_deref(), Some("cli"));
    assert_eq!(s.started_ms, ms("2026-11-01T09:59:59Z"));
    assert_eq!(s.last_ms, ms("2026-11-01T10:05:00Z"));
    // Approximate: every user/assistant line, including the cut last line.
    assert_eq!(s.messages, Some(6));
    let s3 = &find(&out.sessions, Agent::Claude, C3).meta;
    assert_eq!(s3.title.as_deref(), Some("My renamed session"));
    assert_eq!(s3.preview.as_deref(), Some("Write the docs"));
    assert_eq!(s3.cwd, r"C:\src\other");
    // Running-process records: garbage and wrong types are skipped, ids lower-cased.
    assert_eq!(out.running_ids[&Agent::Claude].len(), 1);
    assert_eq!(out.running_ids[&Agent::Claude][&4242], C1);
}

#[test]
fn codex_fixture_metadata() {
    let mut src = sources(&fixtures());
    let out = scan(&mut src, &mut Cache::default(), &Limits::default());
    let codex: Vec<&str> = out
        .sessions
        .iter()
        .filter(|f| f.agent == Agent::Codex)
        .map(|f| f.meta.id.as_str())
        .collect();
    // Sub-agent thread, archived session and non-rollout files are left out.
    assert_eq!(codex.len(), 3, "{codex:?}");
    let s = &find(&out.sessions, Agent::Codex, X1).meta;
    assert_eq!(s.cwd, r"C:\src\demo");
    assert_eq!(s.title.as_deref(), Some("Dark mode"));
    assert_eq!(s.preview.as_deref(), Some("Add a dark mode toggle"));
    assert_eq!(s.model.as_deref(), Some("gpt-test"));
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.version.as_deref(), Some("0.159.0"));
    assert_eq!(s.origin.as_deref(), Some("cli"));
    assert_eq!(s.started_ms, ms("2026-11-02T00:00:00Z"));
    assert_eq!(s.last_ms, ms("2026-11-02T00:10:00Z"));
    assert_eq!(s.messages, Some(3));
    let s2 = &find(&out.sessions, Agent::Codex, X2).meta;
    assert_eq!(s2.preview.as_deref(), Some("Refactor the parser"));
    assert_eq!(s2.model.as_deref(), Some("gpt-new"));
    assert_eq!(s2.branch.as_deref(), Some("feat/x"));
    assert_eq!(s2.origin.as_deref(), Some("vscode"));
    assert_eq!(s2.messages, Some(2));
    // Format drift: no id in session_meta (id from the file name), BOM, cwd from turn_context.
    let s4 = &find(&out.sessions, Agent::Codex, X4).meta;
    assert_eq!(s4.cwd, r"D:\work");
    assert_eq!(s4.model.as_deref(), Some("m"));
    assert_eq!(s4.title, None);
}

#[test]
fn newest_first() {
    let out = scan(
        &mut sources(&fixtures()),
        &mut Cache::default(),
        &Limits::default(),
    );
    let times: Vec<u64> = out
        .sessions
        .iter()
        .map(pod_discover::last_activity)
        .collect();
    assert!(times.windows(2).all(|w| w[0] >= w[1]), "{times:?}");
}

#[test]
fn previews_off_stores_none_and_rereads_when_on() {
    let mut cache = Cache::default();
    let off = Limits {
        preview: false,
        ..Limits::default()
    };
    let out = scan(&mut sources(&fixtures()), &mut cache, &off);
    assert!(out.sessions.iter().all(|s| s.meta.preview.is_none()));
    assert!(
        cache
            .files
            .values()
            .filter_map(|c| c.meta.as_ref())
            .all(|m| m.preview.is_none())
    );
    // Turning previews on re-reads files cached without them.
    let out = scan(&mut sources(&fixtures()), &mut cache, &Limits::default());
    assert_eq!(out.cached, 0);
    assert!(out.sessions.iter().any(|s| s.meta.preview.is_some()));
    cache.strip_previews();
    assert!(
        cache
            .files
            .values()
            .filter_map(|c| c.meta.as_ref())
            .all(|m| m.preview.is_none())
    );
}

#[test]
fn incremental_by_size_and_mtime() {
    let root = copy_fixtures("incr");
    let mut cache = Cache::default();
    let first = scan(&mut sources(&root), &mut cache, &Limits::default());
    assert!(first.parsed > 0);
    let second = scan(&mut sources(&root), &mut cache, &Limits::default());
    assert_eq!(second.parsed, 0, "unchanged files are not re-read");
    assert_eq!(second.sessions, first.sessions);
    // The agent appends a line: only that file is re-read.
    let f = root
        .join("codex")
        .join("sessions")
        .join("2026")
        .join("11")
        .join("03")
        .join(format!("rollout-2026-11-03T09-00-00-{X2}.jsonl"));
    let mut body = std::fs::read(&f).unwrap();
    body.extend_from_slice(b"{\"timestamp\":\"2026-11-05T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"more\"}}\n");
    std::fs::write(&f, body).unwrap();
    let third = scan(&mut sources(&root), &mut cache, &Limits::default());
    assert_eq!(third.parsed, 1);
    assert_eq!(
        find(&third.sessions, Agent::Codex, X2).meta.messages,
        Some(3)
    );
    // A deleted file leaves the cache and the list.
    std::fs::remove_file(&f).unwrap();
    let fourth = scan(&mut sources(&root), &mut cache, &Limits::default());
    assert!(!fourth.sessions.iter().any(|s| s.meta.id == X2));
    assert!(!cache.files.keys().any(|k| k.contains(X2)));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn limits_bound_the_scan() {
    let one = Limits {
        max_files: 1,
        ..Limits::default()
    };
    let out = scan(&mut sources(&fixtures()), &mut Cache::default(), &one);
    assert!(out.truncated);
    assert!(
        out.sessions
            .iter()
            .filter(|s| s.agent == Agent::Codex)
            .count()
            <= 1
    );
    // A tiny read budget: nothing parsed, marked truncated, no panic.
    let tiny = Limits {
        read_bytes: 64,
        ..Limits::default()
    };
    let out = scan(&mut sources(&fixtures()), &mut Cache::default(), &tiny);
    assert!(out.truncated);
    assert!(out.bytes_read <= 64);
    // Small head window: a session is still found from the head + tail.
    let small = Limits {
        head_bytes: 600,
        head_lines: 3,
        tail_bytes: 300,
        count_max_bytes: 10,
        ..Limits::default()
    };
    let out = scan(&mut sources(&fixtures()), &mut Cache::default(), &small);
    let s = &find(&out.sessions, Agent::Codex, X1).meta;
    assert_eq!(s.messages, None, "over count_max_bytes");
    assert_eq!(s.last_ms, ms("2026-11-02T00:10:00Z"), "from the tail");
    // Listing bound.
    let few = Limits {
        max_listed: 2,
        ..Limits::default()
    };
    let src = CodexSource::new(fixtures().join("codex"));
    assert!(src.list(&few).len() <= 2);
}

#[test]
fn garbage_files_never_panic() {
    let root = std::env::temp_dir().join(format!("pod-discover-garbage-{}", std::process::id()));
    let claude = root.join("claude").join("projects").join("p");
    let codex = root
        .join("codex")
        .join("sessions")
        .join("2026")
        .join("01")
        .join("01");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir_all(root.join("claude").join("sessions")).unwrap();
    // Deterministic pseudo-random bytes with JSON-ish fragments.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let pieces: [&[u8]; 12] = [
        b"{",
        b"}",
        b"\"type\":",
        b"\"user\"",
        b"\"session_meta\"",
        b"\"payload\":{",
        b"\n",
        b"\r\n",
        b"\xff\xfe",
        b"\"cwd\":\"C:\\\\x\"",
        b"[",
        b"\"timestamp\":\"2026-99-99T",
    ];
    for i in 0..20 {
        let mut body = Vec::new();
        for _ in 0..(next() % 400) {
            let r = next();
            if r % 3 == 0 {
                body.push((r >> 8) as u8);
            } else {
                body.extend_from_slice(pieces[(r % 12) as usize]);
            }
        }
        std::fs::write(
            claude.join(format!("{:08x}-0000-4000-8000-{:012x}.jsonl", i, i)),
            &body,
        )
        .unwrap();
        std::fs::write(
            codex.join(format!(
                "rollout-x-{:08x}-0000-4000-8000-{:012x}.jsonl",
                i, i
            )),
            &body,
        )
        .unwrap();
        std::fs::write(
            root.join("claude")
                .join("sessions")
                .join(format!("{i}.json")),
            &body,
        )
        .unwrap();
    }
    std::fs::write(
        root.join("codex").join("session_index.jsonl"),
        b"\xff{{{\n[1,2]\nnull\n",
    )
    .unwrap();
    let out = scan(
        &mut sources(&root),
        &mut Cache::default(),
        &Limits::default(),
    );
    for s in &out.sessions {
        assert!(!s.meta.id.is_empty() && !s.meta.cwd.is_empty());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_roots_are_empty() {
    let root = Path::new(r"C:\pod-discover-does-not-exist");
    let out = scan(
        &mut sources(root),
        &mut Cache::default(),
        &Limits::default(),
    );
    assert!(out.sessions.is_empty() && !out.truncated);
    let mut b = Budget::new(&Limits::default());
    assert!(
        ClaudeSource::new(root.into())
            .running_sessions(&mut b)
            .is_empty()
    );
}

#[test]
fn counts_messages_in_spaced_json() {
    // Regression: `"type": "user"` (a space after the colon) was counted as 0.
    let root = std::env::temp_dir().join(format!("pod-discover-spaced-{}", std::process::id()));
    let dir = root.join("claude").join("projects").join("p");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "66666666-2222-4333-8444-555555555555";
    let body = concat!(
        r#"{"type": "user", "cwd": "C:\\x", "message": {"role": "user", "content": "hi"}}"#,
        "\n",
        r#"{"type": "assistant", "message": {"model": "m"}}"#,
        "\n"
    );
    std::fs::write(dir.join(format!("{id}.jsonl")), body).unwrap();
    let out = scan(
        &mut sources(&root),
        &mut Cache::default(),
        &Limits::default(),
    );
    assert_eq!(
        find(&out.sessions, Agent::Claude, id).meta.messages,
        Some(2)
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Scan orchestration over a synthetic store whose read costs are exact:
// budget split, newest-first counting, de-duplication and cache reuse.

/// Bytes [`Fake::parse`] takes from the budget (a head read).
const PARSE_COST: u64 = 100;
/// Bytes [`Fake::running_sessions`] takes from its budget.
const RUNNING_COST: u64 = 700;

/// A store of `*.s` files whose first line is `<id> <last_ms>`; lines with
/// `msg` are messages.
struct Fake {
    agent: Agent,
    root: PathBuf,
    /// Records pid 7 as running `<agent>-run`.
    running: bool,
}

fn fake(agent: Agent, root: PathBuf, running: bool) -> Box<dyn Source> {
    Box::new(Fake {
        agent,
        root,
        running,
    })
}

impl Source for Fake {
    fn agent(&self) -> Agent {
        self.agent
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn list(&self, _limits: &Limits) -> Vec<FileStamp> {
        std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| FileStamp::of(e.path()))
            .collect()
    }

    fn parse(&self, file: &FileStamp, limits: &Limits, budget: &mut Budget) -> Option<SessionMeta> {
        if !budget.take(PARSE_COST) {
            return None;
        }
        let text = std::fs::read_to_string(&file.path).ok()?;
        let mut words = text.split_whitespace();
        Some(SessionMeta {
            id: words.next()?.to_string(),
            cwd: r"C:\w".into(),
            last_ms: words.next()?.parse().ok(),
            preview: limits.preview.then(|| "first prompt".to_string()),
            ..SessionMeta::default()
        })
    }

    fn message_markers(&self) -> &'static [&'static [u8]] {
        &[b"msg"]
    }

    fn running_sessions(&self, budget: &mut Budget) -> HashMap<u32, String> {
        if self.running && budget.take(RUNNING_COST) {
            HashMap::from([(7, format!("{}-run", self.agent.as_str()))])
        } else {
            HashMap::new()
        }
    }
}

/// A fresh scratch directory.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(
        format!(
            "pod-discover-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        )
        .replace([':', ' '], "_"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `<dir>/<id>.s`, see [`put_as`].
fn put(dir: &Path, id: &str, last_ms: u64, size: usize, mtime_ms: u64) {
    put_as(dir, id, id, last_ms, size, mtime_ms);
}

/// A [`Fake`] session file `<dir>/<file>.s` of exactly `size` bytes with one
/// message line.
fn put_as(dir: &Path, file: &str, id: &str, last_ms: u64, size: usize, mtime_ms: u64) {
    let mut body = format!("{id} {last_ms} msg\n").into_bytes();
    assert!(body.len() < size, "{id}: {size} bytes is too small");
    body.resize(size - 1, b'x');
    body.push(b'\n');
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{file}.s"));
    std::fs::write(&path, body).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms))
        .unwrap();
}

/// (agent, id, last activity, messages) of a found session.
type Row = (Agent, String, Option<u64>, Option<u64>);

fn rows(out: &[Found]) -> Vec<Row> {
    out.iter()
        .map(|f| (f.agent, f.meta.id.clone(), f.meta.last_ms, f.meta.messages))
        .collect()
}

fn row(agent: Agent, id: &str, last: u64, messages: Option<u64>) -> Row {
    (agent, id.to_string(), Some(last), messages)
}

#[test]
fn metadata_budget_is_split_between_sources() {
    let root = scratch("share");
    let (a, b) = (root.join("a"), root.join("b"));
    put(&a, "a1", 300, 50, 3000);
    put(&a, "a2", 200, 50, 2000);
    put(&a, "a3", 100, 50, 1000);
    put(&b, "b1", 50, 50, 500);
    let mut sources = vec![fake(Agent::Claude, a, true), fake(Agent::Codex, b, false)];
    // 1000 bytes: each source may spend 1000 / (2 sources * 2 phases) = 250
    // on metadata, so the first store reads 2 of its 3 files and cannot take
    // the second store's share; the counts share the other 500.
    let limits = Limits {
        read_bytes: 1000,
        ..Limits::default()
    };
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &limits);
    assert_eq!(
        rows(&out.sessions),
        [
            row(Agent::Claude, "a1", 300, Some(1)),
            row(Agent::Claude, "a2", 200, Some(1)),
            row(Agent::Codex, "b1", 50, Some(1)),
        ]
    );
    assert!(out.truncated);
    assert_eq!((out.parsed, out.cached), (3, 0));
    // The pid records get the whole budget, not a share.
    assert_eq!(
        out.running_ids[&Agent::Claude],
        HashMap::from([(7, "claude-run".to_string())])
    );
    assert!(out.running_ids[&Agent::Codex].is_empty());
    assert_eq!(out.bytes_read, 3 * PARSE_COST + 3 * 50 + RUNNING_COST);
    // A file never read is not remembered.
    assert_eq!(cache.files.len(), 3);
    assert!(!cache.files.keys().any(|k| k.ends_with("a3.s")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn one_session_per_agent_and_id_keeps_the_newest_activity() {
    let root = scratch("dedupe");
    let (a, b) = (root.join("a"), root.join("b"));
    // A copy of a session: the newer file holds the older activity.
    put_as(&a, "dup-copy", "dup", 100, 50, 5000);
    put(&a, "dup", 200, 50, 1000);
    put(&a, "solo", 150, 50, 3000);
    // The same id under another agent is another session.
    put(&b, "dup", 50, 50, 1000);
    let mut sources = vec![
        fake(Agent::Claude, a.clone(), false),
        fake(Agent::Codex, b, false),
    ];
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &Limits::default());
    assert_eq!(
        rows(&out.sessions),
        [
            row(Agent::Claude, "dup", 200, Some(1)),
            row(Agent::Claude, "solo", 150, Some(1)),
            row(Agent::Codex, "dup", 50, Some(1)),
        ]
    );
    assert_eq!(out.sessions[0].path, a.join("dup.s"));
    // Both copies stay cached.
    assert_eq!(cache.files.len(), 4);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn counts_go_newest_first_and_the_rest_is_retried() {
    let root = scratch("recount");
    let dir = root.join("a");
    put(&dir, "f1", 2000, 300, 2000);
    put(&dir, "f2", 1000, 300, 1000);
    let mut sources = vec![fake(Agent::Claude, dir, false)];
    // One source: metadata 500, counts 500 (room for one 300-byte file).
    let limits = Limits {
        read_bytes: 1000,
        ..Limits::default()
    };
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &limits);
    assert_eq!(
        rows(&out.sessions),
        [
            row(Agent::Claude, "f1", 2000, Some(1)),
            row(Agent::Claude, "f2", 1000, None),
        ]
    );
    assert!(out.truncated);
    assert_eq!((out.parsed, out.cached), (2, 0));
    assert_eq!(out.bytes_read, 2 * PARSE_COST + 300);
    let counted: Vec<bool> = {
        let mut v: Vec<_> = cache.files.iter().collect();
        v.sort_by(|x, y| x.0.cmp(y.0));
        v.iter().map(|(_, c)| c.counted).collect()
    };
    assert_eq!(counted, [true, false]);
    // The next scan reuses both entries and counts what did not fit.
    let out = scan(&mut sources, &mut cache, &limits);
    assert_eq!(
        rows(&out.sessions),
        [
            row(Agent::Claude, "f1", 2000, Some(1)),
            row(Agent::Claude, "f2", 1000, Some(1)),
        ]
    );
    assert!(!out.truncated);
    assert_eq!((out.parsed, out.cached), (0, 2));
    assert_eq!(out.bytes_read, 300);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_changed_file_the_budget_skips_keeps_its_cached_entry() {
    let root = scratch("keep");
    let dir = root.join("a");
    put(&dir, "f1", 2000, 60, 2000);
    put(&dir, "f2", 1000, 60, 1000);
    let mut sources = vec![fake(Agent::Claude, dir.clone(), false)];
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &Limits::default());
    assert_eq!(out.parsed, 2);
    // The agent rewrites f1.
    put(&dir, "f1", 3000, 80, 4000);
    let old = [
        row(Agent::Claude, "f1", 2000, Some(1)),
        row(Agent::Claude, "f2", 1000, Some(1)),
    ];
    let f1 = dir.join("f1.s").display().to_string();
    // Out of bytes, then out of time: f1 is not re-read and keeps what the
    // first scan knew.
    for limits in [
        Limits {
            read_bytes: 100,
            ..Limits::default()
        },
        Limits {
            time: Duration::ZERO,
            ..Limits::default()
        },
    ] {
        let out = scan(&mut sources, &mut cache, &limits);
        assert_eq!(rows(&out.sessions), old);
        assert!(out.truncated);
        assert_eq!((out.parsed, out.cached, out.bytes_read), (0, 1, 0));
        assert_eq!(cache.files[&f1].size, 60);
    }
    // With budget again it is re-read and re-counted.
    let out = scan(&mut sources, &mut cache, &Limits::default());
    assert_eq!(
        rows(&out.sessions),
        [
            row(Agent::Claude, "f1", 3000, Some(1)),
            row(Agent::Claude, "f2", 1000, Some(1)),
        ]
    );
    assert_eq!((out.parsed, out.cached), (1, 1));
    assert_eq!(cache.files[&f1].size, 80);
    let _ = std::fs::remove_dir_all(&root);
}
