//! Narrow allowance for Pod-started Codex workers (docs/19 §18.1, §18.6).
//!
//! Codex runs model commands in its sandbox, which cannot open podd's pipe
//! (`pod orch …` fails with os error 5) and cannot write the worktree's git
//! admin dir (`git add`/`commit` fail on `index.lock`), so every Pod
//! coordination step and commit needed a human approval (docs/19 §17.3-4).
//!
//! What the installed Codex (0.159, Windows) offers, verified with the real
//! binary (`tools/e2e/codex-allowance-probe.ps1`):
//!
//! - Exec-policy rules (`prefix_rule(pattern=[…], decision="allow")`) run a
//!   matching command without approval **and outside the sandbox**. Codex
//!   loads them from `.rules` files only (`$CODEX_HOME/rules`, or a trusted
//!   project's `.codex/rules/`); no `-c` key carries rules inline. So Pod
//!   writes [`RULES_REL`] into the worker's own Pod-created worktree (Codex
//!   already trusts exactly that worktree through the inline `-c projects=…`
//!   override) and hides it from git with one line in the repository's
//!   `info/exclude` (git ignores a linked worktree's own `info/exclude`).
//!   The sandboxed agent cannot change it: Codex keeps `.codex` read-only.
//! - A rule names the program by its base name. Codex also matches a path
//!   to a program by that base name (`C:\any\pod.exe`, `.\pod`) unless a
//!   `host_executable(name=…, paths=[…])` lists the paths allowed for it
//!   (`codex execpolicy check --resolve-host-executables`). Pod's rules pin
//!   `pod` to Pod's own verified `pod.exe` beside podd ([`pinned_pod`]); a
//!   `pod` of the same name anywhere else (the worktree, say) stays in the
//!   sandbox. A bare `pod` is found through the worker's `PATH`, where Pod
//!   puts that same folder first. No pinned `pod.exe`: no allowance.
//! - Every rules file in the folder counts, and a later `host_executable`
//!   for `pod` replaces an earlier one. So Pod adds nothing when the
//!   worktree's `.codex` holds anything besides Pod's file (the repository's
//!   own rules or project config would get Pod's trust), and it never
//!   writes through a link or into a file it did not write
//!   ([`apply`]).
//! - Extra writable roots (`sandbox_workspace_write.writable_roots`) are
//!   refused for anything under a `.git` directory, so the worktree's git
//!   admin dir cannot be made writable.
//! - No `git` command is allowed outside the sandbox. An unsandboxed git
//!   runs hooks, fsmonitor and filters from the repository it finds, and the
//!   sandboxed agent can build one: Codex protects the worktree's own `.git`
//!   file, but not a nested `sub/.git`, and Codex's `workdir` argument runs
//!   an allowed `git commit` there (the probe saw its pre-commit hook and
//!   fsmonitor write outside the worktree; only git's ownership check, gone
//!   with `safe.directory=*`, stood in the way). Commits go through
//!   `pod orch commit` instead: podd commits in the worktree it recorded
//!   with git plumbing only, hooks, fsmonitor and every configured filter
//!   program off, and refuses paths a filter handles
//!   ([`crate::git::commit_for_worker`], §18.5, §18.6).
//!   `git status/diff/log` run in the sandbox (read only) without approval.
//! - Nothing that reads other terminals is allowed (`pod read`, `pod ls`,
//!   `pod orch worker read/show`): outside the sandbox they would show any
//!   Pod terminal, the coordinator's included.
//!
//! Nothing else changes: no writable roots, no network, no sandbox mode, no
//! user file (`~/.codex` is never written). `pod orch` subcommands that start
//! or stop processes (`worker start/stop/…`), change settings (`limits`) or
//! decide gates stay behind approval.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::log;
use crate::git::pin::{facts, final_key, open_no_follow, peek};
use crate::protocol::{AgentKind, AgentSpec};

/// The rules file, relative to the worktree root.
pub const RULES_REL: &str = ".codex/rules/pod-worker.rules";
/// Its line in `<common git dir>/info/exclude`.
pub const EXCLUDE_LINE: &str = "/.codex/rules/pod-worker.rules";

/// The allow rules Pod installs (Starlark, Codex exec-policy format); the
/// file is these plus [`PIN`] ([`rules_text`]).
pub const RULES: &str = r#"# Written by Pod for a Pod-started Codex worker (docs/19 §18.1).
# Pod coordination (commits too: `pod orch commit`, run by podd) runs
# without approval; everything else stays in Codex's sandbox. Turn off per repository: .pod/config.toml
# [worker] codex_allowance = false
prefix_rule(pattern=["pod", "orch", ["ask", "send", "check", "reply", "done", "wait", "commit", "workers", "queue"]], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "run", ["ls", "show"]], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "item", "ls"], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "gate", "request"], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "guide", "orchestration"], decision="allow", justification="Pod guide")
prefix_rule(pattern=["pod", "--version"], decision="allow", justification="Pod version")
"#;

/// The executable pin after [`RULES`]; `{pod}` is the Starlark-escaped
/// path of Pod's verified `pod.exe`.
pub const PIN: &str = r#"# Only this pod.exe matches the rules above (docs/19 §18.6): a `pod` at any other path stays in the sandbox.
host_executable(name="pod", paths=["{pod}"])
"#;

/// The rules file Pod writes for a worker whose `pod` is `pod`.
pub fn rules_text(pod: &Path) -> String {
    let p = pod
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("{RULES}{}", PIN.replace("{pod}", &p))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Written (or already Pod's current file).
    Installed,
    /// Pod's file was removed (allowance off).
    Removed,
    /// Off and nothing of Pod's was there.
    Absent,
    /// A different file is at [`RULES_REL`] (the repository's own): left alone.
    Foreign,
    /// On, but not applied (and Pod's earlier file removed when it safely
    /// could be): a link or a file Pod did not write on the way, other
    /// Codex config in the worktree, or no verified `pod.exe` to pin.
    Refused(String),
}

/// Before a Pod orchestration worker's agent starts in `worktree`: a Codex
/// worker gets the rules when `on` (`[worker] codex_allowance`, default on)
/// and loses Pod's earlier rules when off. Other agents: nothing. Problems
/// are logged, never fatal (the worker then just asks for approvals).
pub fn for_worker(task: &str, agent: &AgentSpec, worktree: &Path, on: bool) {
    if !matches!(agent, AgentSpec::Kind(AgentKind::Codex)) {
        return;
    }
    match apply(worktree, on, pinned_pod(worktree)) {
        Ok(Outcome::Installed) => log(&format!(
            "task {task}: codex worker allowance installed ({RULES_REL})"
        )),
        Ok(Outcome::Removed) => log(&format!(
            "task {task}: codex worker allowance off, rules removed"
        )),
        Ok(Outcome::Absent) => log(&format!("task {task}: codex worker allowance off")),
        Ok(Outcome::Foreign) => log(&format!(
            "task {task}: {RULES_REL} is the repository's own file; allowance not applied"
        )),
        Ok(Outcome::Refused(why)) => log(&format!(
            "task {task}: codex worker allowance not applied: {why}"
        )),
        Err(e) => log(&format!("task {task}: codex worker allowance: {e}")),
    }
}

fn rules_path(worktree: &Path) -> PathBuf {
    worktree.join(RULES_REL.replace('/', "\\"))
}

/// The `pod.exe` beside this podd, verified ([`verify_pod`]).
pub fn pinned_pod(worktree: &Path) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("podd's own path: {e}"))?;
    let dir = exe.parent().ok_or("podd's own folder is unknown")?;
    verify_pod(&dir.join("pod.exe"), worktree)
}

/// `candidate` may be the `pod` a Codex worker runs outside its sandbox: an
/// absolute path to a plain file named `pod.exe` (it and its folder not a
/// link or other reparse point), outside the worker's worktree, with no
/// `pod.com` beside it (`PATHEXT` finds `.COM` first). Returned in its
/// on-disk spelling, which the rules pin.
pub fn verify_pod(candidate: &Path, worktree: &Path) -> Result<PathBuf, String> {
    let why = |w: &str| -> Result<PathBuf, String> {
        Err(format!(
            "{} {w}; the Codex worker allowance pins Pod's own pod.exe beside podd",
            candidate.display()
        ))
    };
    if !candidate.is_absolute() {
        return why("is not an absolute path");
    }
    if !candidate
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("pod.exe"))
    {
        return why("is not named pod.exe");
    }
    match peek(candidate) {
        Ok(Some(f)) if f.is_link() => return why("is a link or other reparse point"),
        Ok(Some(f)) if f.is_dir() => return why("is a folder"),
        Ok(Some(_)) => {}
        Ok(None) => return why("does not exist"),
        Err(e) => return why(&format!("cannot be read ({e})")),
    }
    let dir = candidate.parent().unwrap_or(candidate);
    if !matches!(peek(dir), Ok(Some(f)) if f.is_dir() && !f.is_link()) {
        return why("is in a folder that is a link or other reparse point");
    }
    let pinned = crate::git::normalize(candidate);
    for p in [candidate, pinned.as_path()] {
        if crate::git::is_inside(p, worktree) {
            return why("is inside the worker's worktree, which the worker can change");
        }
    }
    if !matches!(peek(&dir.join("pod.com")), Ok(None)) {
        return why("has a pod.com beside it, which a bare `pod` would run first");
    }
    Ok(pinned)
}

/// Install (`on`) or remove Pod's rules file in `worktree`, pinned to `pod`.
/// Never touches a file at that path that Pod did not write, never writes
/// or removes through a link (symbolic link, junction, other reparse point
/// or hard link), and writes only by a new file in a held, checked folder
/// renamed into place. Applies nothing when `.codex` holds anything else.
pub fn apply(worktree: &Path, on: bool, pod: Result<PathBuf, String>) -> Result<Outcome, String> {
    let codex = worktree.join(".codex");
    let rules = codex.join("rules");
    let path = rules_path(worktree);
    for d in [&codex, &rules] {
        match peek(d) {
            Ok(None) => {}
            Ok(Some(f)) if f.is_link() => {
                return Ok(Outcome::Refused(format!(
                    "{} is a symbolic link, junction or other reparse point; Pod does not write through it",
                    d.display()
                )));
            }
            Ok(Some(f)) if f.is_dir() => {}
            Ok(Some(_)) => {
                return Ok(Outcome::Refused(format!(
                    "{} is a file, not a folder",
                    d.display()
                )));
            }
            Err(e) => return Err(format!("{}: {e}", d.display())),
        }
    }
    let current = match read_plain(&path) {
        Ok(c) => c,
        Err(why) => return Ok(Outcome::Refused(why)),
    };
    let ours = current.as_deref().map(is_ours);
    if ours == Some(false) {
        return Ok(Outcome::Foreign);
    }
    let off = |why: Option<String>| -> Result<Outcome, String> {
        if ours == Some(true) {
            remove_ours(worktree)?;
        }
        Ok(match (why, ours) {
            (Some(w), _) => Outcome::Refused(w),
            (None, Some(true)) => Outcome::Removed,
            (None, _) => Outcome::Absent,
        })
    };
    if !on {
        return off(None);
    }
    let pod = match pod {
        Ok(p) => p,
        Err(why) => return off(Some(why)),
    };
    if let Some(other) = other_codex_files(&codex, &rules)? {
        return off(Some(format!(
            "the worktree's .codex holds other files ({other}); Codex would load them with the trust Pod gives this worktree, so Pod adds no allowance"
        )));
    }
    if let Err(why) = exclude(worktree) {
        return off(Some(why));
    }
    let text = rules_text(&pod);
    if current.as_deref() != Some(text.as_str()) {
        write_rules(worktree, &text)?;
    }
    Ok(Outcome::Installed)
}

/// A file Pod wrote (any version of it).
fn is_ours(text: &str) -> bool {
    text.starts_with("# Written by Pod for a Pod-started Codex worker")
}

/// Temporary names Pod writes next to a file it replaces. Codex loads only
/// `*.rules`, git's exclude only `exclude`.
const TMP_MARK: &str = ".pod-tmp-";

fn tmp_name(dir: &Path, name: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        "{name}{TMP_MARK}{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Anything in `.codex` besides `rules/pod-worker.rules` (and Pod's
/// temporary files): its names, or `None`.
fn other_codex_files(codex: &Path, rules: &Path) -> Result<Option<String>, String> {
    let mut others = Vec::new();
    for (dir, mine) in [(codex, "rules"), (rules, "pod-worker.rules")] {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        for e in entries {
            let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name != mine && !name.contains(TMP_MARK) {
                others.push(e.path().strip_prefix(codex).map_or(name, |p| {
                    format!(".codex/{}", p.display().to_string().replace('\\', "/"))
                }));
            }
        }
    }
    others.sort();
    Ok((!others.is_empty()).then(|| others.join(", ")))
}

/// The text of `p`, read without following a link: `None` when missing,
/// `Err` when it is a link, a folder or a hard link.
fn read_plain(p: &Path) -> Result<Option<String>, String> {
    let mut f = match open_no_follow(p, false) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    check_plain(&f, p)?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn check_plain(f: &File, p: &Path) -> Result<(), String> {
    let x = facts(f).map_err(|e| format!("{}: {e}", p.display()))?;
    if x.is_link() {
        return Err(format!(
            "{} is a symbolic link or other reparse point; Pod does not write through it",
            p.display()
        ));
    }
    if x.is_dir() {
        return Err(format!("{} is a folder", p.display()));
    }
    if x.links != 1 {
        return Err(format!(
            "{} is a hard link (its content is another path's too); Pod does not write it",
            p.display()
        ));
    }
    Ok(())
}

/// `p`, a plain folder (created if missing), held open (it cannot be
/// renamed, deleted or swapped while held) and checked to be `parent\name`
/// itself when `parent` is given. Returns the handle and its path key.
fn hold_dir(p: &Path, parent: Option<&str>) -> Result<(File, String), String> {
    if parent.is_some() {
        match std::fs::create_dir(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    let f = open_no_follow(p, true).map_err(|e| format!("{}: {e}", p.display()))?;
    let x = facts(&f).map_err(|e| format!("{}: {e}", p.display()))?;
    if x.is_link() || !x.is_dir() {
        return Err(format!(
            "{} is not a plain folder (a link, junction or file); Pod does not write there",
            p.display()
        ));
    }
    let key = final_key(&f).map_err(|e| format!("{}: {e}", p.display()))?;
    if let Some(parent) = parent {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if key != format!("{parent}\\{name}") {
            return Err(format!(
                "{} is not the folder it names ({key}); Pod does not write there",
                p.display()
            ));
        }
    }
    Ok((f, key))
}

/// Put `text` at `dir\name` (missing, or a plain file Pod may replace)
/// through a new file in `dir`, held by the caller, renamed over it:
/// nothing is written through a link and no other file's content changes.
/// Checks the result by handle: a plain file, at `dir_key\name`, `text`.
fn replace_in(dir: &Path, dir_key: &str, name: &str, text: &str) -> Result<(), String> {
    let target = dir.join(name);
    let tmp = tmp_name(dir, name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.sync_all()))
        .and_then(|_| std::fs::rename(&tmp, &target));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", target.display()));
    }
    let mut f = open_no_follow(&target, false).map_err(|e| format!("{}: {e}", target.display()))?;
    check_plain(&f, &target)?;
    let key = final_key(&f).map_err(|e| format!("{}: {e}", target.display()))?;
    let mut back = Vec::new();
    f.read_to_end(&mut back)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    if key != format!("{dir_key}\\{}", name.to_lowercase()) || back != text.as_bytes() {
        return Err(format!(
            "{} is not what Pod wrote after writing it",
            target.display()
        ));
    }
    Ok(())
}

/// Write the rules: the worktree, `.codex` and `.codex\rules` held as
/// plain folders, the file replaced in place.
fn write_rules(worktree: &Path, text: &str) -> Result<(), String> {
    let (_root, root_key) = hold_dir(worktree, None)?;
    let codex = worktree.join(".codex");
    let (_codex, codex_key) = hold_dir(&codex, Some(&root_key))?;
    let rules = codex.join("rules");
    let (_rules, rules_key) = hold_dir(&rules, Some(&codex_key))?;
    // Checked again while held: missing, or Pod's own plain file.
    if read_plain(&rules.join("pod-worker.rules"))?.is_some_and(|t| !is_ours(&t)) {
        return Err(format!("{RULES_REL} changed while Pod was writing it"));
    }
    replace_in(&rules, &rules_key, "pod-worker.rules", text)
}

/// Remove Pod's own rules file (a plain file in plain folders) and the
/// folders when nothing else is in them.
fn remove_ours(worktree: &Path) -> Result<(), String> {
    let path = rules_path(worktree);
    {
        let (_root, root_key) = hold_dir(worktree, None)?;
        let codex = worktree.join(".codex");
        let (_codex, codex_key) = hold_dir(&codex, Some(&root_key))?;
        let (_rules, _) = hold_dir(&codex.join("rules"), Some(&codex_key))?;
        if !read_plain(&path)?.as_deref().is_some_and(is_ours) {
            return Ok(());
        }
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    // Drop the folders if Pod's file was all that was in them.
    if let Some(rules) = path.parent() {
        let _ = std::fs::remove_dir(rules);
        if let Some(codex) = rules.parent() {
            let _ = std::fs::remove_dir(codex);
        }
    }
    Ok(())
}

/// Add [`EXCLUDE_LINE`] to the repository's `info/exclude` once: the common
/// git folder and `info` held as plain folders, `exclude` a plain file
/// with one name (or missing), replaced in place.
fn exclude(worktree: &Path) -> Result<(), String> {
    let common = crate::git::run(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = crate::git::native_path(&common);
    let (_common, common_key) = hold_dir(&common, None)?;
    let info = common.join("info");
    let (_info, info_key) = hold_dir(&info, Some(&common_key))?;
    let file = info.join("exclude");
    let text = read_plain(&file)?.unwrap_or_default();
    if text.lines().any(|l| l.trim() == EXCLUDE_LINE) {
        return Ok(());
    }
    let sep = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    replace_in(
        &info,
        &info_key,
        "exclude",
        &format!("{text}{sep}# Pod: Codex worker rules (docs/19 §18.1)\n{EXCLUDE_LINE}\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository with one commit, a linked worktree `wt`, and a fixture
    /// `bin\pod.exe` outside both.
    fn fixture(name: &str) -> (Tmp, PathBuf, PathBuf, PathBuf) {
        let t =
            Tmp(std::env::temp_dir()
                .join(format!("podd-codex-allow-{name}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&t.0);
        let repo = t.0.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "init",
            ],
        );
        let wt = t.0.join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "pod/w1",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::create_dir_all(t.0.join("bin")).unwrap();
        std::fs::write(t.0.join("bin").join("pod.exe"), "MZ").unwrap();
        let pod = crate::git::normalize(&t.0.join("bin").join("pod.exe"));
        (t, repo, wt, pod)
    }

    fn junction(link: &Path, target: &Path) -> bool {
        crate::git::pin::junction(link, target)
    }

    /// TM-21: the exact commands the rules let through, nothing more.
    #[test]
    fn rules_cover_only_coordination_and_commits() {
        let allowed: Vec<&str> = RULES
            .lines()
            .filter(|l| l.starts_with("prefix_rule("))
            .collect();
        assert_eq!(allowed.len(), 6);
        assert!(allowed.iter().all(|l| l.contains(r#"decision="allow""#)));
        // Expand each pattern (a token, or a list of alternatives).
        let mut cmds: Vec<String> = Vec::new();
        for l in &allowed {
            let pat = l
                .strip_prefix("prefix_rule(pattern=[")
                .and_then(|r| r.split_once("], decision=").map(|(p, _)| p))
                .unwrap();
            let mut prefixes = vec![String::new()];
            let mut rest = pat;
            while !rest.is_empty() {
                rest = rest.trim_start_matches([',', ' ']);
                let (alts, tail): (Vec<&str>, &str) = if let Some(r) = rest.strip_prefix('[') {
                    let (inner, tail) = r.split_once(']').unwrap();
                    (
                        inner.split(", ").map(|s| s.trim_matches('"')).collect(),
                        tail,
                    )
                } else {
                    let end = rest.find(", ").unwrap_or(rest.len());
                    (vec![rest[..end].trim_matches('"')], &rest[end..])
                };
                prefixes = prefixes
                    .iter()
                    .flat_map(|p| {
                        alts.iter()
                            .map(move |a| format!("{p} {a}").trim().to_string())
                    })
                    .collect();
                rest = tail;
            }
            cmds.extend(prefixes);
        }
        assert_eq!(
            cmds,
            [
                "pod orch ask",
                "pod orch send",
                "pod orch check",
                "pod orch reply",
                "pod orch done",
                "pod orch wait",
                "pod orch commit",
                "pod orch workers",
                "pod orch queue",
                "pod orch run ls",
                "pod orch run show",
                "pod orch item ls",
                "pod orch gate request",
                "pod guide orchestration",
                "pod --version",
            ]
        );
        // Never a bare `pod`/`pod orch` prefix, never process control, no
        // git at all (an unsandboxed git runs hooks/fsmonitor from a repo
        // the agent can build; commits go through `pod orch commit`), no
        // reading other terminals (`pod read`/`ls`, `worker read`).
        for bad in [
            r#"["pod"]"#,
            r#"["pod", "orch"]"#,
            "\"git\"",
            "\"start\"",
            "\"stop\"",
            "\"limits\"",
            "\"decide\"",
            "\"worker\"",
            r#"["pod", ["ls""#,
            r#"["pod", "read""#,
            r#"["pod", "ls""#,
        ] {
            assert!(!RULES.contains(bad), "{bad}");
        }
    }

    /// TM-21: the file pins `pod` to one escaped absolute path.
    #[test]
    fn the_rules_pin_pods_executable() {
        let text = rules_text(Path::new(r"C:\Program Files\Pod\pod.exe"));
        assert!(text.starts_with(RULES));
        assert!(text.ends_with(
            "host_executable(name=\"pod\", paths=[\"C:\\\\Program Files\\\\Pod\\\\pod.exe\"])\n"
        ));
        assert_eq!(text.matches("host_executable(").count(), 1);
    }

    #[test]
    fn installs_hidden_from_git_and_removes_only_its_own_file() {
        let (_t, repo, wt, pod) = fixture("install");
        let on = |on| apply(&wt, on, Ok(pod.clone())).unwrap();
        assert_eq!(on(true), Outcome::Installed);
        assert_eq!(
            std::fs::read_to_string(wt.join(".codex/rules/pod-worker.rules")).unwrap(),
            rules_text(&pod)
        );
        assert_eq!(
            git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
            "",
            "hidden from git"
        );
        // Idempotent: one exclude line however often it runs.
        assert_eq!(on(true), Outcome::Installed);
        let ex = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches(EXCLUDE_LINE).count(), 1, "{ex}");
        // An older, unpinned file of Pod's is replaced by the pinned one.
        std::fs::write(wt.join(".codex/rules/pod-worker.rules"), RULES).unwrap();
        assert_eq!(on(true), Outcome::Installed);
        assert!(
            std::fs::read_to_string(wt.join(".codex/rules/pod-worker.rules"))
                .unwrap()
                .contains("host_executable")
        );
        // Off: Pod's file and the folders it made are gone.
        assert_eq!(on(false), Outcome::Removed);
        assert!(!wt.join(".codex").exists());
        assert_eq!(on(false), Outcome::Absent);
        // The repository's own file at that path is never touched.
        std::fs::create_dir_all(wt.join(".codex/rules")).unwrap();
        std::fs::write(wt.join(".codex/rules/pod-worker.rules"), "# mine\n").unwrap();
        assert_eq!(on(true), Outcome::Foreign);
        assert_eq!(on(false), Outcome::Foreign);
        assert_eq!(
            std::fs::read_to_string(wt.join(".codex/rules/pod-worker.rules")).unwrap(),
            "# mine\n"
        );
    }

    #[test]
    fn keeps_existing_exclude_lines() {
        let (_t, repo, wt, pod) = fixture("exclude");
        std::fs::write(repo.join(".git/info/exclude"), "*.log").unwrap();
        apply(&wt, true, Ok(pod)).unwrap();
        let ex = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(ex.starts_with("*.log\n# Pod"), "{ex}");
    }

    /// TM-21 "fake executable with the allowed name": only Pod's own plain
    /// `pod.exe` outside the worktree is pinned; without one, no allowance
    /// (and Pod's earlier, unpinned file goes).
    #[test]
    fn only_a_verified_pod_exe_is_pinned() {
        let (t, _repo, wt, pod) = fixture("pin");
        assert_eq!(verify_pod(&pod, &wt).unwrap(), pod);
        // The worker's own `pod.exe` (in its worktree): never the pin.
        std::fs::write(wt.join("pod.exe"), "MZ fake").unwrap();
        let e = verify_pod(&wt.join("pod.exe"), &wt).unwrap_err();
        assert!(e.contains("inside the worker's worktree"), "{e}");
        // Not named pod.exe, missing, relative, a folder.
        std::fs::write(t.0.join("bin").join("pod.cmd"), "@echo fake").unwrap();
        assert!(verify_pod(&t.0.join("bin").join("pod.cmd"), &wt).is_err());
        assert!(verify_pod(&t.0.join("nope").join("pod.exe"), &wt).is_err());
        assert!(verify_pod(Path::new(r"bin\pod.exe"), &wt).is_err());
        std::fs::create_dir_all(t.0.join("dir").join("pod.exe")).unwrap();
        assert!(verify_pod(&t.0.join("dir").join("pod.exe"), &wt).is_err());
        // A folder that is a junction to the real one.
        let jbin = t.0.join("jbin");
        assert!(junction(&jbin, &t.0.join("bin")));
        let e = verify_pod(&jbin.join("pod.exe"), &wt).unwrap_err();
        assert!(e.contains("link"), "{e}");
        // A file symbolic link (only where this machine lets us make one).
        let sl = t.0.join("sl");
        std::fs::create_dir_all(&sl).unwrap();
        if std::os::windows::fs::symlink_file(&pod, sl.join("pod.exe")).is_ok() {
            assert!(verify_pod(&sl.join("pod.exe"), &wt).is_err());
        }
        // A pod.com beside it would run first for a bare `pod`.
        std::fs::write(t.0.join("bin").join("pod.com"), "x").unwrap();
        assert!(verify_pod(&pod, &wt).unwrap_err().contains("pod.com"));
        std::fs::remove_file(t.0.join("bin").join("pod.com")).unwrap();
        // No pin: refused, and Pod's earlier (unpinned) rules are removed.
        std::fs::create_dir_all(wt.join(".codex/rules")).unwrap();
        std::fs::write(wt.join(".codex/rules/pod-worker.rules"), RULES).unwrap();
        let r = apply(&wt, true, verify_pod(&wt.join("pod.exe"), &wt)).unwrap();
        assert!(
            matches!(r, Outcome::Refused(ref w) if w.contains("worktree")),
            "{r:?}"
        );
        assert!(!wt.join(".codex").exists());
    }

    /// TM-21 "rules file link": Pod never writes through a junction, a
    /// symbolic link or a hard link, never overwrites or removes a file
    /// outside the worktree, and adds nothing next to other Codex config.
    #[test]
    fn links_and_other_codex_files_are_refused() {
        let (t, repo, wt, pod) = fixture("links");
        let outside = t.0.join("user-codex");
        std::fs::create_dir_all(outside.join("rules")).unwrap();
        std::fs::write(outside.join("config.toml"), "model = 'x'\n").unwrap();
        let refused = |r: Outcome, what: &str| {
            assert!(
                matches!(&r, Outcome::Refused(w) if w.contains(what)),
                "{what}: {r:?}"
            );
        };
        // `.codex` is a junction to a folder outside (say ~/.codex).
        assert!(junction(&wt.join(".codex"), &outside));
        refused(apply(&wt, true, Ok(pod.clone())).unwrap(), "reparse point");
        refused(apply(&wt, false, Ok(pod.clone())).unwrap(), "reparse point");
        assert!(!outside.join("rules").join("pod-worker.rules").exists());
        std::fs::remove_dir(wt.join(".codex")).unwrap();
        // `.codex\rules` is the junction.
        std::fs::create_dir_all(wt.join(".codex")).unwrap();
        assert!(junction(
            &wt.join(".codex").join("rules"),
            &outside.join("rules")
        ));
        refused(apply(&wt, true, Ok(pod.clone())).unwrap(), "reparse point");
        assert!(!outside.join("rules").join("pod-worker.rules").exists());
        std::fs::remove_dir(wt.join(".codex").join("rules")).unwrap();
        // The rules file is a hard link to a file outside that looks like
        // Pod's: not rewritten, not removed.
        std::fs::create_dir_all(wt.join(".codex/rules")).unwrap();
        let theirs = outside.join("rules").join("theirs.rules");
        std::fs::write(&theirs, RULES).unwrap();
        std::fs::hard_link(&theirs, wt.join(".codex/rules/pod-worker.rules")).unwrap();
        refused(apply(&wt, true, Ok(pod.clone())).unwrap(), "hard link");
        refused(apply(&wt, false, Ok(pod.clone())).unwrap(), "hard link");
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), RULES);
        std::fs::remove_file(wt.join(".codex/rules/pod-worker.rules")).unwrap();
        // Other Codex config in the worktree: no allowance, and Pod's own
        // earlier file is removed.
        apply(&wt, true, Ok(pod.clone())).unwrap();
        std::fs::write(
            wt.join(".codex").join("config.toml"),
            "sandbox_mode = 'x'\n",
        )
        .unwrap();
        refused(
            apply(&wt, true, Ok(pod.clone())).unwrap(),
            ".codex/config.toml",
        );
        assert!(!wt.join(RULES_REL).exists());
        std::fs::remove_file(wt.join(".codex").join("config.toml")).unwrap();
        std::fs::create_dir_all(wt.join(".codex/rules")).unwrap();
        std::fs::write(
            wt.join(".codex/rules/repo.rules"),
            "prefix_rule(pattern=[\"git\"], decision=\"allow\")\n",
        )
        .unwrap();
        refused(
            apply(&wt, true, Ok(pod.clone())).unwrap(),
            ".codex/rules/repo.rules",
        );
        std::fs::remove_dir_all(wt.join(".codex")).unwrap();
        // The repository's info/exclude is a hard link to a file outside.
        let ex_out = t.0.join("exclude-outside");
        std::fs::write(&ex_out, "# theirs\n").unwrap();
        let ex = repo.join(".git").join("info").join("exclude");
        let _ = std::fs::remove_file(&ex);
        std::fs::hard_link(&ex_out, &ex).unwrap();
        refused(apply(&wt, true, Ok(pod.clone())).unwrap(), "hard link");
        assert_eq!(std::fs::read_to_string(&ex_out).unwrap(), "# theirs\n");
        assert!(!wt.join(".codex").exists());
        // `info` itself a junction to a folder outside.
        std::fs::remove_file(&ex).unwrap();
        std::fs::remove_dir_all(repo.join(".git").join("info")).unwrap();
        let info_out = t.0.join("info-outside");
        std::fs::create_dir_all(&info_out).unwrap();
        assert!(junction(&repo.join(".git").join("info"), &info_out));
        refused(
            apply(&wt, true, Ok(pod.clone())).unwrap(),
            "not a plain folder",
        );
        assert!(!info_out.join("exclude").exists());
        // The files outside are as they were.
        assert_eq!(
            std::fs::read_to_string(outside.join("config.toml")).unwrap(),
            "model = 'x'\n"
        );
    }
}
