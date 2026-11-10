//! [`Forge`] over the `gh` CLI (2.x, JSON output) and `git push`.
//!
//! Every call names the repository explicitly (`--repo owner/name`) so `gh`
//! never guesses from the current directory, and never prompts. Commands:
//!
//! | operation | command |
//! |---|---|
//! | push | `git push --set-upstream <remote> HEAD:refs/heads/<branch>` |
//! | create | `gh pr create --repo R --head B --base B --title T --body-file - [--draft]` |
//! | find | `gh pr list --repo R --head B --state all --json FIELDS --limit 20` |
//! | view | `gh pr view N --repo R --json FIELDS` |
//! | checks | `gh pr checks N --repo R --json CHECK_FIELDS` (exit 8 = pending) |
//! | comments | `gh api repos/O/R/pulls/N/comments --paginate` |
//! | merge | `gh pr merge N --repo R --merge\|--squash\|--rebase` |
//! | close | `gh pr close N --repo R` |

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{ForgeError, ForgeErrorKind};
use crate::runner::{Invocation, Runner, run_ok};
use crate::{
    CheckBucket, CreatePr, Forge, ForgeRemote, MergeMethod, PrCheck, PrComment, PrState, PrView,
    ReviewDecision,
};

/// `--json` fields of `gh pr view` / `gh pr list`.
pub const VIEW_FIELDS: &str = "number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt";
/// `--json` fields of `gh pr checks`.
pub const CHECK_FIELDS: &str =
    "bucket,completedAt,description,event,link,name,startedAt,state,workflow";

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);

pub struct GhForge<R: Runner> {
    runner: R,
}

impl<R: Runner> GhForge<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }

    fn gh(&self, dir: &Path, args: &[&str], timeout: Duration) -> Result<String, ForgeError> {
        Ok(run_ok(&self.runner, dir, &Invocation::new("gh", args, timeout))?.stdout)
    }
}

// ---- JSON shapes (gh 2.100) -----------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhAuthor {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    url: String,
    #[serde(default)]
    title: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    head_ref_oid: Option<String>,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    merge_state_status: Option<String>,
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    changed_files: u64,
    #[serde(default)]
    author: Option<GhAuthor>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCheck {
    name: String,
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    workflow: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    completed_at: String,
}

#[derive(Deserialize)]
struct GhUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct GhComment {
    id: u64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    original_line: Option<u64>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    user: Option<GhUser>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    created_at: String,
}

fn bad_json(what: &str, e: impl std::fmt::Display) -> ForgeError {
    ForgeError::failed(format!("unexpected {what} output from gh: {e}"))
}

fn some_text(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// gh prints `0001-01-01T00:00:00Z` for times that have not happened.
fn some_time(s: String) -> Option<String> {
    some_text(Some(s)).filter(|t| !t.starts_with("0001-01-01"))
}

fn pr_from(g: GhPr) -> PrView {
    let state = match g.state.to_ascii_uppercase().as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Open,
    };
    let review_decision = match g.review_decision.as_deref().unwrap_or("") {
        "APPROVED" => Some(ReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
        _ => None,
    };
    PrView {
        number: g.number,
        url: g.url,
        title: g.title,
        state,
        draft: g.is_draft,
        head: g.head_ref_name,
        head_oid: some_text(g.head_ref_oid),
        base: g.base_ref_name,
        review_decision,
        merge_state: some_text(g.merge_state_status),
        mergeable: some_text(g.mergeable),
        additions: g.additions,
        deletions: g.deletions,
        changed_files: g.changed_files,
        author: g.author.map(|a| a.login).filter(|l| !l.is_empty()),
        merged_at: g.merged_at.and_then(some_time),
        closed_at: g.closed_at.and_then(some_time),
    }
}

/// `gh pr view --json VIEW_FIELDS` output.
pub fn parse_view(json: &str) -> Result<PrView, ForgeError> {
    serde_json::from_str::<GhPr>(json.trim_start_matches('\u{feff}'))
        .map(pr_from)
        .map_err(|e| bad_json("pr view", e))
}

/// `gh pr list --json VIEW_FIELDS` output.
pub fn parse_list(json: &str) -> Result<Vec<PrView>, ForgeError> {
    serde_json::from_str::<Vec<GhPr>>(json.trim_start_matches('\u{feff}'))
        .map(|v| v.into_iter().map(pr_from).collect())
        .map_err(|e| bad_json("pr list", e))
}

/// `gh pr checks --json CHECK_FIELDS` output.
pub fn parse_checks(json: &str) -> Result<Vec<PrCheck>, ForgeError> {
    let raw: Vec<GhCheck> = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|e| bad_json("pr checks", e))?;
    Ok(raw
        .into_iter()
        .map(|c| {
            let bucket = match c.bucket.as_str() {
                "pass" => CheckBucket::Pass,
                "fail" => CheckBucket::Fail,
                "skipping" => CheckBucket::Skipping,
                "cancel" => CheckBucket::Cancel,
                _ => CheckBucket::Pending,
            };
            PrCheck {
                name: c.name,
                workflow: some_text(Some(c.workflow)),
                bucket,
                state: c.state,
                link: some_text(Some(c.link)),
                description: some_text(Some(c.description)),
                started_at: some_time(c.started_at),
                completed_at: some_time(c.completed_at),
            }
        })
        .collect())
}

/// `gh api …/comments --paginate` output: one JSON array per page, printed
/// back to back (or a single array).
pub fn parse_comments(json: &str) -> Result<Vec<PrComment>, ForgeError> {
    let mut out = Vec::new();
    for page in
        serde_json::Deserializer::from_str(json.trim_start_matches('\u{feff}')).into_iter::<Value>()
    {
        let page = page.map_err(|e| bad_json("review comments", e))?;
        let items = match page {
            Value::Array(a) => a,
            other => vec![other],
        };
        for item in items {
            let c: GhComment =
                serde_json::from_value(item).map_err(|e| bad_json("review comment", e))?;
            out.push(PrComment {
                id: c.id,
                path: c.path,
                line: c.line.or(c.original_line),
                body: c.body,
                author: c.user.map(|u| u.login).unwrap_or_default(),
                url: c.html_url,
                created_at: c.created_at,
            });
        }
    }
    Ok(out)
}

/// The pull request number at the end of a URL printed by `gh pr create`.
pub fn number_from_url(s: &str) -> Option<u64> {
    let url = s
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("http"))?;
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// The open pull request, else the newest.
pub fn pick_for_branch(mut prs: Vec<PrView>) -> Option<PrView> {
    prs.sort_by_key(|p| std::cmp::Reverse(p.number));
    prs.iter()
        .position(|p| p.state == PrState::Open)
        .map(|i| prs.swap_remove(i))
        .or_else(|| prs.into_iter().next())
}

/// Branch names are passed as arguments; refuse ones git would read as options.
fn check_branch(branch: &str) -> Result<(), ForgeError> {
    if branch.is_empty() || branch.starts_with('-') || branch.contains(char::is_whitespace) {
        return Err(ForgeError::failed(format!(
            "invalid branch name {branch:?}"
        )));
    }
    Ok(())
}

impl<R: Runner> Forge for GhForge<R> {
    fn detect(&self, dir: &Path) -> Result<ForgeRemote, ForgeError> {
        crate::remote::detect(&self.runner, dir)
    }

    fn push(&self, dir: &Path, remote: &ForgeRemote, branch: &str) -> Result<(), ForgeError> {
        check_branch(branch)?;
        let refspec = format!("HEAD:refs/heads/{branch}");
        let out = self.runner.run(
            dir,
            &Invocation::new(
                "git",
                &[
                    "push",
                    "--porcelain",
                    "--set-upstream",
                    &remote.name,
                    &refspec,
                ],
                PUSH_TIMEOUT,
            ),
        )?;
        if !out.ok() {
            return Err(ForgeError::classify("git", out.code, &out.stderr));
        }
        // `--set-upstream` with a HEAD:ref refspec does not set the branch's
        // upstream; set it so `git status` in the worktree shows ahead/behind.
        let upstream = format!("{}/{branch}", remote.name);
        let _ = self.runner.run(
            dir,
            &Invocation::new(
                "git",
                &["branch", "--set-upstream-to", &upstream, branch],
                QUERY_TIMEOUT,
            ),
        );
        Ok(())
    }

    fn create(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        req: &CreatePr,
    ) -> Result<PrView, ForgeError> {
        check_branch(&req.head)?;
        check_branch(&req.base)?;
        let repo = remote.slug.repo_arg();
        let mut args = vec![
            "pr",
            "create",
            "--repo",
            &repo,
            "--head",
            &req.head,
            "--base",
            &req.base,
            "--title",
            &req.title,
            "--body-file",
            "-",
        ];
        if req.draft {
            args.push("--draft");
        }
        let mut inv = Invocation::new("gh", &args, WRITE_TIMEOUT);
        inv.stdin = Some(req.body.clone());
        match run_ok(&self.runner, dir, &inv) {
            Ok(out) => {
                let n = number_from_url(&out.stdout).ok_or_else(|| {
                    ForgeError::failed(format!(
                        "gh pr create printed no pull request URL: {}",
                        out.stdout.trim()
                    ))
                })?;
                self.view(dir, remote, n)
            }
            Err(e) if e.kind == ForgeErrorKind::AlreadyExists => {
                match number_from_url(&e.message) {
                    Some(n) => self.view(dir, remote, n),
                    None => self.find_for_branch(dir, remote, &req.head)?.ok_or(e),
                }
            }
            // GitHub answers a missing base with several GraphQL errors
            // ("Head sha can't be blank, Base sha can't be blank, … Base ref
            // must be a branch"); say what is wrong instead.
            Err(e) if e.message.contains("Base ref must be a branch") => Err(ForgeError::new(
                ForgeErrorKind::NotFound,
                format!(
                    "base branch {:?} does not exist on {repo}; push it first or choose another base ({})",
                    req.base,
                    e.message.lines().next().unwrap_or("").trim()
                ),
            )),
            Err(e) => Err(e),
        }
    }

    fn find_for_branch(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        branch: &str,
    ) -> Result<Option<PrView>, ForgeError> {
        check_branch(branch)?;
        let repo = remote.slug.repo_arg();
        let out = self.gh(
            dir,
            &[
                "pr",
                "list",
                "--repo",
                &repo,
                "--head",
                branch,
                "--state",
                "all",
                "--json",
                VIEW_FIELDS,
                "--limit",
                "20",
            ],
            QUERY_TIMEOUT,
        )?;
        Ok(pick_for_branch(parse_list(&out)?))
    }

    fn view(&self, dir: &Path, remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError> {
        let (repo, n) = (remote.slug.repo_arg(), number.to_string());
        parse_view(&self.gh(
            dir,
            &["pr", "view", &n, "--repo", &repo, "--json", VIEW_FIELDS],
            QUERY_TIMEOUT,
        )?)
    }

    fn checks(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrCheck>, ForgeError> {
        let (repo, n) = (remote.slug.repo_arg(), number.to_string());
        let inv = Invocation::new(
            "gh",
            &["pr", "checks", &n, "--repo", &repo, "--json", CHECK_FIELDS],
            QUERY_TIMEOUT,
        );
        let out = self.runner.run(dir, &inv)?;
        // Exit 8: some checks are pending (the JSON is complete).
        if out.ok() || out.code == Some(8) {
            return parse_checks(&out.stdout);
        }
        if out
            .stderr
            .to_ascii_lowercase()
            .contains("no checks reported")
        {
            return Ok(Vec::new());
        }
        Err(ForgeError::classify("gh", out.code, &out.stderr))
    }

    fn review_comments(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrComment>, ForgeError> {
        let path = format!("repos/{}/pulls/{number}/comments", remote.slug.full_name());
        let mut args = vec!["api", path.as_str(), "--paginate"];
        let host_flag;
        if !remote.slug.host.eq_ignore_ascii_case("github.com") {
            host_flag = remote.slug.host.clone();
            args.extend(["--hostname", host_flag.as_str()]);
        }
        parse_comments(&self.gh(dir, &args, QUERY_TIMEOUT)?)
    }

    fn merge(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
        method: MergeMethod,
    ) -> Result<PrView, ForgeError> {
        let (repo, n) = (remote.slug.repo_arg(), number.to_string());
        self.gh(
            dir,
            &["pr", "merge", &n, "--repo", &repo, method.flag()],
            WRITE_TIMEOUT,
        )?;
        self.view(dir, remote, number)
    }

    fn close(&self, dir: &Path, remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError> {
        let (repo, n) = (remote.slug.repo_arg(), number.to_string());
        self.gh(dir, &["pr", "close", &n, "--repo", &repo], WRITE_TIMEOUT)?;
        self.view(dir, remote, number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::runner::RunOutput;
    use crate::{ChecksSummary, RepoSlug};
    use std::sync::Mutex;

    #[test]
    fn parses_recorded_view_fixtures() {
        let open = parse_view(fixtures::VIEW_OPEN).unwrap();
        assert_eq!(
            (open.number, open.state, open.draft),
            (14964, PrState::Open, false)
        );
        assert_eq!(open.url, "https://github.com/cli/cli/pull/14964");
        assert_eq!(open.review_decision, Some(ReviewDecision::ReviewRequired));
        assert_eq!(open.merge_state.as_deref(), Some("BLOCKED"));
        assert!(open.merged_at.is_none());
        let merged = parse_view(fixtures::VIEW_MERGED).unwrap();
        assert_eq!(merged.state, PrState::Merged);
        assert_eq!(merged.review_decision, Some(ReviewDecision::Approved));
        assert_eq!(merged.merged_at.as_deref(), Some("2026-11-07T01:48:47Z"));
        let draft = parse_view(fixtures::VIEW_DRAFT).unwrap();
        assert!(draft.draft && draft.state == PrState::Open);
        assert!(draft.author.is_some());
    }

    #[test]
    fn parses_recorded_check_fixtures() {
        let open = parse_checks(fixtures::CHECKS_OPEN).unwrap();
        let s = ChecksSummary::of(&open);
        assert_eq!((s.total, s.failed, s.pending), (21, 0, 0));
        assert!(s.passed > 0 && s.skipped > 0);
        assert_eq!(s.verdict(), "passed");
        let failed = parse_checks(fixtures::CHECKS_FAILED).unwrap();
        let bad: Vec<&PrCheck> = failed
            .iter()
            .filter(|c| c.bucket == CheckBucket::Fail)
            .collect();
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].name, "build (windows-latest)");
        assert_eq!(bad[0].state, "FAILURE");
        assert_eq!(
            bad[0].workflow.as_deref(),
            Some("Unit and Integration Tests")
        );
        assert!(
            bad[0]
                .link
                .as_deref()
                .unwrap()
                .starts_with("https://github.com/cli/cli/actions/runs/")
        );
        assert_eq!(ChecksSummary::of(&failed).verdict(), "failed");
        // Pending rows carry gh's zero time for "not completed".
        let pending = parse_checks(r#"[{"bucket":"pending","completedAt":"0001-01-01T00:00:00Z","description":"","event":"pull_request","link":"https://x","name":"build","startedAt":"2026-11-09T00:00:00Z","state":"IN_PROGRESS","workflow":"CI"}]"#).unwrap();
        assert_eq!(pending[0].bucket, CheckBucket::Pending);
        assert!(pending[0].completed_at.is_none());
    }

    #[test]
    fn parses_paginated_comments() {
        let c = parse_comments(fixtures::REVIEW_COMMENTS).unwrap();
        assert!(!c.is_empty());
        assert!(c[0].url.contains("#discussion_r"));
        assert!(!c[0].author.is_empty());
        let two_pages = format!(
            "{}\n{}",
            fixtures::REVIEW_COMMENTS.trim(),
            fixtures::REVIEW_COMMENTS.trim()
        );
        assert_eq!(parse_comments(&two_pages).unwrap().len(), c.len() * 2);
        assert!(parse_comments("[]").unwrap().is_empty());
    }

    #[test]
    fn url_numbers_and_branch_choice() {
        assert_eq!(
            number_from_url("https://github.com/o/r/pull/42\n"),
            Some(42)
        );
        assert_eq!(
            number_from_url("Creating pull request…\n\nhttps://github.com/o/r/pull/7"),
            Some(7)
        );
        assert_eq!(number_from_url("nothing"), None);
        let mut a = parse_view(fixtures::VIEW_MERGED).unwrap();
        a.number = 3;
        let mut b = parse_view(fixtures::VIEW_OPEN).unwrap();
        b.number = 2;
        let mut c = a.clone();
        c.number = 5;
        assert_eq!(
            pick_for_branch(vec![a.clone(), b.clone(), c.clone()])
                .unwrap()
                .number,
            2,
            "open wins"
        );
        assert_eq!(
            pick_for_branch(vec![a, c]).unwrap().number,
            5,
            "else newest"
        );
        assert!(pick_for_branch(Vec::new()).is_none());
    }

    /// Answers invocations from a script and records them.
    struct Recorded {
        script: Vec<(&'static str, RunOutput)>,
        seen: Mutex<Vec<Invocation>>,
    }

    impl Runner for Recorded {
        fn run(&self, _dir: &Path, inv: &Invocation) -> Result<RunOutput, ForgeError> {
            self.seen.lock().unwrap().push(inv.clone());
            let line = inv.line();
            self.script
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix))
                .map(|(_, o)| o.clone())
                .ok_or_else(|| ForgeError::failed(format!("unscripted: {line}")))
        }
    }

    fn ok(stdout: &str) -> RunOutput {
        RunOutput {
            code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn fail(code: i32, stderr: &str) -> RunOutput {
        RunOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    fn remote() -> ForgeRemote {
        ForgeRemote {
            name: "origin".into(),
            url: "https://github.com/cli/cli.git".into(),
            slug: RepoSlug {
                host: "github.com".into(),
                owner: "cli".into(),
                name: "cli".into(),
            },
        }
    }

    #[test]
    fn create_sends_body_on_stdin_then_views() {
        let gh = GhForge::new(Recorded {
            script: vec![
                (
                    "gh pr create",
                    ok("https://github.com/cli/cli/pull/14964\n"),
                ),
                ("gh pr view 14964", ok(fixtures::VIEW_OPEN)),
            ],
            seen: Mutex::new(Vec::new()),
        });
        let req = CreatePr {
            head: "pod/x".into(),
            base: "main".into(),
            title: "제목 \"quoted\"".into(),
            body: "본문\n- a\n".into(),
            draft: true,
        };
        let pr = gh.create(Path::new("."), &remote(), &req).unwrap();
        assert_eq!(pr.number, 14964);
        let seen = gh.runner().seen.lock().unwrap();
        let create = &seen[0];
        assert_eq!(
            create.args,
            [
                "pr",
                "create",
                "--repo",
                "cli/cli",
                "--head",
                "pod/x",
                "--base",
                "main",
                "--title",
                "제목 \"quoted\"",
                "--body-file",
                "-",
                "--draft"
            ]
        );
        assert_eq!(create.stdin.as_deref(), Some("본문\n- a\n"));
        assert_eq!(
            seen[1].args[..5],
            ["pr", "view", "14964", "--repo", "cli/cli"]
        );
    }

    #[test]
    fn create_reuses_an_existing_pull_request() {
        let gh = GhForge::new(Recorded {
            script: vec![
                (
                    "gh pr create",
                    fail(
                        1,
                        "a pull request for branch \"pod/x\" into branch \"main\" already exists:\nhttps://github.com/cli/cli/pull/14964",
                    ),
                ),
                ("gh pr view 14964", ok(fixtures::VIEW_OPEN)),
            ],
            seen: Mutex::new(Vec::new()),
        });
        let req = CreatePr {
            head: "pod/x".into(),
            base: "main".into(),
            title: "t".into(),
            body: String::new(),
            draft: false,
        };
        assert_eq!(
            gh.create(Path::new("."), &remote(), &req).unwrap().number,
            14964
        );
    }

    #[test]
    fn create_with_a_base_missing_on_the_remote() {
        // gh 2.100 output recorded against a real repository (docs/15 §9).
        let gh = GhForge::new(Recorded {
            script: vec![(
                "gh pr create",
                fail(
                    1,
                    "pull request create failed: GraphQL: Head sha can't be blank, Base sha can't be blank, No commits between local-only and pod/x, Base ref must be a branch (createPullRequest)",
                ),
            )],
            seen: Mutex::new(Vec::new()),
        });
        let req = CreatePr {
            head: "pod/x".into(),
            base: "local-only".into(),
            title: "t".into(),
            body: String::new(),
            draft: false,
        };
        let e = gh.create(Path::new("."), &remote(), &req).unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::NotFound);
        assert!(
            e.message
                .starts_with("base branch \"local-only\" does not exist on cli/cli"),
            "{e:?}"
        );
    }

    #[test]
    fn checks_pending_exit_code_and_no_checks() {
        let pending = GhForge::new(Recorded {
            script: vec![(
                "gh pr checks",
                RunOutput {
                    code: Some(8),
                    stdout: fixtures::CHECKS_OPEN.into(),
                    stderr: String::new(),
                },
            )],
            seen: Mutex::new(Vec::new()),
        });
        assert_eq!(
            pending.checks(Path::new("."), &remote(), 1).unwrap().len(),
            21
        );
        let none = GhForge::new(Recorded {
            script: vec![(
                "gh pr checks",
                fail(1, "no checks reported on the 'pod/x' branch"),
            )],
            seen: Mutex::new(Vec::new()),
        });
        assert!(
            none.checks(Path::new("."), &remote(), 1)
                .unwrap()
                .is_empty()
        );
        let auth = GhForge::new(Recorded {
            script: vec![(
                "gh pr checks",
                fail(
                    4,
                    "To get started with GitHub CLI, please run:  gh auth login",
                ),
            )],
            seen: Mutex::new(Vec::new()),
        });
        assert_eq!(
            auth.checks(Path::new("."), &remote(), 1).unwrap_err().kind,
            ForgeErrorKind::NotLoggedIn
        );
    }

    #[test]
    fn push_merge_close_and_find_commands() {
        let gh = GhForge::new(Recorded {
            script: vec![
                ("git push", ok("")),
                ("git branch", ok("")),
                ("gh pr merge", ok("")),
                ("gh pr close", ok("")),
                ("gh pr view", ok(fixtures::VIEW_MERGED)),
                ("gh pr list", ok(&format!("[{}]", fixtures::VIEW_OPEN))),
            ],
            seen: Mutex::new(Vec::new()),
        });
        gh.push(Path::new("."), &remote(), "pod/x").unwrap();
        assert_eq!(
            gh.merge(Path::new("."), &remote(), 14976, MergeMethod::Squash)
                .unwrap()
                .state,
            PrState::Merged
        );
        gh.close(Path::new("."), &remote(), 14976).unwrap();
        assert_eq!(
            gh.find_for_branch(Path::new("."), &remote(), "pod/x")
                .unwrap()
                .unwrap()
                .number,
            14964
        );
        let seen: Vec<String> = gh
            .runner()
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(Invocation::line)
            .collect();
        assert_eq!(
            seen[0],
            "git push --porcelain --set-upstream origin HEAD:refs/heads/pod/x"
        );
        assert_eq!(seen[1], "git branch --set-upstream-to origin/pod/x pod/x");
        assert_eq!(seen[2], "gh pr merge 14976 --repo cli/cli --squash");
        assert!(seen[4].starts_with("gh pr close 14976 --repo cli/cli"));
        assert!(seen[6].starts_with("gh pr list --repo cli/cli --head pod/x --state all --json "));
        assert!(
            gh.push(Path::new("."), &remote(), "--force").is_err(),
            "option-like branch refused"
        );
    }

    #[test]
    fn push_errors_are_classified() {
        let gh = GhForge::new(Recorded {
            script: vec![(
                "git push",
                fail(
                    128,
                    "remote: Permission to cli/cli.git denied to SJin-Choi.\nfatal: unable to access 'https://github.com/cli/cli.git/': The requested URL returned error: 403",
                ),
            )],
            seen: Mutex::new(Vec::new()),
        });
        assert_eq!(
            gh.push(Path::new("."), &remote(), "pod/x")
                .unwrap_err()
                .kind,
            ForgeErrorKind::NoPermission
        );
    }
}
