//! PR section of the Review panel (P6, docs/15): push the task branch and
//! open a pull request, then follow it — state badge, review decision,
//! checks with links, merge with a method choice and a confirmation, open in
//! the browser. Talks to podd's `task.pr.*` (protocol v10) only.
//!
//! GitHub is contacted only while the PR tab is shown (or on its buttons);
//! otherwise the section answers from `TaskInfo.pr` (`cached`). Changes made
//! elsewhere arrive as `task.pr.changed` → task list → [`PrSection::set_task`].
//! Without a GitHub remote the section explains how to connect one; Pod never
//! creates repositories or remotes.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::{
    self as p, CheckBucket, ChecksSummary, ForgeErrorKind, MergeMethod, PrCheck, PrState,
    ReviewDecision, TaskPrInfo,
};
use serde_json::{Value, json};

use crate::orch::{tc, tokens};
use crate::qa_marks::QaId;
use crate::source::{PoddSource, SourceError};
use crate::vm::TaskVm;
use crate::workspace::{ToastKind, Workspace};

/// Korean copy of the PR section (docs/09 strings rule; kept here so this
/// feature merges without touching the shared catalog).
pub mod s {
    pub const TAB_COMMENTS: &str = "의견";
    pub const TAB_PR: &str = "PR";
    pub const TAB_COMMENTS_TIP: &str = "리뷰 의견과 에이전트에 보내기";
    pub const TAB_PR_TIP: &str = "GitHub 풀 리퀘스트 (브랜치 푸시, 체크, 병합)";
    pub const CHECKING: &str = "PR 상태를 확인하는 중…";
    pub const REFRESH: &str = "새로 고침";
    pub const REFRESH_TIP: &str = "GitHub에서 PR 상태와 체크를 다시 읽기";
    pub const NO_REMOTE_TITLE: &str = "원격 저장소가 연결되지 않았습니다";
    pub const NO_REMOTE_BODY: &str = "이 저장소에는 GitHub 원격(origin)이 없어 브랜치를 푸시하거나 PR을 만들 수 없습니다. Pod는 저장소를 만들거나 원격을 추가하지 않습니다.";
    pub const NO_REMOTE_STEPS: &str = "연결 방법: GitHub에 저장소를 만든 뒤 저장소 폴더에서 git remote add origin https://github.com/<소유자>/<저장소>.git 을 실행하고 gh auth login으로 로그인하세요. 그다음 다시 확인을 누르세요.";
    pub const UNSUPPORTED_TITLE: &str = "GitHub 저장소가 아닌 원격입니다";
    pub const UNSUPPORTED_BODY: &str =
        "PR은 GitHub 원격에서만 만들 수 있습니다. GitHub 저장소를 가리키는 원격을 추가하세요.";
    pub const CHECK_AGAIN: &str = "다시 확인";
    pub const NO_PR: &str = "PR 없음";
    pub const CREATE: &str = "브랜치 푸시 및 PR 만들기";
    pub const CREATE_TIP: &str = "작업 브랜치를 원격에 푸시하고 GitHub PR을 엽니다";
    pub const NO_COMMITS: &str =
        "기준보다 앞선 커밋이 없어 PR을 만들 수 없습니다. 먼저 변경을 커밋하세요.";
    pub const TITLE: &str = "제목";
    pub const BODY: &str = "본문";
    pub const BASE: &str = "기준 브랜치";
    pub const DRAFT: &str = "초안으로 만들기";
    pub const CANCEL: &str = "취소";
    pub const SUBMIT: &str = "푸시하고 PR 만들기";
    pub const PUSHING: &str = "푸시하고 PR을 만드는 중…";
    pub const MERGING: &str = "병합하는 중…";
    pub const OPEN_BROWSER: &str = "브라우저에서 열기";
    pub const STATE_OPEN: &str = "열림";
    pub const STATE_DRAFT: &str = "◌ 초안";
    pub const STATE_MERGED: &str = "병합됨";
    pub const STATE_CLOSED: &str = "닫힘";
    pub const REVIEW_APPROVED: &str = "리뷰 승인됨";
    pub const REVIEW_CHANGES: &str = "변경 요청됨";
    pub const REVIEW_REQUIRED: &str = "○ 리뷰 필요";
    pub const REVIEW_NONE: &str = "리뷰 필요 없음";
    pub const CHECKS_NONE: &str = "체크 없음";
    pub const CHECKS_CACHED: &str = "체크 목록은 새로 고침하면 보입니다";
    pub const SHOW_SKIPPED: &str = "건너뛴 체크 보기";
    pub const HIDE_SKIPPED: &str = "건너뛴 체크 숨기기";
    pub const OPEN_CHECK: &str = "열기";
    pub const METHOD_MERGE: &str = "병합 커밋";
    pub const METHOD_SQUASH: &str = "스쿼시";
    pub const METHOD_REBASE: &str = "리베이스";
    pub const MERGE: &str = "병합…";
    pub const MERGE_CONFIRM_TITLE: &str = "PR 병합";
    pub const MERGE_OK: &str = "병합";
    pub const MERGE_DRAFT_NOTE: &str =
        "초안 PR은 병합할 수 없습니다. GitHub에서 '검토 준비 완료'로 바꾸세요.";
    pub const MERGE_FAILED_CHECKS: &str =
        "실패한 체크가 있습니다. 저장소 규칙에 따라 병합이 거절될 수 있습니다.";
    pub const MERGE_CONFLICT: &str = "기준 브랜치와 충돌해 GitHub가 병합할 수 없습니다. 작업 폴더에서 기준 브랜치를 병합하거나 리베이스한 뒤 다시 푸시하세요.";
    pub const CREATED: &str = "PR을 만들었습니다";
    pub const MERGED: &str = "PR을 병합했습니다";
    pub const CREATE_FAILED: &str = "PR을 만들지 못했습니다";
    pub const MERGE_FAILED: &str = "PR을 병합하지 못했습니다";
    pub const OPEN_FAILED: &str = "브라우저를 열지 못했습니다";
    pub const NEEDS_NEWER_PODD: &str = "PR 기능에는 podd 프로토콜 v10이 필요합니다";
    pub const CREATE_PR_FOR_WINNER: &str = "PR 만들기";
    pub const CREATE_PR_FOR_WINNER_TIP: &str = "선택한 작업의 브랜치를 푸시하고 PR을 만듭니다";
    pub const GITHUB_COMMENTS: &str = "GitHub 리뷰 의견";
    pub const DIRTY_NOTE: &str = "커밋하지 않은 변경은 PR에 들어가지 않습니다";
    pub const BODY_SUMMARY: &str = "## 변경 요약";
    pub const BODY_REVIEW: &str = "## 리뷰";
}

/// Korean title and guidance for a forge failure kind.
pub fn kind_text(kind: ForgeErrorKind) -> (&'static str, &'static str) {
    match kind {
        ForgeErrorKind::NoRemote => (s::NO_REMOTE_TITLE, s::NO_REMOTE_BODY),
        ForgeErrorKind::UnsupportedHost => (s::UNSUPPORTED_TITLE, s::UNSUPPORTED_BODY),
        ForgeErrorKind::ToolMissing => (
            "GitHub CLI(gh)가 없습니다",
            "winget install GitHub.cli 로 설치한 뒤 gh auth login으로 로그인하세요.",
        ),
        ForgeErrorKind::NotLoggedIn => (
            "GitHub에 로그인되어 있지 않습니다",
            "터미널에서 gh auth login을 실행한 뒤 다시 확인하세요.",
        ),
        ForgeErrorKind::NoPermission => (
            "이 저장소에 권한이 없습니다",
            "푸시·병합 권한이 있는 계정으로 로그인했는지 확인하세요.",
        ),
        ForgeErrorKind::RateLimited => {
            ("GitHub 요청 한도에 걸렸습니다", "잠시 뒤 다시 시도하세요.")
        }
        ForgeErrorKind::NotFound => (
            "GitHub에서 찾을 수 없습니다",
            "원격 저장소 주소와 권한을 확인하세요.",
        ),
        ForgeErrorKind::Network => ("GitHub에 연결할 수 없습니다", "네트워크 연결을 확인하세요."),
        ForgeErrorKind::Timeout => (
            "GitHub 명령이 시간 안에 끝나지 않았습니다",
            "잠시 뒤 다시 시도하세요.",
        ),
        ForgeErrorKind::Rejected => (
            "원격 브랜치가 바뀌어 푸시가 거절되었습니다",
            "다른 곳에서 같은 브랜치에 푸시했습니다. 작업 터미널에서 git pull --rebase 후 다시 만드세요.",
        ),
        ForgeErrorKind::AlreadyExists | ForgeErrorKind::Failed => {
            ("GitHub 작업이 실패했습니다", "")
        }
    }
}

/// podd's `FORGE_FAILED` message starts with a fixed phrase per kind
/// (`server/pr.rs::forge_err`); the app gets code + message only.
pub fn kind_of_message(m: &str) -> Option<ForgeErrorKind> {
    let k = [
        (
            "the repository has no GitHub remote",
            ForgeErrorKind::NoRemote,
        ),
        (
            "the remote is not a GitHub repository",
            ForgeErrorKind::UnsupportedHost,
        ),
        (
            "the GitHub CLI (gh) or git is not installed",
            ForgeErrorKind::ToolMissing,
        ),
        ("not logged in to GitHub", ForgeErrorKind::NotLoggedIn),
        (
            "no permission on the GitHub repository",
            ForgeErrorKind::NoPermission,
        ),
        ("GitHub rate limit reached", ForgeErrorKind::RateLimited),
        ("not found on GitHub", ForgeErrorKind::NotFound),
        ("GitHub is unreachable", ForgeErrorKind::Network),
        ("the GitHub command timed out", ForgeErrorKind::Timeout),
        (
            "the remote branch has commits the task does not have",
            ForgeErrorKind::Rejected,
        ),
    ];
    k.iter()
        .find(|(prefix, _)| m.starts_with(prefix))
        .map(|(_, kind)| *kind)
}

/// One-line Korean text for an action error.
pub fn error_text(e: &SourceError) -> String {
    if e.code == Some(p::codes::METHOD_NOT_FOUND) {
        return s::NEEDS_NEWER_PODD.into();
    }
    match kind_of_message(&e.message) {
        Some(k) => {
            let (title, hint) = kind_text(k);
            let detail = e.message.split_once(": ").map(|x| x.1).unwrap_or("");
            [title, hint, detail]
                .iter()
                .filter(|x| !x.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ")
        }
        None => e.message.clone(),
    }
}

/// Default title: the first line of the task prompt (≤ 72 chars), else the task name.
pub fn default_title(task: &TaskVm) -> String {
    task.prompt
        .as_deref()
        .and_then(|p| p.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| l.chars().take(72).collect())
        .unwrap_or_else(|| task.id.rsplit('/').next().unwrap_or(&task.id).to_string())
}

/// Changed files from `task.diff`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffStats {
    pub files: Vec<(String, Option<u64>, Option<u64>)>,
    pub added: u64,
    pub deleted: u64,
}

impl DiffStats {
    pub fn from_json(v: &Value) -> Self {
        let files = v
            .get("files")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some((
                            f.get("path")?.as_str()?.to_string(),
                            f.get("added").and_then(Value::as_u64),
                            f.get("deleted").and_then(Value::as_u64),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            files,
            added: v.get("added").and_then(Value::as_u64).unwrap_or(0),
            deleted: v.get("deleted").and_then(Value::as_u64).unwrap_or(0),
        }
    }
}

/// Default body: the prompt, a change summary (diff stats) and the Pod
/// review summary (comment counts).
pub fn default_body(
    task: &TaskVm,
    diff: Option<&DiffStats>,
    comments: Option<(usize, usize)>,
) -> String {
    let mut out = String::new();
    if let Some(p) = task
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        out.push_str(p);
        out.push_str("\n\n");
    }
    if let Some(d) = diff {
        out.push_str(s::BODY_SUMMARY);
        out.push('\n');
        out.push_str(&format!(
            "- 파일 {}개, +{} −{}\n",
            d.files.len(),
            d.added,
            d.deleted
        ));
        for (path, a, del) in d.files.iter().take(20) {
            match (a, del) {
                (Some(a), Some(del)) => out.push_str(&format!("- `{path}` (+{a} −{del})\n")),
                _ => out.push_str(&format!("- `{path}` (바이너리)\n")),
            }
        }
        if d.files.len() > 20 {
            out.push_str(&format!("- … 외 {}개\n", d.files.len() - 20));
        }
        out.push('\n');
    }
    if let Some((total, resolved)) = comments.filter(|(t, _)| *t > 0) {
        out.push_str(s::BODY_REVIEW);
        out.push('\n');
        out.push_str(&format!(
            "- Pod 리뷰 코멘트 {total}개 (해결 {resolved}개)\n\n"
        ));
    }
    out.push_str(&format!("---\nPod 작업 `{}`에서 만들었습니다.\n", task.id));
    out
}

pub fn checks_line(s: &ChecksSummary) -> String {
    if s.total == 0 {
        return s::CHECKS_NONE.into();
    }
    let mut parts = vec![format!("통과 {}", s.passed)];
    if s.failed > 0 {
        parts.push(format!("실패 {}", s.failed));
    }
    if s.pending > 0 {
        parts.push(format!("진행 중 {}", s.pending));
    }
    if s.skipped > 0 {
        parts.push(format!("건너뜀 {}", s.skipped));
    }
    format!("체크 {}개 · {}", s.total, parts.join(" · "))
}
/// Icon for a checks verdict (drawn before [`checks_line`]).
fn checks_icon(s: &ChecksSummary) -> pod_ui::Icon {
    match s.verdict() {
        "failed" => pod_ui::Icon::OctagonX,
        "pending" => pod_ui::Icon::Clock,
        _ => pod_ui::Icon::CircleCheck,
    }
}

fn bucket_glyph(b: CheckBucket) -> (pod_ui::Icon, u32) {
    let t = tokens();
    match b {
        CheckBucket::Pass => (pod_ui::Icon::CircleCheck, t.success_fg),
        CheckBucket::Fail => (pod_ui::Icon::OctagonX, t.danger_fg),
        CheckBucket::Cancel => (pod_ui::Icon::X, t.danger_fg),
        CheckBucket::Pending => (pod_ui::Icon::Clock, t.warning_fg),
        CheckBucket::Skipping => (pod_ui::Icon::Minus, t.text_muted),
    }
}

fn bucket_word(b: CheckBucket) -> &'static str {
    match b {
        CheckBucket::Pass => "통과",
        CheckBucket::Fail => "실패",
        CheckBucket::Cancel => "취소됨",
        CheckBucket::Pending => "진행 중",
        CheckBucket::Skipping => "건너뜀",
    }
}

/// Failed first, then pending, passed, skipped.
fn check_order(c: &PrCheck) -> u8 {
    match c.bucket {
        CheckBucket::Fail | CheckBucket::Cancel => 0,
        CheckBucket::Pending => 1,
        CheckBucket::Pass => 2,
        CheckBucket::Skipping => 3,
    }
}

/// GitHub reports a conflict with the base (`mergeable: CONFLICTING`,
/// `mergeStateStatus: DIRTY`); merging fails with "the merge commit cannot be
/// cleanly created" (seen on a real repository, docs/15 §9).
pub fn conflicted(pr: &p::PrView) -> bool {
    pr.mergeable.as_deref() == Some("CONFLICTING") || pr.merge_state.as_deref() == Some("DIRTY")
}

pub fn method_label(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Merge => s::METHOD_MERGE,
        MergeMethod::Squash => s::METHOD_SQUASH,
        MergeMethod::Rebase => s::METHOD_REBASE,
    }
}

struct Form {
    title: Entity<InputState>,
    base: Entity<InputState>,
    body: Entity<TextareaState>,
    /// The prefilled body (replaced when diff stats arrive, unless edited).
    body_seed: String,
    draft: bool,
    /// Focus the title on the first frame that shows the form.
    focus_title: bool,
}

pub struct PrSection {
    pub task_id: String,
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    task: TaskVm,
    info: Option<TaskPrInfo>,
    loading: bool,
    /// The tab is shown: refreshes go to GitHub (else cached).
    visible: bool,
    load_error: Option<String>,
    action_error: Option<String>,
    busy: Option<&'static str>,
    form: Option<Form>,
    method: MergeMethod,
    show_skipped: bool,
}

impl PrSection {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        task: &TaskVm,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            task_id: task.id.clone(),
            source,
            workspace,
            task: task.clone(),
            info: None,
            loading: false,
            visible: false,
            load_error: None,
            action_error: None,
            busy: None,
            form: None,
            method: MergeMethod::Squash,
            show_skipped: false,
        };
        this.refresh(cx);
        this
    }

    pub fn info(&self) -> Option<&TaskPrInfo> {
        self.info.as_ref()
    }

    /// The task list was re-read; a changed `TaskInfo.pr` (event or poll) re-reads the section.
    pub fn set_task(&mut self, task: &TaskVm, cx: &mut Context<Self>) {
        let changed = task.pr != self.task.pr
            || task.git.as_ref().map(|g| (g.ahead, g.unpushed, g.dirty))
                != self
                    .task
                    .git
                    .as_ref()
                    .map(|g| (g.ahead, g.unpushed, g.dirty));
        self.task = task.clone();
        if changed && self.busy.is_none() {
            self.refresh(cx);
        }
    }

    /// The PR tab became visible (or hidden).
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let was = self.visible;
        self.visible = visible;
        if visible && !was {
            self.refresh(cx);
        }
    }

    /// Re-read `task.pr.status` (from GitHub while visible, else cached).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::TaskPrStatusMethod>(&p::TaskPrStatusParams {
                task: self.task_id.clone(),
                cached: !self.visible,
            });
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |s, cx| {
                s.loading = false;
                match r {
                    Ok(i) => {
                        s.info = Some(i);
                        s.load_error = None;
                    }
                    Err(e) => s.load_error = Some(error_text(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "브랜치 푸시 및 PR 만들기": the form, prefilled (diff stats and the
    /// review summary follow in the background).
    pub fn open_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() {
            return;
        }
        let base_default = self
            .info
            .as_ref()
            .and_then(|i| i.default_base.clone())
            .unwrap_or_default();
        let seed = default_body(&self.task, None, None);
        let title_text = default_title(&self.task);
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(title_text)
                .placeholder(s::TITLE)
        });
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base_default)
                .placeholder(s::BASE)
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(5, 14)
                .placeholder(s::BODY)
        });
        body.update(cx, |b, cx| b.set_value(seed.clone(), window, cx));
        self.form = Some(Form {
            title,
            base,
            body,
            body_seed: seed,
            draft: false,
            focus_title: true,
        });
        self.action_error = None;
        // Diff stats + Pod review comment counts for the body.
        let diff = self.source.request(
            <p::TaskDiff as p::Method>::NAME,
            json!({"task": self.task_id}),
        );
        let comments = self
            .source
            .request("review.comments.list", json!({"task": self.task_id}));
        cx.spawn_in(window, async move |this, cx| {
            let d = diff
                .await
                .ok()
                .and_then(Result::ok)
                .map(|v| DiffStats::from_json(&v));
            let c = comments.await.ok().and_then(Result::ok).and_then(|v| {
                let list = v.get("comments")?.as_array()?.clone();
                let resolved = list
                    .iter()
                    .filter(|c| c.get("state").and_then(Value::as_str) == Some("resolved"))
                    .count();
                Some((list.len(), resolved))
            });
            let _ = this.update_in(cx, |s, window, cx| {
                let full = default_body(&s.task, d.as_ref(), c);
                if let Some(f) = s.form.as_mut() {
                    let current = f.body.read(cx).value().to_string();
                    if current == f.body_seed {
                        f.body
                            .update(cx, |b, cx| b.set_value(full.clone(), window, cx));
                        f.body_seed = full;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        cx.notify();
    }

    fn toast(&self, kind: ToastKind, title: String, body: String, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| w.raise_toast(kind, title, body, cx));
        });
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.form else { return };
        if self.busy.is_some() {
            return;
        }
        let title = f.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            self.action_error = Some(format!("{}: 비어 있습니다", s::TITLE));
            cx.notify();
            return;
        }
        let base = f.base.read(cx).value().trim().to_string();
        let params = p::TaskPrCreateParams {
            task: self.task_id.clone(),
            title,
            body: f.body.read(cx).value().to_string(),
            draft: f.draft,
            base: (!base.is_empty()).then_some(base),
            push: Some(true),
        };
        let rx = self.source.call::<p::TaskPrCreate>(&params);
        self.busy = Some(s::PUSHING);
        self.action_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = None;
                match r {
                    Ok(i) => {
                        let body = i
                            .status
                            .pr
                            .as_ref()
                            .map(|p| format!("#{} {}", p.number, p.url))
                            .unwrap_or_default();
                        s.toast(ToastKind::Success, s::CREATED.into(), body, cx);
                        s.info = Some(i);
                        s.form = None;
                    }
                    Err(e) => {
                        let text = error_text(&e);
                        s.toast(ToastKind::Error, s::CREATE_FAILED.into(), text.clone(), cx);
                        s.action_error = Some(text);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn confirm_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pr) = self.info.as_ref().and_then(|i| i.status.pr.clone()) else {
            return;
        };
        let method = self.method;
        let me = cx.entity().downgrade();
        let failed = self
            .info
            .as_ref()
            .is_some_and(|i| i.status.summary.failed > 0);
        let conflict = conflicted(&pr);
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let t = tokens();
            let me = me.clone();
            dialog
                .title(s::MERGE_CONFIRM_TITLE)
                .w(px(520.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(format!("PR #{} \"{}\"을(를) {}에 {} 방식으로 병합할까요?", pr.number, pr.title, pr.base, method_label(method)))
                        .child(crate::review::words("GitHub 저장소가 바뀌며 되돌릴 수 없습니다. 로컬 작업 폴더와 브랜치는 그대로 남습니다.", "pr-text").text_xs().text_color(tc(t.text_secondary)))
                        .when(failed, |d| d.child(crate::review::words(s::MERGE_FAILED_CHECKS, "pr-text").text_xs().text_color(tc(t.warning_fg))))
                        .when(conflict, |d| d.child(crate::review::words(s::MERGE_CONFLICT, "pr-text").text_xs().text_color(tc(t.warning_fg)))),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("pr-merge-cancel").label(s::CANCEL).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("pr-merge-ok").label(s::MERGE_OK).primary().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = me.update(cx, |s, cx| s.merge(cx));
                        })),
                )
        });
    }

    pub fn merge(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let rx = self.source.call::<p::TaskPrMerge>(&p::TaskPrMergeParams {
            task: self.task_id.clone(),
            method: self.method,
        });
        self.busy = Some(s::MERGING);
        self.action_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = None;
                match r {
                    Ok(i) => {
                        let body = i
                            .status
                            .pr
                            .as_ref()
                            .map(|p| format!("#{} → {}", p.number, p.base))
                            .unwrap_or_default();
                        s.toast(ToastKind::Success, s::MERGED.into(), body, cx);
                        s.info = Some(i);
                    }
                    Err(e) => {
                        let text = error_text(&e);
                        s.toast(ToastKind::Error, s::MERGE_FAILED.into(), text.clone(), cx);
                        s.action_error = Some(text);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_url(&self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = crate::term_links::open_url(url) {
            self.toast(ToastKind::Error, s::OPEN_FAILED.into(), e, cx);
        }
    }

    // ---- rendering --------------------------------------------------------

    fn muted(text: impl Into<SharedString>) -> Div {
        div()
            .text_xs()
            .text_color(tc(tokens().text_muted))
            .child(text.into())
    }

    fn card() -> Div {
        let t = tokens();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(tc(t.bg_surface))
            .border_1()
            .border_color(tc(t.border_subtle))
    }

    fn refresh_button(cx: &mut Context<Self>, label: &'static str) -> Button {
        Button::new("pr-refresh")
            .label(label)
            .xsmall()
            .ghost()
            .tooltip(s::REFRESH_TIP)
            .on_click(cx.listener(|s, _, _, cx| s.refresh(cx)))
    }

    fn branch_line(i: &TaskPrInfo) -> String {
        let mut parts = vec![
            format!(
                "{} → {}",
                i.branch,
                i.default_base.as_deref().unwrap_or("?")
            ),
            format!("커밋 {}개", i.ahead),
        ];
        if i.unpushed > 0 {
            parts.push(format!("푸시 안 됨 {}", i.unpushed));
        }
        if i.dirty > 0 {
            parts.push(format!("커밋 안 한 변경 {}", i.dirty));
        }
        parts.join(" · ")
    }

    fn render_problem(&self, kind: ForgeErrorKind, message: &str, cx: &mut Context<Self>) -> Div {
        let t = tokens();
        let (title, body) = kind_text(kind);
        let no_remote = matches!(
            kind,
            ForgeErrorKind::NoRemote | ForgeErrorKind::UnsupportedHost
        );
        Self::card()
            .qa_id(|| "pr-problem".into())
            .child(
                crate::review::words(title, "pr-title")
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tc(if no_remote {
                        t.text_primary
                    } else {
                        t.warning_fg
                    })),
            )
            .when(!body.is_empty(), |d| {
                d.child(
                    crate::review::words(body, "pr-text")
                        .text_xs()
                        .text_color(tc(t.text_secondary)),
                )
            })
            .when(kind == ForgeErrorKind::NoRemote, |d| {
                d.child(
                    crate::review::words(s::NO_REMOTE_STEPS, "pr-text")
                        .text_xs()
                        .text_color(tc(t.text_secondary)),
                )
            })
            .when(!no_remote && !message.is_empty(), |d| {
                d.child(Self::muted(message.to_string()))
            })
            .child(
                div().flex().child(
                    Button::new("pr-check-again")
                        .label(s::CHECK_AGAIN)
                        .small()
                        .on_click(cx.listener(|s, _, _, cx| s.refresh(cx))),
                ),
            )
    }

    fn render_form(&self, f: &Form, cx: &mut Context<Self>) -> Div {
        let label = |t: &'static str| Self::muted(t);
        let draft = f.draft;
        Self::card()
            .qa_id(|| "pr-form".into())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::CREATE),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(s::TITLE))
                    .child(Input::new(&f.title)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(s::BASE))
                    .child(Input::new(&f.base)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(s::BODY))
                    .child(Textarea::new(&f.body)),
            )
            .child(
                Checkbox::new("pr-draft")
                    .label(s::DRAFT)
                    .checked(draft)
                    .on_click(cx.listener(|s, v: &bool, _, cx| {
                        if let Some(f) = s.form.as_mut() {
                            f.draft = *v;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("pr-form-cancel")
                            .label(s::CANCEL)
                            .small()
                            .on_click(cx.listener(|s, _, _, cx| s.close_form(cx))),
                    )
                    .child(
                        Button::new("pr-form-submit")
                            .label(self.busy.unwrap_or(s::SUBMIT))
                            .small()
                            .primary()
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(|s, _, _, cx| s.submit(cx))),
                    ),
            )
    }

    fn render_checks(&self, i: &TaskPrInfo, cx: &mut Context<Self>) -> Div {
        let t = tokens();
        let mut checks: Vec<&PrCheck> = i.status.checks.iter().collect();
        checks.sort_by_key(|c| check_order(c));
        let skipped = checks
            .iter()
            .filter(|c| c.bucket == CheckBucket::Skipping)
            .count();
        let shown: Vec<&PrCheck> = checks
            .into_iter()
            .filter(|c| self.show_skipped || c.bucket != CheckBucket::Skipping)
            .collect();
        let verdict_color = match i.status.summary.verdict() {
            "failed" => t.danger_fg,
            "pending" => t.warning_fg,
            "passed" => t.success_fg,
            _ => t.text_muted,
        };
        let mut list = div().flex().flex_col().gap_1();
        for (ix, c) in shown.iter().enumerate() {
            let (glyph, color) = bucket_glyph(c.bucket);
            let link = c.link.clone();
            let name = c.name.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(pod_ui::icon_sized(glyph, 14., color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(name.clone()))
                            .child(div().truncate().text_color(tc(t.text_muted)).child(format!(
                                    "{}{}",
                                    bucket_word(c.bucket),
                                    c.workflow
                                        .as_deref()
                                        .map(|w| format!(" · {w}"))
                                        .unwrap_or_default()
                                ))),
                    )
                    .when_some(link, |d, url| {
                        d.child(
                            Button::new(("pr-check-open", ix))
                                .label(s::OPEN_CHECK)
                                .xsmall()
                                .ghost()
                                .accessibility_label(format!(
                                    "체크 {name} ({}) 열기",
                                    bucket_word(c.bucket)
                                ))
                                .tooltip(url.clone())
                                .on_click(cx.listener(move |s, _, _, cx| s.open_url(&url, cx))),
                        )
                    }),
            );
        }
        let cached = i.status.checks.is_empty() && i.status.summary.total > 0;
        div()
            .qa_id(|| "pr-checks".into())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                pod_ui::icon_text(
                    checks_icon(&i.status.summary),
                    verdict_color,
                    checks_line(&i.status.summary),
                )
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD),
            )
            .when(cached, |d| d.child(Self::muted(s::CHECKS_CACHED)))
            .child(list)
            .when(skipped > 0, |d| {
                d.child(
                    div().flex().child(
                        Button::new("pr-skipped")
                            .label(if self.show_skipped {
                                s::HIDE_SKIPPED.to_string()
                            } else {
                                format!("{} ({skipped})", s::SHOW_SKIPPED)
                            })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.show_skipped = !s.show_skipped;
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    fn render_pr(&self, i: &TaskPrInfo, pr: &p::PrView, cx: &mut Context<Self>) -> Div {
        let t = tokens();
        let (state, fg, bg, state_icon) = match (pr.state, pr.draft) {
            (PrState::Open, true) => (
                s::STATE_DRAFT,
                t.text_secondary,
                t.bg_raised,
                pod_ui::Icon::Pencil,
            ),
            (PrState::Open, false) => (
                s::STATE_OPEN,
                t.info_fg,
                t.info_bg,
                pod_ui::Icon::GitPullRequest,
            ),
            (PrState::Merged, _) => (
                s::STATE_MERGED,
                t.success_fg,
                t.success_bg,
                pod_ui::Icon::GitMerge,
            ),
            (PrState::Closed, _) => (s::STATE_CLOSED, t.danger_fg, t.danger_bg, pod_ui::Icon::X),
        };
        let (review, review_fg, review_icon) = match pr.review_decision {
            Some(ReviewDecision::Approved) => (
                s::REVIEW_APPROVED,
                t.success_fg,
                Some(pod_ui::Icon::CircleCheck),
            ),
            Some(ReviewDecision::ChangesRequested) => {
                (s::REVIEW_CHANGES, t.danger_fg, Some(pod_ui::Icon::OctagonX))
            }
            Some(ReviewDecision::ReviewRequired) => (s::REVIEW_REQUIRED, t.warning_fg, None),
            None => (s::REVIEW_NONE, t.text_muted, None),
        };
        let url = pr.url.clone();
        let open = pr.state == PrState::Open;
        let mergeable = open && !pr.draft;
        let method_button = |m: MergeMethod, id: &'static str, cx: &mut Context<Self>| {
            Button::new(id)
                .label(method_label(m))
                .xsmall()
                .when(self.method == m, |b| b.primary())
                .when(self.method != m, |b| b.ghost())
                .accessibility_label(format!(
                    "병합 방식: {}{}",
                    method_label(m),
                    if self.method == m { " (선택됨)" } else { "" }
                ))
                .on_click(cx.listener(move |s, _, _, cx| {
                    s.method = m;
                    cx.notify();
                }))
        };
        let merged_at = pr
            .merged_at
            .as_deref()
            .map(|m| m.replace('T', " ").trim_end_matches('Z').to_string());
        Self::card()
            .qa_id(|| "pr-card".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .rounded_sm()
                            .text_xs()
                            .bg(tc(bg))
                            .child(pod_ui::icon_text(state_icon, fg, state)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("#{}", pr.number)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(pr.title.clone()),
                    ),
            )
            .child(Self::muted(format!(
                "{} → {}{}",
                pr.head,
                if pr.base.is_empty() {
                    i.default_base.as_deref().unwrap_or("?")
                } else {
                    &pr.base
                },
                merged_at
                    .map(|m| format!(" · 병합 {m} UTC"))
                    .unwrap_or_default()
            )))
            .child(match review_icon {
                Some(icon) => pod_ui::icon_text(icon, review_fg, review).text_xs(),
                None => div().text_xs().text_color(tc(review_fg)).child(review),
            })
            .child(
                div().flex().gap_2().child(
                    Button::new("pr-open-browser")
                        .label(s::OPEN_BROWSER)
                        .xsmall()
                        .tooltip(url.clone())
                        .on_click(cx.listener(move |s, _, _, cx| s.open_url(&url, cx))),
                ),
            )
            .when(open && pr.draft, |d| {
                d.child(
                    crate::review::words(s::MERGE_DRAFT_NOTE, "pr-text")
                        .text_xs()
                        .text_color(tc(t.text_secondary)),
                )
            })
            .when(mergeable, |d| {
                d.child(
                    div()
                        .qa_id(|| "pr-merge".into())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_1()
                        .border_t_1()
                        .border_color(tc(t.border_subtle))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .child(method_button(MergeMethod::Squash, "pr-method-squash", cx))
                                .child(method_button(MergeMethod::Merge, "pr-method-merge", cx))
                                .child(method_button(MergeMethod::Rebase, "pr-method-rebase", cx))
                                .child(div().flex_1())
                                .child(
                                    Button::new("pr-merge")
                                        .label(self.busy.unwrap_or(s::MERGE))
                                        .small()
                                        .primary()
                                        .disabled(self.busy.is_some())
                                        .on_click(cx.listener(|s, _, window, cx| {
                                            s.confirm_merge(window, cx)
                                        })),
                                ),
                        )
                        .when(i.status.summary.failed > 0, |d| {
                            d.child(
                                crate::review::words(s::MERGE_FAILED_CHECKS, "pr-text")
                                    .text_xs()
                                    .text_color(tc(t.warning_fg)),
                            )
                        })
                        .when(conflicted(pr), |d| {
                            d.child(
                                crate::review::words(s::MERGE_CONFLICT, "pr-text")
                                    .text_xs()
                                    .text_color(tc(t.warning_fg)),
                            )
                        }),
                )
            })
            .child(self.render_checks(i, cx))
            .when(!i.status.comments.is_empty(), |d| {
                d.child(Self::muted(format!(
                    "{} {}개",
                    s::GITHUB_COMMENTS,
                    i.status.comments.len()
                )))
            })
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens();
        if let Some(e) = &self.load_error {
            return Self::card()
                .child(div().text_xs().text_color(tc(t.danger_fg)).child(e.clone()))
                .child(div().flex().child(Self::refresh_button(cx, s::CHECK_AGAIN)))
                .into_any_element();
        }
        let Some(i) = self.info.clone() else {
            return crate::chrome::loading_placeholder(s::CHECKING)
                .p_2()
                .into_any_element();
        };
        if i.status.remote.is_none()
            && let Some(e) = &i.status.error
        {
            return self
                .render_problem(e.kind, &e.message, cx)
                .into_any_element();
        }
        let mut col = div().flex().flex_col().gap_2();
        col = col.child(Self::muted(Self::branch_line(&i)));
        if let Some(r) = &i.status.remote {
            col = col.child(Self::muted(format!("{} · {}", r.name, r.slug.web_url())));
        }
        match &i.status.pr {
            Some(pr) => col = col.child(self.render_pr(&i, pr, cx)),
            None => {
                if let Some(f) = &self.form {
                    col = col.child(self.render_form(f, cx));
                } else {
                    let can = i.ahead > 0 && self.busy.is_none();
                    col = col.child(
                        Self::card()
                            .qa_id(|| "pr-none".into())
                            .child(
                                crate::review::words(s::NO_PR, "pr-title")
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(
                                div().flex().child(
                                    Button::new("pr-create")
                                        .label(s::CREATE)
                                        .small()
                                        .primary()
                                        .disabled(!can)
                                        .tooltip(s::CREATE_TIP)
                                        .on_click(
                                            cx.listener(|s, _, window, cx| s.open_form(window, cx)),
                                        ),
                                ),
                            )
                            .when(i.ahead == 0, |d| {
                                d.child(
                                    crate::review::words(s::NO_COMMITS, "pr-text")
                                        .text_xs()
                                        .text_color(tc(t.warning_fg)),
                                )
                            }),
                    );
                }
                if i.dirty > 0 {
                    col = col.child(Self::muted(s::DIRTY_NOTE));
                }
            }
        }
        if let Some(e) = i
            .status
            .error
            .as_ref()
            .filter(|_| i.status.remote.is_some())
        {
            let (title, _) = kind_text(e.kind);
            col = col.child(
                div()
                    .text_xs()
                    .text_color(tc(t.warning_fg))
                    .child(format!("{title} · {}", e.message)),
            );
        }
        if let Some(e) = &self.action_error {
            col = col.child(
                div()
                    .qa_id(|| "pr-action-error".into())
                    .text_xs()
                    .text_color(tc(t.danger_fg))
                    .child(e.clone()),
            );
        }
        col.into_any_element()
    }
}

impl Render for PrSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let body = self.render_body(cx);
        let form_shown = self
            .info
            .as_ref()
            .is_some_and(|i| i.status.pr.is_none() && i.status.remote.is_some());
        if let Some(f) = self.form.as_mut().filter(|f| f.focus_title && form_shown) {
            // The form may open before the first status (group winner → PR): fill the base now.
            if f.base.read(cx).value().is_empty()
                && let Some(b) = self.info.as_ref().and_then(|i| i.default_base.clone())
            {
                f.base.update(cx, |s, cx| s.set_value(b, window, cx));
            }
            f.focus_title = false;
            let focus = f.title.read(cx).focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        div()
            .id("pr-section")
            .qa_id(|| "pr-section".into())
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .text_color(tc(t.text_primary))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(tc(t.text_muted))
                            .child(if self.loading { s::CHECKING } else { "GitHub" }),
                    )
                    .child(Self::refresh_button(cx, s::REFRESH)),
            )
            .child(body)
    }
}

// ---- fixtures (`--ui-fixture --fixture-scene pr-*`) -----------------------

/// Scene names with PR fixture data.
pub const SCENES: &[&str] = &[
    "pr-no-remote",
    "pr-ready",
    "pr-form",
    "pr-pending",
    "pr-failed",
    "pr-passed",
    "pr-merged",
];

static FIXTURE_SCENE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static FIXTURE_FORGE: std::sync::Mutex<Option<pod_forge::fake::FakeForge>> =
    std::sync::Mutex::new(None);

pub fn set_fixture_scene(scene: &str) {
    let _ = FIXTURE_SCENE.set(scene.to_string());
}

fn fixture_remote() -> p::ForgeRemote {
    p::ForgeRemote {
        name: "origin".into(),
        url: "https://github.com/pod-fixtures/sample.git".into(),
        slug: p::RepoSlug {
            host: "github.com".into(),
            owner: "pod-fixtures".into(),
            name: "sample".into(),
        },
    }
}

/// The fake forge prepared for the scene (no network, no git).
fn fixture_forge(scene: &str) -> pod_forge::fake::FakeForge {
    use pod_forge::Forge as _;
    use pod_forge::fake::{FakeForge, FakeState};
    let state = FakeState {
        remote: Some(fixture_remote()),
        no_remote: scene == "pr-no-remote",
        ..FakeState::default()
    };
    let f = FakeForge::new(state);
    let dir = std::path::Path::new(".");
    let checks = match scene {
        "pr-failed" => Some("failed"),
        "pr-passed" | "pr-merged" => Some("passed"),
        "pr-pending" => Some("pending"),
        _ => None,
    };
    if let Some(v) = checks {
        let r = fixture_remote();
        let _ = f.push(dir, &r, "pod/task-000");
        let req = pod_forge::CreatePr {
            head: "pod/task-000".into(),
            base: "main".into(),
            title: "로그인 화면 오류 메시지 정리".into(),
            body: String::new(),
            draft: false,
        };
        if let Ok(pr) = f.create(dir, &r, &req) {
            f.edit(|s| {
                let x = s.pr_mut(pr.number).expect("fixture pr");
                x.view.number = 42;
                x.view.url = "https://github.com/pod-fixtures/sample/pull/42".into();
                x.view.additions = 38;
                x.view.deletions = 12;
                x.checks = pod_forge::fixtures::checks_for(v);
                if v == "passed" {
                    x.view.review_decision = Some(p::ReviewDecision::Approved);
                    x.view.merge_state = Some("CLEAN".into());
                }
            });
            if scene == "pr-merged" {
                let _ = f.merge(dir, &r, 42, MergeMethod::Squash);
                f.edit(|s| {
                    if let Some(x) = s.pr_mut(42) {
                        x.view.merged_at = Some("2026-11-09T07:30:00Z".into());
                    }
                });
            }
        }
    }
    f
}

/// Answers `task.pr.*` for the read-only UI fixture from the fake forge.
pub fn fixture_answer(method: &str, params: &Value) -> Option<Result<Value, SourceError>> {
    let scene = FIXTURE_SCENE.get()?.clone();
    if !method.starts_with("task.pr.") {
        return None;
    }
    let mut guard = FIXTURE_FORGE.lock().unwrap();
    let forge = guard.get_or_insert_with(|| fixture_forge(&scene));
    let dir = std::path::Path::new(".");
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or("sample/task-000")
        .to_string();
    let branch = "pod/task-000";
    let info = |status: pod_forge::PrStatus, pushed: bool| p::TaskPrInfo {
        task: task.clone(),
        branch: branch.into(),
        default_base: Some("main".into()),
        ahead: 2,
        unpushed: if pushed { 0 } else { 2 },
        dirty: 0,
        cached: status.remote.is_none(),
        status,
        refreshed_ms: 0,
    };
    let r = fixture_remote();
    use pod_forge::Forge as _;
    let out = match method {
        "task.pr.create" => {
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let req = pod_forge::CreatePr {
                head: branch.into(),
                base: params
                    .get("base")
                    .and_then(Value::as_str)
                    .unwrap_or("main")
                    .into(),
                title,
                body: params
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                draft: params
                    .get("draft")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            let res = forge
                .detect(dir)
                .and_then(|_| forge.push(dir, &r, branch))
                .and_then(|_| forge.create(dir, &r, &req));
            match res {
                Ok(_) => info(pod_forge::status::collect(forge, dir, branch, None), true),
                Err(e) => {
                    return Some(Err(SourceError {
                        code: Some(p::pr_codes::FORGE_FAILED),
                        message: e.message,
                    }));
                }
            }
        }
        "task.pr.merge" => {
            let m = params
                .get("method")
                .and_then(Value::as_str)
                .and_then(MergeMethod::parse)
                .unwrap_or(MergeMethod::Squash);
            let n = forge
                .state()
                .pr_for(branch)
                .map(|x| x.view.number)
                .unwrap_or(0);
            if let Err(e) = forge.merge(dir, &r, n, m) {
                return Some(Err(SourceError {
                    code: Some(p::pr_codes::FORGE_FAILED),
                    message: e.message,
                }));
            }
            info(pod_forge::status::collect(forge, dir, branch, None), true)
        }
        _ => {
            let st = pod_forge::status::collect(forge, dir, branch, None);
            let pushed = st.pr.is_some();
            info(st, pushed)
        }
    };
    Some(serde_json::to_value(out).map_err(|e| SourceError {
        code: None,
        message: e.to_string(),
    }))
}

/// Opens the scene: the Review panel of task-000 on its PR tab.
pub fn run_scene(scene: &str, w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    w.open_pr("sample/task-000", scene == "pr-form", window, cx);
    // Room for the PR column next to the diff at the capture size.
    if let Some(r) = w
        .live_reviews()
        .into_iter()
        .find(|r| r.read(cx).task_id == "sample/task-000")
    {
        r.update(cx, |r, cx| r.set_file_list(false, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;

    #[test]
    fn comment_terms_are_one_word() {
        // ui-15: the review sidebar says "의견" everywhere (tab, list, shortcuts).
        for text in [s::TAB_COMMENTS, s::TAB_COMMENTS_TIP, s::GITHUB_COMMENTS] {
            assert!(!text.contains("코멘트"), "{text}");
        }
        assert_eq!(s::TAB_COMMENTS, "의견");
    }

    fn task(prompt: Option<&str>) -> TaskVm {
        let mut t = TaskVm::from_json(&json!({"id": "sample/login-fix", "name": "login-fix", "repo": "sample", "terminals": [], "branch": "pod/login-fix"})).unwrap();
        t.prompt = prompt.map(str::to_owned);
        t
    }

    #[test]
    fn title_and_body_prefill() {
        assert_eq!(default_title(&task(None)), "login-fix");
        assert_eq!(
            default_title(&task(Some("\n  로그인 오류 메시지를 정리해 줘\n자세히는…"))),
            "로그인 오류 메시지를 정리해 줘"
        );
        let long = "가".repeat(100);
        assert_eq!(default_title(&task(Some(&long))).chars().count(), 72);
        let d = DiffStats::from_json(
            &json!({"files": [{"path": "src/a.rs", "added": 3, "deleted": 1}, {"path": "img.png", "binary": true}], "added": 3, "deleted": 1}),
        );
        let body = default_body(&task(Some("요청 내용")), Some(&d), Some((3, 1)));
        assert!(body.starts_with("요청 내용\n\n## 변경 요약\n- 파일 2개, +3 −1\n- `src/a.rs` (+3 −1)\n- `img.png` (바이너리)\n"), "{body}");
        assert!(body.contains("- Pod 리뷰 코멘트 3개 (해결 1개)"), "{body}");
        assert!(
            body.ends_with("Pod 작업 `sample/login-fix`에서 만들었습니다.\n"),
            "{body}"
        );
        let bare = default_body(&task(None), None, Some((0, 0)));
        assert!(
            !bare.contains("## 리뷰") && !bare.contains("## 변경"),
            "{bare}"
        );
    }

    #[test]
    fn conflicts_from_github_merge_state() {
        // Values of a real conflicting PR (gh pr view --json mergeable,mergeStateStatus).
        let mut pr = p::PrView {
            mergeable: Some("CONFLICTING".into()),
            merge_state: Some("DIRTY".into()),
            ..Default::default()
        };
        assert!(conflicted(&pr));
        pr.mergeable = Some("UNKNOWN".into());
        assert!(conflicted(&pr), "DIRTY alone");
        pr.merge_state = Some("UNSTABLE".into());
        pr.mergeable = Some("MERGEABLE".into());
        assert!(!conflicted(&pr), "failing checks are not a conflict");
        assert!(!conflicted(&p::PrView::default()));
    }

    #[test]
    fn checks_and_error_texts() {
        assert_eq!(checks_line(&ChecksSummary::default()), "체크 없음");
        let s = ChecksSummary {
            total: 6,
            passed: 3,
            failed: 1,
            pending: 1,
            skipped: 1,
        };
        assert_eq!(
            checks_line(&s),
            "체크 6개 · 통과 3 · 실패 1 · 진행 중 1 · 건너뜀 1"
        );
        assert_eq!(
            checks_icon(&s),
            pod_ui::Icon::OctagonX,
            "the verdict is an icon, not a text glyph"
        );
        let e = SourceError {
            code: Some(p::pr_codes::FORGE_FAILED),
            message: "not logged in to GitHub (run `gh auth login`): To get started".into(),
        };
        assert_eq!(
            kind_of_message(&e.message),
            Some(ForgeErrorKind::NotLoggedIn)
        );
        assert!(
            error_text(&e)
                .starts_with("GitHub에 로그인되어 있지 않습니다 · 터미널에서 gh auth login"),
            "{}",
            error_text(&e)
        );
        // A push refused because someone else pushed to the branch (seen on a real repository).
        let rejected = SourceError {
            code: Some(p::pr_codes::FORGE_FAILED),
            message: "the remote branch has commits the task does not have (push rejected; pull or rebase in the task first): error: failed to push some refs".into(),
        };
        assert_eq!(
            kind_of_message(&rejected.message),
            Some(ForgeErrorKind::Rejected)
        );
        assert!(
            error_text(&rejected).starts_with("원격 브랜치가 바뀌어 푸시가 거절되었습니다 · "),
            "{}",
            error_text(&rejected)
        );
        let old = SourceError {
            code: Some(p::codes::METHOD_NOT_FOUND),
            message: "unknown method".into(),
        };
        assert_eq!(error_text(&old), s::NEEDS_NEWER_PODD);
        let other = SourceError {
            code: Some(-32020),
            message: "pod/x has no commits".into(),
        };
        assert_eq!(error_text(&other), "pod/x has no commits");
    }

    #[test]
    fn fixture_scenes_come_from_the_fake_forge() {
        for scene in SCENES {
            let f = fixture_forge(scene);
            let st =
                pod_forge::status::collect(&f, std::path::Path::new("."), "pod/task-000", None);
            match *scene {
                "pr-no-remote" => assert!(st.no_remote()),
                "pr-ready" | "pr-form" => assert!(st.remote.is_some() && st.pr.is_none()),
                "pr-pending" => assert_eq!(st.summary.verdict(), "pending"),
                "pr-failed" => assert_eq!(st.summary.verdict(), "failed"),
                "pr-passed" => assert_eq!(
                    (st.summary.verdict(), st.pr.unwrap().review_decision),
                    ("passed", Some(ReviewDecision::Approved))
                ),
                "pr-merged" => assert_eq!(st.pr.unwrap().state, PrState::Merged),
                _ => unreachable!(),
            }
        }
    }
}
