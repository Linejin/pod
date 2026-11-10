//! Test double of `ssh.exe` for `crates/pod-cli/tests/remote_ssh.rs` (docs/17 R3).
//! Not shipped (the installer copies named binaries only).
//!
//! It first checks what Pod passed: `-F <config> -T`, the enforced `-o`
//! options, `-- pod-remote pod remote-stdio` and a config with only the
//! profile directives. Any deviation: a message on stderr and exit 254.
//! Then it plays the scenario named by the config's `HostName` (the part
//! before the first `.`):
//!
//! - `ok`: bridge to a local podd by running `$POD_FAKE_SSH_REMOTE_POD --pipe
//!   $POD_FAKE_SSH_PIPE remote-stdio` with this process's stdio.
//! - `banner`: a login banner on stdout, then the same bridge.
//! - `unknownkey`, `changedkey`, `auth`: OpenSSH's messages, exit 255.
//! - `hang`: no output for two minutes.
//! - `nopod`: the shell's "command not found", exit 127.
//! - `oldpod`: an older pod's usage error for `remote-stdio`, exit 2.
//! - `v11`: answers like a protocol-11 podd (no `connection.hello`).
//!
//! `$POD_FAKE_SSH_LOG` (if set) gets one JSON line per run with argv and
//! config text, and one line per request method in the `v11` scenario.

use std::io::{BufRead, Write};
use std::process::{Command, ExitCode};

const REQUIRED: &[&str] = &[
    "BatchMode=yes",
    "StrictHostKeyChecking=yes",
    "PasswordAuthentication=no",
    "KbdInteractiveAuthentication=no",
    "ForwardAgent=no",
    "ForwardX11=no",
    "ClearAllForwardings=yes",
    "RequestTTY=no",
];
const ALLOWED_EXTRA: &[&str] = &[
    "PermitLocalCommand=no",
    "UpdateHostKeys=no",
    "ConnectTimeout=10",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
];
const ALLOWED_DIRECTIVES: &[&str] = &[
    "Host",
    "HostName",
    "User",
    "Port",
    "IdentityFile",
    "IdentitiesOnly",
    "UserKnownHostsFile",
];

fn log(v: serde_json::Value) {
    if let Some(p) = std::env::var_os("POD_FAKE_SSH_LOG") {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .expect("log");
        writeln!(f, "{v}").unwrap();
    }
}

/// The config's HostName, or why argv/config is wrong.
fn check(args: &[String]) -> Result<String, String> {
    let mut it = args.iter();
    if it.next().map(String::as_str) != Some("-F") {
        return Err("argv must start with -F".into());
    }
    let cfg = it.next().ok_or("no config after -F")?;
    if !std::path::Path::new(cfg).is_absolute() {
        return Err(format!("config path not absolute: {cfg}"));
    }
    if it.next().map(String::as_str) != Some("-T") {
        return Err("-T must follow the config".into());
    }
    let mut opts = Vec::new();
    loop {
        match it.next().map(String::as_str) {
            Some("-o") => opts.push(it.next().ok_or("-o without value")?.clone()),
            Some("--") => break,
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    for r in REQUIRED {
        if !opts.iter().any(|o| o == r) {
            return Err(format!("missing -o {r}"));
        }
    }
    for o in &opts {
        if !REQUIRED.contains(&o.as_str()) && !ALLOWED_EXTRA.contains(&o.as_str()) {
            return Err(format!("unexpected -o {o}"));
        }
    }
    let rest: Vec<&str> = it.map(String::as_str).collect();
    if rest != ["pod-remote", "pod", "remote-stdio"] {
        return Err(format!(
            "destination/command must be pod-remote pod remote-stdio, got {rest:?}"
        ));
    }
    let text = std::fs::read_to_string(cfg).map_err(|e| format!("read {cfg}: {e}"))?;
    log(serde_json::json!({"argv0": std::env::args().next(), "argv": args, "config": text}));
    let mut host = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once(' ').ok_or(format!("bad line {line:?}"))?;
        if !ALLOWED_DIRECTIVES.contains(&k) {
            return Err(format!("unexpected config directive {k}"));
        }
        if k == "Host" && v != "pod-remote" {
            return Err(format!("Host must be pod-remote, got {v}"));
        }
        if k == "HostName" {
            host = Some(v.to_string());
        }
    }
    host.ok_or_else(|| "no HostName".into())
}

fn bridge() -> ExitCode {
    let pod = std::env::var("POD_FAKE_SSH_REMOTE_POD").expect("POD_FAKE_SSH_REMOTE_POD");
    let pipe = std::env::var("POD_FAKE_SSH_PIPE").expect("POD_FAKE_SSH_PIPE");
    let status = Command::new(pod)
        .args(["--pipe", &pipe, "remote-stdio"])
        .status()
        .expect("run remote pod");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

fn v11_daemon() -> ExitCode {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let req: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let method = req["method"].as_str().unwrap_or("").to_string();
        log(serde_json::json!({"method": method}));
        let reply = if method == "server.status" {
            serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": {
                "version": "0.0.11", "protocol": 11, "pid": 1, "pipe": "fake", "started_ms": 0,
                "uptime_ms": 0, "terminals": 0, "running": 0, "state_dir": "fake"}})
        } else {
            serde_json::json!({"jsonrpc": "2.0", "id": req["id"],
                "error": {"code": -32601, "message": format!("unknown method {method}")}})
        };
        writeln!(out, "{reply}").unwrap();
        out.flush().unwrap();
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let host = match check(&args) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("fake-ssh: {e}");
            return ExitCode::from(254);
        }
    };
    let scenario = host.split('.').next().unwrap_or("").to_string();
    let fail = |msg: &str, code: u8| {
        eprint!("{msg}");
        ExitCode::from(code)
    };
    match scenario.as_str() {
        "ok" => bridge(),
        "banner" => {
            print!("Welcome to the fake host!\r\nLast login: Mon Nov  9 10:00:00 2026\r\n");
            std::io::stdout().flush().unwrap();
            bridge()
        }
        "unknownkey" => fail(
            "No ED25519 host key is known for unknownkey.fake and you have requested strict checking.\r\n\
             Host key verification failed.\r\n",
            255,
        ),
        "changedkey" => fail(
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\r\n\
             @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\r\n\
             @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\r\n\
             IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!\r\n\
             Host key for changedkey.fake has changed and you have requested strict checking.\r\n\
             Host key verification failed.\r\n",
            255,
        ),
        "auth" => fail("alice@auth.fake: Permission denied (publickey).\r\n", 255),
        "hang" => {
            std::thread::sleep(std::time::Duration::from_secs(120));
            ExitCode::SUCCESS
        }
        "nopod" => fail("bash: line 1: pod: command not found\n", 127),
        "oldpod" => fail(
            "error: unrecognized subcommand 'remote-stdio'\n\nUsage: pod [OPTIONS] <COMMAND>\n",
            2,
        ),
        "v11" => v11_daemon(),
        other => fail(&format!("fake-ssh: unknown scenario {other:?}\n"), 254),
    }
}
