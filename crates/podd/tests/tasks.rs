//! End-to-end tasks: a temporary git repository (under the system temp dir),
//! real `podd`/`pod-holder` binaries, a shell agent that edits and commits,
//! git facts and diff, a daemon restart, and removal rules.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;
use podd::client::Client;
use podd::protocol::*;

/// A throwaway repository whose path contains a space.
struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("repo"));
        let path = root.join("pod repo");
        std::fs::create_dir_all(path.join("docs")).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&path, &["config", k, v]);
        }
        // CRLF like PowerShell's Set-Content, so rewriting it to one line is -1.
        std::fs::write(path.join("README.md"), "line1\r\nline2\r\n").unwrap();
        std::fs::write(path.join("docs").join("guide.md"), "guide\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }

    fn path_str(&self) -> String {
        self.path.display().to_string()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn create(c: &mut Client, p: TaskCreateParams) -> TaskInfo {
    c.call::<TaskCreate>(&p).expect("task.create")
}

fn show(c: &mut Client, task: &str) -> TaskInfo {
    c.call::<TaskShow>(&TaskRef { task: task.into() })
        .expect("task.show")
}

fn shell_task(repo: &str, name: &str) -> TaskCreateParams {
    TaskCreateParams {
        repo: repo.into(),
        name: name.into(),
        agent: Some(AgentSpec::Kind(AgentKind::Shell)),
        ..Default::default()
    }
}

fn file<'a>(d: &'a TaskDiffResult, path: &str) -> &'a DiffFile {
    d.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in diff {:#?}", d.files))
}

#[test]
fn task_lifecycle_with_restart_and_removal_rules() {
    let repo = TempRepo::new();
    let daemon = Daemon::start("tasks");
    let mut c = daemon.client();

    // --- repo.add / list -------------------------------------------------
    assert_eq!(
        rpc_code(c.call::<RepoAdd>(&RepoAddParams {
            path: repo.path.join("docs").display().to_string(),
            name: Some("x".into()),
        })),
        codes::NOT_A_REPO,
        "a subdirectory is not a repository root"
    );
    assert_eq!(
        rpc_code(c.call::<RepoAdd>(&RepoAddParams {
            path: repo.root.display().to_string(),
            name: Some("x".into()),
        })),
        codes::NOT_A_REPO
    );
    // "pod repo" is not a valid default name (space): a name is required.
    assert_eq!(
        rpc_code(c.call::<RepoAdd>(&RepoAddParams {
            path: repo.path_str(),
            name: None,
        })),
        codes::INVALID_PARAMS
    );
    let r = c
        .call::<RepoAdd>(&RepoAddParams {
            path: repo.path_str(),
            name: Some("demo".into()),
        })
        .expect("repo.add");
    assert_eq!(r.default_base.as_deref(), Some("main"));
    assert!(Path::new(&r.path).ends_with("pod repo"), "{}", r.path);
    assert_eq!(
        rpc_code(c.call::<RepoAdd>(&RepoAddParams {
            path: repo.path_str().to_uppercase(),
            name: Some("other".into()),
        })),
        codes::REPO_EXISTS,
        "same path, different case"
    );
    assert_eq!(c.call::<RepoList>(&Empty {}).unwrap().repos.len(), 1);

    // --- task.create + validation -----------------------------------------
    let t = create(&mut c, shell_task("demo", "feat-1"));
    println!("created {t:#?}");
    assert_eq!(t.id, "demo/feat-1");
    assert_eq!(t.branch, "pod/feat-1");
    assert_eq!(t.base, "main");
    // Recorded in the long spelling git prints; TEMP (so the test's root)
    // may be an 8.3 short name such as RUNNER~1.
    assert_eq!(
        std::fs::canonicalize(&t.worktree).unwrap(),
        std::fs::canonicalize(daemon.worktrees_root().join("demo").join("feat-1")).unwrap(),
        "{}",
        t.worktree
    );
    assert!(t.worktree_exists);
    let tid = t.terminals[0].id.clone();
    let info = t.terminals[0].info.clone().expect("terminal info");
    assert_eq!(info.state, TerminalState::Running);
    assert_eq!(info.name.as_deref(), Some("demo/feat-1"));
    let g = t.git.clone().expect("git facts");
    assert_eq!(
        (g.ahead, g.behind, g.dirty, g.branch.as_deref()),
        (0, 0, 0, Some("pod/feat-1"))
    );

    let code = |c: &mut Client, p: TaskCreateParams| rpc_code(c.call::<TaskCreate>(&p));
    assert_eq!(
        code(&mut c, shell_task("demo", "feat-1")),
        codes::TASK_EXISTS
    );
    assert_eq!(
        code(&mut c, shell_task("demo", "a b")),
        codes::INVALID_PARAMS
    );
    assert_eq!(code(&mut c, shell_task("nope", "z")), codes::REPO_NOT_FOUND);
    assert_eq!(
        code(
            &mut c,
            TaskCreateParams {
                base: Some("no-such-ref".into()),
                ..shell_task("demo", "z")
            }
        ),
        codes::INVALID_PARAMS
    );
    assert_eq!(
        code(
            &mut c,
            TaskCreateParams {
                prompt: Some("hi".into()),
                ..shell_task("demo", "z")
            }
        ),
        codes::INVALID_PARAMS,
        "a prompt needs claude/codex"
    );
    // A branch that exists outside pod is a collision too.
    git(&repo.path, &["branch", "pod/taken"]);
    assert_eq!(
        code(&mut c, shell_task("demo", "taken")),
        codes::TASK_EXISTS
    );
    assert!(
        !daemon.worktrees_root().join("demo").join("z").exists(),
        "failed creates leave no worktree"
    );

    // --- the agent works in its worktree ------------------------------------
    send(
        &mut c,
        &tid,
        "echo \"task=$env:POD_TASK_ID leaf=$(Split-Path -Leaf (Get-Location))\"",
    );
    wait_output(&mut c, &tid, "task=demo/feat-1 leaf=feat-1");
    send(
        &mut c,
        &tid,
        "Set-Content -Path f.txt -Value 'a','b','c'; Set-Content -Path README.md -Value 'line1'; \
         git add -A; git commit -q -m 'add f'; if ($LASTEXITCODE -eq 0) { echo commit-ok } else { echo commit-failed }",
    );
    wait_output(&mut c, &tid, "commit-ok");
    send(
        &mut c,
        &tid,
        "Set-Content -Path notes.txt -Value 'x','y'; echo notes-ok",
    );
    wait_output(&mut c, &tid, "notes-ok");
    // The base moves on.
    std::fs::write(repo.path.join("other.txt"), "o\n").unwrap();
    git(&repo.path, &["add", "other.txt"]);
    git(&repo.path, &["commit", "-q", "-m", "base moves"]);

    let s = show(&mut c, "feat-1"); // bare name works when unique
    let g = s.git.clone().expect("git facts");
    println!("after commit: {g:#?}");
    assert_eq!((g.ahead, g.behind, g.dirty, g.unpushed), (1, 1, 1, 1));
    assert_eq!(g.last_commit.as_ref().unwrap().summary, "add f");

    let d = c
        .call::<TaskDiff>(&TaskRef {
            task: "demo/feat-1".into(),
        })
        .expect("task.diff");
    println!("diff: {d:#?}");
    let f = file(&d, "f.txt");
    assert_eq!(
        (f.status.as_str(), f.added, f.deleted),
        ("A", Some(3), Some(0))
    );
    let rd = file(&d, "README.md");
    assert_eq!(
        (rd.status.as_str(), rd.added, rd.deleted),
        ("M", Some(0), Some(1))
    );
    let n = file(&d, "notes.txt");
    assert_eq!((n.status.as_str(), n.added), ("?", Some(2)));
    assert!(
        !d.files.iter().any(|f| f.path == "other.txt"),
        "base-only changes are not part of the task diff"
    );
    assert_eq!(d.files.len(), 3);
    assert_eq!((d.added, d.deleted), (5, 1));

    // --- child task: parent recorded from the caller's terminal ------------
    let child = create(
        &mut c,
        TaskCreateParams {
            base: Some("pod/feat-1".into()),
            agent: Some(AgentSpec::Argv {
                argv: vec!["cmd".into(), "/c".into(), "echo child-ok".into()],
            }),
            parent_terminal: Some(tid.clone()),
            ..shell_task("demo", "feat-2")
        },
    );
    assert_eq!(child.parent_task.as_deref(), Some("demo/feat-1"));
    assert_eq!(child.parent_terminal.as_deref(), Some(tid.as_str()));
    assert_eq!(
        Some(child.base_commit.clone()),
        s.git.as_ref().unwrap().head.clone()
    );
    // An extra terminal in the task.
    let extra = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into(), "/c".into(), "echo %POD_TASK_ID% & cd".into()],
            task: Some("demo/feat-2".into()),
            ..Default::default()
        })
        .expect("run in task");
    wait_output(&mut c, &extra.id, "demo/feat-2");
    assert!(Path::new(&extra.cwd).ends_with("feat-2"), "{}", extra.cwd);
    let shown = show(&mut c, "demo/feat-2");
    assert_eq!(shown.terminals.len(), 2);
    // Aggregate agent status over the task's terminals.
    let ts = shown.status.expect("task status");
    assert_eq!(ts.terminals, 2);
    assert!(shown.terminals.iter().any(|t| t.id == ts.terminal));
    assert_eq!(
        rpc_code(c.call::<RepoRemove>(&RepoRef {
            repo: "demo".into()
        })),
        codes::REPO_IN_USE
    );

    // --- daemon restart: tasks and terminals survive ------------------------
    let pid = info.pid.unwrap();
    drop(c);
    let (pipe, dir) = daemon.stop(false);
    assert!(process_alive(pid), "agent died with the daemon");
    let daemon = Daemon::start_on(pipe, dir);
    let mut c = daemon.client();
    let list = c.call::<TaskList>(&TaskListParams::default()).unwrap();
    assert_eq!(list.tasks.len(), 2);
    let t1 = list.tasks.iter().find(|t| t.id == "demo/feat-1").unwrap();
    let ti = t1.terminals[0].info.as_ref().expect("terminal re-attached");
    assert_eq!((ti.state, ti.pid), (TerminalState::Running, Some(pid)));
    assert_eq!(t1.git.as_ref().unwrap().ahead, 1);
    send(&mut c, &tid, "echo after-restart");
    wait_output(&mut c, &tid, "after-restart");
    // New ids do not collide with ids tasks refer to.
    let fresh = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into(), "/c".into(), "exit 0".into()],
            cwd: cwd(),
            ..Default::default()
        })
        .unwrap();
    let used: Vec<String> = list
        .tasks
        .iter()
        .flat_map(|t| t.terminals.iter().map(|x| x.id.clone()))
        .collect();
    assert!(!used.contains(&fresh.id), "{} reused", fresh.id);

    // --- removal rules ------------------------------------------------------
    let err = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "demo/feat-1".into(),
            force: false,
            delete_branch: true,
        })
        .unwrap_err();
    match err {
        podd::client::ClientError::Rpc(e) => {
            assert_eq!(e.code, codes::TASK_DIRTY, "{e}");
            assert_eq!(e.data, Some(serde_json::json!({"dirty": 1, "unpushed": 1})));
        }
        other => panic!("{other:?}"),
    }
    assert!(
        Path::new(&t1.worktree).is_dir(),
        "a refused remove changes nothing"
    );
    let r = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "demo/feat-1".into(),
            force: true,
            delete_branch: true,
        })
        .expect("forced remove");
    assert_eq!(r.terminals, vec![tid.clone()]);
    // docs/19 §3.6: an unmerged branch is kept even with force + delete_branch.
    assert!(r.worktree_removed && !r.branch_deleted, "{r:?}");
    assert!(
        r.branch_kept
            .as_deref()
            .unwrap_or("")
            .contains("not merged"),
        "{r:?}"
    );
    assert!(!Path::new(&t1.worktree).exists());
    assert!(git(&repo.path, &["branch", "--list", "pod/feat-1"]).contains("pod/feat-1"));
    let terms = c.call::<TerminalList>(&Empty {}).unwrap().terminals;
    assert!(!terms.iter().any(|t| t.id == tid));
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!process_alive(pid), "agent process survived task removal");

    // feat-2 is clean and has nothing unpushed (its base branch is gone; the
    // recorded base commit is used), so no force is needed.
    let r = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "feat-2".into(),
            force: false,
            delete_branch: false,
        })
        .expect("clean remove");
    assert!(r.worktree_removed && !r.branch_deleted, "{r:?}");
    assert!(git(&repo.path, &["branch", "--list", "pod/feat-2"]).contains("pod/feat-2"));
    let wts = git(&repo.path, &["worktree", "list", "--porcelain"]);
    assert_eq!(wts.matches("worktree ").count(), 1, "{wts}");
    assert!(
        c.call::<TaskList>(&TaskListParams::default())
            .unwrap()
            .tasks
            .is_empty()
    );
    c.call::<RepoRemove>(&RepoRef {
        repo: "demo".into(),
    })
    .expect("repo.remove");
    assert!(c.call::<RepoList>(&Empty {}).unwrap().repos.is_empty());
}

#[test]
fn registry_survives_restart_and_unknown_tasks_error() {
    let repo = TempRepo::new();
    let daemon = Daemon::start("registry");
    let mut c = daemon.client();
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path_str(),
        name: Some("keep".into()),
    })
    .unwrap();
    let (pipe, dir) = daemon.stop(false);
    let daemon = Daemon::start_on(pipe, dir);
    let mut c = daemon.client();
    let repos = c.call::<RepoList>(&Empty {}).unwrap().repos;
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "keep");
    assert_eq!(
        rpc_code(c.call::<TaskShow>(&TaskRef {
            task: "keep/none".into()
        })),
        codes::TASK_NOT_FOUND
    );
    assert_eq!(
        rpc_code(c.call::<TaskDiff>(&TaskRef {
            task: "none".into()
        })),
        codes::TASK_NOT_FOUND
    );
    // By path as well as by name.
    c.call::<RepoRemove>(&RepoRef {
        repo: repo.path_str(),
    })
    .expect("remove by path");
}
