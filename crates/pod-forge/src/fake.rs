//! A fake GitHub for tests, demos and screenshots: pull requests, checks and
//! pushes live in a [`FakeState`], optionally a JSON file re-read on every
//! call (so a test can flip a PR's checks from outside the daemon:
//! `POD_FORGE=fake:<file>`). Nothing leaves the machine.
//!
//! Remote detection is the real one (`git remote -v`, local only) unless the
//! state overrides it, so "no remote" behaves exactly as with `gh`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, ForgeErrorKind};
use crate::runner::ProcessRunner;
use crate::{
    CreatePr, Forge, ForgeRemote, MergeMethod, PrCheck, PrComment, PrState, PrView, ReviewDecision,
    fixtures,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FakePr {
    pub view: PrView,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub checks: Vec<PrCheck>,
    #[serde(default)]
    pub comments: Vec<PrComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_with: Option<MergeMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakePush {
    pub remote: String,
    pub branch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FakeState {
    /// Use this remote instead of reading `git remote -v`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<ForgeRemote>,
    /// Pretend the repository has no remote.
    #[serde(default)]
    pub no_remote: bool,
    #[serde(default)]
    pub next_number: u64,
    #[serde(default)]
    pub prs: Vec<FakePr>,
    #[serde(default)]
    pub pushes: Vec<FakePush>,
    /// Checks a new pull request starts with (default: fixture "pending").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_checks: Option<Vec<PrCheck>>,
    /// Injected failures by operation (`detect`, `push`, `create`, `find`,
    /// `view`, `checks`, `comments`, `merge`, `close`). They stay until removed.
    #[serde(default)]
    pub fail: BTreeMap<String, ForgeError>,
    /// Every operation, in order (`push origin pod/x`, `create pod/x`...).
    #[serde(default)]
    pub calls: Vec<String>,
}

impl Default for PrView {
    fn default() -> Self {
        Self {
            number: 0,
            url: String::new(),
            title: String::new(),
            state: PrState::Open,
            draft: false,
            head: String::new(),
            head_oid: None,
            base: String::new(),
            review_decision: None,
            merge_state: None,
            mergeable: None,
            additions: 0,
            deletions: 0,
            changed_files: 0,
            author: None,
            merged_at: None,
            closed_at: None,
        }
    }
}

impl FakeState {
    pub fn pr_mut(&mut self, number: u64) -> Option<&mut FakePr> {
        self.prs.iter_mut().find(|p| p.view.number == number)
    }

    pub fn pr_for(&self, branch: &str) -> Option<&FakePr> {
        let mut v: Vec<&FakePr> = self.prs.iter().filter(|p| p.view.head == branch).collect();
        v.sort_by_key(|p| std::cmp::Reverse(p.view.number));
        v.iter()
            .find(|p| p.view.state == PrState::Open)
            .or(v.first())
            .copied()
    }
}

pub struct FakeForge {
    file: Option<PathBuf>,
    mem: Mutex<FakeState>,
}

impl FakeForge {
    pub fn new(state: FakeState) -> Self {
        Self {
            file: None,
            mem: Mutex::new(state),
        }
    }

    /// State kept in `file` (created on first write; missing = empty state).
    pub fn with_file(file: PathBuf) -> Self {
        Self {
            file: Some(file),
            mem: Mutex::new(FakeState::default()),
        }
    }

    pub fn load_file(file: &Path) -> FakeState {
        std::fs::read(file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_file(file: &Path, state: &FakeState) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("json.tmp");
        let bytes = pod_storage::json::to_pretty_bytes(state)?;
        pod_storage::replace::write_then_rename(file, &tmp, &bytes)
    }

    /// A copy of the current state.
    pub fn state(&self) -> FakeState {
        let mut mem = self.mem.lock().unwrap();
        if let Some(f) = &self.file {
            *mem = Self::load_file(f);
        }
        mem.clone()
    }

    /// Change the state (re-read from and written back to the file, if any).
    pub fn edit<T>(&self, f: impl FnOnce(&mut FakeState) -> T) -> T {
        let mut mem = self.mem.lock().unwrap();
        if let Some(file) = &self.file {
            *mem = Self::load_file(file);
        }
        let out = f(&mut mem);
        if let Some(file) = &self.file {
            let _ = Self::save_file(file, &mem);
        }
        out
    }

    /// Record `call`, then fail if `op` has an injected failure.
    fn op<T>(
        &self,
        op: &str,
        call: String,
        f: impl FnOnce(&mut FakeState) -> Result<T, ForgeError>,
    ) -> Result<T, ForgeError> {
        self.edit(|s| {
            s.calls.push(call);
            if let Some(e) = s.fail.get(op) {
                return Err(e.clone());
            }
            f(s)
        })
    }
}

fn not_found(number: u64) -> ForgeError {
    ForgeError::new(
        ForgeErrorKind::NotFound,
        format!(
            "GraphQL: Could not resolve to a PullRequest with the number of {number}. (repository.pullRequest)"
        ),
    )
}

fn merge_state(checks: &[PrCheck]) -> String {
    match crate::ChecksSummary::of(checks).verdict() {
        "failed" | "pending" => "BLOCKED".into(),
        _ => "CLEAN".into(),
    }
}

impl Forge for FakeForge {
    fn detect(&self, dir: &Path) -> Result<ForgeRemote, ForgeError> {
        let s = self.op("detect", "detect".into(), |s| Ok(s.clone()))?;
        if s.no_remote {
            return Err(ForgeError::no_remote("this repository has no git remote"));
        }
        match s.remote {
            Some(r) => Ok(r),
            None => crate::remote::detect(&ProcessRunner, dir),
        }
    }

    fn push(&self, _dir: &Path, remote: &ForgeRemote, branch: &str) -> Result<(), ForgeError> {
        self.op("push", format!("push {} {branch}", remote.name), |s| {
            let p = FakePush {
                remote: remote.name.clone(),
                branch: branch.into(),
            };
            if !s.pushes.contains(&p) {
                s.pushes.push(p);
            }
            Ok(())
        })
    }

    fn create(
        &self,
        _dir: &Path,
        remote: &ForgeRemote,
        req: &CreatePr,
    ) -> Result<PrView, ForgeError> {
        self.op("create", format!("create {} -> {}{}", req.head, req.base, if req.draft { " draft" } else { "" }), |s| {
            if !s.pushes.iter().any(|p| p.branch == req.head) {
                return Err(ForgeError::failed(format!(
                    "pull request create failed: GraphQL: Head sha can't be blank, Base sha can't be blank, No commits between {} and {}, Head ref must be a branch (createPullRequest)",
                    req.base, req.head
                )));
            }
            if let Some(open) = s.prs.iter().find(|p| p.view.head == req.head && p.view.state == PrState::Open) {
                return Ok(open.view.clone());
            }
            s.next_number = s.next_number.max(s.prs.iter().map(|p| p.view.number).max().unwrap_or(0)) + 1;
            let checks = s.new_checks.clone().unwrap_or_else(fixtures::checks_pending);
            let view = PrView {
                number: s.next_number,
                url: format!("{}/pull/{}", remote.slug.web_url(), s.next_number),
                title: req.title.clone(),
                state: PrState::Open,
                draft: req.draft,
                head: req.head.clone(),
                base: req.base.clone(),
                review_decision: Some(ReviewDecision::ReviewRequired),
                merge_state: Some(merge_state(&checks)),
                mergeable: Some("MERGEABLE".into()),
                author: Some("pod-fixture".into()),
                ..PrView::default()
            };
            s.prs.push(FakePr { view: view.clone(), body: req.body.clone(), checks, comments: Vec::new(), merged_with: None });
            Ok(view)
        })
    }

    fn find_for_branch(
        &self,
        _dir: &Path,
        _remote: &ForgeRemote,
        branch: &str,
    ) -> Result<Option<PrView>, ForgeError> {
        self.op("find", format!("find {branch}"), |s| {
            Ok(s.pr_for(branch).map(|p| p.view.clone()))
        })
    }

    fn view(&self, _dir: &Path, _remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError> {
        self.op("view", format!("view {number}"), |s| {
            s.prs
                .iter()
                .find(|p| p.view.number == number)
                .map(|p| p.view.clone())
                .ok_or_else(|| not_found(number))
        })
    }

    fn checks(
        &self,
        _dir: &Path,
        _remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrCheck>, ForgeError> {
        self.op("checks", format!("checks {number}"), |s| {
            s.prs
                .iter()
                .find(|p| p.view.number == number)
                .map(|p| p.checks.clone())
                .ok_or_else(|| not_found(number))
        })
    }

    fn review_comments(
        &self,
        _dir: &Path,
        _remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrComment>, ForgeError> {
        self.op("comments", format!("comments {number}"), |s| {
            s.prs
                .iter()
                .find(|p| p.view.number == number)
                .map(|p| p.comments.clone())
                .ok_or_else(|| not_found(number))
        })
    }

    fn merge(
        &self,
        dir: &Path,
        _remote: &ForgeRemote,
        number: u64,
        method: MergeMethod,
    ) -> Result<PrView, ForgeError> {
        // The merged commit is the head branch's tip in `dir` right now (like
        // GitHub's headRefOid of a merged pull request).
        let tip = |head: &str| {
            let inv = crate::runner::Invocation::new(
                "git",
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{head}"),
                ],
                std::time::Duration::from_secs(20),
            );
            crate::runner::Runner::run(&ProcessRunner, dir, &inv)
                .ok()
                .filter(|o| o.ok())
                .map(|o| o.stdout.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        self.op("merge", format!("merge {number} {}", method.flag()), |s| {
            let pr = s.pr_mut(number).ok_or_else(|| not_found(number))?;
            match pr.view.state {
                PrState::Merged => {
                    return Err(ForgeError::failed(format!(
                        "Pull request #{number} was already merged"
                    )));
                }
                PrState::Closed => {
                    return Err(ForgeError::failed(format!(
                        "Pull request #{number} is closed"
                    )));
                }
                PrState::Open => {}
            }
            if pr.view.draft {
                return Err(ForgeError::failed(format!(
                    "Pull request #{number} is still a draft"
                )));
            }
            let now = crate::time::now();
            pr.view.state = PrState::Merged;
            pr.view.head_oid = tip(&pr.view.head).or(pr.view.head_oid.take());
            pr.view.merged_at = Some(now.clone());
            pr.view.closed_at = Some(now);
            pr.view.merge_state = Some("UNKNOWN".into());
            pr.view.mergeable = Some("UNKNOWN".into());
            pr.merged_with = Some(method);
            Ok(pr.view.clone())
        })
    }

    fn close(&self, _dir: &Path, _remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError> {
        self.op("close", format!("close {number}"), |s| {
            let pr = s.pr_mut(number).ok_or_else(|| not_found(number))?;
            if pr.view.state == PrState::Open {
                pr.view.state = PrState::Closed;
                pr.view.closed_at = Some(crate::time::now());
            }
            Ok(pr.view.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::collect;
    use crate::{CheckBucket, RepoSlug};

    fn remote() -> ForgeRemote {
        ForgeRemote {
            name: "origin".into(),
            url: "https://github.com/pod-fixtures/demo.git".into(),
            slug: RepoSlug {
                host: "github.com".into(),
                owner: "pod-fixtures".into(),
                name: "demo".into(),
            },
        }
    }

    fn forge() -> FakeForge {
        FakeForge::new(FakeState {
            remote: Some(remote()),
            ..FakeState::default()
        })
    }

    fn req(draft: bool) -> CreatePr {
        CreatePr {
            head: "pod/feat".into(),
            base: "main".into(),
            title: "기능 추가".into(),
            body: "본문".into(),
            draft,
        }
    }

    #[test]
    fn create_requires_a_push_then_tracks_checks_to_merge() {
        let f = forge();
        let dir = Path::new(".");
        let r = f.detect(dir).unwrap();
        assert!(f.create(dir, &r, &req(false)).is_err(), "not pushed yet");
        f.push(dir, &r, "pod/feat").unwrap();
        let pr = f.create(dir, &r, &req(false)).unwrap();
        assert_eq!(
            (pr.number, pr.state, pr.url.as_str()),
            (
                1,
                PrState::Open,
                "https://github.com/pod-fixtures/demo/pull/1"
            )
        );
        assert_eq!(
            f.create(dir, &r, &req(false)).unwrap().number,
            1,
            "an open PR for the branch is reused"
        );

        let st = collect(&f, dir, "pod/feat", None);
        assert_eq!(st.summary.verdict(), "pending");
        assert_eq!(
            st.pr.as_ref().unwrap().merge_state.as_deref(),
            Some("BLOCKED")
        );

        f.edit(|s| s.pr_mut(1).unwrap().checks = fixtures::checks_failed());
        assert_eq!(
            collect(&f, dir, "pod/feat", Some(1)).summary.verdict(),
            "failed"
        );
        f.edit(|s| s.pr_mut(1).unwrap().checks = fixtures::checks_passed());
        let st = collect(&f, dir, "pod/feat", Some(1));
        assert_eq!(st.summary.verdict(), "passed");
        assert!(st.checks.iter().all(|c| c.bucket == CheckBucket::Pass));

        let merged = f.merge(dir, &r, 1, MergeMethod::Squash).unwrap();
        assert_eq!(merged.state, PrState::Merged);
        assert!(merged.merged_at.is_some());
        assert!(
            f.merge(dir, &r, 1, MergeMethod::Squash).is_err(),
            "merged twice"
        );
        // A merged PR is still the branch's PR; a new one may be opened after.
        assert_eq!(
            collect(&f, dir, "pod/feat", None).pr.unwrap().state,
            PrState::Merged
        );
        assert_eq!(
            f.state()
                .calls
                .iter()
                .filter(|c| c.starts_with("merge"))
                .count(),
            2
        );
    }

    #[test]
    fn drafts_cannot_merge_and_close_works() {
        let f = forge();
        let dir = Path::new(".");
        let r = remote();
        f.push(dir, &r, "pod/feat").unwrap();
        let pr = f.create(dir, &r, &req(true)).unwrap();
        assert!(pr.draft);
        assert!(
            f.merge(dir, &r, pr.number, MergeMethod::Merge)
                .unwrap_err()
                .message
                .contains("draft")
        );
        assert_eq!(f.close(dir, &r, pr.number).unwrap().state, PrState::Closed);
    }

    #[test]
    fn injected_failures_and_no_remote() {
        let f = FakeForge::new(FakeState {
            no_remote: true,
            ..FakeState::default()
        });
        let st = collect(&f, Path::new("."), "pod/feat", None);
        assert!(st.no_remote());
        let f = forge();
        f.edit(|s| {
            s.fail.insert("push".into(), ForgeError::classify("git", Some(128), "fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
        });
        assert_eq!(
            f.push(Path::new("."), &remote(), "pod/x").unwrap_err().kind,
            ForgeErrorKind::NotLoggedIn
        );
        f.edit(|s| {
            s.fail.insert(
                "view".into(),
                ForgeError::new(ForgeErrorKind::RateLimited, "API rate limit exceeded"),
            );
        });
        let st = collect(&f, Path::new("."), "pod/x", Some(3));
        assert_eq!(st.error.unwrap().kind, ForgeErrorKind::RateLimited);
        assert!(st.remote.is_some() && st.pr.is_none());
    }

    #[test]
    fn file_backed_state_is_shared() {
        let file = std::env::temp_dir().join(format!("pod-forge-fake-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let a = FakeForge::with_file(file.clone());
        a.edit(|s| s.remote = Some(remote()));
        a.push(Path::new("."), &remote(), "pod/feat").unwrap();
        a.create(Path::new("."), &remote(), &req(false)).unwrap();
        // Another instance (another process in podd tests) sees it and edits it.
        let b = FakeForge::with_file(file.clone());
        assert_eq!(b.state().prs.len(), 1);
        b.edit(|s| s.pr_mut(1).unwrap().view.review_decision = Some(ReviewDecision::Approved));
        assert_eq!(
            a.view(Path::new("."), &remote(), 1)
                .unwrap()
                .review_decision,
            Some(ReviewDecision::Approved)
        );
        let _ = std::fs::remove_file(&file);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "pod-forge-fake-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn pinned_state() -> FakeState {
        let mut s = FakeState {
            remote: Some(remote()),
            next_number: 2,
            pushes: vec![FakePush {
                remote: "origin".into(),
                branch: "pod/feat".into(),
            }],
            calls: vec!["push origin pod/feat".into(), "create 기능".into()],
            ..FakeState::default()
        };
        s.fail.insert(
            "view".into(),
            ForgeError::new(ForgeErrorKind::RateLimited, "API rate limit exceeded"),
        );
        s
    }

    /// The state file's bytes (pretty, two-space indent, no trailing
    /// newline, UTF-8 as is) and its temporary name `<name>.json.tmp`,
    /// pinned before the storage primitive took over the write
    /// (docs/20-refactor/libs.md L2).
    #[test]
    fn save_file_bytes_and_temporary_name_are_pinned() {
        let d = temp_dir("pin");
        // A missing parent is created.
        let file = d.join("fake").join("forge.json");
        FakeForge::save_file(&file, &pinned_state()).unwrap();
        let golden = r#"{
  "remote": {
    "name": "origin",
    "url": "https://github.com/pod-fixtures/demo.git",
    "slug": {
      "host": "github.com",
      "owner": "pod-fixtures",
      "name": "demo"
    }
  },
  "no_remote": false,
  "next_number": 2,
  "prs": [],
  "pushes": [
    {
      "remote": "origin",
      "branch": "pod/feat"
    }
  ],
  "fail": {
    "view": {
      "kind": "rate_limited",
      "message": "API rate limit exceeded"
    }
  },
  "calls": [
    "push origin pod/feat",
    "create 기능"
  ]
}"#;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), golden);
        assert_eq!(FakeForge::load_file(&file), pinned_state());
        let dir = file.parent().unwrap();
        assert_eq!(names(dir), ["forge.json"]);
        // A stale temporary file from a crash is the one written and renamed.
        std::fs::write(dir.join("forge.json.tmp"), b"stale stale stale").unwrap();
        FakeForge::save_file(&file, &FakeState::default()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\n  \"no_remote\": false,\n  \"next_number\": 0,\n  \"prs\": [],\n  \"pushes\": [],\n  \"fail\": {},\n  \"calls\": []\n}"
        );
        assert_eq!(names(dir), ["forge.json"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// What a failed save leaves behind: nothing removes the temporary file,
    /// and the old state file stays readable.
    #[test]
    fn save_file_failures_leave_the_old_state() {
        let d = temp_dir("fail");
        // The rename fails (the destination is a non-empty directory): the
        // temporary file stays with the full bytes.
        let file = d.join("blocked.json");
        std::fs::create_dir_all(file.join("inner")).unwrap();
        assert!(FakeForge::save_file(&file, &pinned_state()).is_err());
        assert_eq!(
            std::fs::read(d.join("blocked.json.tmp")).unwrap(),
            serde_json::to_vec_pretty(&pinned_state()).unwrap()
        );
        assert!(file.join("inner").is_dir());

        // The temporary file cannot be written (a directory holds its name):
        // the old state is kept, and `edit` still runs on it in memory.
        let file = d.join("state.json");
        FakeForge::save_file(&file, &pinned_state()).unwrap();
        std::fs::create_dir(d.join("state.json.tmp")).unwrap();
        assert!(FakeForge::save_file(&file, &FakeState::default()).is_err());
        assert_eq!(FakeForge::load_file(&file), pinned_state());
        let f = FakeForge::with_file(file.clone());
        assert_eq!(f.edit(|s| s.next_number), 2);
        assert_eq!(FakeForge::load_file(&file), pinned_state());

        // The parent cannot be created (a file holds its name).
        let file = d.join("state.json").join("nested.json");
        assert!(FakeForge::save_file(&file, &pinned_state()).is_err());
        assert_eq!(FakeForge::load_file(&d.join("state.json")), pinned_state());
        let _ = std::fs::remove_dir_all(&d);
    }
}
