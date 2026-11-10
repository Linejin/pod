//! v6 exited-terminal retention (docs/03 §7-1): exited terminals are removed
//! (holder released) after the retention period unless kept or part of a
//! task; `pod ls` hides exited terminals unless `--all`.

mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::*;
use podd::protocol::*;

fn pod(pipe: &str, args: &[&str]) -> String {
    static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let out = Command::new(EXE.get_or_init(pod_exe))
        .arg("--pipe")
        .arg(pipe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run pod");
    assert!(out.status.success(), "pod {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_cmd(c: &mut podd::client::Client, text: &str) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv: vec!["cmd".into(), "/c".into(), format!("echo {text}")],
        cwd: cwd(),
        ..Default::default()
    })
    .unwrap()
}

fn wait_exit(c: &mut podd::client::Client, id: &str) {
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::id(id),
            until: WaitUntil::Exit,
            timeout_ms: Some(30_000),
        })
        .unwrap();
    assert_eq!(w.outcome, WaitOutcome::Matched);
}

fn ids(c: &mut podd::client::Client) -> Vec<String> {
    c.call::<TerminalList>(&Empty {})
        .unwrap()
        .terminals
        .into_iter()
        .map(|t| t.id)
        .collect()
}

fn pipe_of_nothing() -> String {
    format!(r"\\.\pipe\{}", unique("retention-none"))
}

#[test]
fn exited_terminals_expire_unless_kept_and_ls_hides_them() {
    let name = unique("retention");
    let pipe = format!(r"\\.\pipe\{name}");
    let dir = std::env::temp_dir().join(&name);
    // Build pod.exe before the clock starts.
    let _ = pod(&pipe_of_nothing(), &["--version"]);
    let d = Daemon::start_with(pipe.clone(), dir.clone(), &["--retain-exited", "3s"]);
    let mut c = d.client();

    let running = powershell(&mut c, "keeps-running");
    let a = run_cmd(&mut c, "a");
    let b = run_cmd(&mut c, "b");
    wait_exit(&mut c, &a.id);
    wait_exit(&mut c, &b.id);
    let kept = c
        .call::<TerminalKeep>(&KeepParams {
            target: TerminalRef::id(&b.id),
            keep: true,
        })
        .unwrap();
    assert!(kept.kept && kept.expires_ms.is_none());
    assert!(dir.join(format!("{}.keep", b.id)).exists());

    let list = c.call::<TerminalList>(&Empty {}).unwrap();
    assert_eq!(list.retention_ms, Some(3000));
    let ra = list
        .retention
        .iter()
        .find(|r| r.id == a.id)
        .expect("a expires");
    let a_exited = list
        .terminals
        .iter()
        .find(|t| t.id == a.id)
        .and_then(|t| t.exited_ms)
        .unwrap();
    assert_eq!(ra.expires_ms, Some(a_exited + 3000));
    assert!(list.retention.iter().any(|r| r.id == b.id && r.kept));
    assert!(!list.retention.iter().any(|r| r.id == running.id));

    // pod ls: running only (with a hint); --all shows the exited ones (b is
    // kept, so it is there whatever the timing).
    let ls = pod(&pipe, &["ls"]);
    assert!(ls.contains(&running.id), "{ls}");
    assert!(
        !ls.contains(&format!("{} ", b.id)) && ls.contains("hidden"),
        "{ls}"
    );
    let all = pod(&pipe, &["ls", "--all"]);
    assert!(all.contains(&b.id) && all.contains(&running.id), "{all}");
    let json: ListResult = serde_json::from_str(&pod(&pipe, &["ls", "--json"])).unwrap();
    assert_eq!(json.terminals.len(), 1);

    // a goes (record removed, holder released); b and the running one stay.
    until("a removed by retention", 20, || {
        (!ids(&mut c).contains(&a.id)).then_some(())
    });
    let left = ids(&mut c);
    assert!(
        left.contains(&b.id) && left.contains(&running.id),
        "{left:?}"
    );
    until("a's holder record gone", 10, || {
        (!dir.join(format!("{}.json", a.id)).exists()).then_some(())
    });

    // The keep marker survives a daemon restart.
    drop(c);
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_with(pipe.clone(), dir.clone(), &["--retain-exited", "3s"]);
    let mut c = d.client();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        ids(&mut c).contains(&b.id),
        "kept terminal survived the restart"
    );
    let r = c
        .call::<TerminalKeep>(&KeepParams {
            target: TerminalRef::id(&b.id),
            keep: false,
        })
        .unwrap();
    assert!(!r.kept && r.expires_ms.is_some());
    assert!(!dir.join(format!("{}.keep", b.id)).exists());
    until("b removed once unkept", 20, || {
        (!ids(&mut c).contains(&b.id)).then_some(())
    });
    assert!(ids(&mut c).contains(&running.id));

    // Retention off: nothing expires.
    drop(c);
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_with(pipe, dir, &["--retain-exited", "off"]);
    let mut c = d.client();
    let e = run_cmd(&mut c, "e");
    wait_exit(&mut c, &e.id);
    let list = c.call::<TerminalList>(&Empty {}).unwrap();
    assert_eq!(list.retention_ms, None);
    assert!(list.retention.is_empty());
    std::thread::sleep(Duration::from_millis(1500));
    assert!(ids(&mut c).contains(&e.id));
}
