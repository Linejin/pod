//! Language detection and the language-server registry.
//!
//! Built-in entries cover rust-analyzer, pyright and
//! typescript-language-server. A JSON file (see [`ServerRegistry::merge_json`])
//! can override or add servers: `%APPDATA%\Pod\lsp.json` for the user and
//! `<worktree>\.pod\lsp.json` per repository.
//!
//! The repository file is code someone else wrote (an agent or a cloned
//! repository can write it), and its `command`/`args`/`probe_args` and
//! `initialization_options` (e.g. rust-analyzer's `overrideCommand`) run
//! programs. Like `.pod/config.toml` (docs/19 §3.2) it is only used after the
//! user approved its exact content: approvals are `(repository, SHA-256 of
//! the file)` in `<user config>\lsp-trust.json`. Until then only its
//! `"enabled": false` entries apply (turning a server off runs nothing).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A language the editor knows: gpui-component highlighter name + LSP id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Language {
    /// Name passed to `EditorState::language` (Tree-sitter highlighter).
    pub highlight: &'static str,
    /// `TextDocumentItem.languageId`.
    pub lsp_id: &'static str,
    /// Short label for the status bar.
    pub label: &'static str,
}

const fn lang(highlight: &'static str, lsp_id: &'static str, label: &'static str) -> Language {
    Language {
        highlight,
        lsp_id,
        label,
    }
}

pub fn language_for_path(path: &Path) -> Option<Language> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name == "cargo.lock" {
        return Some(lang("toml", "toml", "TOML"));
    }
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "rs" => lang("rust", "rust", "Rust"),
        "py" | "pyi" => lang("python", "python", "Python"),
        "ts" | "mts" | "cts" => lang("typescript", "typescript", "TypeScript"),
        "tsx" => lang("tsx", "typescriptreact", "TSX"),
        "js" | "mjs" | "cjs" => lang("javascript", "javascript", "JavaScript"),
        "jsx" => lang("javascript", "javascriptreact", "JSX"),
        "json" | "jsonc" => lang("json", "json", "JSON"),
        "toml" => lang("toml", "toml", "TOML"),
        "md" | "markdown" => lang("markdown", "markdown", "Markdown"),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RootStrategy {
    /// Topmost directory (inside the worktree) containing a marker: Cargo
    /// workspaces, where rust-analyzer wants the workspace root.
    #[default]
    Outermost,
    /// Closest directory containing a marker: JS/Python monorepos.
    Nearest,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ServerConfig {
    pub id: String,
    /// LSP language ids this server handles.
    pub languages: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub root_markers: Vec<String>,
    #[serde(default)]
    pub root_strategy: RootStrategy,
    #[serde(default)]
    pub initialization_options: Option<Value>,
    /// Args for an "is it really installed" probe (rustup proxies exist even
    /// when the component is missing). Empty: PATH presence is enough.
    #[serde(default)]
    pub probe_args: Vec<String>,
    /// Shown in the "server not installed" state.
    #[serde(default)]
    pub install_hint: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl ServerConfig {
    pub fn handles(&self, lsp_id: &str) -> bool {
        self.enabled && self.languages.iter().any(|l| l == lsp_id)
    }
}

#[derive(Clone, Debug)]
pub struct ServerRegistry {
    pub servers: Vec<ServerConfig>,
}

impl Default for ServerRegistry {
    fn default() -> Self {
        let s = |id: &str,
                 langs: &[&str],
                 cmd: &str,
                 args: &[&str],
                 markers: &[&str],
                 strategy,
                 probe: &[&str],
                 hint: &str| ServerConfig {
            id: id.into(),
            languages: langs.iter().map(|x| x.to_string()).collect(),
            command: cmd.into(),
            args: args.iter().map(|x| x.to_string()).collect(),
            root_markers: markers.iter().map(|x| x.to_string()).collect(),
            root_strategy: strategy,
            initialization_options: None,
            probe_args: probe.iter().map(|x| x.to_string()).collect(),
            install_hint: hint.into(),
            enabled: true,
        };
        ServerRegistry {
            servers: vec![
                s(
                    "rust-analyzer",
                    &["rust"],
                    "rust-analyzer",
                    &[],
                    &["Cargo.toml"],
                    RootStrategy::Outermost,
                    &["--version"],
                    "rustup component add rust-analyzer",
                ),
                s(
                    "pyright",
                    &["python"],
                    "pyright-langserver",
                    &["--stdio"],
                    &[
                        "pyproject.toml",
                        "setup.py",
                        "setup.cfg",
                        "pyrightconfig.json",
                        "requirements.txt",
                    ],
                    RootStrategy::Nearest,
                    &[],
                    "npm install -g pyright",
                ),
                s(
                    "typescript-language-server",
                    &[
                        "typescript",
                        "typescriptreact",
                        "javascript",
                        "javascriptreact",
                    ],
                    "typescript-language-server",
                    &["--stdio"],
                    &["tsconfig.json", "jsconfig.json", "package.json"],
                    RootStrategy::Nearest,
                    &["--version"],
                    "npm install -g typescript typescript-language-server",
                ),
            ],
        }
    }
}

#[derive(Deserialize)]
struct FileFormat {
    #[serde(default)]
    servers: Vec<ServerConfig>,
}

impl ServerRegistry {
    /// Adds or replaces (by `id`) servers from a JSON document:
    /// `{"servers":[{"id":"rust-analyzer","languages":["rust"],"command":"C:/x/ra.exe"}]}`
    pub fn merge_json(&mut self, json: &str) -> Result<(), String> {
        let f: FileFormat = serde_json::from_str(json).map_err(|e| e.to_string())?;
        self.merge(f.servers);
        Ok(())
    }

    /// Defaults + user file + worktree file (later wins; the worktree file
    /// only once trusted, see the module docs). Bad files are reported but
    /// do not disable the defaults.
    pub fn load(worktree: &Path) -> (Self, Vec<String>, Option<RepoConfig>) {
        Self::load_with(worktree, user_config_dir().as_deref())
    }

    /// [`Self::load`] with an explicit user config folder (tests).
    pub fn load_with(
        worktree: &Path,
        user_dir: Option<&Path>,
    ) -> (Self, Vec<String>, Option<RepoConfig>) {
        let mut reg = ServerRegistry::default();
        let mut errors = Vec::new();
        if let Some(f) = user_dir.map(|d| d.join("lsp.json"))
            && let Ok(s) = std::fs::read_to_string(&f)
            && let Err(e) = reg.merge_json(&s)
        {
            errors.push(format!("{}: {e}", f.display()));
        }
        let path = worktree.join(".pod").join("lsp.json");
        let Ok(bytes) = std::fs::read(&path) else {
            return (reg, errors, None);
        };
        let text = String::from_utf8_lossy(&bytes);
        let parsed: FileFormat = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                return (reg, errors, None);
            }
        };
        let repo = RepoConfig::new(worktree, path, &bytes, &parsed.servers, user_dir);
        if repo.trusted {
            reg.merge(parsed.servers);
        } else {
            for s in parsed.servers.into_iter().filter(|s| !s.enabled) {
                if let Some(x) = reg.servers.iter_mut().find(|x| x.id == s.id) {
                    x.enabled = false;
                }
            }
        }
        (reg, errors, Some(repo))
    }

    fn merge(&mut self, servers: Vec<ServerConfig>) {
        for s in servers {
            match self.servers.iter_mut().find(|x| x.id == s.id) {
                Some(x) => *x = s,
                None => self.servers.push(s),
            }
        }
    }

    pub fn for_language(&self, lsp_id: &str) -> Option<&ServerConfig> {
        self.servers.iter().find(|s| s.handles(lsp_id))
    }
}

/// The user's Pod config folder: `POD_EDITOR_CONFIG_DIR`, else
/// `%APPDATA%\Pod`. Unit tests never see the real one.
pub fn user_config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("POD_EDITOR_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    if cfg!(test) {
        return None;
    }
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Pod"))
}

/// A repository's `.pod/lsp.json` and whether the user trusts its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoConfig {
    pub path: PathBuf,
    /// The repository (git common dir, shared by all its worktrees; the
    /// worktree itself outside git).
    pub repo: String,
    /// `sha256:<hex>` of the file's bytes.
    pub hash: String,
    /// What the file would run, one line per server, for the approval prompt.
    pub commands: Vec<String>,
    pub trusted: bool,
}

impl RepoConfig {
    fn new(
        worktree: &Path,
        path: PathBuf,
        bytes: &[u8],
        servers: &[ServerConfig],
        user_dir: Option<&Path>,
    ) -> Self {
        let repo = crate::git::git_dirs(worktree)
            .map(|d| d.common_dir)
            .unwrap_or_else(|| worktree.to_path_buf());
        let repo = crate::uri::path_key(&repo);
        let hash = format!("sha256:{:x}", Sha256::digest(bytes));
        let commands = servers
            .iter()
            .map(|s| {
                let mut line = format!(
                    "{}: {}",
                    s.id,
                    std::iter::once(&s.command)
                        .chain(&s.args)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if !s.probe_args.is_empty() {
                    line.push_str(&format!(" (확인: {})", s.probe_args.join(" ")));
                }
                if let Some(o) = &s.initialization_options {
                    line.push_str(&format!(" · 초기화 옵션 {o}"));
                }
                if !s.enabled {
                    line.push_str(" · 끔");
                }
                line
            })
            .collect();
        let trusted = user_dir.is_some_and(|d| {
            TrustFile::read(d)
                .approved
                .iter()
                .any(|a| a.repo == repo && a.hash == hash)
        });
        RepoConfig {
            path,
            repo,
            hash,
            commands,
            trusted,
        }
    }

    /// Records the approval of this exact content in `user_dir`.
    pub fn trust(&self, user_dir: &Path) -> std::io::Result<()> {
        let mut f = TrustFile::read(user_dir);
        // One approval per repository: a new content replaces the old one.
        f.approved.retain(|a| a.repo != self.repo);
        f.approved.push(Approval {
            repo: self.repo.clone(),
            hash: self.hash.clone(),
        });
        std::fs::create_dir_all(user_dir)?;
        let json = serde_json::to_vec_pretty(&f).map_err(std::io::Error::other)?;
        crate::text::write_atomic(&user_dir.join(TRUST_FILE), &json)
    }
}

const TRUST_FILE: &str = "lsp-trust.json";

#[derive(Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default)]
    approved: Vec<Approval>,
}

#[derive(Serialize, Deserialize)]
struct Approval {
    repo: String,
    hash: String,
}

impl TrustFile {
    fn read(dir: &Path) -> Self {
        std::fs::read(dir.join(TRUST_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

/// Finds the server root for `file`, never leaving `worktree`.
pub fn find_root(
    file: &Path,
    worktree: &Path,
    markers: &[String],
    strategy: RootStrategy,
) -> PathBuf {
    let mut found: Option<PathBuf> = None;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if !d.starts_with(worktree) {
            break;
        }
        if markers.iter().any(|m| d.join(m).exists()) {
            found = Some(d.to_path_buf());
            if strategy == RootStrategy::Nearest {
                break;
            }
        }
        if d == worktree {
            break;
        }
        dir = d.parent();
    }
    found.unwrap_or_else(|| worktree.to_path_buf())
}

/// Resolves a command on PATH honouring PATHEXT (`.exe`, `.cmd`, ...).
pub fn which(cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.is_absolute() || cmd.contains(['/', '\\']) {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pe = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut v: Vec<String> = pe
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_lowercase())
            .collect();
        v.insert(0, String::new());
        v
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            if cfg!(windows) && ext.is_empty() && p.extension().is_none() {
                continue; // bare names without extension are not runnable on Windows
            }
            let cand = dir.join(format!("{cmd}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Installed(PathBuf),
    /// Not on PATH, or the probe failed (message).
    Missing(String),
}

/// Checks a server can actually run. Blocking (probe up to 10 s): call off
/// the UI thread.
pub fn check_available(cfg: &ServerConfig) -> Availability {
    let Some(exe) = which(&cfg.command) else {
        return Availability::Missing(format!("{} 을(를) PATH에서 찾지 못했습니다", cfg.command));
    };
    if cfg.probe_args.is_empty() {
        return Availability::Installed(exe);
    }
    probe(exe, cfg, PROBE_TIMEOUT)
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs the probe through the bounded runner: one deadline for the run
/// *and* reading its output (a failed probe whose child keeps stderr open
/// no longer blocks), both streams read while it runs into bounded buffers,
/// and the whole process tree is killed at the end.
fn probe(exe: PathBuf, cfg: &ServerConfig, timeout: Duration) -> Availability {
    use pod_process::bounded::{self, End, Keep};
    let mut c = Command::new(&exe);
    c.args(&cfg.probe_args);
    let mut opts = bounded::Options::new(timeout, Keep::Tail(1024));
    opts.stderr = Keep::Tail(16 * 1024);
    opts.after_exit_grace = Duration::from_millis(500);
    let o = match bounded::run(&mut c, &opts, None) {
        Ok(o) => o,
        Err(_) => return Availability::Missing(format!("{} 실행 실패", exe.display())),
    };
    match o.end {
        End::Exited { success: true, .. } => Availability::Installed(exe),
        End::Exited { .. } | End::TooLarge { .. } => {
            let err = o.stderr.text();
            let first = err.lines().next().unwrap_or("").trim().to_string();
            Availability::Missing(if first.is_empty() {
                format!("{} 확인 실패", cfg.id)
            } else {
                first
            })
        }
        End::TimedOut | End::Cancelled => Availability::Missing(format!("{} 응답 없음", cfg.id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_by_extension() {
        assert_eq!(
            language_for_path(Path::new("a/b.rs")).unwrap().lsp_id,
            "rust"
        );
        assert_eq!(
            language_for_path(Path::new("x.TSX")).unwrap().highlight,
            "tsx"
        );
        assert_eq!(
            language_for_path(Path::new("Cargo.lock"))
                .unwrap()
                .highlight,
            "toml"
        );
        assert_eq!(
            language_for_path(Path::new("README.md")).unwrap().lsp_id,
            "markdown"
        );
        assert!(language_for_path(Path::new("image.png")).is_none());
    }

    #[test]
    fn registry_merge_overrides_and_adds() {
        let mut r = ServerRegistry::default();
        assert_eq!(r.for_language("rust").unwrap().id, "rust-analyzer");
        r.merge_json(r#"{"servers":[{"id":"rust-analyzer","languages":["rust"],"command":"C:/tools/ra.exe","args":["-v"]},{"id":"taplo","languages":["toml"],"command":"taplo","args":["lsp","stdio"],"root_strategy":"nearest"}]}"#).unwrap();
        assert_eq!(r.for_language("rust").unwrap().command, "C:/tools/ra.exe");
        assert_eq!(
            r.for_language("toml").unwrap().root_strategy,
            RootStrategy::Nearest
        );
        assert!(r.merge_json("{oops").is_err());
        r.merge_json(r#"{"servers":[{"id":"taplo","languages":["toml"],"command":"taplo","enabled":false}]}"#).unwrap();
        assert!(r.for_language("toml").is_none());
    }

    /// editor-01: a repository's `.pod/lsp.json` runs nothing until its
    /// exact content is approved; a changed file needs a new approval.
    #[test]
    fn repo_lsp_json_needs_content_approval() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path().join("wt");
        let user = d.path().join("user");
        std::fs::create_dir_all(w.join(".pod")).unwrap();
        let evil = r#"{"servers":[{"id":"rust-analyzer","languages":["rust"],"command":"cmd.exe","args":["/c","calc"],"probe_args":["/c","calc"]},{"id":"pyright","languages":["python"],"command":"x","enabled":false}]}"#;
        std::fs::write(w.join(".pod/lsp.json"), evil).unwrap();
        let (reg, errors, repo) = ServerRegistry::load_with(&w, Some(&user));
        assert!(errors.is_empty());
        let repo = repo.expect("repo file found");
        assert!(!repo.trusted);
        assert!(
            repo.commands[0].contains("cmd.exe /c calc"),
            "{:?}",
            repo.commands
        );
        assert_eq!(
            reg.for_language("rust").unwrap().command,
            "rust-analyzer",
            "untrusted: built-in command"
        );
        assert!(
            reg.for_language("python").is_none(),
            "turning a server off needs no approval"
        );

        repo.trust(&user).unwrap();
        let (reg, _, repo2) = ServerRegistry::load_with(&w, Some(&user));
        assert!(repo2.unwrap().trusted);
        assert_eq!(reg.for_language("rust").unwrap().command, "cmd.exe");

        // Any change to the file: untrusted again.
        std::fs::write(w.join(".pod/lsp.json"), evil.replace("calc", "notepad")).unwrap();
        let (reg, _, repo3) = ServerRegistry::load_with(&w, Some(&user));
        assert!(!repo3.unwrap().trusted);
        assert_eq!(reg.for_language("rust").unwrap().command, "rust-analyzer");
        // No user folder (unit tests, no APPDATA): never trusted.
        assert!(!ServerRegistry::load_with(&w, None).2.unwrap().trusted);
    }

    #[test]
    fn linked_worktrees_share_the_repository_identity() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let wt = d.path().join("wt");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        let a = crate::git::git_dirs(&main).unwrap();
        let b = crate::git::git_dirs(&wt).unwrap();
        assert_eq!(a.common_dir, b.common_dir);
        assert_eq!(
            b.git_dir,
            crate::uri::simplify(&main.join(".git/worktrees/wt").canonicalize().unwrap())
        );
    }

    #[test]
    fn roots_stay_inside_worktree() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path().join("wt");
        std::fs::create_dir_all(w.join("crates/a/src")).unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap(); // outside the worktree
        std::fs::write(w.join("Cargo.toml"), "").unwrap();
        std::fs::write(w.join("crates/a/Cargo.toml"), "").unwrap();
        let f = w.join("crates/a/src/lib.rs");
        let m = vec!["Cargo.toml".to_string()];
        assert_eq!(find_root(&f, &w, &m, RootStrategy::Outermost), w);
        assert_eq!(
            find_root(&f, &w, &m, RootStrategy::Nearest),
            w.join("crates/a")
        );
        assert_eq!(
            find_root(&f, &w, &["nope".into()], RootStrategy::Nearest),
            w
        );
    }

    #[cfg(windows)]
    #[test]
    fn which_uses_pathext() {
        assert!(which("cmd").is_some_and(|p| p.extension().unwrap().eq_ignore_ascii_case("exe")));
        assert!(which("definitely-not-a-real-command-xyz").is_none());
        let missing = ServerConfig {
            command: "definitely-not-a-real-command-xyz".into(),
            ..ServerRegistry::default().servers[0].clone()
        };
        assert!(matches!(
            check_available(&missing),
            Availability::Missing(_)
        ));
    }

    /// A probe that fails at once while a grandchild keeps its stderr open
    /// far past the limit: the answer still comes back within the probe
    /// deadline, with the stderr line, and the grandchild dies with the tree.
    #[cfg(windows)]
    #[test]
    fn failed_probe_with_stderr_held_open_returns_within_the_limit() {
        let cfg = ServerConfig {
            command: "cmd".into(),
            probe_args: vec![
                "/d".into(),
                "/c".into(),
                "start /b ping -n 60 127.0.0.47 & echo probe-broke 1>&2 & exit 1".into(),
            ],
            ..ServerRegistry::default().servers[0].clone()
        };
        let exe = which("cmd").unwrap();
        let t = std::time::Instant::now();
        let a = probe(exe, &cfg, Duration::from_secs(4));
        assert!(
            t.elapsed() < Duration::from_secs(7),
            "probe bounded: {:?}",
            t.elapsed()
        );
        assert_eq!(a, Availability::Missing("probe-broke".into()));
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*127.0.0.47*' -and $_.Name -eq 'ping.exe' }).Count",
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "0",
            "grandchild killed"
        );
    }
}
