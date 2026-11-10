//! Backend seeds and on-disk worktrees shared by several scenario files.

use crate::source::TestBackend;

/// One repo, a working terminal and a blocked one, each in its own task.
pub(crate) fn seed_two_tasks(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    b.tasks.push(TestBackend::task(
        "sample/로그인-수정",
        "로그인-수정",
        "sample",
        &["w1"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/review-bot",
        "review-bot",
        "sample",
        &["b1"],
    ));
    b.terminals.push(TestBackend::terminal(
        "w1",
        "로그인 작업",
        Some("sample/로그인-수정"),
        "working",
    ));
    b.terminals.push(TestBackend::terminal(
        "b1",
        "리뷰 봇",
        Some("sample/review-bot"),
        "blocked",
    ));
}

/// A task whose worktree (and terminal cwd) is a real temporary folder.
pub(super) fn seed_link_task(b: &mut TestBackend, wt: &str) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    let mut t = TestBackend::task("sample/links", "links", "sample", &["L1"]);
    t.worktree = Some(wt.to_string());
    t.worktree_exists = true;
    b.tasks.push(t);
    let mut term = TestBackend::terminal("L1", "링크 터미널", Some("sample/links"), "working");
    term.cwd = wt.to_string();
    b.terminals.push(term);
}

pub(super) fn link_worktree() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let wt = dir.path().join("wt");
    std::fs::create_dir_all(wt.join("docs")).expect("worktree");
    std::fs::write(
        wt.join("docs").join("notes.txt"),
        "첫 줄\n둘째 줄 abc\n셋째 줄\n",
    )
    .expect("file");
    let s = wt.to_string_lossy().replace('\\', "/");
    (dir, s)
}
