//! Local, inert fixtures for TASK06 and the backend portions of TERM04/TERM09.
mod common;

use common::*;
use pod_test_support::git::{git_ok, git_output};
use pod_test_support::wait;
use podd::client::{AttachEvent, AttachStream, Client};
use podd::protocol::*;
use podd::vt::Vt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let name = unique("lv");
        let p = std::env::temp_dir().join(name.trim_start_matches("podd-test-"));
        std::fs::create_dir(&p).expect("create a fresh fixture directory");
        println!("fixture={}", p.display());
        Self(p)
    }

    fn pipe(&self) -> String {
        format!(
            r"\\.\pipe\{}",
            self.0.file_name().unwrap().to_str().unwrap()
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("fixture cleanup");
    }
}

fn task(c: &mut Client, name: &str) -> Result<TaskInfo, podd::client::ClientError> {
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "fixture".into(),
        name: name.into(),
        agent: Some(AgentSpec::Argv {
            argv: vec!["cmd".into(), "/d".into(), "/q".into()],
        }),
        ..Default::default()
    })
}

fn commit_config(repo: &Path, dirs: &str) {
    std::fs::write(
        repo.join(".pod/config.toml"),
        format!("[worktree]\nsparse = [{dirs}]\n"),
    )
    .unwrap();
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["commit", "-q", "-m", "fixture config"]);
}

#[test]
fn task06_sparse_failure_recovery_and_existing_task_isolation() {
    let f = Fixture::new();
    let repo = f.0.join("r");
    for dir in [".pod", "one", "two"] {
        std::fs::create_dir_all(repo.join(dir)).unwrap();
    }
    git_ok(&repo, &["init", "-q", "-b", "fixture"]);
    for (k, v) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git_ok(&repo, &["config", k, v]);
    }
    for name in ["README.md", "one/a.txt", "two/b.txt"] {
        std::fs::write(repo.join(name), name).unwrap();
    }
    commit_config(&repo, "\"one\"");
    // Fault only the sparse subprocess; all other calls use the real Git.
    // A tracked filename does not reliably fail cone setup after --no-checkout.
    let fault = f.0.join("fail-sparse");
    let wrapper = f.0.join("git-fault.cmd");
    std::fs::write(
        &wrapper,
        format!(
            "@echo off\r\nif exist \"{}\" (\r\n for %%A in (%*) do (\r\n  if \"%%~A\"==\"sparse-checkout\" (\r\n   echo fixture: injected sparse-checkout exit 73 1>&2\r\n   exit /b 73\r\n  )\r\n )\r\n)\r\ngit.exe %*\r\nexit /b %ERRORLEVEL%\r\n",
            fault.display()
        ),
    )
    .unwrap();
    let d = Daemon::start_full(
        f.pipe(),
        f.0.join("s"),
        &[],
        &[("POD_GIT", wrapper.to_str().unwrap())],
    );
    let mut c = d.client();
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.display().to_string(),
        name: Some("fixture".into()),
    })
    .unwrap();
    let first = task(&mut c, "first").unwrap();
    let w = PathBuf::from(&first.worktree);
    let sparse_before = git_output(&w, &["sparse-checkout", "list"]);
    let config_before = git_output(&w, &["config", "--worktree", "--list"]);
    let head_before = git_output(&w, &["rev-parse", "HEAD"]);
    let first_config = std::fs::read(w.join(".pod/config.toml")).unwrap();
    std::fs::write(w.join("one/a.txt"), "first task private edit\n").unwrap();
    std::fs::write(w.join("private.txt"), "untracked first task\n").unwrap();
    let status_before = git_output(&w, &["status", "--porcelain"]);

    commit_config(&repo, "\"two\"");
    std::fs::write(&fault, "fail only sparse-checkout").unwrap();
    let main_head = git_output(&repo, &["rev-parse", "HEAD"]);
    let error = task(&mut c, "second").unwrap_err().to_string();
    println!("sparse command failure: {error}");
    assert!(
        error.contains("sparse checkout") && error.contains("injected sparse-checkout exit 73"),
        "{error}"
    );
    assert!(!d.worktrees_root().join("fixture/second").exists());
    assert!(
        git_output(&repo, &["branch", "--list", "pod/second"])
            .trim()
            .is_empty()
    );
    assert_eq!(
        git_output(&repo, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    assert_eq!(
        c.call::<TaskList>(&TaskListParams::default())
            .unwrap()
            .tasks
            .len(),
        1
    );
    assert_eq!(
        c.call::<TerminalList>(&Empty {}).unwrap().terminals.len(),
        1
    );
    assert_eq!(git_output(&repo, &["rev-parse", "HEAD"]), main_head);
    assert!(
        git_output(&repo, &["status", "--porcelain"])
            .trim()
            .is_empty()
    );
    println!("ASSERT failure rolled back worktree, branch, task and terminal; base unchanged");

    std::fs::remove_file(&fault).unwrap();
    let second = task(&mut c, "second").expect("same task name recovers after corrected config");
    assert!(Path::new(&second.worktree).join("two/b.txt").exists());
    assert!(!Path::new(&second.worktree).join("one/a.txt").exists());
    assert_eq!(git_output(&w, &["sparse-checkout", "list"]), sparse_before);
    assert_eq!(
        git_output(&w, &["config", "--worktree", "--list"]),
        config_before
    );
    assert_eq!(git_output(&w, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git_output(&w, &["status", "--porcelain"]), status_before);
    assert_eq!(
        std::fs::read(w.join(".pod/config.toml")).unwrap(),
        first_config
    );
    assert_eq!(
        std::fs::read_to_string(w.join("one/a.txt")).unwrap(),
        "first task private edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.join("private.txt")).unwrap(),
        "untracked first task\n"
    );
    assert!(!w.join("two/b.txt").exists());
    assert!(repo.join("one/a.txt").exists() && repo.join("two/b.txt").exists());
    assert!(!repo.join(".git/info/sparse-checkout").exists());
    let shown = c
        .call::<TaskShow>(&TaskRef {
            task: first.id.clone(),
        })
        .unwrap();
    assert_eq!(
        shown.terminals[0].info.as_ref().unwrap().pid,
        first.terminals[0].info.as_ref().unwrap().pid
    );
    println!(
        "ASSERT same-name recovery, distinct cones, first config/HEAD/dirty contents/PID and base checkout preserved"
    );
    drop(c);
    drop(d);
}

fn inert_shell(c: &mut Client, root: &Path) -> TerminalInfo {
    let script = root.join("shell.ps1");
    std::fs::write(&script, r#"[Console]::WriteLine('READY')
while ($null -ne ($line = [Console]::ReadLine())) {
  if ($line -eq 'offline') {
    [Console]::WriteLine('ARMED')
    while (!(Test-Path -LiteralPath (Join-Path $PSScriptRoot 'go'))) { Start-Sleep -Milliseconds 20 }
    [Console]::WriteLine('OFFLINE-OUTPUT')
    Set-Content -LiteralPath (Join-Path $PSScriptRoot 'done') -Value done
  } else {
    1..20 | ForEach-Object { [Console]::WriteLine(('ROW {0} {1}' -f $line, $_)) }
  }
}
"#).unwrap();
    let argv = vec![
        "powershell.exe".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-File".into(),
        script.display().to_string(),
    ];
    println!("terminal.run argv={argv:?} cwd={}", root.display());
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv,
            cwd: Some(root.display().to_string()),
            cols: Some(120),
            rows: Some(30),
            ..Default::default()
        })
        .unwrap();
    wait_output(c, &t.id, "READY");
    println!(
        "terminal={} child={:?} holder={:?}",
        t.id, t.pid, t.holder_pid
    );
    t
}

fn info(c: &mut Client, id: &str) -> TerminalInfo {
    c.call::<TerminalList>(&Empty {})
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

fn consume(
    mut stream: AttachStream,
    mut vt: Vt,
    mut offset: u64,
    until: &'static str,
) -> mpsc::Receiver<(AttachStream, Vt, u64)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            match stream
                .next_event()
                .expect("attach read")
                .expect("attach closed")
            {
                AttachEvent::Output { seq, data } => {
                    assert_eq!(seq, offset, "contiguous ordered bytes");
                    offset += data.len() as u64;
                    vt.feed(&data);
                }
                other => panic!("unexpected attach event {other:?}"),
            }
            if vt
                .read_lines(ReadMode::History)
                .0
                .iter()
                .any(|line| line == until)
            {
                break;
            }
        }
        tx.send((stream, vt, offset)).ok();
    });
    rx
}

#[test]
fn term04_two_live_attachments_order_input_and_single_detach() {
    let f = Fixture::new();
    let d = Daemon::start_on(f.pipe(), f.0.join("s"));
    let mut a = d.client();
    let mut b = d.client();
    let t = inert_shell(&mut a, &f.0);
    let attach = || {
        let (r, snap, stream) = d
            .client()
            .attach(&AttachParams {
                target: TerminalRef::id(&t.id),
                scrollback: Some(1000),
                ..Default::default()
            })
            .unwrap();
        let vt = Vt::from_snapshot(r.terminal.cols, r.terminal.rows, r.seq, &snap);
        consume(stream, vt, r.seq, "ROW two 20")
    };
    let ra = attach();
    let rb = attach();
    assert_eq!(info(&mut a, &t.id).attached, 2);
    send(&mut a, &t.id, "one");
    wait_output(&mut a, &t.id, "ROW one 20");
    send(&mut b, &t.id, "two");
    let (sa, va, oa) = ra.recv_timeout(Duration::from_secs(30)).unwrap();
    let (sb, vb, ob) = rb.recv_timeout(Duration::from_secs(30)).unwrap();
    let expected: Vec<String> = ["one", "two"]
        .into_iter()
        .flat_map(|s| (1..=20).map(move |i| format!("ROW {s} {i}")))
        .collect();
    for vt in [&va, &vb] {
        let rows: Vec<String> = vt
            .read_lines(ReadMode::History)
            .0
            .into_iter()
            .filter(|l| l.starts_with("ROW "))
            .collect();
        assert_eq!(
            rows, expected,
            "both views receive every line in exact order once"
        );
    }
    println!(
        "ASSERT two attachments, input via both clients, 40 ordered unique rows; offsets {oa}/{ob}"
    );
    drop(sa);
    wait::until(Duration::from_secs(10), POLL, || {
        let n = info(&mut a, &t.id).attached;
        if n == 1 { Ok(()) } else { Err(n) }
    })
    .unwrap();
    let remaining = consume(sb, vb, ob, "ROW after 20");
    send(&mut b, &t.id, "after");
    let (sb, vb, _) = remaining.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        vb.read_lines(ReadMode::History)
            .0
            .iter()
            .any(|l| l == "ROW after 20")
    );
    let now = info(&mut a, &t.id);
    assert_eq!(
        (now.pid, now.holder_pid, now.state),
        (t.pid, t.holder_pid, TerminalState::Running)
    );
    drop(sb);
    wait::until(Duration::from_secs(10), POLL, || {
        let n = info(&mut a, &t.id).attached;
        if n == 0 { Ok(()) } else { Err(n) }
    })
    .unwrap();
    assert!(process_alive(t.pid.unwrap()) && process_alive(t.holder_pid.unwrap()));
    println!(
        "ASSERT one-view detach leaves other input/output and original PIDs live; all-detach also leaves process live"
    );
    drop(a);
    drop(b);
    drop(d);
    assert!(
        !process_alive(t.pid.unwrap()) && !process_alive(t.holder_pid.unwrap()),
        "cleanup ends owned processes"
    );
}

#[test]
fn term09_stop_and_crash_rediscover_original_pids_offline_output_and_input() {
    for crash in [false, true] {
        let f = Fixture::new();
        let mut d = Daemon::start_on(f.pipe(), f.0.join("s"));
        let mut c = d.client();
        let t = inert_shell(&mut c, &f.0);
        send(&mut c, &t.id, "before");
        wait_output(&mut c, &t.id, "ROW before 20");
        send(&mut c, &t.id, "offline");
        wait_output(&mut c, &t.id, "ARMED");
        drop(c);
        let old_daemon = d.child.id();
        let (pipe, state) = if crash {
            d.child.kill().unwrap();
            println!(
                "daemon crash pid={old_daemon} exit={:?}",
                d.child.wait().unwrap()
            );
            let pipe = std::mem::take(&mut d.pipe);
            let state = std::mem::take(&mut d.state_dir);
            drop(d);
            (pipe, state)
        } else {
            d.stop(false)
        };
        assert!(Client::connect(&pipe).is_err());
        assert!(process_alive(t.pid.unwrap()) && process_alive(t.holder_pid.unwrap()));
        std::fs::write(f.0.join("go"), "go").unwrap();
        wait::until(Duration::from_secs(10), POLL, || {
            if f.0.join("done").exists() {
                Ok(())
            } else {
                Err("offline output not produced")
            }
        })
        .unwrap();
        let d = Daemon::start_on(pipe, state);
        assert_ne!(d.child.id(), old_daemon);
        let mut c = d.client();
        let now = info(&mut c, &t.id);
        assert_eq!(
            (now.pid, now.holder_pid, now.state),
            (t.pid, t.holder_pid, TerminalState::Running)
        );
        let history = read(&mut c, &t.id, ReadMode::History);
        for marker in ["ROW before 20", "OFFLINE-OUTPUT"] {
            assert!(
                history.iter().any(|l| l == marker),
                "missing {marker}: {history:?}"
            );
        }
        send(&mut c, &t.id, "after");
        wait_output(&mut c, &t.id, "ROW after 20");
        println!(
            "ASSERT crash={crash} daemon {old_daemon}->{}; child={:?}/holder={:?} unchanged, before/offline output recovered and new input works",
            d.child.id(),
            t.pid,
            t.holder_pid
        );
        drop(c);
        drop(d);
        assert!(
            !process_alive(t.pid.unwrap()) && !process_alive(t.holder_pid.unwrap()),
            "cleanup ends owned processes"
        );
    }
}
