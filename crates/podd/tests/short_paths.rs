//! Windows 8.3 short names end to end (GitHub runners keep TEMP under
//! `C:\Users\RUNNER~1`): a daemon whose state dir and worktrees root are
//! named through the short spelling, a repository added through it, and a
//! registry written with the other spelling (older state). git prints the
//! long spelling; Pod must still find, remove and clean up its worktrees.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use podd::protocol::*;

/// The 8.3 short spelling of an existing path, `None` when the volume has
/// no short names.
fn short_spelling(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated; `buf` is as long as the size passed.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    let n = n as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let s = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    s.to_string_lossy().contains('~').then_some(s)
}

/// `canonicalize` without the `\\?\` prefix: git's spelling.
fn long_spelling(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap();
    PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"))
}

fn key(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn shell_task(repo: &str, name: &str) -> TaskCreateParams {
    TaskCreateParams {
        repo: repo.into(),
        name: name.into(),
        agent: Some(AgentSpec::Kind(AgentKind::Shell)),
        ..Default::default()
    }
}

#[test]
fn short_spelled_roots_and_old_registry_spellings_match_git() {
    let base = std::env::temp_dir().join(unique("short"));
    std::fs::create_dir_all(base.join("RepositoryFolder")).unwrap();
    let long = long_spelling(&base);
    let Some(short) = short_spelling(&long) else {
        eprintln!("skipped: no 8.3 short names on this volume");
        let _ = std::fs::remove_dir_all(&base);
        return;
    };
    let repo = long.join("RepositoryFolder");
    git(&repo, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Pod Test"),
        ("user.email", "pod-test@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        git(&repo, &["config", k, v]);
    }
    std::fs::write(repo.join("README.md"), "x\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    // State dir and worktrees root named through the short spelling.
    let pipe = format!(r"\\.\pipe\{}", unique("short"));
    let d = Daemon::start_on(pipe.clone(), short.join("state"));
    let mut c = d.client();
    let r = c
        .call::<RepoAdd>(&RepoAddParams {
            path: short.join("RepositoryFolder").display().to_string(),
            name: Some("demo".into()),
        })
        .expect("repo.add through the short spelling");
    assert_eq!(
        key(&r.path),
        key(&repo.display().to_string()),
        "git's spelling"
    );
    assert_eq!(
        rpc_code(c.call::<RepoAdd>(&RepoAddParams {
            path: repo.display().to_string(),
            name: Some("again".into()),
        })),
        codes::REPO_EXISTS,
        "the long spelling is the same repository"
    );

    // Task worktrees are recorded in the long spelling git prints.
    let t1 = c.call::<TaskCreate>(&shell_task("demo", "one")).unwrap();
    assert!(
        key(&t1.worktree).starts_with(&key(&long.display().to_string())),
        "{}",
        t1.worktree
    );
    let gone = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "demo/one".into(),
            force: true,
            delete_branch: true,
        })
        .expect("task.remove");
    assert!(gone.worktree_removed && gone.branch_deleted, "{gone:?}");
    assert!(!Path::new(&t1.worktree).exists());

    // A registry written with the short spelling (older Pod) still matches.
    let t2 = c.call::<TaskCreate>(&shell_task("demo", "two")).unwrap();
    let (pipe, state_dir) = d.stop(true);
    let reg = state_dir.join("tasks").join("registry.json");
    let text = std::fs::read_to_string(&reg).unwrap();
    let esc = |p: &Path| p.display().to_string().replace('\\', r"\\");
    let rewritten = text.replace(&esc(&long), &esc(&short));
    assert_ne!(rewritten, text, "the registry names the long spelling");
    std::fs::write(&reg, rewritten).unwrap();
    let d = Daemon::start_on(pipe, state_dir);
    let mut c = d.client();
    let listed = c.call::<TaskList>(&TaskListParams::default()).unwrap();
    let rec = listed.tasks.iter().find(|t| t.id == "demo/two").unwrap();
    assert!(rec.worktree.contains('~'), "{}", rec.worktree);
    let gone = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "demo/two".into(),
            force: true,
            delete_branch: true,
        })
        .expect("task.remove with an old registry spelling");
    assert!(gone.worktree_removed && gone.branch_deleted, "{gone:?}");
    assert!(!Path::new(&t2.worktree).exists());
    assert!(
        git(&repo, &["branch", "--list", "pod/two"])
            .trim()
            .is_empty()
    );
    d.stop(true);
    let _ = std::fs::remove_dir_all(&base);
}
