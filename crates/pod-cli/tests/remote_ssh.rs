//! `pod machine …` and `pod --machine <name> …` over a FAKE ssh (docs/17 R3).
//!
//! `pod-fake-ssh.exe` stands in for ssh.exe (`POD_SSH`). It checks the argv
//! and generated config Pod passes, then either bridges to an isolated
//! local podd through `pod remote-stdio` or plays a failure (see its
//! module doc). No SSH server is involved: real SSH stays unverified here.
//!
//! Needs `podd.exe` and `pod-holder.exe` next to `pod.exe` (workspace build).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use podd::client::{Client, ProcessWatch};
use podd::protocol::*;

const POD: &str = env!("CARGO_BIN_EXE_pod");
const FAKE_SSH: &str = env!("CARGO_BIN_EXE_pod-fake-ssh");

struct Env {
    root: PathBuf,
    pipe: String,
}

impl Env {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("pod-rssh-{tag}-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let podd = Path::new(POD).with_file_name("podd.exe");
        assert!(
            podd.exists() && podd.with_file_name("pod-holder.exe").exists(),
            "{} / pod-holder.exe missing: build the workspace first",
            podd.display()
        );
        Self {
            root,
            pipe: format!(r"\\.\pipe\{name}"),
        }
    }

    fn log_path(&self) -> PathBuf {
        self.root.join("fake-ssh.log")
    }

    /// `pod <args>` with every path isolated and the fake ssh.
    fn pod(&self, args: &[&str]) -> Output {
        self.pod_env(args, &[])
    }

    fn pod_env(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let r = &self.root;
        let mut c = Command::new(POD);
        c.args(args)
            .env("LOCALAPPDATA", r.join("local"))
            .env("APPDATA", r.join("roaming"))
            .env("USERPROFILE", r.join("home"))
            .env("POD_CONFIG_DIR", r.join("config"))
            .env("POD_WORKTREES_ROOT", r.join("worktrees"))
            .env("POD_MODS_DIR", r.join("mods"))
            .env("POD_PR_POLL", "off")
            .env("POD_SSH", FAKE_SSH)
            .env("POD_FAKE_SSH_REMOTE_POD", POD)
            .env("POD_FAKE_SSH_PIPE", &self.pipe)
            .env("POD_FAKE_SSH_LOG", self.log_path())
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_DAEMON")
            .env_remove("POD_HOLDER")
            .env_remove("POD_SSH_TIMEOUT");
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().expect("run pod")
    }

    fn add(&self, name: &str, host: &str) {
        let o = self.pod(&["machine", "add", name, "--host", host, "--user", "alice"]);
        assert!(o.status.success(), "{}", text(&o.stderr));
    }

    fn log(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn direct(&self) -> Client {
        Client::connect(&self.pipe).expect("direct connect")
    }

    /// `pod machine test <name> --json` → (exit code, JSON, stderr).
    fn test_machine(&self, name: &str, extra: &[(&str, &str)]) -> (i32, serde_json::Value, String) {
        let o = self.pod_env(&["machine", "test", name, "--json"], extra);
        let v = serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", text(&o.stdout)));
        (o.status.code().unwrap(), v, text(&o.stderr))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.pipe)
            && let Ok(s) = c.call::<ServerStatus>(&Empty {})
        {
            let watch = ProcessWatch::open(s.pid);
            let _ = c.call::<ServerStop>(&StopParams { kill: true });
            drop(c);
            if let Some(w) = watch {
                w.wait(Duration::from_secs(20));
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn machine_profiles_add_list_remove_and_validation() {
    let env = Env::new("prof");
    let o = env.pod(&[
        "machine",
        "add",
        "dev",
        "--host",
        "example.com",
        "--user",
        "alice",
        "--port",
        "2222",
        "--identity-file",
        r"C:\keys\id_ed25519",
        "--known-hosts",
        r"C:\keys\known_hosts",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = env.pod(&["machine", "list", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v["machines"][0],
        serde_json::json!({"name": "dev", "host": "example.com", "user": "alice", "port": 2222,
            "identity_file": r"C:\keys\id_ed25519", "known_hosts": r"C:\keys\known_hosts"})
    );
    // Stored in the Pod config dir, not ~/.ssh.
    assert!(env.root.join("config").join("machines.json").exists());

    for bad in [
        vec!["--host", "-oProxyCommand=calc"],
        vec!["--host", "h\nx"],
        vec!["--host", "h", "--user", "-lroot"],
        vec!["--host", "h", "--user", "a\u{1b}b"],
        vec!["--host", "h", "--identity-file", "relative\\key"],
    ] {
        let mut args = vec!["machine", "add", "bad"];
        args.extend(bad.iter().copied());
        let o = env.pod(&args);
        assert_eq!(o.status.code(), Some(2), "{bad:?} accepted");
    }
    let o = env.pod(&["machine", "add", "dev", "--host", "other"]);
    assert_eq!(o.status.code(), Some(2), "duplicate accepted");
    assert!(text(&o.stderr).contains("이미 있습니다"));

    let o = env.pod(&["machine", "remove", "dev"]);
    assert!(o.status.success());
    let o = env.pod(&["machine", "remove", "dev"]);
    assert_eq!(o.status.code(), Some(2));
    let o = env.pod(&["--machine", "dev", "ls"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        text(&o.stderr).contains("머신 \"dev\"이(가) 없습니다"),
        "{}",
        text(&o.stderr)
    );
    assert!(
        env.log().is_empty(),
        "ssh must not run for a missing profile"
    );
}

#[test]
fn machine_routes_commands_over_ssh_to_the_remote_daemon() {
    let env = Env::new("ok");
    env.add("dev", "ok.fake");
    let (code, v, err) = env.test_machine("dev", &[]);
    assert_eq!(code, 0, "{v} {err}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["hello"]["protocol"], PROTOCOL_VERSION);
    let machine_id = v["hello"]["machine_id"].as_str().unwrap().to_string();

    // Exactly what ssh got: absolute fake path, -F <config> -T, the enforced
    // options, the fixed alias and remote command; the config only has
    // profile directives and is gone afterwards.
    let runs = env.log();
    assert_eq!(runs.len(), 1);
    let argv: Vec<&str> = runs[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[0], "-F");
    assert!(Path::new(argv[1]).is_absolute());
    assert!(
        argv[1].starts_with(
            &*env
                .root
                .join("local")
                .join("pod")
                .join("ssh")
                .to_string_lossy()
        ),
        "{}",
        argv[1]
    );
    assert!(!Path::new(argv[1]).exists(), "generated config not deleted");
    assert_eq!(argv[2], "-T");
    for o in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "RequestTTY=no",
    ] {
        let i = argv
            .iter()
            .position(|a| *a == o)
            .unwrap_or_else(|| panic!("{o}"));
        assert_eq!(argv[i - 1], "-o");
    }
    assert_eq!(
        &argv[argv.len() - 4..],
        ["--", "pod-remote", "pod", "remote-stdio"]
    );
    assert!(
        !argv
            .iter()
            .any(|a| a.contains("ok.fake") || a.contains("alice"))
    );
    let config = runs[0]["config"].as_str().unwrap();
    assert!(config.contains("HostName ok.fake\n") && config.contains("User alice\n"));
    // Byte for byte: the program, every argument (the config path aside),
    // the config file's name and its text.
    assert_eq!(runs[0]["argv0"], FAKE_SSH);
    let mut pinned = argv.clone();
    pinned[1] = "<config>";
    assert_eq!(
        pinned,
        [
            "-F",
            "<config>",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "RequestTTY=no",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "--",
            "pod-remote",
            "pod",
            "remote-stdio",
        ]
    );
    // <machine>-<pid>-<counter>-<nanos>.conf
    let file = Path::new(argv[1]).file_name().unwrap().to_str().unwrap();
    let stem = file.strip_suffix(".conf").expect(file);
    let parts: Vec<&str> = stem.split('-').collect();
    assert_eq!(parts.len(), 4, "{file}");
    assert_eq!(parts[0], "dev", "{file}");
    assert!(
        parts[1..]
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
        "{file}"
    );
    assert_eq!(
        config,
        "# Generated by Pod for one connection (docs/17 2.1); deleted after use.\n\
         Host pod-remote\n  \
         HostName ok.fake\n  \
         User alice\n"
    );

    // A command over the machine reaches the remote daemon (the same one a
    // direct local client sees); the local cwd is not sent.
    let o = env.pod(&[
        "--machine",
        "dev",
        "run",
        "--name",
        "far",
        "--",
        "cmd.exe",
        "/k",
        "echo remote",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let id = text(&o.stdout).trim().to_string();
    let mut direct = env.direct();
    let list = direct.call::<TerminalList>(&Empty {}).unwrap();
    let t = list
        .terminals
        .iter()
        .find(|t| t.id == id)
        .expect("terminal on remote");
    assert_eq!(t.name.as_deref(), Some("far"));
    let o = env.pod(&["--machine", "dev", "ls", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("\"far\""));
    let o = env.pod(&["--machine", "dev", "server", "status", "--json"]);
    let s: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        s["pipe"].as_str().unwrap().to_lowercase(),
        env.pipe.to_lowercase()
    );
    // The same machine_id on a second connection.
    let (_, v2, _) = env.test_machine("dev", &[]);
    assert_eq!(v2["hello"]["machine_id"], machine_id.as_str());

    // Commands that make no sense remotely are refused before ssh runs.
    let before = env.log().len();
    for args in [
        vec!["--machine", "dev", "attach", "t1"],
        vec!["--machine", "dev", "server", "stop"],
        vec!["--machine", "dev", "remote-stdio"],
        vec!["--machine", "dev", "run", "--inherit-env", "--", "cmd"],
        vec!["--machine", "dev", "machine", "list"],
    ] {
        let o = env.pod(&args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(
            text(&o.stderr).contains("--machine dev:"),
            "{}",
            text(&o.stderr)
        );
    }
    assert_eq!(env.log().len(), before);
    let o = env.pod(&["--machine", "dev", "--pipe", "x", "ls"]);
    assert_eq!(o.status.code(), Some(2), "--machine and --pipe conflict");
}

#[test]
fn login_banner_on_stdout_is_refused() {
    let env = Env::new("banner");
    env.add("dev", "banner.fake");
    let (code, v, err) = env.test_machine("dev", &[]);
    assert_eq!(code, 2);
    assert_eq!(v["kind"], "stdout_polluted", "{v}");
    // The whole `--json` failure, minus the exit code: ssh is still alive
    // here and gets 500 ms to end by itself before it is killed.
    let mut pinned = v.clone();
    pinned["exit_code"] = serde_json::Value::Null;
    assert_eq!(
        pinned,
        serde_json::json!({
            "ok": false,
            "machine": "dev",
            "kind": "stdout_polluted",
            "message": "원격 출력에 프로토콜이 아닌 내용이 섞였습니다",
            "detail": "protocol error: non-protocol output on the daemon connection: \"Welcome to the fake host!\\r\"",
            "stderr": [],
            "exit_code": null,
        })
    );
    assert!(
        err.contains("원격 출력에 프로토콜이 아닌 내용이 섞였습니다"),
        "{err}"
    );
    assert!(
        err.contains("Welcome"),
        "the offending line is shown: {err}"
    );
    // Not skipped and carried on: no command went through.
    let o = env.pod(&["--machine", "dev", "run", "--", "cmd.exe", "/k", "echo x"]);
    assert_eq!(o.status.code(), Some(2));
    if let Ok(mut c) = Client::connect(&env.pipe) {
        assert!(
            c.call::<TerminalList>(&Empty {})
                .unwrap()
                .terminals
                .is_empty()
        );
    }
}

#[test]
fn ssh_failures_map_to_korean_error_kinds() {
    let env = Env::new("fail");
    let at = "@".repeat(59);
    let cases = [
        (
            "uk",
            "unknownkey.fake",
            "host_key_unknown",
            "호스트 키를 알 수 없어 연결을 막았습니다",
            255,
            vec![
                "No ED25519 host key is known for unknownkey.fake and you have requested strict checking.",
                "Host key verification failed.",
            ],
        ),
        (
            "ck",
            "changedkey.fake",
            "host_key_changed",
            "호스트 키가 바뀌어 연결을 막았습니다",
            255,
            vec![
                at.as_str(),
                "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @",
                at.as_str(),
                "IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!",
                "Host key for changedkey.fake has changed and you have requested strict checking.",
                "Host key verification failed.",
            ],
        ),
        (
            "au",
            "auth.fake",
            "auth_failed",
            "SSH 인증에 실패했습니다",
            255,
            vec!["alice@auth.fake: Permission denied (publickey)."],
        ),
        (
            "np",
            "nopod.fake",
            "remote_pod_missing",
            "원격에 pod가 없습니다",
            127,
            vec!["bash: line 1: pod: command not found"],
        ),
        (
            "op",
            "oldpod.fake",
            "version_mismatch",
            "원격 Pod 버전이 맞지 않습니다",
            2,
            vec![
                "error: unrecognized subcommand 'remote-stdio'",
                "Usage: pod [OPTIONS] <COMMAND>",
            ],
        ),
    ];
    for (name, host, kind, msg, exit, stderr) in cases {
        env.add(name, host);
        let (code, v, err) = env.test_machine(name, &[]);
        assert_eq!(code, 2, "{name}");
        assert_eq!(v["kind"], kind, "{name}: {v}");
        assert!(err.contains(msg), "{name}: {err}");
        assert!(
            !v["stderr"].as_array().unwrap().is_empty(),
            "{name}: diagnostics kept"
        );
        // The whole `--json` failure: ssh's exit code and its bounded,
        // cleaned stderr lines (CR blanked and trimmed, empty lines dropped).
        assert_eq!(
            v,
            serde_json::json!({
                "ok": false,
                "machine": name,
                "kind": kind,
                "message": msg,
                "detail": format!("ssh가 연결을 닫았습니다 (종료 코드 {exit})"),
                "stderr": stderr,
                "exit_code": exit,
            }),
            "{name}"
        );
        if name == "au" {
            // The human message on stderr, whole.
            assert_eq!(
                err,
                "pod: 머신 au: SSH 인증에 실패했습니다. 키를 ssh-agent에 올리거나(암호가 걸린 키는 \
                 직접 해제) 프로필의 키 경로와 사용자를 확인하세요. 비밀번호 로그인은 쓰지 \
                 않습니다.\n  원인: ssh가 연결을 닫았습니다 (종료 코드 255)\n  ssh: \
                 alice@auth.fake: Permission denied (publickey).\n"
            );
        }
        // The same error through a routed command.
        let o = env.pod(&["--machine", name, "ls"]);
        assert_eq!(o.status.code(), Some(2));
        assert!(text(&o.stderr).contains(msg), "{name}: {}", text(&o.stderr));
    }
    // Pod never started a local daemon for any of these.
    assert!(Client::connect(&env.pipe).is_err());
}

#[test]
fn v11_remote_gets_version_guidance_only() {
    let env = Env::new("v11");
    env.add("old", "v11.fake");
    let (code, v, err) = env.test_machine("old", &[]);
    assert_eq!(code, 2);
    assert_eq!(v["kind"], "version_mismatch", "{v}");
    assert!(
        err.contains("원격 Pod 버전이 맞지 않습니다") && err.contains("protocol 11"),
        "{err}"
    );
    // The client label is `ssh:<machine>`. The exit code is left out: ssh
    // is alive here and gets 500 ms to end by itself before it is killed.
    let mut pinned = v.clone();
    pinned["exit_code"] = serde_json::Value::Null;
    assert_eq!(
        pinned,
        serde_json::json!({
            "ok": false,
            "machine": "old",
            "kind": "version_mismatch",
            "message": "원격 Pod 버전이 맞지 않습니다",
            "detail": "podd 0.0.11 on ssh:old speaks protocol 11; remote connections need 12 \
                       or newer. Install the matching Pod version there (running terminals \
                       are not touched)",
            "stderr": [],
            "exit_code": null,
        })
    );
    let methods: Vec<String> = env
        .log()
        .iter()
        .filter_map(|l| l["method"].as_str().map(String::from))
        .collect();
    assert_eq!(
        methods,
        ["server.status"],
        "nothing but the version check was sent"
    );
    // A routed mutating command sends nothing either.
    let o = env.pod(&["--machine", "old", "run", "--", "cmd.exe"]);
    assert_eq!(o.status.code(), Some(2));
    let methods: Vec<String> = env
        .log()
        .iter()
        .filter_map(|l| l["method"].as_str().map(String::from))
        .collect();
    assert_eq!(methods, ["server.status", "server.status"]);
}

#[test]
fn handshake_timeout_kills_ssh() {
    let env = Env::new("hang");
    env.add("slow", "hang.fake");
    let start = Instant::now();
    let (code, v, err) = env.test_machine("slow", &[("POD_SSH_TIMEOUT", "1.5")]);
    assert_eq!(code, 2);
    assert_eq!(v["kind"], "timeout", "{v}");
    assert!(err.contains("시간 안에 응답이 없습니다"), "{err}");
    // Killed at the deadline (TerminateProcess exit code 1), nothing on stderr.
    assert_eq!(
        v,
        serde_json::json!({
            "ok": false,
            "machine": "slow",
            "kind": "timeout",
            "message": "시간 안에 응답이 없습니다",
            "detail": "1.5초 안에 핸드셰이크가 끝나지 않아 ssh를 종료했습니다",
            "stderr": [],
            "exit_code": 1,
        })
    );
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn missing_or_relative_ssh_is_reported() {
    let env = Env::new("nossh");
    env.add("dev", "ok.fake");
    let missing = env.root.join("no-such").join("ssh.exe");
    let missing = missing.to_str().unwrap();
    for (ssh, detail) in [
        (missing, format!("POD_SSH의 파일이 없습니다: {missing}")),
        ("ssh.exe", "POD_SSH는 절대 경로여야 합니다: ssh.exe".into()),
    ] {
        let (code, v, err) = env.test_machine("dev", &[("POD_SSH", ssh)]);
        assert_eq!(code, 2);
        assert_eq!(v["kind"], "ssh_missing", "{ssh}: {v}");
        assert_eq!(
            v,
            serde_json::json!({
                "ok": false,
                "machine": "dev",
                "kind": "ssh_missing",
                "message": "ssh 실행 파일을 찾을 수 없습니다",
                "detail": detail,
                "stderr": [],
                "exit_code": null,
            })
        );
        assert!(
            err.contains("ssh 실행 파일을 찾을 수 없습니다") && err.contains("OpenSSH 클라이언트"),
            "{err}"
        );
    }
}
