//! Thin wrapper over the `git` CLI for worktree-based tasks.
//!
//! Everything goes through the `git` executable (not libgit2) so behaviour
//! matches what the user and the agents see in their shells. Output is read
//! in porcelain / `-z` form only. Every invocation adds
//! `-c core.longpaths=true -c core.quotepath=false -c core.fsmonitor=false`,
//! clears inherited
//! `GIT_DIR`-style variables, never prompts and never opens a console window
//! (the daemon runs detached, so a console child would get a fresh window).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::protocol::{CommitInfo, DiffFile, TaskGit};

/// Environment override for the git executable.
pub const ENV_GIT: &str = "POD_GIT";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Untracked files larger than this are reported without line counts.
const MAX_COUNT_BYTES: u64 = 8 * 1024 * 1024;

/// Variables that would point git somewhere other than `-C <dir>`.
const GIT_ENV_OVERRIDES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_PREFIX",
];

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn command(dir: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let exe = std::env::var_os(ENV_GIT).unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(exe);
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "core.longpaths=true", "-c", "core.quotepath=false"])
        // fsmonitor is only a speed-up, and its program comes from config
        // and may be a script inside a worktree an agent writes (§18.5).
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for v in GIT_ENV_OVERRIDES {
        cmd.env_remove(v);
    }
    cmd
}

/// Run git in `dir`; never fails on a non-zero exit (see [`Output::ok`]).
pub fn output(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let out = command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

/// Run git in `dir`; a non-zero exit becomes `Err("git <args>: <stderr>")`.
pub fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = output(dir, args)?;
    if o.ok {
        Ok(o.stdout)
    } else {
        Err(format!("git {}: {}", args.join(" "), o.stderr))
    }
}

/// Like [`run`] for path arguments.
fn run_os(dir: &Path, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
        Err(format!(
            "git {}: {}",
            shown.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Whole-call limit for each git child on the worker commit path.
const GUARDED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Most stdout a git child on that path may print (listings of a large repo).
const GUARDED_STDOUT: usize = 64 * 1024 * 1024;

/// Config (command-line scope, so it beats every config file and include)
/// that keeps repository-configured programs from running in a tree a
/// worker controls (fsmonitor is off for every call, see [`command`]):
/// every configured filter driver emptied (git skips a driver whose command is empty) and not required.
/// With `hooks`: `core.hooksPath` is that empty folder and signing and
/// automatic maintenance are off (the commit path). Also returns the
/// driver names.
type Guard = (Vec<(String, String)>, Vec<String>);

fn guard(dir: &Path, hooks: Option<&Path>) -> Result<Guard, String> {
    let mut cfg = Vec::new();
    if let Some(h) = hooks {
        let h = h.display().to_string().replace('\\', "/");
        for (k, v) in [
            ("core.hooksPath", h.as_str()),
            ("commit.gpgSign", "false"),
            ("gc.auto", "0"),
            ("maintenance.auto", "false"),
        ] {
            cfg.push((k.into(), v.into()));
        }
    }
    let drivers = filter_drivers(dir)?;
    for d in &drivers {
        for (key, value) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            cfg.push((format!("filter.{d}.{key}"), value.into()));
        }
    }
    Ok((cfg, drivers))
}

/// Names of the filter drivers the effective config gives a program
/// (`filter.<name>.clean|smudge|process`). Reading config runs nothing.
fn filter_drivers(dir: &Path) -> Result<Vec<String>, String> {
    let o = output(
        dir,
        &[
            "config",
            "-z",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ],
    )?;
    if !o.ok && !o.stderr.is_empty() {
        return Err(format!("git config: {}", o.stderr));
    }
    Ok(parse_filter_drivers(&o.stdout))
}

/// `config -z --get-regexp` output (`key\nvalue\0`…) → driver names with a
/// non-empty program.
fn parse_filter_drivers(s: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for entry in s.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if value.trim().is_empty() {
            continue;
        }
        let Some((name, _)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        else {
            continue;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Pass `cfg` as command-line config through the environment (any key
/// spelling works, unlike `-c k=v`; child gits inherit it).
fn with_config(cmd: &mut Command, cfg: &[(String, String)]) {
    cmd.env("GIT_CONFIG_COUNT", cfg.len().to_string());
    for (i, (k, v)) in cfg.iter().enumerate() {
        cmd.env(format!("GIT_CONFIG_KEY_{i}"), k);
        cmd.env(format!("GIT_CONFIG_VALUE_{i}"), v);
    }
}

/// [`run`] with [`guard`] config.
fn run_guarded(dir: &Path, cfg: &[(String, String)], args: &[&str]) -> Result<String, String> {
    let mut cmd = command(dir);
    with_config(&mut cmd, cfg);
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// One bounded git child (pod_forge::bounded: process tree in a job, a
/// whole-call deadline, capped stdout) with `cfg` and optional stdin.
fn run_bounded(
    dir: &Path,
    cfg: &[(String, String)],
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<String, String> {
    use pod_forge::bounded::{self, End, Keep};
    let mut cmd = command(dir);
    with_config(&mut cmd, cfg);
    cmd.args(args);
    let mut opts = bounded::Options::new(GUARDED_TIMEOUT, Keep::Cap(GUARDED_STDOUT));
    opts.stderr = Keep::Tail(4096);
    opts.stdin = stdin;
    let o = bounded::run(&mut cmd, &opts, None).map_err(|e| format!("cannot run git: {e}"))?;
    let shown = args.iter().take(3).copied().collect::<Vec<_>>().join(" ");
    match o.end {
        End::Exited { success: true, .. } => Ok(o.stdout.text()),
        End::Exited { .. } => Err(format!("git {shown}: {}", o.stderr.text().trim())),
        End::TimedOut => Err(format!(
            "git {shown} took longer than {}s and was stopped",
            GUARDED_TIMEOUT.as_secs()
        )),
        End::TooLarge { .. } => Err(format!(
            "git {shown} printed more than {} MB and was stopped",
            GUARDED_STDOUT / (1024 * 1024)
        )),
        End::Cancelled => Err(format!("git {shown} was cancelled")),
    }
}

/// `path` for `hash-object --stdin-paths`, which C-unquotes a line that
/// starts with `"`: always quote so every name means itself.
fn c_quote(path: &str) -> String {
    let mut s = String::with_capacity(path.len() + 2);
    s.push('"');
    for c in path.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                s.push_str(&format!("\\{:03o}", c as u32))
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// `commit -m` cleanup ("whitespace"): trailing spaces off each line,
/// blank runs collapsed, no blank lines at either end, one final newline.
fn clean_message(m: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in m.lines().map(str::trim_end) {
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

/// `rel` (as git printed it: `/`-separated, relative to `top`) and every
/// folder above it are real entries, not links or junctions.
fn no_link_on_the_way(top: &Path, rel: &str) -> Result<(), String> {
    let mut p = top.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        if part == ".." {
            return Err(format!("{rel}: the path leaves the worktree"));
        }
        p.push(part);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{rel} is (or is under) a symbolic link or junction; podd does not commit links for workers. {ASK_HUMAN}"
            ));
        }
    }
    Ok(())
}

const ASK_HUMAN: &str = "Commit it yourself with git (with approval) or ask a human";

/// Stage and commit in a worktree on behalf of a sandboxed worker
/// (`orch.commit`, docs/19 §18.1, §18.5). git runs here, outside any
/// sandbox, in the worktree Pod recorded, so nothing the repository names
/// as a program may run: plumbing only (`ls-files`, `diff-files`,
/// `hash-object`, `update-index`, `write-tree`, `commit-tree`,
/// `update-ref`), hooks from `no_hooks` (an empty folder), fsmonitor,
/// signing and automatic maintenance off and every configured filter
/// driver emptied. A path whose attributes name a configured filter driver
/// (LFS, say) is refused: committing it unfiltered would be wrong, and
/// running the filter is what this guards against. Links, junctions and
/// nested repositories are refused too; submodule pointers are left alone.
/// Each git child is bounded (time, output). The worktree's top level must
/// be `worktree` itself. Returns (full hash, subject).
pub fn commit_for_worker(
    worktree: &Path,
    no_hooks: &Path,
    message: &str,
    all: bool,
    paths: &[String],
) -> Result<(String, String), String> {
    let top = run(worktree, &["rev-parse", "--show-toplevel"])?;
    if !same_path(&native_path(&top), worktree) {
        return Err(format!(
            "{} is not the top of its git worktree ({})",
            worktree.display(),
            top.trim()
        ));
    }
    let message = clean_message(message);
    if message.is_empty() {
        return Err("the commit message must not be empty".into());
    }
    if !all && paths.is_empty() {
        return Err("say what to commit: --all or paths".into());
    }
    let (cfg, drivers) = guard(worktree, Some(no_hooks))?;
    let git = |args: &[&str]| run_bounded(worktree, &cfg, args, None);
    let git_in = |args: &[&str], stdin: Vec<u8>| run_bounded(worktree, &cfg, args, Some(stdin));
    // `--all`: the whole tree; otherwise the paths (git pathspecs).
    let spec: Vec<&str> = if all {
        Vec::new()
    } else {
        paths.iter().map(String::as_str).collect()
    };
    let with_spec = |head: &[&'static str]| -> Vec<&str> {
        let mut a: Vec<&str> = head.to_vec();
        if !spec.is_empty() {
            a.push("--");
            a.extend(spec.iter().copied());
        }
        a
    };
    if !spec.is_empty() {
        // Every path names something tracked, or new and not ignored,
        // inside the worktree (like `git add`).
        git(&with_spec(&[
            "ls-files",
            "-z",
            "--error-unmatch",
            "--cached",
            "--others",
            "--exclude-standard",
        ]))?;
    }
    // Tracked paths whose file differs from the index (no filter program
    // can run while comparing), then new files.
    let mut changed: Vec<String> = Vec::new();
    for list in [
        git(&with_spec(&[
            "diff-files",
            "-z",
            "--name-only",
            "--ignore-submodules=all",
            "--no-ext-diff",
            "--no-textconv",
        ]))?,
        git(&with_spec(&[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
        ]))?,
    ] {
        for p in list.split('\0').filter(|p| !p.is_empty()) {
            if !changed.iter().any(|c| c == p) {
                changed.push(p.to_string());
            }
        }
    }
    // Index modes: kept for tracked files, 100644 for new ones.
    let mut modes = std::collections::HashMap::new();
    for e in git(&with_spec(&["ls-files", "-z", "--stage"]))?.split('\0') {
        if let Some((meta, path)) = e.split_once('\t') {
            let mode = meta.split(' ').next().unwrap_or_default();
            modes.insert(path.to_string(), mode.to_string());
        }
    }
    let (mut stage, mut remove) = (Vec::new(), Vec::new());
    for p in changed {
        if p.ends_with('/') {
            return Err(format!(
                "{p} is a nested git repository; podd does not commit repositories for workers. {ASK_HUMAN}"
            ));
        }
        if modes.get(&p).is_some_and(|m| m == "160000") {
            continue;
        }
        no_link_on_the_way(worktree, &p)?;
        let is_file = std::fs::symlink_metadata(worktree.join(&p)).is_ok_and(|m| m.is_file());
        if is_file {
            stage.push(p);
        } else if modes.contains_key(&p) {
            // Deleted, or a folder now (its files are listed as new).
            remove.push(p);
        }
    }
    if !stage.is_empty() && !drivers.is_empty() {
        let mut input = Vec::new();
        for p in &stage {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let attrs = git_in(&["check-attr", "-z", "--stdin", "filter"], input)?;
        let fields: Vec<&str> = attrs.split('\0').collect();
        let filtered: Vec<String> = fields
            .chunks(3)
            .filter_map(|t| match t {
                [path, _, value] if drivers.iter().any(|d| d == value) => {
                    Some(format!("{path} (filter={value})"))
                }
                _ => None,
            })
            .collect();
        if !filtered.is_empty() {
            return Err(format!(
                "these paths go through a git filter program the repository configures, and podd does not run repository programs for workers: {}. {ASK_HUMAN}",
                filtered.join(", ")
            ));
        }
    }
    let mut info = String::new();
    if !stage.is_empty() {
        let input: String = stage.iter().map(|p| format!("{}\n", c_quote(p))).collect();
        let hashes = git_in(&["hash-object", "-w", "--stdin-paths"], input.into_bytes())?;
        let hashes: Vec<&str> = hashes.lines().collect();
        if hashes.len() != stage.len() {
            return Err(format!(
                "git hash-object: {} hashes for {} files",
                hashes.len(),
                stage.len()
            ));
        }
        for (p, h) in stage.iter().zip(hashes) {
            let mode = modes.get(p).map(String::as_str).unwrap_or("100644");
            info.push_str(&format!("{mode} {h}\t{p}\0"));
        }
    }
    if !remove.is_empty() {
        let sha256 = git(&["rev-parse", "--show-object-format"])?.trim() == "sha256";
        let zero = "0".repeat(if sha256 { 64 } else { 40 });
        for p in &remove {
            info.push_str(&format!("0 {zero}\t{p}\0"));
        }
    }
    if !info.is_empty() {
        git_in(&["update-index", "-z", "--index-info"], info.into_bytes())?;
    }
    let tree = git(&["write-tree"])?.trim().to_string();
    let parent = output(worktree, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
    let parent = parent.ok.then(|| parent.stdout.trim().to_string());
    if let Some(p) = &parent
        && git(&["rev-parse", &format!("{p}^{{tree}}")])?.trim() == tree
    {
        return Err("nothing to commit".into());
    }
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(p) = &parent {
        args.extend(["-p", p.as_str()]);
    }
    args.extend(["-F", "-"]);
    let sha = git_in(&args, message.clone().into_bytes())?
        .trim()
        .to_string();
    let subject = message.lines().next().unwrap_or_default().to_string();
    let initial = if parent.is_none() { " (initial)" } else { "" };
    let reflog = format!("commit{initial}: {subject}");
    let old = parent.unwrap_or_else(|| "0".repeat(sha.len()));
    git(&["update-ref", "-m", &reflog, "HEAD", &sha, &old])?;
    Ok((sha, subject))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

// Pod's one path-identity rule. Windows spells one folder many ways: 8.3
// short names (`C:\Users\RUNNER~1`), any letter case, `/` or `\`, the `\\?\`
// prefix. git prints the long, on-disk spelling. So:
// - paths Pod registers or creates (worktrees roots, repository roots, task
//   worktrees) are stored [`normalize`]d;
// - paths from outside (git output, stored state written by older versions,
//   client arguments, session cwds) are compared with [`same_path`] /
//   [`is_inside`], which fall back to [`normalize`] on both sides when the
//   spellings differ.

/// Absolute path without the `\\?\` prefix that `canonicalize` adds (git and
/// most tools do not accept it): the deepest existing ancestor is resolved
/// with `canonicalize` (long names, on-disk letter case, links), the rest
/// is appended as given. A path with no existing ancestor stays lexical.
pub fn normalize(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let mut out = strip_verbatim(&c);
            out.extend(tail.iter().rev());
            return out;
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                base = parent;
            }
            _ => return abs,
        }
    }
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Lexical comparison key (Windows semantics): no `\\?\`, `\` separators,
/// no trailing separator, lowercase.
pub fn path_key(p: &Path) -> String {
    let s = strip_verbatim(p).to_string_lossy().replace('/', "\\");
    s.trim_end_matches('\\').to_lowercase()
}

/// The same folder or file however it is spelled: equal [`path_key`]s, else
/// equal keys after [`normalize`] (8.3 short names, links).
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b) || path_key(&normalize(a)) == path_key(&normalize(b))
}

/// `child` is strictly inside `parent` (on a component boundary), however
/// either is spelled.
pub fn is_inside(child: &Path, parent: &Path) -> bool {
    fn lexical(c: &str, p: &str) -> bool {
        c.len() > p.len() && c.starts_with(p) && c.as_bytes()[p.len()] == b'\\'
    }
    lexical(&path_key(child), &path_key(parent))
        || lexical(&path_key(&normalize(child)), &path_key(&normalize(parent)))
}

/// git prints `C:/x/y`; make it `C:\x\y`.
pub fn native_path(git_path: &str) -> PathBuf {
    PathBuf::from(git_path.trim().replace('/', "\\"))
}

// ---------------------------------------------------------------------------
// Repositories, refs, worktrees
// ---------------------------------------------------------------------------

/// The repository whose root is `dir`. Errors when `dir` is not inside a git
/// work tree, is bare, is a linked worktree, or is a subdirectory.
pub fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let o = output(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--is-bare-repository",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ],
    )?;
    if !o.ok {
        return Err(format!("{} is not a git repository", dir.display()));
    }
    let lines: Vec<&str> = o.stdout.lines().collect();
    if lines.first() == Some(&"true") {
        return Err(format!(
            "{} is a bare repository (a work tree is needed)",
            dir.display()
        ));
    }
    let [_, git_dir, common_dir, top] = lines[..] else {
        return Err(format!("{} is not inside a work tree", dir.display()));
    };
    if !same_path(&native_path(git_dir), &native_path(common_dir)) {
        return Err(format!(
            "{} is a linked worktree; add the main repository instead",
            dir.display()
        ));
    }
    let top = native_path(top);
    if !same_path(&normalize(&top), &normalize(dir)) {
        return Err(format!(
            "{} is inside the repository {}; add its root",
            dir.display(),
            top.display()
        ));
    }
    Ok(top)
}

/// `origin/HEAD`'s branch when it exists locally (else the remote ref), else
/// the checked-out branch when it has a commit, else `HEAD`.
pub fn default_base(repo: &Path) -> String {
    if let Ok(o) = output(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && o.ok
    {
        let remote = o.stdout.trim().to_string();
        if let Some(local) = remote.strip_prefix("origin/")
            && branch_exists(repo, local)
        {
            return local.to_string();
        }
        if !remote.is_empty() {
            return remote;
        }
    }
    if let Ok(o) = output(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        && o.ok
    {
        let b = o.stdout.trim();
        if !b.is_empty() && resolve_commit(repo, b).is_some() {
            return b.to_string();
        }
    }
    "HEAD".into()
}

/// Full commit id of `rev`, if it names a commit.
pub fn resolve_commit(dir: &Path, rev: &str) -> Option<String> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let spec = format!("{rev}^{{commit}}");
    let o = output(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .ok()?;
    let s = o.stdout.trim();
    (o.ok && !s.is_empty()).then(|| s.to_string())
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    output(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok_and(|o| o.ok)
}

/// `git check-ref-format --branch`.
pub fn valid_branch_name(repo: &Path, branch: &str) -> bool {
    output(repo, &["check-ref-format", "--branch", branch]).is_ok_and(|o| o.ok)
}

/// `git worktree add -b <branch> <path> <commit>`.
pub fn worktree_add(repo: &Path, path: &Path, branch: &str, commit: &str) -> Result<(), String> {
    run_os(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path.as_os_str(),
            commit.as_ref(),
        ],
    )
    .map(|_| ())
}

/// Cone-mode sparse checkout of a worktree added with `--no-checkout`:
/// `git sparse-checkout set --cone <dirs>` then `git read-tree -mu HEAD`
/// (fills only the cone). Git keeps the setting per worktree
/// (`extensions.worktreeConfig`), so the main checkout is untouched.
pub fn sparse_checkout(worktree: &Path, dirs: &[String]) -> Result<(), String> {
    let mut args = vec!["sparse-checkout", "set", "--cone"];
    args.extend(dirs.iter().map(String::as_str));
    run(worktree, &args)?;
    run(worktree, &["read-tree", "-mu", "HEAD"]).map(|_| ())
}

/// `git worktree add --no-checkout` (new branch at `commit`, or an existing
/// branch when `commit` is `None`), for [`sparse_checkout`].
pub fn worktree_add_no_checkout(
    repo: &Path,
    path: &Path,
    branch: &str,
    commit: Option<&str>,
) -> Result<(), String> {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "worktree".as_ref(),
        "add".as_ref(),
        "--no-checkout".as_ref(),
    ];
    match commit {
        Some(c) => args.extend(["-b".as_ref(), branch.as_ref(), path.as_os_str(), c.as_ref()]),
        None => args.extend([path.as_os_str(), branch.as_ref()]),
    }
    run_os(repo, &args).map(|_| ())
}

/// `git worktree remove [--force] <path>`.
pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<(), String> {
    let mut args: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
    if force {
        args.push("--force".as_ref());
    }
    args.push(path.as_os_str());
    // Without --force git first runs `status` in the worktree (an agent's
    // tree): no filter program there (§18.5).
    let (cfg, _) = guard(repo, None)?;
    let out = {
        let mut cmd = command(repo);
        with_config(&mut cmd, &cfg);
        cmd.args(&args)
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?
    };
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git worktree remove {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `git worktree add <path> <branch>` for an existing branch.
pub fn worktree_add_existing(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    run_os(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            path.as_os_str(),
            branch.as_ref(),
        ],
    )
    .map(|_| ())
}

/// A full ref (`refs/heads/x`, `refs/tags/x`...) exists.
pub fn ref_exists(repo: &Path, full_ref: &str) -> bool {
    !full_ref.starts_with('-')
        && output(repo, &["show-ref", "--verify", "--quiet", full_ref]).is_ok_and(|o| o.ok)
}

/// `a` is an ancestor of (or equal to) `b`.
pub fn is_ancestor(repo: &Path, a: &str, b: &str) -> bool {
    !a.starts_with('-')
        && !b.starts_with('-')
        && output(repo, &["merge-base", "--is-ancestor", a, b]).is_ok_and(|o| o.ok)
}

pub fn worktree_prune(repo: &Path) -> Result<(), String> {
    run(repo, &["worktree", "prune"]).map(|_| ())
}

pub fn branch_delete(repo: &Path, branch: &str) -> Result<(), String> {
    run(repo, &["branch", "-D", "--end-of-options", branch]).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    /// `refs/heads/...` stripped to the short name.
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub prunable: bool,
    /// `git worktree lock`ed.
    pub locked: bool,
}

pub fn worktree_list(repo: &Path) -> Result<Vec<WorktreeEntry>, String> {
    run(repo, &["worktree", "list", "--porcelain", "-z"]).map(|s| parse_worktree_list(&s))
}

/// Parse `git worktree list --porcelain -z`: attribute lines separated by
/// NUL, records separated by an empty attribute.
pub fn parse_worktree_list(s: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    for field in s.split('\0') {
        if field.is_empty() {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        match key {
            "worktree" => {
                if let Some(w) = cur.take() {
                    out.push(w);
                }
                cur = Some(WorktreeEntry {
                    path: native_path(value),
                    ..Default::default()
                });
            }
            _ => {
                let Some(w) = cur.as_mut() else { continue };
                match key {
                    "HEAD" => w.head = Some(value.to_string()),
                    "branch" => {
                        w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into())
                    }
                    "bare" => w.bare = true,
                    "detached" => w.detached = true,
                    "prunable" => w.prunable = true,
                    "locked" => w.locked = true,
                    _ => {}
                }
            }
        }
    }
    if let Some(w) = cur {
        out.push(w);
    }
    out
}

// ---------------------------------------------------------------------------
// Task facts and diff
// ---------------------------------------------------------------------------

/// The base to compare against: `base` if it still resolves, else the commit
/// recorded at creation.
pub fn base_rev(dir: &Path, base: &str, base_commit: &str) -> String {
    resolve_commit(dir, base).unwrap_or_else(|| base_commit.to_string())
}

/// `merge-base(base, HEAD)` of a worktree (the base itself when there is none).
pub fn merge_base(worktree: &Path, base: &str, base_commit: &str) -> String {
    let base = base_rev(worktree, base, base_commit);
    match output(worktree, &["merge-base", &base, "HEAD"]) {
        Ok(o) if o.ok && !o.stdout.trim().is_empty() => o.stdout.trim().to_string(),
        _ => base,
    }
}

/// Contents of `path` (`/`-separated, relative to the repository root) at `rev`.
pub fn show_file(dir: &Path, rev: &str, path: &str) -> Result<String, String> {
    if rev.starts_with('-') {
        return Err(format!("invalid revision {rev:?}"));
    }
    run(dir, &["show", &format!("{rev}:{path}")])
}

/// Branch, ahead/behind, dirty count, unpushed count and last commit of a worktree.
pub fn facts(worktree: &Path, base: &str, base_commit: &str) -> Result<TaskGit, String> {
    let base = base_rev(worktree, base, base_commit);
    let mut g = TaskGit {
        head: resolve_commit(worktree, "HEAD"),
        ..Default::default()
    };
    let b = output(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if b.ok {
        g.branch = Some(b.stdout.trim().to_string());
    }
    let counts = run(
        worktree,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base}...HEAD"),
        ],
    )?;
    (g.behind, g.ahead) = parse_left_right(&counts)
        .ok_or_else(|| format!("unexpected rev-list output {counts:?}"))?;
    let unpushed = run(
        worktree,
        &["rev-list", "--count", "HEAD", "--not", &base, "--remotes"],
    )?;
    g.unpushed = unpushed.trim().parse().unwrap_or(0);
    // A worker's tree: no filter program, no status run in a nested repo.
    let (cfg, _) = guard(worktree, None)?;
    let status = run_guarded(
        worktree,
        &cfg,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
        ],
    )?;
    g.dirty = count_status_entries(&status);
    let log = output(
        worktree,
        &["log", "-1", "--format=%H%x00%s%x00%an%x00%ct", "HEAD"],
    )?;
    if log.ok {
        g.last_commit = parse_commit(&log.stdout);
    }
    Ok(g)
}

fn parse_left_right(s: &str) -> Option<(u32, u32)> {
    let mut it = s.split_whitespace();
    let left = it.next()?.parse().ok()?;
    let right = it.next()?.parse().ok()?;
    Some((left, right))
}

/// Entries in `git status --porcelain=v1 -z` (a rename/copy carries an extra
/// NUL-terminated source path that is not an entry of its own).
pub fn count_status_entries(s: &str) -> u32 {
    let mut n = 0;
    let mut fields = s.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        n += 1;
        let xy = f.as_bytes();
        if xy.len() >= 2 && (matches!(xy[0], b'R' | b'C') || matches!(xy[1], b'R' | b'C')) {
            fields.next();
        }
    }
    n
}

fn parse_commit(s: &str) -> Option<CommitInfo> {
    let mut it = s.trim_end_matches(['\r', '\n']).split('\0');
    let sha = it.next()?.to_string();
    let summary = it.next()?.to_string();
    let author = it.next()?.to_string();
    let secs: u64 = it.next()?.trim().parse().ok()?;
    (!sha.is_empty()).then_some(CommitInfo {
        sha,
        summary,
        author,
        time_ms: secs * 1000,
    })
}

/// Worktree against the merge base of the base and HEAD: tracked changes
/// (committed, staged or not) from `git diff --numstat` + `--name-status`,
/// plus untracked files counted as fully added.
pub fn diff(
    worktree: &Path,
    base: &str,
    base_commit: &str,
) -> Result<(String, Vec<DiffFile>), String> {
    let base = base_rev(worktree, base, base_commit);
    let mb = output(worktree, &["merge-base", &base, "HEAD"])?;
    let merge_base = if mb.ok && !mb.stdout.trim().is_empty() {
        mb.stdout.trim().to_string()
    } else {
        base.clone()
    };
    // A worker's tree: no filter program, no status run in a nested repo.
    let (cfg, _) = guard(worktree, None)?;
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=dirty",
        "-M",
        "-z",
    ];
    let numstat = run_guarded(
        worktree,
        &cfg,
        &[&common[..], &["--numstat", &merge_base, "--"]].concat(),
    )?;
    let names = run_guarded(
        worktree,
        &cfg,
        &[&common[..], &["--name-status", &merge_base, "--"]].concat(),
    )?;
    let mut files = merge_diff(&parse_name_status(&names), &parse_numstat(&numstat));
    let untracked = run(
        worktree,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        let (added, binary) = count_lines(&worktree.join(path));
        files.push(DiffFile {
            path: path.to_string(),
            old_path: None,
            status: "?".into(),
            added,
            deleted: (!binary).then_some(0),
            binary,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((merge_base, files))
}

/// `(status, old_path, path)` from `--name-status -z`.
pub fn parse_name_status(s: &str) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::new();
    let mut it = s.split('\0').filter(|f| !f.is_empty());
    while let Some(code) = it.next() {
        let letter = code.chars().next().unwrap_or('M');
        if matches!(letter, 'R' | 'C') {
            let (Some(old), Some(new)) = (it.next(), it.next()) else {
                break;
            };
            out.push((letter.to_string(), Some(old.to_string()), new.to_string()));
        } else {
            let Some(path) = it.next() else { break };
            out.push((letter.to_string(), None, path.to_string()));
        }
    }
    out
}

/// `(added, deleted, path)` from `--numstat -z`; counts are `None` for binary
/// files. A rename is `a\td\t\0old\0new\0` and keyed by the new path.
pub fn parse_numstat(s: &str) -> Vec<(Option<u64>, Option<u64>, String)> {
    let mut out = Vec::new();
    let mut it = s.split('\0');
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        let mut cols = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = it.next();
            match it.next() {
                Some(new) => new.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        out.push((a.parse().ok(), d.parse().ok(), path));
    }
    out
}

fn merge_diff(
    names: &[(String, Option<String>, String)],
    stats: &[(Option<u64>, Option<u64>, String)],
) -> Vec<DiffFile> {
    names
        .iter()
        .map(|(status, old, path)| {
            let st = stats.iter().find(|(_, _, p)| p == path);
            let (added, deleted) = st.map(|(a, d, _)| (*a, *d)).unwrap_or((Some(0), Some(0)));
            DiffFile {
                path: path.clone(),
                old_path: old.clone(),
                status: status.clone(),
                binary: added.is_none() && deleted.is_none(),
                added,
                deleted,
            }
        })
        .collect()
}

/// Lines in a new file (`None` + binary for files with NUL bytes, or too big to read).
fn count_lines(path: &Path) -> (Option<u64>, bool) {
    let too_big = std::fs::metadata(path).map_or(true, |m| m.len() > MAX_COUNT_BYTES);
    if too_big {
        return (None, false);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (None, false);
    };
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return (None, true);
    }
    let mut n = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes.last().is_some_and(|&b| b != b'\n') {
        n += 1;
    }
    (Some(n), false)
}

/// The 8.3 short spelling of an existing path (`C:\Users\RUNNER~1`),
/// `None` when the volume has no short names. Tests only.
#[cfg(test)]
pub(crate) fn short_spelling(p: &Path) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pod's one path-identity rule: a folder named through its 8.3 short
    /// spelling (GitHub runners: `C:\Users\RUNNER~1`) is the folder git
    /// prints in the long one, for comparisons and for the worker commit's
    /// top-level check.
    #[test]
    fn short_names_are_the_same_path() {
        let t = std::env::temp_dir().join(format!("podd-git-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let dir = t.join("LongFolderName");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let long = normalize(&dir);
        let Some(short) = short_spelling(&long) else {
            eprintln!("skipped: no 8.3 short names on this volume");
            let _ = std::fs::remove_dir_all(&t);
            return;
        };
        assert_ne!(path_key(&short), path_key(&long));
        assert_eq!(normalize(&short), long);
        // Missing tail: the deepest existing ancestor is resolved.
        assert_eq!(
            normalize(&short.join("gone").join("x")),
            long.join("gone").join("x")
        );
        assert!(same_path(&short, &long));
        assert!(same_path(&short.join("gone"), &long.join("GONE")));
        assert!(!same_path(&short, &long.join("inner")));
        assert!(is_inside(&short.join("inner"), &long));
        assert!(is_inside(&long.join("inner"), &short));
        assert!(!is_inside(&long, &short));

        let (repo, no_hooks) = (long.join("repo"), long.join("no-hooks"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&no_hooks).unwrap();
        let g = |args: &[&str]| run(&repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            g(&["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        g(&["add", "a.txt"]);
        g(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("b.txt"), "b").unwrap();
        let short_repo = short.join("repo");
        let (sha, _) = commit_for_worker(&short_repo, &no_hooks, "short", true, &[]).unwrap();
        assert_eq!(sha.len(), 40);
        assert_eq!(path_key(&repo_root(&short_repo).unwrap()), path_key(&repo));
        let _ = std::fs::remove_dir_all(&t);
    }

    /// docs/19 §18.1: the repository's own config names a pre-commit hook
    /// and an fsmonitor that each write a marker; a worker's commit through
    /// podd runs neither, in a linked worktree too.
    #[test]
    fn commit_for_worker_runs_no_hooks_or_fsmonitor() {
        let t = std::env::temp_dir().join(format!("podd-git-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (repo, wt, no_hooks) = (t.join("repo"), t.join("wt"), t.join("no-hooks"));
        std::fs::create_dir_all(repo.join("h")).unwrap();
        std::fs::create_dir_all(&no_hooks).unwrap();
        let g = |dir: &Path, args: &[&str]| run(dir, args).unwrap();
        g(&repo, &["init", "-q", "-b", "main"]);
        let u = |p: &Path| p.display().to_string().replace('\\', "/");
        let marker = t.join("marker");
        std::fs::write(
            repo.join("h").join("pre-commit"),
            format!("#!/bin/sh\necho hook >> '{}'\n", u(&marker)),
        )
        .unwrap();
        std::fs::write(
            repo.join("h").join("post-commit"),
            format!("#!/bin/sh\necho post >> '{}'\n", u(&marker)),
        )
        .unwrap();
        std::fs::write(
            repo.join("fsm"),
            format!("#!/bin/sh\necho fsm >> '{}'\nexit 1\n", u(&marker)),
        )
        .unwrap();
        for (k, v) in [
            ("user.name", "t".to_string()),
            ("user.email", "t@example.invalid".into()),
            ("commit.gpgsign", "false".into()),
            ("core.hooksPath", u(&repo.join("h"))),
            ("core.fsmonitor", u(&repo.join("fsm"))),
        ] {
            g(&repo, &["config", k, &v]);
        }
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        // The payloads are live: a plain commit fires them.
        g(&repo, &["add", "a.txt"]);
        g(&repo, &["commit", "-q", "-m", "init"]);
        assert!(
            marker.exists(),
            "control: the repository's hooks run on a plain commit"
        );
        std::fs::remove_file(&marker).unwrap();
        g(
            &repo,
            &["worktree", "add", "-q", "-b", "pod/w", wt.to_str().unwrap()],
        );
        std::fs::remove_file(&marker).ok();
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        std::fs::write(wt.join("c.txt"), "c").unwrap();
        let (sha, subject) =
            commit_for_worker(&wt, &no_hooks, "worker change", false, &["b.txt".into()]).unwrap();
        assert_eq!((sha.len(), subject.as_str()), (40, "worker change"));
        assert!(!marker.exists(), "no hook or fsmonitor ran");
        assert_eq!(
            g(&wt, &["show", "--name-only", "--format=", "HEAD"]).trim(),
            "b.txt",
            "only the named path"
        );
        commit_for_worker(&wt, &no_hooks, "rest", true, &[]).unwrap();
        assert!(!marker.exists());
        assert_eq!(g(&wt, &["status", "--porcelain"]).trim(), "");
        // Nothing to commit, a path outside, or a folder that is not the
        // worktree's top level: refused.
        assert!(commit_for_worker(&wt, &no_hooks, "empty", true, &[]).is_err());
        assert!(
            commit_for_worker(
                &wt,
                &no_hooks,
                "x",
                false,
                &[t.join("marker2").display().to_string()]
            )
            .is_err()
        );
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        assert!(
            commit_for_worker(&wt.join("sub"), &no_hooks, "x", true, &[])
                .unwrap_err()
                .contains("not the top")
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn guard_parsers() {
        let cfg = "filter.lfs.clean\ngit-lfs clean -- %f\0filter.lfs.process\ngit-lfs filter-process\0\
                   filter.a.b.smudge\nsh x.sh\0filter.off.clean\n\0";
        assert_eq!(parse_filter_drivers(cfg), ["lfs", "a.b"]);
        assert_eq!(c_quote("a\"b\\c\nd\u{1}é"), "\"a\\\"b\\\\c\\nd\\001é\"");
        assert_eq!(clean_message("  \n a  \n\n\n b\t\n\n"), " a\n\n b\n");
        assert_eq!(clean_message(" \n\t\n"), "");
    }

    #[test]
    fn worktree_list_porcelain() {
        let s = "worktree C:/src/app\0HEAD 1111\0branch refs/heads/main\0\0\
                 worktree C:/Users/me/pod/worktrees/app/feat x\0HEAD 2222\0branch refs/heads/pod/feat-x\0\0\
                 worktree C:/tmp/gone\0HEAD 3333\0detached\0prunable gitdir file points to non-existent location\0\0";
        let w = parse_worktree_list(s);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].path, PathBuf::from(r"C:\src\app"));
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert_eq!(
            w[1].path,
            PathBuf::from(r"C:\Users\me\pod\worktrees\app\feat x")
        );
        assert_eq!(w[1].branch.as_deref(), Some("pod/feat-x"));
        assert!(w[2].detached && w[2].prunable && w[2].branch.is_none());
    }

    #[test]
    fn numstat_and_name_status() {
        let numstat =
            "3\t0\tnew.txt\x001\t2\tREADME.md\0-\t-\timg.png\x000\t0\t\0old name.rs\0new name.rs\0";
        let n = parse_numstat(numstat);
        assert_eq!(
            n,
            vec![
                (Some(3), Some(0), "new.txt".into()),
                (Some(1), Some(2), "README.md".into()),
                (None, None, "img.png".into()),
                (Some(0), Some(0), "new name.rs".into()),
            ]
        );
        let names = "A\0new.txt\0M\0README.md\0M\0img.png\0R100\0old name.rs\0new name.rs\0";
        let s = parse_name_status(names);
        assert_eq!(
            s[3],
            ("R".into(), Some("old name.rs".into()), "new name.rs".into())
        );
        let files = merge_diff(&s, &n);
        assert_eq!(files[0].added, Some(3));
        assert_eq!(files[1].deleted, Some(2));
        assert!(files[2].binary && files[2].added.is_none());
        assert_eq!(files[3].old_path.as_deref(), Some("old name.rs"));
    }

    #[test]
    fn status_entries_skip_rename_sources() {
        assert_eq!(count_status_entries(""), 0);
        assert_eq!(
            count_status_entries(" M a.txt\0R  new.txt\0old.txt\0?? b c.txt\0"),
            3
        );
    }

    #[test]
    fn misc_parsers() {
        assert_eq!(parse_left_right("2\t5\n"), Some((2, 5)));
        let c = parse_commit("abc\0fix: thing\0Pod Test\x001700000000\n").unwrap();
        assert_eq!(c.summary, "fix: thing");
        assert_eq!(c.time_ms, 1_700_000_000_000);
    }

    #[test]
    fn path_helpers() {
        assert!(same_path(Path::new(r"C:\A\b\"), Path::new("c:/a/B")));
        assert!(!same_path(Path::new(r"C:\a\b"), Path::new(r"C:\a\bc")));
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\x y\z")),
            PathBuf::from(r"C:\x y\z")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share\x")),
            PathBuf::from(r"\\srv\share\x")
        );
        assert_eq!(native_path("C:/a b/c\n"), PathBuf::from(r"C:\a b\c"));
    }
}
