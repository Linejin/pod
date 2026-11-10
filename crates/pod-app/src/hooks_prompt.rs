//! In-app agent hook prompt (docs/14 §11.5, docs/07 §9.1).
//!
//! At launch the app runs `pod hooks status --json` (the `pod.exe` next to
//! this app) in the background. When Pod's hooks are missing, or point at a
//! different `pod-hook.exe` than the one shipped with this app, a
//! non-blocking banner offers **훅 설치** and **나중에**:
//!
//! - 훅 설치 first runs `pod hooks install --dry-run --json` and shows what
//!   it would change (Claude `settings.json`, Codex `hooks.json` and
//!   `config.toml` trust, the `pod-hook.exe` copy) and that every changed
//!   file is backed up first. Only the dialog's 설치 button runs the real
//!   install. Nothing is ever installed silently.
//! - 나중에 remembers the dismissal for this app version + `pod-hook.exe`
//!   build ([`HookCli::source_key`]); a new build asks again.
//!
//! `pod hooks uninstall` is deliberately not offered anywhere in the app
//! (installer and CLI only).
//!
//! The CLI sits behind [`HookCli`] so tests and the visual fixture use
//! [`MockHookCli`] and never touch the real agent configuration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::{Disableable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::Deserialize;

use crate::qa_marks::QaId;
use crate::workspace::{ToastKind, Workspace};

/// `pod hooks …` as the app needs it. Implementations run on a background thread.
pub trait HookCli: Send + Sync {
    /// `pod hooks status --json` (stdout).
    fn status(&self) -> Result<String, String>;
    /// `pod hooks install [--dry-run] --json` (stdout).
    fn install(&self, dry_run: bool) -> Result<String, String>;
    /// Identifies the `pod-hook.exe` this app ships (dismissals are per build).
    fn source_key(&self) -> String;
}

/// Startup option: which CLI the launch check uses (`None`: no check).
#[derive(Clone)]
pub struct HookCliHandle(pub Arc<dyn HookCli>);

impl std::fmt::Debug for HookCliHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookCliHandle")
    }
}

// ─────────────────────────── JSON (pod-cli hooks_install.rs) ───────────────────────────

#[derive(Clone, Debug, Deserialize)]
pub struct AgentStatus {
    pub agent: String,
    pub path: PathBuf,
    #[serde(default)]
    pub exists: bool,
    #[serde(default)]
    pub pod_events: Vec<String>,
    #[serde(default)]
    pub hook_exes: Vec<(String, bool)>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HookStatus {
    pub bin: PathBuf,
    #[serde(default)]
    pub bin_exists: bool,
    #[serde(default)]
    pub bin_current: Option<bool>,
    /// `pod-hook --version` of the installed copy (fu-daemon; `None`: missing
    /// or a build from before `--version`, i.e. old).
    #[serde(default)]
    pub bin_version: Option<String>,
    /// Version of the `pod-hook.exe` next to `pod.exe` (what install copies).
    #[serde(default)]
    pub source_version: Option<String>,
    #[serde(default)]
    pub agents: Vec<AgentStatus>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FileResult {
    pub agent: String,
    pub path: PathBuf,
    pub action: String,
    #[serde(default)]
    pub backup: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct InstallOutcome {
    pub hook_exe: PathBuf,
    #[serde(default)]
    pub files: Vec<FileResult>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// What the launch check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// No installed `pod-hook.exe`, or agents without Pod hooks.
    Missing(Vec<String>),
    /// Hooks exist but run another or an older `pod-hook.exe`.
    Outdated(String),
}

fn norm(p: &str) -> String {
    p.trim().trim_matches('"').replace('\\', "/").to_lowercase()
}

/// Decide from `pod hooks status`. `agent_present(config_path)` tells whether
/// an agent is used on this machine (its config folder exists): an agent the
/// user does not have is not "missing".
pub fn assess(st: &HookStatus, agent_present: impl Fn(&Path) -> bool) -> Verdict {
    let used: Vec<&AgentStatus> = st
        .agents
        .iter()
        .filter(|a| a.exists || agent_present(&a.path))
        .collect();
    let mut missing: Vec<String> = used
        .iter()
        .filter(|a| a.pod_events.is_empty())
        .map(|a| a.agent.clone())
        .collect();
    if !st.bin_exists && missing.is_empty() && !used.is_empty() {
        // Hooks configured but their binary is gone.
        return Verdict::Outdated(format!("{} 없음", st.bin.display()));
    }
    if !missing.is_empty() {
        missing.sort();
        return Verdict::Missing(missing);
    }
    if used.is_empty() {
        return Verdict::Ok;
    }
    let bin = norm(&st.bin.to_string_lossy());
    for a in &used {
        for (exe, exists) in &a.hook_exes {
            if norm(exe) != bin {
                return Verdict::Outdated(format!("{}: {exe}", a.agent));
            }
            if !exists {
                return Verdict::Outdated(format!("{}: {exe} 없음", a.agent));
            }
        }
    }
    // Versions (newer `pod`): an installed copy without `--version` is older than any that has one.
    if let Some(src) = &st.source_version
        && st.bin_version.as_ref() != Some(src)
    {
        let installed = st.bin_version.clone().unwrap_or_else(|| "?".into());
        return Verdict::Outdated(crate::tr!(HOOKS_VERSION_BODY; ; installed, src));
    }
    if st.bin_current == Some(false) {
        return Verdict::Outdated(format!("{} ≠ 이 Pod의 pod-hook.exe", st.bin.display()));
    }
    Verdict::Ok
}

/// Dismissal key: app version + the shipped `pod-hook.exe` build.
pub fn dismiss_key(source_key: &str) -> String {
    format!("{}/{source_key}", env!("CARGO_PKG_VERSION"))
}

/// Size + FNV-1a of a file: cheap and stable per build.
pub fn file_fingerprint(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in &bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            format!("{}-{h:016x}", bytes.len())
        }
        Err(_) => "none".into(),
    }
}

// ─────────────────────────── real CLI ───────────────────────────

/// `pod.exe` next to this app.
pub struct PodCli {
    exe: PathBuf,
    hook: PathBuf,
}

impl PodCli {
    /// `None` when there is no `pod.exe` beside the app (no check then).
    pub fn next_to_app() -> Option<Self> {
        let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let exe = dir.join(if cfg!(windows) { "pod.exe" } else { "pod" });
        exe.is_file().then(|| PodCli {
            exe,
            hook: dir.join(if cfg!(windows) {
                "pod-hook.exe"
            } else {
                "pod-hook"
            }),
        })
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(args).stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("{}: {e}", self.exe.display()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("{} 종료 코드 {:?}", self.exe.display(), out.status.code())
            } else {
                err
            })
        }
    }
}

impl HookCli for PodCli {
    fn status(&self) -> Result<String, String> {
        self.run(&["hooks", "status", "--json"])
    }
    fn install(&self, dry_run: bool) -> Result<String, String> {
        if dry_run {
            self.run(&["hooks", "install", "--dry-run", "--json"])
        } else {
            self.run(&["hooks", "install", "--json"])
        }
    }
    fn source_key(&self) -> String {
        file_fingerprint(&self.hook)
    }
}

// ─────────────────────────── mock (tests, visual fixture) ───────────────────────────

/// Scripted CLI: never runs `pod`, records install calls.
pub struct MockHookCli {
    pub status_json: String,
    pub key: String,
    pub installs: std::sync::Mutex<Vec<bool>>,
}

impl MockHookCli {
    /// Claude and Codex in use, no Pod hooks yet (fixture paths only).
    pub fn missing() -> Self {
        let status = serde_json::json!({
            "bin": "C:/Users/fixture/AppData/Local/Pod/bin/pod-hook.exe", "bin_exists": false,
            "agents": [
                {"agent": "claude", "path": "C:/Users/fixture/.claude/settings.json", "exists": true, "pod_events": [], "foreign_groups": 2, "hook_exes": []},
                {"agent": "codex", "path": "C:/Users/fixture/.codex/hooks.json", "exists": true, "pod_events": [], "foreign_groups": 1, "hook_exes": []}
            ]
        });
        MockHookCli {
            status_json: status.to_string(),
            key: "fixture-hook-1".into(),
            installs: Default::default(),
        }
    }
}

impl HookCli for MockHookCli {
    fn status(&self) -> Result<String, String> {
        Ok(self.status_json.clone())
    }
    fn install(&self, dry_run: bool) -> Result<String, String> {
        self.installs
            .lock()
            .map_err(|e| e.to_string())?
            .push(dry_run);
        let (claude, codex) = (
            "C:/Users/fixture/.claude/settings.json",
            "C:/Users/fixture/.codex/hooks.json",
        );
        let backup = |p: &str| (!dry_run).then(|| format!("{p}.pod-backup-20261109-120000"));
        Ok(serde_json::json!({
            "dry_run": dry_run, "hook_exe": "C:/Users/fixture/AppData/Local/Pod/bin/pod-hook.exe", "binary_copied": !dry_run,
            "files": [
                {"agent": "claude", "path": claude, "action": if dry_run { "would update" } else { "updated" }, "backup": backup(claude)},
                {"agent": "codex", "path": codex, "action": if dry_run { "would update" } else { "updated" }, "backup": backup(codex)}
            ],
            "notes": []
        })
        .to_string())
    }
    fn source_key(&self) -> String {
        self.key.clone()
    }
}

// ─────────────────────────── workspace state and UI ───────────────────────────

#[derive(Default)]
pub struct HookPromptState {
    cli: Option<Arc<dyn HookCli>>,
    /// Shown while `Some(Missing|Outdated)`.
    pub verdict: Option<Verdict>,
    key: String,
    pub busy: bool,
    _task: Option<Task<()>>,
}

fn action_label(action: &str) -> &'static str {
    match action {
        "would create" | "created" => crate::strings::HOOKS_FILE_CREATE,
        "would update" | "updated" => crate::strings::HOOKS_FILE_UPDATE,
        _ => crate::strings::HOOKS_FILE_UNCHANGED,
    }
}

fn agent_label(agent: &str) -> &str {
    match agent {
        "claude" => "Claude",
        "codex" => "Codex",
        other => other,
    }
}

/// Lines of the confirmation dialog for a dry-run outcome.
pub fn preview_lines(o: &InstallOutcome) -> Vec<String> {
    let mut v = vec![crate::tr!(HOOKS_PREVIEW_BINARY; ; o.hook_exe.display())];
    for f in &o.files {
        v.push(format!(
            "{} · {} — {}",
            agent_label(&f.agent),
            f.path.display(),
            action_label(&f.action)
        ));
    }
    if o.files.iter().any(|f| f.agent == "codex") {
        v.push(crate::strings::HOOKS_PREVIEW_CODEX_TRUST.into());
    }
    v.extend(o.notes.iter().cloned());
    v
}

impl Workspace {
    /// Launch check (background). Shows the banner only for a problem the
    /// user has not dismissed for this build.
    pub(crate) fn start_hook_check(&mut self, cli: Arc<dyn HookCli>, cx: &mut Context<Self>) {
        self.hooks.cli = Some(cli.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { (cli.status(), cli.source_key()) })
                .await;
            let _ = this.update(cx, |w, cx| {
                let (status, key) = result;
                let Ok(json) = status else { return };
                let Ok(st) = serde_json::from_str::<HookStatus>(&json) else {
                    return;
                };
                let verdict = assess(&st, |p| p.parent().is_some_and(Path::is_dir));
                let key = dismiss_key(&key);
                w.hooks.key = key.clone();
                if verdict != Verdict::Ok
                    && cx.global::<crate::theme::Settings>().hooks_dismissed != key
                {
                    w.hooks.verdict = Some(verdict);
                    cx.notify();
                }
            });
        });
        self.hooks._task = Some(task);
    }

    /// 나중에: hide, and do not ask again for this app + pod-hook build.
    pub(crate) fn dismiss_hook_prompt(&mut self, cx: &mut Context<Self>) {
        self.hooks.verdict = None;
        let mut settings = cx.global::<crate::theme::Settings>().clone();
        settings.hooks_dismissed = self.hooks.key.clone();
        if settings.save().is_err() {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::SETTINGS_COULD_NOT_BE_SAVED,
                String::new(),
                cx,
            );
        }
        cx.set_global(settings);
        cx.notify();
    }

    /// 훅 설치: dry run first, then a confirmation listing the changes.
    pub(crate) fn request_hook_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cli) = self.hooks.cli.clone() else {
            self.raise_toast(
                ToastKind::Info,
                crate::strings::HOOKS_NO_CLI,
                crate::strings::HOOKS_NO_CLI_BODY,
                cx,
            );
            return;
        };
        if self.hooks.busy {
            return;
        }
        self.hooks.busy = true;
        cx.notify();
        self.hooks._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { cli.install(true) }).await;
            let _ = this.update_in(cx, |w, window, cx| {
                w.hooks.busy = false;
                match result.and_then(|j| {
                    serde_json::from_str::<InstallOutcome>(&j).map_err(|e| e.to_string())
                }) {
                    Ok(o) => w.confirm_hook_install(o, window, cx),
                    Err(e) => w.raise_toast(
                        ToastKind::Error,
                        crate::strings::HOOKS_INSTALL_FAILED,
                        e,
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
    }

    fn confirm_hook_install(
        &mut self,
        preview: InstallOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = preview_lines(&preview);
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            let ws = ws.clone();
            let mut body = div()
                .id("hooks-preview")
                .flex()
                .flex_col()
                .gap_2()
                .text_sm()
                .role(Role::List)
                .aria_label(crate::strings::HOOKS_PREVIEW_TITLE);
            body = body.child(div().child(crate::strings::HOOKS_PREVIEW_INTRO));
            for (i, l) in lines.iter().enumerate() {
                body = body.child(
                    div()
                        .id(("hooks-line", i))
                        .role(Role::ListItem)
                        .aria_label(l.clone())
                        .px_2()
                        .child(l.clone()),
                );
            }
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(crate::theme::tokens().text_muted))
                    .child(crate::strings::HOOKS_PREVIEW_BACKUP),
            );
            d.title(crate::strings::HOOKS_PREVIEW_TITLE)
                .w(px(640.))
                .child(body)
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            div().qa_id(|| "hooks-cancel".into()).child(
                                Button::new("hooks-cancel")
                                    .label(crate::strings::CANCEL)
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                        )
                        .child(
                            div().qa_id(|| "hooks-confirm".into()).child(
                                Button::new("hooks-confirm")
                                    .label(crate::strings::HOOKS_INSTALL_CONFIRM)
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ =
                                            ws.update(cx, |w, cx| w.run_hook_install(window, cx));
                                    }),
                            ),
                        ),
                )
        });
    }

    /// The user confirmed: the real `pod hooks install`.
    fn run_hook_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cli) = self.hooks.cli.clone() else {
            return;
        };
        self.hooks.busy = true;
        cx.notify();
        self.hooks._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { cli.install(false) }).await;
            let _ = this.update(cx, |w, cx| {
                w.hooks.busy = false;
                match result.and_then(|j| {
                    serde_json::from_str::<InstallOutcome>(&j).map_err(|e| e.to_string())
                }) {
                    Ok(o) => {
                        w.hooks.verdict = None;
                        let backups: Vec<String> = o
                            .files
                            .iter()
                            .filter_map(|f| f.backup.as_ref().map(|b| b.display().to_string()))
                            .collect();
                        let body = if backups.is_empty() {
                            crate::strings::HOOKS_INSTALLED_NO_BACKUP.to_string()
                        } else {
                            crate::tr!(HOOKS_INSTALLED_BACKUPS; ; backups.join(", "))
                        };
                        w.raise_toast(
                            ToastKind::Success,
                            crate::strings::HOOKS_INSTALLED,
                            body,
                            cx,
                        );
                    }
                    Err(e) => w.raise_toast(
                        ToastKind::Error,
                        crate::strings::HOOKS_INSTALL_FAILED,
                        e,
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
    }

    /// Non-blocking banner above the dock while a hook problem is open.
    pub(crate) fn render_hook_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let verdict = self.hooks.verdict.as_ref()?;
        let (title, detail) = match verdict {
            Verdict::Ok => return None,
            Verdict::Missing(agents) => (
                crate::strings::HOOKS_MISSING_TITLE.to_string(),
                crate::tr!(HOOKS_MISSING_BODY; ; agents.iter().map(|a| agent_label(a)).collect::<Vec<_>>().join(", ")),
            ),
            Verdict::Outdated(reason) => (
                crate::strings::HOOKS_OUTDATED_TITLE.to_string(),
                crate::tr!(HOOKS_OUTDATED_BODY; ; reason),
            ),
        };
        let t = crate::theme::tokens();
        let busy = self.hooks.busy;
        Some(
            div()
                .id("hooks-banner")
                .qa_id(|| "hooks-banner".into())
                .role(Role::Alert)
                .aria_label(format!("{title}. {detail}"))
                .px_3()
                .py_1p5()
                .flex()
                .items_center()
                .gap_3()
                .bg(rgb(t.info_bg))
                .border_b_1()
                .border_color(rgb(t.border_subtle))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.text_primary))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(t.text_secondary))
                                .truncate()
                                .child(detail),
                        ),
                )
                .child(
                    div().qa_id(|| "hooks-install".into()).child(
                        Button::new("hooks-install")
                            .label(if busy {
                                crate::strings::HOOKS_CHECKING
                            } else {
                                crate::strings::HOOKS_INSTALL
                            })
                            .tooltip(crate::strings::HOOKS_INSTALL_TOOLTIP)
                            .primary()
                            .compact()
                            .disabled(busy)
                            .on_click(
                                cx.listener(|w, _, window, cx| w.request_hook_install(window, cx)),
                            ),
                    ),
                )
                .child(
                    div().qa_id(|| "hooks-later".into()).child(
                        Button::new("hooks-later")
                            .label(crate::strings::HOOKS_LATER)
                            .tooltip(crate::strings::HOOKS_LATER_TOOLTIP)
                            .compact()
                            .when(busy, |b| b.disabled(true))
                            .on_click(cx.listener(|w, _, _, cx| w.dismiss_hook_prompt(cx))),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn status(v: serde_json::Value) -> HookStatus {
        serde_json::from_value(v).unwrap()
    }

    const BIN: &str = "C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe";

    fn agent(name: &str, events: &[&str], exes: &[(&str, bool)]) -> serde_json::Value {
        serde_json::json!({"agent": name, "path": format!("C:/Users/u/.{name}/x.json"), "exists": true, "pod_events": events,
            "foreign_groups": 0, "hook_exes": exes.iter().map(|(e, ok)| serde_json::json!([e, ok])).collect::<Vec<_>>()})
    }

    #[test]
    fn missing_hooks_name_the_agents() {
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": false, "agents": [agent("claude", &[], &[]), agent("codex", &[], &[])]}),
        );
        assert_eq!(
            assess(&st, |_| true),
            Verdict::Missing(vec!["claude".into(), "codex".into()])
        );
        // Real `pod hooks status --json` shape (pod-cli hooks_install.rs `Status`).
        let real = r#"{"bin":"C:\\Users\\u\\AppData\\Local\\Pod\\bin\\pod-hook.exe","bin_exists":true,"bin_current":true,
            "agents":[{"agent":"claude","path":"C:\\Users\\u\\.claude\\settings.json","exists":true,"pod_events":["Stop"],"foreign_groups":3,
            "hook_exes":[["C:\\Users\\u\\AppData\\Local\\Pod\\bin\\pod-hook.exe",true]]}]}"#;
        assert_eq!(
            assess(&serde_json::from_str(real).unwrap(), |_| true),
            Verdict::Ok
        );
    }

    #[test]
    fn agents_not_in_use_are_not_missing() {
        let mut codex = agent("codex", &[], &[]);
        codex["exists"] = false.into();
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": true, "bin_current": true,
            "agents": [agent("claude", &["Stop"], &[(BIN, true)]), codex]}),
        );
        assert_eq!(
            assess(&st, |_| false),
            Verdict::Ok,
            "no ~/.codex: Codex is not in use"
        );
        assert_eq!(
            assess(&st, |_| true),
            Verdict::Missing(vec!["codex".into()]),
            "~/.codex exists, hooks.json does not"
        );
    }

    #[test]
    fn other_or_older_binaries_are_outdated() {
        let other = "C:/repo/target/release/pod-hook.exe";
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": true, "bin_current": true, "agents": [agent("claude", &["Stop"], &[(other, true)])]}),
        );
        assert!(
            matches!(assess(&st, |_| true), Verdict::Outdated(r) if r.contains("target/release"))
        );
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": true, "bin_current": false, "agents": [agent("claude", &["Stop"], &[(BIN, true)])]}),
        );
        assert!(
            matches!(assess(&st, |_| true), Verdict::Outdated(_)),
            "installed binary differs from this build"
        );
        // Same path in another spelling is the same binary.
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": true, "bin_current": true, "agents": [agent("claude", &["Stop"], &[(&BIN.replace('/', "\\").to_uppercase(), true)])]}),
        );
        assert_eq!(assess(&st, |_| true), Verdict::Ok);
        let st = status(
            serde_json::json!({"bin": BIN, "bin_exists": false, "agents": [agent("claude", &["Stop"], &[(BIN, false)])]}),
        );
        assert!(
            matches!(assess(&st, |_| true), Verdict::Outdated(_)),
            "hooks point at a deleted binary"
        );
    }

    #[test]
    fn versions_from_newer_pod_decide_outdated() {
        let base = |bin_version: Option<&str>| {
            status(
                serde_json::json!({"bin": BIN, "bin_exists": true, "bin_current": true, "bin_version": bin_version,
                "source": "C:/Program Files/Pod/pod-hook.exe", "source_version": "0.2.0", "agents": [agent("claude", &["Stop"], &[(BIN, true)])]}),
            )
        };
        assert_eq!(assess(&base(Some("0.2.0")), |_| true), Verdict::Ok);
        assert!(
            matches!(assess(&base(Some("0.1.0")), |_| true), Verdict::Outdated(r) if r.contains("0.1.0") && r.contains("0.2.0"))
        );
        assert!(
            matches!(assess(&base(None), |_| true), Verdict::Outdated(r) if r.contains('?')),
            "no --version: an old build"
        );
    }

    #[test]
    fn preview_lists_files_backups_and_codex_trust() {
        let cli = MockHookCli::missing();
        let o: InstallOutcome = serde_json::from_str(&cli.install(true).unwrap()).unwrap();
        let lines = preview_lines(&o);
        assert!(lines[0].contains("pod-hook.exe"));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Claude · C:/Users/fixture/.claude/settings.json"))
        );
        assert!(lines.iter().any(|l| l.starts_with("Codex · ")));
        assert!(lines.iter().any(|l| l.contains("config.toml")));
        assert_eq!(*cli.installs.lock().unwrap(), vec![true], "dry run only");
    }

    #[test]
    fn dismissal_key_changes_with_the_hook_build() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("pod-hook.exe");
        std::fs::write(&f, b"one").unwrap();
        let a = dismiss_key(&file_fingerprint(&f));
        std::fs::write(&f, b"two").unwrap();
        let b = dismiss_key(&file_fingerprint(&f));
        assert_ne!(a, b);
        assert!(a.starts_with(env!("CARGO_PKG_VERSION")));
        assert_eq!(file_fingerprint(&d.path().join("none.exe")), "none");
    }
}
