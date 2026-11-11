//! Wire snapshots (docs/20-refactor/libs.md, L8): the serialized forms of
//! the discovery types, and a scan plus process match over fixed inputs,
//! recorded before scan and procs were split into modules. JSON is compared
//! as values: object key order is ignored; array order, null vs missing,
//! enum strings and error messages must match exactly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use pod_discover::claude::ClaudeSource;
use pod_discover::codex::CodexSource;
use pod_discover::procs::{ProcInfo, attach_sessions, find_agents, mark_pod_terminals};
use pod_discover::{
    Agent, Cache, CachedFile, Found, Limits, RunningAgent, SessionMeta, Source, scan,
};
use serde_json::{Value, json};

const C1: &str = "11111111-2222-4333-8444-555555555555";
const C3: &str = "33333333-2222-4333-8444-555555555555";

/// Serialized as the daemon writes it, read back as a value.
fn wire<T: serde::Serialize>(v: &T) -> Value {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

fn same(what: &str, actual: &Value, golden: &str) {
    let want: Value =
        serde_json::from_str(golden).unwrap_or_else(|e| panic!("{what}: bad golden: {e}"));
    assert!(
        *actual == want,
        "{what} changed; now:
{}",
        serde_json::to_string_pretty(actual).unwrap()
    );
}

fn full_meta() -> SessionMeta {
    SessionMeta {
        id: C1.into(),
        cwd: r"C:\src\demo".into(),
        title: Some("Fix login bug".into()),
        preview: Some("Fix the login bug".into()),
        model: Some("claude-test-model".into()),
        branch: Some("feature/login".into()),
        version: Some("2.1.0".into()),
        origin: Some("cli".into()),
        started_ms: Some(1),
        last_ms: Some(2),
        messages: Some(3),
    }
}

#[test]
fn session_meta_wire() {
    same("full meta", &wire(&full_meta()), FULL_META);
    same(
        "empty meta",
        &wire(&SessionMeta::default()),
        r#"{"id":"","cwd":""}"#,
    );
    // Missing and null optional fields read as None; unknown fields are ignored.
    let read: SessionMeta =
        serde_json::from_str(r#"{"id":"a","cwd":"b","title":null,"future":1}"#).unwrap();
    assert_eq!(
        read,
        SessionMeta {
            id: "a".into(),
            cwd: "b".into(),
            ..SessionMeta::default()
        }
    );
    let err = serde_json::from_str::<SessionMeta>(r#"{"cwd":"b"}"#).unwrap_err();
    assert_eq!(err.to_string(), "missing field `id` at line 1 column 11");
}

#[test]
fn cache_wire() {
    let mut cache = Cache::default();
    cache.files.insert(
        r"C:\s\a.jsonl".into(),
        CachedFile {
            agent: Agent::Claude,
            size: 10,
            mtime_ms: 20,
            with_preview: true,
            counted: true,
            meta: Some(full_meta()),
        },
    );
    cache.files.insert(
        r"C:\s\b.jsonl".into(),
        CachedFile {
            agent: Agent::Codex,
            size: 30,
            mtime_ms: 40,
            with_preview: false,
            counted: false,
            meta: None,
        },
    );
    same("cache", &wire(&cache), CACHE);
    cache.strip_previews();
    same(
        "cache without previews",
        &wire(&cache),
        CACHE_WITHOUT_PREVIEWS,
    );
    // A cache written before the flags existed reads them as false.
    let old: Cache =
        serde_json::from_str(r#"{"files":{"k":{"agent":"codex","size":1,"mtime_ms":2}}}"#).unwrap();
    same(
        "old cache",
        &wire(&old),
        r#"{"files":{"k":{"agent":"codex","size":1,"mtime_ms":2,"with_preview":false,"counted":false}}}"#,
    );
    let empty: Cache = serde_json::from_str("{}").unwrap();
    same("empty cache", &wire(&empty), r#"{"files":{}}"#);
    let err = serde_json::from_str::<Cache>(
        r#"{"files":{"k":{"agent":"gemini","size":1,"mtime_ms":2}}}"#,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown variant `gemini`, expected `claude` or `codex` at line 1 column 31"
    );
}

#[test]
fn running_agent_wire() {
    let full = RunningAgent {
        agent: Some(Agent::Codex),
        pid: 20,
        pids: vec![20, 21],
        start_ms: 100,
        cwd: Some(r"C:\src\a".into()),
        session_id: Some(C1.into()),
        guessed: true,
        pod_terminal: Some("t1".into()),
    };
    same("full agent", &wire(&full), FULL_AGENT);
    same(
        "default agent",
        &wire(&RunningAgent::default()),
        r#"{"agent":null,"pid":0,"pids":[],"start_ms":0,"guessed":false}"#,
    );
    let read: RunningAgent = serde_json::from_str(r#"{"pid":5,"start_ms":0}"#).unwrap();
    assert_eq!(
        read,
        RunningAgent {
            pid: 5,
            ..RunningAgent::default()
        }
    );
    let err = serde_json::from_str::<RunningAgent>("{}").unwrap_err();
    assert_eq!(err.to_string(), "missing field `pid` at line 1 column 2");
    same(
        "agent schema",
        &wire(&schemars::schema_for!(RunningAgent)),
        AGENT_SCHEMA,
    );
}

/// Fixed modification time of the copied fixtures (2026-01-01T00:00:00Z).
const MTIME_MS: u64 = 1_767_225_600_000;

/// The fixtures copied to a scratch directory with every file's mtime set to
/// [`MTIME_MS`].
fn fixed_fixtures() -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), &t).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&t)
                    .unwrap()
                    .set_modified(UNIX_EPOCH + Duration::from_millis(MTIME_MS))
                    .unwrap();
            }
        }
    }
    let dir = std::env::temp_dir().join(
        format!(
            "pod-discover-wire-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        )
        .replace([':', ' '], "_"),
    );
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures"),
        &dir,
    );
    dir
}

fn proc(
    pid: u32,
    ppid: u32,
    name: &str,
    start_ms: u64,
    cmd: Option<&str>,
    cwd: Option<&str>,
) -> ProcInfo {
    ProcInfo {
        pid,
        ppid,
        name: name.into(),
        start_ms,
        cmdline: cmd.map(Into::into),
        cwd: cwd.map(Into::into),
    }
}

#[test]
fn fixture_scan_and_process_match() {
    let root = fixed_fixtures();
    let rel = |p: &Path| {
        p.strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut sources: Vec<Box<dyn Source>> = vec![
        Box::new(ClaudeSource::new(root.join("claude"))),
        Box::new(CodexSource::new(root.join("codex"))),
    ];
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &Limits::default());
    let again = scan(&mut sources, &mut cache, &Limits::default());
    assert_eq!(again.sessions, out.sessions);
    let snapshot = json!({
        "sessions": out.sessions.iter().map(|f| json!({
            "agent": wire(&f.agent),
            "meta": wire(&f.meta),
            "path": rel(&f.path),
            "mtime_ms": f.mtime_ms,
        })).collect::<Vec<_>>(),
        "running_ids": wire(&out.running_ids),
        "truncated": out.truncated,
        "parsed": out.parsed,
        "cached": out.cached,
        "bytes_read": out.bytes_read,
        "cache": cache.files.iter()
            .map(|(k, v)| (rel(Path::new(k)), wire(v)))
            .collect::<serde_json::Map<_, _>>(),
        "rescan": {
            "truncated": again.truncated,
            "parsed": again.parsed,
            "cached": again.cached,
            "bytes_read": again.bytes_read,
        },
    });
    same("fixture scan", &snapshot, FIXTURE_SCAN);

    // A process list as the snapshot reports it, with the fallbacks of
    // processes that could not be opened (no command line, cwd or start time).
    let resume = format!("claude --resume {C3}");
    let procs = vec![
        proc(4, 0, "System", 0, None, None),
        // Pod terminal: holder -> shell -> claude; the store records its session.
        proc(100, 4, "pod-holder.exe", 10, None, None),
        proc(101, 100, "cmd.exe", 11, None, None),
        proc(4242, 101, "claude.exe", 0, None, None),
        // npm shim -> native codex without a cwd: the shim's cwd, a guessed session.
        proc(
            300,
            4,
            "node.exe",
            1,
            Some(r#"node "C:\npm\node_modules\@openai\codex\bin\codex.js""#),
            Some(r"C:\src\demo"),
        ),
        proc(301, 300, "codex.exe", 2, Some("codex.exe"), None),
        // Resumed by id on the command line.
        proc(500, 4, "claude.exe", 3, Some(&resume), None),
        // Same cwd as 500's session, which is taken.
        proc(600, 4, "claude.exe", 0, None, Some(r"C:\src\other")),
        // A node process without a command line is not an agent.
        proc(700, 4, "node.exe", 5, None, Some(r"C:\src\demo")),
        proc(
            800,
            4,
            "codex.exe",
            6,
            Some("codex app-server --listen stdio"),
            None,
        ),
        // A reused parent pid: two agents in one cwd, one session to guess.
        proc(900, 4, "codex.exe", 500, Some("codex"), Some(r"D:\work")),
        proc(901, 900, "codex.exe", 100, Some("codex"), Some(r"D:\work")),
    ];
    let pod = [("t1".to_string(), vec![100, 101])];
    // The daemon's order: find, mark Pod terminals, attach sessions.
    let matched = |procs: &[ProcInfo]| {
        let mut agents = find_agents(procs);
        mark_pod_terminals(&mut agents, procs, &pod);
        attach_sessions(&mut agents, &out.running_ids, &out.sessions);
        agents
    };
    let agents = matched(&procs);
    same("process match", &wire(&agents), PROCESS_MATCH);
    // The snapshot's order does not matter.
    let reversed: Vec<ProcInfo> = procs.iter().rev().cloned().collect();
    assert_eq!(matched(&reversed), agents);
    let _ = std::fs::remove_dir_all(&root);
}

/// The paths and signatures podd and the example use keep resolving after
/// the split (`scan` and the `procs` functions are re-exported).
#[test]
fn public_paths_and_signatures() {
    let _: fn(&mut [Box<dyn Source>], &mut Cache, &Limits) -> pod_discover::ScanOutput =
        pod_discover::scan;
    let _: fn(&Found) -> u64 = pod_discover::last_activity;
    let _: fn() -> Vec<ProcInfo> = pod_discover::procs::snapshot;
    let _: fn(&[ProcInfo]) -> Vec<RunningAgent> = find_agents;
    type Recorded = HashMap<Agent, HashMap<u32, String>>;
    let _: fn(&mut [RunningAgent], &Recorded, &[Found]) = attach_sessions;
    type PodTerminals = [(String, Vec<u32>)];
    let _: fn(&mut [RunningAgent], &[ProcInfo], &PodTerminals) = mark_pod_terminals;
    let _: fn(&str) -> Vec<String> = pod_discover::procs::split_cmdline;
    let _: fn(&str) -> bool = pod_discover::procs::is_candidate_name;
    let _ = (
        pod_discover::ScanOutput::default(),
        pod_discover::ProcInfo::default(),
        pod_discover::RunningAgent::default(),
    );
    assert!(pod_discover::procs::is_candidate_name("Claude.EXE"));
    assert_eq!(
        pod_discover::procs::split_cmdline(r#"codex "a b""#),
        ["codex", "a b"]
    );
}

// Goldens recorded from the code before the split (see the module docs).

const FULL_META: &str = r#"
{
    "branch":"feature/login",
    "cwd":"C:\\src\\demo",
    "id":"11111111-2222-4333-8444-555555555555",
    "last_ms":2,
    "messages":3,
    "model":"claude-test-model",
    "origin":"cli",
    "preview":"Fix the login bug",
    "started_ms":1,
    "title":"Fix login bug",
    "version":"2.1.0"
}
"#;

const CACHE: &str = r#"
{
    "files":{
        "C:\\s\\a.jsonl":{
            "agent":"claude",
            "counted":true,
            "meta":{
                "branch":"feature/login",
                "cwd":"C:\\src\\demo",
                "id":"11111111-2222-4333-8444-555555555555",
                "last_ms":2,
                "messages":3,
                "model":"claude-test-model",
                "origin":"cli",
                "preview":"Fix the login bug",
                "started_ms":1,
                "title":"Fix login bug",
                "version":"2.1.0"
            },
            "mtime_ms":20,
            "size":10,
            "with_preview":true
        },
        "C:\\s\\b.jsonl":{"agent":"codex","counted":false,"mtime_ms":40,"size":30,"with_preview":false}
    }
}
"#;

const CACHE_WITHOUT_PREVIEWS: &str = r#"
{
    "files":{
        "C:\\s\\a.jsonl":{
            "agent":"claude",
            "counted":true,
            "meta":{
                "branch":"feature/login",
                "cwd":"C:\\src\\demo",
                "id":"11111111-2222-4333-8444-555555555555",
                "last_ms":2,
                "messages":3,
                "model":"claude-test-model",
                "origin":"cli",
                "started_ms":1,
                "title":"Fix login bug",
                "version":"2.1.0"
            },
            "mtime_ms":20,
            "size":10,
            "with_preview":false
        },
        "C:\\s\\b.jsonl":{"agent":"codex","counted":false,"mtime_ms":40,"size":30,"with_preview":false}
    }
}
"#;

const FULL_AGENT: &str = r#"
{
    "agent":"codex",
    "cwd":"C:\\src\\a",
    "guessed":true,
    "pid":20,
    "pids":[20,21],
    "pod_terminal":"t1",
    "session_id":"11111111-2222-4333-8444-555555555555",
    "start_ms":100
}
"#;

const AGENT_SCHEMA: &str = r##"
{
    "$defs":{
        "Agent":{
            "description":"The agents Pod can discover sessions of.",
            "enum":["claude","codex"],
            "type":"string"
        }
    },
    "$schema":"https://json-schema.org/draft/2020-12/schema",
    "description":"An interactive agent process (the outermost of a shim -> binary chain).",
    "properties":{
        "agent":{"anyOf":[{"$ref":"#/$defs/Agent"},{"type":"null"}]},
        "cwd":{"type":["string","null"]},
        "guessed":{
            "default":false,
            "description":"The session was inferred from cwd and timing, not recorded.",
            "type":"boolean"
        },
        "pid":{"format":"uint32","minimum":0,"type":"integer"},
        "pids":{
            "default":[],
            "description":"This process and the same-agent processes below it.",
            "items":{"format":"uint32","minimum":0,"type":"integer"},
            "type":"array"
        },
        "pod_terminal":{"description":"The Pod terminal this process runs in.","type":["string","null"]},
        "session_id":{"type":["string","null"]},
        "start_ms":{"format":"uint64","minimum":0,"type":"integer"}
    },
    "required":["pid","start_ms"],
    "title":"RunningAgent",
    "type":"object"
}
"##;

const FIXTURE_SCAN: &str = r#"
{
    "bytes_read":10603,
    "cache":{
        "claude/projects/C--src-demo/11111111-2222-4333-8444-555555555555.jsonl":{
            "agent":"claude",
            "counted":true,
            "meta":{
                "branch":"feature/login",
                "cwd":"C:\\src\\demo",
                "id":"11111111-2222-4333-8444-555555555555",
                "last_ms":1793527500000,
                "messages":6,
                "model":"claude-test-model",
                "origin":"cli",
                "preview":"Fix the login bug in auth.rs please",
                "started_ms":1793527199000,
                "title":"Fix login bug",
                "version":"2.1.0"
            },
            "mtime_ms":1767225600000,
            "size":1969,
            "with_preview":true
        },
        "claude/projects/C--src-demo/22222222-2222-4333-8444-555555555555.jsonl":{
            "agent":"claude",
            "counted":false,
            "mtime_ms":1767225600000,
            "size":188,
            "with_preview":true
        },
        "claude/projects/C--src-other/33333333-2222-4333-8444-555555555555.jsonl":{
            "agent":"claude",
            "counted":true,
            "meta":{
                "branch":"main",
                "cwd":"C:\\src\\other",
                "id":"33333333-2222-4333-8444-555555555555",
                "last_ms":1790841600000,
                "messages":1,
                "origin":"cli",
                "preview":"Write the docs",
                "started_ms":1790841600000,
                "title":"My renamed session",
                "version":"2.1.0"
            },
            "mtime_ms":1767225600000,
            "size":545,
            "with_preview":true
        },
        "codex/sessions/2026/11/02/rollout-2026-11-02T09-00-00-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl":{
            "agent":"codex",
            "counted":true,
            "meta":{
                "branch":"main",
                "cwd":"C:\\src\\demo",
                "id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793578200000,
                "messages":3,
                "model":"gpt-test",
                "origin":"cli",
                "preview":"Add a dark mode toggle",
                "started_ms":1793577600000,
                "title":"Dark mode",
                "version":"0.159.0"
            },
            "mtime_ms":1767225600000,
            "size":1154,
            "with_preview":true
        },
        "codex/sessions/2026/11/03/rollout-2026-11-03T09-00-00-bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl":{
            "agent":"codex",
            "counted":true,
            "meta":{
                "branch":"feat/x",
                "cwd":"C:\\src\\other",
                "id":"bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793664009000,
                "messages":2,
                "model":"gpt-new",
                "origin":"vscode",
                "preview":"Refactor the parser",
                "started_ms":1793664000000,
                "version":"0.167.0"
            },
            "mtime_ms":1767225600000,
            "size":960,
            "with_preview":true
        },
        "codex/sessions/2026/11/03/rollout-2026-11-03T10-00-00-cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl":{
            "agent":"codex",
            "counted":false,
            "mtime_ms":1767225600000,
            "size":356,
            "with_preview":true
        },
        "codex/sessions/2026/11/04/rollout-2026-11-04T10-00-00-dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl":{
            "agent":"codex",
            "counted":true,
            "meta":{
                "cwd":"D:\\work",
                "id":"dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793750401000,
                "messages":0,
                "model":"m",
                "started_ms":1793750400000
            },
            "mtime_ms":1767225600000,
            "size":188,
            "with_preview":true
        }
    },
    "cached":0,
    "parsed":7,
    "rescan":{"bytes_read":427,"cached":7,"parsed":0,"truncated":false},
    "running_ids":{"claude":{"4242":"11111111-2222-4333-8444-555555555555"},"codex":{}},
    "sessions":[
        {
            "agent":"codex",
            "meta":{
                "cwd":"D:\\work",
                "id":"dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793750401000,
                "messages":0,
                "model":"m",
                "started_ms":1793750400000
            },
            "mtime_ms":1767225600000,
            "path":"codex/sessions/2026/11/04/rollout-2026-11-04T10-00-00-dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl"
        },
        {
            "agent":"codex",
            "meta":{
                "branch":"feat/x",
                "cwd":"C:\\src\\other",
                "id":"bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793664009000,
                "messages":2,
                "model":"gpt-new",
                "origin":"vscode",
                "preview":"Refactor the parser",
                "started_ms":1793664000000,
                "version":"0.167.0"
            },
            "mtime_ms":1767225600000,
            "path":"codex/sessions/2026/11/03/rollout-2026-11-03T09-00-00-bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl"
        },
        {
            "agent":"codex",
            "meta":{
                "branch":"main",
                "cwd":"C:\\src\\demo",
                "id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "last_ms":1793578200000,
                "messages":3,
                "model":"gpt-test",
                "origin":"cli",
                "preview":"Add a dark mode toggle",
                "started_ms":1793577600000,
                "title":"Dark mode",
                "version":"0.159.0"
            },
            "mtime_ms":1767225600000,
            "path":"codex/sessions/2026/11/02/rollout-2026-11-02T09-00-00-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl"
        },
        {
            "agent":"claude",
            "meta":{
                "branch":"feature/login",
                "cwd":"C:\\src\\demo",
                "id":"11111111-2222-4333-8444-555555555555",
                "last_ms":1793527500000,
                "messages":6,
                "model":"claude-test-model",
                "origin":"cli",
                "preview":"Fix the login bug in auth.rs please",
                "started_ms":1793527199000,
                "title":"Fix login bug",
                "version":"2.1.0"
            },
            "mtime_ms":1767225600000,
            "path":"claude/projects/C--src-demo/11111111-2222-4333-8444-555555555555.jsonl"
        },
        {
            "agent":"claude",
            "meta":{
                "branch":"main",
                "cwd":"C:\\src\\other",
                "id":"33333333-2222-4333-8444-555555555555",
                "last_ms":1790841600000,
                "messages":1,
                "origin":"cli",
                "preview":"Write the docs",
                "started_ms":1790841600000,
                "title":"My renamed session",
                "version":"2.1.0"
            },
            "mtime_ms":1767225600000,
            "path":"claude/projects/C--src-other/33333333-2222-4333-8444-555555555555.jsonl"
        }
    ],
    "truncated":false
}
"#;

const PROCESS_MATCH: &str = r#"
[
    {
        "agent":"codex",
        "cwd":"C:\\src\\demo",
        "guessed":true,
        "pid":300,
        "pids":[300,301],
        "session_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "start_ms":1
    },
    {
        "agent":"claude",
        "guessed":false,
        "pid":500,
        "pids":[500],
        "session_id":"33333333-2222-4333-8444-555555555555",
        "start_ms":3
    },
    {
        "agent":"claude",
        "cwd":"C:\\src\\other",
        "guessed":false,
        "pid":600,
        "pids":[600],
        "start_ms":0
    },
    {
        "agent":"codex",
        "cwd":"D:\\work",
        "guessed":true,
        "pid":900,
        "pids":[900],
        "session_id":"dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "start_ms":500
    },
    {"agent":"codex","cwd":"D:\\work","guessed":false,"pid":901,"pids":[901],"start_ms":100},
    {
        "agent":"claude",
        "guessed":false,
        "pid":4242,
        "pids":[4242],
        "pod_terminal":"t1",
        "session_id":"11111111-2222-4333-8444-555555555555",
        "start_ms":0
    }
]
"#;
