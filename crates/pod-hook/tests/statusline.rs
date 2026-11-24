//! `pod-hook statusline`: the real collector process, fed Claude Code's
//! documented status-line JSON, writes only normalized limits to the usage
//! cache and answers within its budget, with or without passthrough.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pod-hook");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pod-hook-statusline-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn run(
    dir: &Path,
    config_dir: Option<&str>,
    args: &[&str],
    stdin: &[u8],
) -> (Vec<u8>, i32, Duration) {
    let mut cmd = Command::new(BIN);
    cmd.arg("statusline")
        .args(args)
        .env("POD_USAGE_DIR", dir)
        .env_remove("POD_TERMINAL_ID")
        .env_remove("POD_SOCKET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match config_dir {
        Some(c) => cmd.env("CLAUDE_CONFIG_DIR", c),
        None => cmd.env_remove("CLAUDE_CONFIG_DIR"),
    };
    let start = Instant::now();
    let mut child = cmd.spawn().expect("spawn pod-hook");
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(stdin);
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert!(out.stderr.is_empty(), "nothing on stderr: {:?}", out.stderr);
    (out.stdout, out.status.code().unwrap_or(-1), start.elapsed())
}

fn sample(five_used: f64) -> Vec<u8> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    serde_json::json!({
        "session_id": "sess-should-not-leak",
        "transcript_path": "C:/Users/someone/.claude/projects/p/t.jsonl",
        "cwd": "C:/private/repo",
        "model": {"display_name": "Opus"},
        "rate_limits": {
            "five_hour": {"used_percentage": five_used, "resets_at": now + 3600},
            "seven_day": {"used_percentage": 41.2, "resets_at": now + 86400}
        }
    })
    .to_string()
    .into_bytes()
}

fn files(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .map(|r| {
            r.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn stores_normalized_limits_and_prints_a_short_line() {
    let d = dir("basic");
    let (out, code, t) = run(&d, None, &[], &sample(23.5));
    assert_eq!(code, 0);
    assert!(t < Duration::from_secs(10), "{t:?}");
    assert_eq!(
        String::from_utf8_lossy(&out).trim(),
        "5h 77% 남음 · 7d 59% 남음"
    );
    assert_eq!(files(&d), ["claude-default.json"]);
    let text = std::fs::read_to_string(d.join("claude-default.json")).unwrap();
    for leak in [
        "sess-should-not-leak",
        "transcript",
        "private",
        "Opus",
        "someone",
    ] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    let rec = pod_status_record(&d, "default");
    assert_eq!(rec["five_hour"]["used_percentage"], 23.5);
    assert_eq!(rec["source"], "claude-statusline");
    let _ = std::fs::remove_dir_all(&d);
}

/// Windows PowerShell pipes text with a UTF-8 BOM (seen live): still a sample.
#[test]
fn a_utf8_bom_from_powershell_is_accepted() {
    let d = dir("bom");
    let mut input = b"\xEF\xBB\xBF".to_vec();
    input.extend(sample(12.0));
    input.extend(b"\r\n");
    let (out, code, _) = run(&d, None, &[], &input);
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8_lossy(&out).trim(),
        "5h 88% 남음 · 7d 59% 남음"
    );
    assert_eq!(
        pod_status_record(&d, "default")["five_hour"]["used_percentage"],
        12.0
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn passthrough_keeps_the_input_for_an_existing_status_line() {
    let d = dir("pass");
    let input = sample(50.0);
    let (out, code, _) = run(&d, None, &["--passthrough"], &input);
    assert_eq!((out, code), (input, 0), "input unchanged on stdout");
    assert_eq!(
        pod_status_record(&d, "default")["five_hour"]["used_percentage"],
        50.0
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// PowerShell setup: the same JSON in ASCII (Windows PowerShell would turn
/// non-ASCII text piped to the next command into `?`), still a sample.
#[test]
fn ascii_passthrough_escapes_non_ascii_text_only() {
    let d = dir("ascii");
    let mut v: serde_json::Value = serde_json::from_slice(&sample(30.0)).unwrap();
    v["cwd"] = "C:/작업/저장소".into();
    let input = serde_json::to_vec(&v).unwrap();
    let (out, code, _) = run(&d, None, &["--passthrough", "--ascii"], &input);
    assert_eq!(code, 0);
    assert!(out.is_ascii(), "{}", String::from_utf8_lossy(&out));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out).unwrap(),
        v
    );
    assert_eq!(
        pod_status_record(&d, "default")["five_hour"]["used_percentage"],
        30.0
    );
    // `--ascii` alone is a usage error: nothing read, nothing written.
    let d2 = dir("ascii-alone");
    let (out, code, _) = run(&d2, None, &["--ascii"], &input);
    assert_eq!((out.is_empty(), code), (true, 2));
    assert!(files(&d2).is_empty());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn separate_claude_profiles_never_overwrite_each_other() {
    let d = dir("profiles");
    run(&d, None, &[], &sample(10.0));
    run(&d, Some("C:/Users/x/.claude-work"), &[], &sample(90.0));
    let names = files(&d);
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(
        pod_status_record(&d, "default")["five_hour"]["used_percentage"],
        10.0
    );
    let other = names.iter().find(|n| n.starts_with("claude-cfg-")).unwrap();
    let key = other
        .trim_start_matches("claude-")
        .trim_end_matches(".json");
    let rec = pod_status_record(&d, key);
    assert_eq!(rec["five_hour"]["used_percentage"], 90.0);
    assert_eq!(rec["profile_label"], ".claude-work");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unusable_input_writes_nothing_and_still_exits_zero() {
    let d = dir("bad");
    for input in [&b"{not json"[..], b"", b"[]"] {
        let (out, code, _) = run(&d, None, &[], input);
        assert_eq!((out.is_empty(), code), (true, 0));
    }
    assert!(files(&d).is_empty());
    // Without rate_limits (API key, or before the first response): the
    // collector records that it runs, with no limits and no output line.
    let (out, code, _) = run(&d, None, &[], br#"{"model":{"display_name":"Opus"}}"#);
    assert_eq!((out.is_empty(), code), (true, 0));
    let rec = pod_status_record(&d, "default");
    assert!(rec.get("five_hour").is_none() && rec.get("limits_at_ms").is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_open_stdin_never_holds_the_status_line() {
    let d = dir("open");
    let mut cmd = Command::new(BIN);
    cmd.arg("statusline")
        .env("POD_USAGE_DIR", &d)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let start = Instant::now();
    let mut child = cmd.spawn().unwrap();
    let keep = child.stdin.take();
    let out = child.wait_with_output().unwrap();
    drop(keep);
    assert_eq!(out.status.code(), Some(0));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(files(&d).is_empty());
}

fn pod_status_record(d: &Path, key: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(d.join(format!("claude-{key}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}
