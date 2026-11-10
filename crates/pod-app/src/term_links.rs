//! Links in terminal output (docs/09 §12): resolve a printed file reference
//! against the terminal's folders and find the task editor that owns it, or
//! open a URL in the default browser.
//!
//! Detection lives in `pod_terminal_view::links`; this module only decides
//! what a link points at on this machine.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use pod_terminal_view::LinkTarget;

use crate::workspace::{ToastKind, Workspace};

/// Git Bash / MSYS prints `/c/Users/...`: the Windows form of that path.
fn msys_to_windows(p: &str) -> Option<String> {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b'/')
        .then(|| format!("{}:{}", (b[1] as char).to_ascii_uppercase(), &p[2..]))
}

/// The existing file `printed` names, tried as given (absolute) or against
/// each of `roots` in order (terminal cwd first, then the task worktree).
pub fn resolve_file(printed: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let printed = printed.trim();
    if printed.is_empty() {
        return None;
    }
    let given = msys_to_windows(printed).unwrap_or_else(|| printed.to_string());
    let p = Path::new(&given);
    let candidates: Vec<PathBuf> = if p.is_absolute() || p.has_root() {
        vec![p.to_path_buf()]
    } else {
        roots.iter().map(|r| r.join(p)).collect()
    };
    candidates.into_iter().find(|c| c.is_file())
}

/// `\\?\`-free canonical form for prefix comparison.
fn canonical(p: &Path) -> Option<PathBuf> {
    let c = std::fs::canonicalize(p).ok()?;
    let s = c.to_string_lossy();
    Some(PathBuf::from(
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
    ))
}

/// The task whose worktree contains `file` (`preferred` first: the
/// terminal's own task), with the worktree-relative path.
pub fn owning_task(
    file: &Path,
    preferred: Option<&str>,
    worktrees: &[(String, PathBuf)],
) -> Option<(String, String)> {
    let file = canonical(file)?;
    let mut order: Vec<&(String, PathBuf)> = worktrees.iter().collect();
    order.sort_by_key(|(id, _)| Some(id.as_str()) != preferred);
    for (id, root) in order {
        let Some(root) = canonical(root) else {
            continue;
        };
        if let Ok(rel) = file.strip_prefix(&root) {
            return Some((id.clone(), rel.to_string_lossy().replace('\\', "/")));
        }
    }
    None
}

/// Accept a link for underlining/listing: URLs always, files only when they exist.
pub fn accept(target: &LinkTarget, roots: &[PathBuf]) -> bool {
    match target {
        LinkTarget::Url(u) => u.starts_with("https://") || u.starts_with("http://"),
        LinkTarget::File { path, .. } => resolve_file(path, roots).is_some(),
    }
}

#[cfg(test)]
thread_local! {
    /// URLs "opened" by tests (no browser is started under `cargo test`).
    pub static OPENED_URLS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens an http(s) URL in the default browser (`ShellExecuteW "open"`).
/// Anything else is refused: a link never runs a program.
pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url}: http/https 주소만 열 수 있습니다"));
    }
    #[cfg(test)]
    {
        OPENED_URLS.with(|o| o.borrow_mut().push(url.to_string()));
        Ok(())
    }
    #[cfg(not(test))]
    {
        shell_open(url)
    }
}

#[cfg(all(windows, not(test)))]
fn shell_open(url: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (verb, target) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated UTF-16 and outlive the call.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if r as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute 오류 {}", r as isize))
    }
}

#[cfg(all(not(windows), not(test)))]
fn shell_open(url: &str) -> Result<(), String> {
    Err(format!("{url}: 이 플랫폼에서는 브라우저를 열 수 없습니다"))
}

// ─────────────────────────── workspace side ───────────────────────────

impl Workspace {
    /// Folders a terminal's relative links resolve against: its cwd, then its task's worktree.
    pub(crate) fn link_roots(&self, terminal: &str) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(t) = self.terminal(terminal)
            && !t.cwd.trim().is_empty()
        {
            roots.push(PathBuf::from(&t.cwd));
        }
        if let Some(wt) = self
            .task_of_terminal(terminal)
            .and_then(|t| t.worktree.clone())
            .filter(|w| !w.is_empty())
        {
            let wt = PathBuf::from(wt);
            if !roots.contains(&wt) {
                roots.push(wt);
            }
        }
        roots
    }

    fn task_of_terminal(&self, terminal: &str) -> Option<&crate::vm::TaskVm> {
        let by_field = self.terminal(terminal).and_then(|t| t.task_id.as_deref());
        self.tasks
            .iter()
            .find(|t| Some(t.id.as_str()) == by_field || t.terminals.iter().any(|x| x == terminal))
    }

    /// Push current link roots to every open terminal panel.
    pub(crate) fn refresh_link_roots(&mut self, cx: &mut Context<Self>) {
        for p in self.live_panels() {
            let roots = self.link_roots(&p.read(cx).terminal_id);
            p.update(cx, |p, cx| p.set_link_roots(roots, cx));
        }
    }

    /// Open a link from terminal `terminal`: a file in the owning task's
    /// editor at its line, or a URL in the default browser.
    pub(crate) fn open_terminal_link(
        &mut self,
        terminal: &str,
        target: LinkTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            LinkTarget::Url(url) => {
                if let Err(e) = open_url(&url) {
                    self.raise_toast(ToastKind::Error, crate::strings::LINK_OPEN_FAILED, e, cx);
                }
            }
            LinkTarget::File { path, line, col } => {
                let Some(file) = resolve_file(&path, &self.link_roots(terminal)) else {
                    self.raise_toast(
                        ToastKind::Warning,
                        crate::strings::LINK_FILE_NOT_FOUND,
                        path,
                        cx,
                    );
                    return;
                };
                let worktrees: Vec<(String, PathBuf)> = self
                    .tasks
                    .iter()
                    .filter_map(|t| {
                        t.worktree
                            .as_ref()
                            .filter(|w| !w.is_empty())
                            .map(|w| (t.id.clone(), PathBuf::from(w)))
                    })
                    .collect();
                let preferred = self.task_of_terminal(terminal).map(|t| t.id.clone());
                match owning_task(&file, preferred.as_deref(), &worktrees) {
                    Some((task, rel)) => self.open_editor(
                        &task,
                        Some((rel, line.max(1), col.unwrap_or(1).max(1))),
                        false,
                        window,
                        cx,
                    ),
                    None => self.raise_toast(
                        ToastKind::Info,
                        crate::strings::LINK_OUTSIDE_TASKS,
                        file.to_string_lossy().into_owned(),
                        cx,
                    ),
                }
            }
        }
    }

    /// The panel showing the focused terminal (the one the user typed in last).
    fn focused_terminal_panel(&self, cx: &App) -> Option<Entity<crate::panel::TerminalPanel>> {
        let id = self.focused_terminal.as_deref()?;
        self.live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
    }

    /// Palette "터미널에서 링크 열기": pick one of the links on the current
    /// terminal's screen with the keyboard.
    pub(crate) fn open_link_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.focused_terminal_panel(cx) else {
            self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_TERMINAL_FOR_LINKS,
                crate::strings::PICK_A_TERMINAL_FIRST,
                cx,
            );
            return;
        };
        let (terminal, links) = {
            let p = panel.read(cx);
            (p.terminal_id.clone(), p.view.read(cx).visible_links())
        };
        if links.is_empty() {
            self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_LINKS_IN_VIEW,
                crate::strings::NO_LINKS_IN_VIEW_BODY,
                cx,
            );
            return;
        }
        let entries = links
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let kind = match l.target {
                    LinkTarget::Url(_) => crate::strings::LINK_KIND_URL,
                    LinkTarget::File { .. } => crate::strings::LINK_KIND_FILE,
                };
                (format!("link:{i}"), l.target.label(), kind.to_string())
            })
            .collect();
        self.link_choices = links
            .into_iter()
            .map(|l| (terminal.clone(), l.target))
            .collect();
        self.open_picker(crate::strings::OPEN_LINK_FROM_TERMINAL, entries, window, cx);
    }

    /// A `link:<n>` palette id: open that link from the last picker.
    pub(crate) fn open_link_choice(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((terminal, target)) = self.link_choices.get(n).cloned() {
            self.open_terminal_link(&terminal, target, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The gpui glob (above) exports its own `test` attribute.
    use core::prelude::v1::test;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("wt/src")).unwrap();
        std::fs::write(d.path().join("wt/src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(d.path().join("wt/README.md"), "# x\n").unwrap();
        std::fs::create_dir_all(d.path().join("other")).unwrap();
        d
    }

    #[test]
    fn resolves_relative_to_cwd_then_worktree_with_both_separators() {
        let d = tree();
        let (wt, src) = (d.path().join("wt"), d.path().join("wt/src"));
        // cwd = wt/src: `main.rs:3` resolves there; `src/main.rs` via the worktree root.
        let roots = vec![src.clone(), wt.clone()];
        assert_eq!(resolve_file("main.rs", &roots), Some(src.join("main.rs")));
        assert_eq!(
            resolve_file("src/main.rs", &roots),
            Some(wt.join("src/main.rs"))
        );
        assert_eq!(
            resolve_file(r"src\main.rs", &roots),
            Some(wt.join(r"src\main.rs"))
        );
        assert_eq!(
            resolve_file("./README.md", &roots).map(|p| p.is_file()),
            Some(true)
        );
        assert_eq!(resolve_file("missing.rs", &roots), None);
        // Absolute paths are used as given.
        let abs = wt.join("src").join("main.rs");
        assert_eq!(resolve_file(&abs.to_string_lossy(), &[]), Some(abs.clone()));
        assert_eq!(
            resolve_file(&abs.to_string_lossy().replace('\\', "/"), &[]).map(|p| p.is_file()),
            Some(true)
        );
    }

    #[test]
    fn msys_paths_become_drive_paths() {
        assert_eq!(
            msys_to_windows("/c/Users/x/a.rs").as_deref(),
            Some("C:/Users/x/a.rs")
        );
        assert_eq!(msys_to_windows("/usr/bin"), None);
        assert_eq!(msys_to_windows("src/a.rs"), None);
    }

    #[test]
    fn owning_task_prefers_the_terminal_task_and_gives_a_slash_path() {
        let d = tree();
        let wt = d.path().join("wt");
        let file = wt.join(r"src\main.rs");
        let tasks = vec![
            ("a/outer".to_string(), d.path().to_path_buf()),
            ("a/inner".to_string(), wt.clone()),
        ];
        assert_eq!(
            owning_task(&file, Some("a/inner"), &tasks),
            Some(("a/inner".into(), "src/main.rs".into()))
        );
        assert_eq!(
            owning_task(&file, Some("a/outer"), &tasks),
            Some(("a/outer".into(), "wt/src/main.rs".into()))
        );
        assert_eq!(
            owning_task(&d.path().join("other"), None, &tasks[1..]),
            None
        );
    }

    #[test]
    fn accept_checks_files_and_url_schemes() {
        let d = tree();
        let roots = vec![d.path().join("wt")];
        assert!(accept(
            &LinkTarget::File {
                path: "src/main.rs".into(),
                line: 1,
                col: None
            },
            &roots
        ));
        assert!(!accept(
            &LinkTarget::File {
                path: "nope.rs".into(),
                line: 1,
                col: None
            },
            &roots
        ));
        assert!(accept(
            &LinkTarget::Url("https://example.com".into()),
            &roots
        ));
        assert!(
            open_url("file:///C:/Windows/notepad.exe").is_err(),
            "only http(s)"
        );
        assert!(open_url("https://example.com/x").is_ok());
        OPENED_URLS.with(|o| {
            assert_eq!(
                o.borrow().last().map(String::as_str),
                Some("https://example.com/x")
            )
        });
    }
}
