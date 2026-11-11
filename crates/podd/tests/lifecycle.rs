//! v9 lifecycle events on `status.subscribe` (`lifecycle: true`):
//! `terminal.changed`, `task.changed`, `task.removing`, and `task.touch`.
//! Real `podd`/`pod-holder`, a throwaway git repository under the temp dir.

mod common;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use pod_test_support::git::git_ok;
use podd::client::SubscribeEvent;
use podd::protocol::*;

struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("lc-repo"));
        let path = root.join("repo");
        std::fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git_ok(&path, &["config", k, v]);
        }
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Events from a lifecycle subscription, read on a thread.
struct Events {
    rx: mpsc::Receiver<SubscribeEvent>,
    log: Vec<String>,
}

impl Events {
    /// The next event `f` accepts (others are skipped and logged).
    fn wait<T>(
        &mut self,
        what: &str,
        secs: u64,
        mut f: impl FnMut(&SubscribeEvent) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(ev) => {
                    if let Some(v) = f(&ev) {
                        return v;
                    }
                    self.log.push(format!("{ev:?}").chars().take(160).collect());
                }
                Err(_) => panic!("no {what} within {secs} s; skipped: {:#?}", self.log),
            }
        }
    }
}

fn task_ev(ev: &SubscribeEvent, id: &str, change: TaskChange) -> Option<TaskChangedEvent> {
    match ev {
        SubscribeEvent::Task(e) if e.id == id && e.change == change => Some((**e).clone()),
        _ => None,
    }
}

fn term_ev(
    ev: &SubscribeEvent,
    id: Option<&str>,
    change: TerminalChange,
) -> Option<TerminalChangedEvent> {
    match ev {
        SubscribeEvent::Terminal(e) if id.is_none_or(|i| e.id == i) && e.change == change => {
            Some((**e).clone())
        }
        _ => None,
    }
}

#[test]
fn lifecycle_events_and_task_touch() {
    let repo = TempRepo::new();
    let name = unique("lifecycle");
    // A long grace so the test can save a file between `task.removing` and deletion.
    // SAFETY: this test binary has a single test; nothing reads the
    // environment concurrently.
    unsafe { std::env::set_var("POD_TASK_REMOVE_GRACE_MS", "3000") };
    let d = Daemon::start_on(
        format!(r"\\.\pipe\{name}"),
        std::env::temp_dir().join(&name),
    );
    let mut c = d.client();

    // A plain subscriber (no lifecycle) must not see the new kinds.
    let (_, mut plain) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (ptx, plain_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = plain.next_any() {
            if matches!(
                ev,
                SubscribeEvent::Terminal(_)
                    | SubscribeEvent::Task(_)
                    | SubscribeEvent::TaskRemoving(_)
            ) && ptx.send(format!("{ev:?}")).is_err()
            {
                return;
            }
        }
    });

    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams {
            lifecycle: true,
            ..Default::default()
        })
        .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if tx.send(ev).is_err() {
                return;
            }
        }
    });
    let mut ev = Events {
        rx,
        log: Vec::new(),
    };

    // --- terminal created / exited / removed ------------------------------
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd.exe".into(), "/c".into(), "exit 3".into()],
            cwd: cwd(),
            ..Default::default()
        })
        .unwrap();
    let created = ev.wait("terminal created", 10, |e| {
        term_ev(e, Some(&t.id), TerminalChange::Created)
    });
    assert_eq!(created.terminal.as_ref().unwrap().argv[0], "cmd.exe");
    let exited = ev.wait("terminal exited", 20, |e| {
        term_ev(e, Some(&t.id), TerminalChange::Exited)
    });
    let info = exited.terminal.expect("terminal info on exit");
    assert_eq!(
        (info.state, info.exit_code),
        (TerminalState::Exited, Some(3))
    );
    assert!(exited.seq > created.seq, "one daemon-wide sequence");
    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id(&t.id),
        force: false,
    })
    .unwrap();
    let removed = ev.wait("terminal removed", 10, |e| {
        term_ev(e, Some(&t.id), TerminalChange::Removed)
    });
    assert!(removed.terminal.is_none());

    // --- task created ------------------------------------------------------
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some("lc".into()),
    })
    .unwrap();
    let shell = AgentSpec::Argv {
        argv: vec!["cmd.exe".into(), "/q".into(), "/k".into()],
    };
    let task = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "lc".into(),
            name: "one".into(),
            agent: Some(shell.clone()),
            ..Default::default()
        })
        .unwrap();
    let agent_term = task.terminals[0].id.clone();
    ev.wait("agent terminal created", 10, |e| {
        term_ev(e, Some(&agent_term), TerminalChange::Created)
    });
    let created = ev.wait("task created", 10, |e| {
        task_ev(e, "lc/one", TaskChange::Created)
    });
    // docs/19 §3.1: a task made outside a Pod terminal opens with focus.
    assert_eq!(
        created.open_hint,
        Some(OpenHint {
            focus: true,
            reason: OpenReason::User
        })
    );
    let info = created.task.expect("task info");
    assert_eq!(info.worktree, task.worktree);
    assert_eq!(info.git.as_ref().map(|g| g.dirty), Some(0));

    // --- task.touch: git refreshed now, `git` then `touched` ---------------
    let wt = PathBuf::from(&task.worktree);
    std::fs::write(wt.join("new.txt"), "saved by an editor\n").unwrap();
    let r = c
        .call::<TaskTouch>(&TaskTouchParams {
            task: "one".into(),
            paths: vec!["new.txt".into()],
        })
        .unwrap();
    assert_eq!(r.task, "lc/one");
    assert_eq!(r.git.as_ref().map(|g| g.dirty), Some(1));
    assert!(r.git_changed, "{r:?}");
    let g = ev.wait("git change after touch", 10, |e| {
        task_ev(e, "lc/one", TaskChange::Git)
    });
    assert_eq!(g.git.map(|g| g.dirty), Some(1));
    let touched = ev.wait("touched", 10, |e| task_ev(e, "lc/one", TaskChange::Touched));
    assert_eq!(touched.paths, vec!["new.txt".to_string()]);
    // Touching again without changes: no git change, still `touched`.
    let r = c
        .call::<TaskTouch>(&TaskTouchParams {
            task: "lc/one".into(),
            paths: vec![],
        })
        .unwrap();
    assert!(!r.git_changed);
    ev.wait("second touched", 10, |e| {
        task_ev(e, "lc/one", TaskChange::Touched)
    });

    // --- an edit made outside Pod: picked up by the poll (lifecycle subscriber)
    std::fs::write(wt.join("other.txt"), "outside\n").unwrap();
    git_ok(&wt, &["add", "-A"]);
    git_ok(&wt, &["commit", "-q", "-m", "work"]);
    let g = ev.wait("polled git change", 20, |e| {
        task_ev(e, "lc/one", TaskChange::Git)
    });
    let g = g.git.unwrap();
    assert_eq!((g.ahead, g.dirty), (1, 0), "{g:?}");

    // --- a second terminal in the task -------------------------------------
    let t2 = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd.exe".into(), "/q".into(), "/k".into()],
            task: Some("lc/one".into()),
            ..Default::default()
        })
        .unwrap();
    let terms = ev.wait("task terminals", 10, |e| {
        task_ev(e, "lc/one", TaskChange::Terminals)
    });
    assert_eq!(terms.terminals, vec![agent_term.clone(), t2.id.clone()]);

    // --- task.removing; a save during the grace keeps the task -------------
    // (`one` has an unpushed commit, so use a clean second task.)
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "lc".into(),
        name: "two".into(),
        agent: Some(shell),
        ..Default::default()
    })
    .unwrap();
    ev.wait("task two created", 10, |e| {
        task_ev(e, "lc/two", TaskChange::Created)
    });
    let pipe = d.pipe.clone();
    let remover = std::thread::spawn(move || {
        podd::client::Client::connect(&pipe)
            .unwrap()
            .call::<TaskRemove>(&TaskRemoveParams {
                task: "lc/two".into(),
                force: false,
                delete_branch: false,
            })
    });
    let removing = ev.wait("task.removing", 10, |e| match e {
        SubscribeEvent::TaskRemoving(r) if r.id == "lc/two" => Some((**r).clone()),
        _ => None,
    });
    assert_eq!((removing.force, removing.grace_ms), (false, 3000));
    let wt2 = PathBuf::from(&removing.worktree);
    std::fs::write(wt2.join("unsaved.txt"), "editor saved on task.removing\n").unwrap();
    let err = remover.join().unwrap().expect_err("dirty after the grace");
    assert_eq!(rpc_code::<()>(Err(err)), codes::TASK_DIRTY);
    assert!(wt2.join("unsaved.txt").exists(), "the saved file survives");

    // --- forced removal: removing, terminals removed, task removed --------
    c.call::<TaskRemove>(&TaskRemoveParams {
        task: "lc/one".into(),
        force: true,
        delete_branch: true,
    })
    .unwrap();
    let removing = ev.wait("task.removing one", 10, |e| match e {
        SubscribeEvent::TaskRemoving(r) if r.id == "lc/one" => Some((**r).clone()),
        _ => None,
    });
    assert!(removing.force);
    ev.wait("agent terminal removed", 15, |e| {
        term_ev(e, Some(&agent_term), TerminalChange::Removed)
    });
    ev.wait("task removed", 15, |e| {
        task_ev(e, "lc/one", TaskChange::Removed)
    });

    // The plain subscriber saw none of it.
    assert!(
        plain_rx.try_recv().is_err(),
        "lifecycle events leaked to a subscriber without `lifecycle`"
    );
    let _ = c.call::<TaskRemove>(&TaskRemoveParams {
        task: "lc/two".into(),
        force: true,
        delete_branch: true,
    });
}
