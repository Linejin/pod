//! Terminal environment hygiene.
//!
//! `pod` auto-starts `podd` from whatever shell it happens to run in: an agent
//! tool shell, another IDE's terminal, VS Code, Windows Terminal... Variables from
//! that launcher must not leak into the terminals we start. Examples seen in
//! practice: `NO_COLOR=1` (exported by agent tool shells) turns off Claude
//! Code's colors, and `CLAUDECODE` / `CLAUDE_CODE_*` session markers make
//! Claude believe it is a child session (no transcript, warning banner).
//!
//! The daemon therefore builds each terminal's environment as:
//! 1. the user's *registry* environment (what a fresh logon would get,
//!    `CreateEnvironmentBlock`), falling back to the daemon's own environment;
//! 2. minus everything [`is_leaky`] matches;
//! 3. plus `TERM`, `COLORTERM`, `POD_TERMINAL_ID`, `POD_SOCKET`, and the
//!    daemon's own folder first on `PATH` when `pod.exe` is there, so agents
//!    in Pod terminals run the `pod` that matches this daemon even when Pod
//!    is not on the user's `PATH` (dev builds, portable zip, isolated tests);
//! 4. plus the per-run `env` overrides from `terminal.run`, which may set anything.

use std::collections::BTreeMap;

/// Exact names (case-insensitive) that describe the *launching* terminal or agent.
const DROP_EXACT: &[&str] = &[
    "NO_COLOR",
    "FORCE_COLOR",
    "CLICOLOR_FORCE",
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "COLORTERM",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "ConEmuPID",
    "ConEmuANSI",
    "POD_TERMINAL_ID",
    "POD_SOCKET",
    "HC_TERMINAL_ID",
];

/// Prefixes (case-insensitive) of session / sandbox / host markers, including
/// the markers that parent IDEs and terminal multiplexers export.
const DROP_PREFIX: &[&str] = &[
    "CLAUDE_CODE_",
    "CODEX_",
    "ORCA_",
    "VSCODE_",
    "WT_",
    "HERDR_",
    "HC_",
];

/// User configuration that shares a dropped prefix but is not a session marker.
const KEEP: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "CLAUDE_CODE_GIT_BASH_PATH",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];

/// True for variables that belong to the launcher, not to the user.
pub fn is_leaky(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if KEEP.iter().any(|k| *k == upper) {
        return false;
    }
    DROP_EXACT.iter().any(|d| d.eq_ignore_ascii_case(name))
        || DROP_PREFIX.iter().any(|p| upper.starts_with(p))
}

/// Remove launcher markers from an environment map.
pub fn sanitize(env: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    env.into_iter()
        .filter(|(k, _)| !k.is_empty() && !k.starts_with('=') && !is_leaky(k))
        .collect()
}

/// A Pod worker's attempt (docs/19 §3.4), set only in worker terminals.
pub const ENV_ATTEMPT_ID: &str = "POD_ATTEMPT_ID";
/// The worker's attempt token (256 random bits; the daemon keeps a hash).
pub const ENV_ATTEMPT_TOKEN: &str = "POD_ATTEMPT_TOKEN";

/// Variables a Pod terminal or worker carries that a daemon must never
/// inherit from the shell that auto-starts it (docs/19 §20): the
/// terminal's and the worker's identity, the hook token, and the program
/// overrides an agent could set (`POD_DAEMON` is the client's own).
/// Other `POD_*` daemon settings (`POD_HOLDER`, `POD_WORKTREES_ROOT`, ...)
/// pass; every `GIT_*` goes ([`is_daemon_env_leak`]).
const NOT_FOR_DAEMON: &[&str] = &[
    "POD_SOCKET",
    "POD_TERMINAL_ID",
    "POD_TASK_ID",
    "POD_BRIEF",
    "POD_RUN_ID",
    "POD_ITEM_ID",
    ENV_ATTEMPT_ID,
    ENV_ATTEMPT_TOKEN,
    "POD_HOOK_TOKEN",
    "POD_DAEMON",
    "POD_GIT",
    "POD_GH",
];

/// True for a variable `pod` must not pass to a daemon it starts: worker
/// and terminal identity ([`NOT_FOR_DAEMON`]) and any `GIT_*` (a worker's
/// git guard, `GIT_DIR`, `GIT_CONFIG_*`...).
pub fn is_daemon_env_leak(name: &str) -> bool {
    name.to_ascii_uppercase().starts_with("GIT_")
        || NOT_FOR_DAEMON.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// This process runs in a Pod worker's environment (`POD_ATTEMPT_ID` or
/// `POD_ATTEMPT_TOKEN` set): it never starts a daemon (docs/19 §20).
pub fn in_worker_env() -> bool {
    [ENV_ATTEMPT_ID, ENV_ATTEMPT_TOKEN]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// The worker identity in this process's environment, when complete.
pub fn worker_identity() -> Option<(String, String)> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    Some((get(ENV_ATTEMPT_ID)?, get(ENV_ATTEMPT_TOKEN)?))
}

/// Insert or replace `key`, matching existing keys case-insensitively (Windows semantics).
pub fn set_var(env: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    if let Some(existing) = env.keys().find(|k| k.eq_ignore_ascii_case(key)).cloned() {
        env.remove(&existing);
    }
    env.insert(key.to_string(), value.into());
}

/// Put `dir` first on `PATH`, dropping a later copy of it (compared
/// case-insensitively, ignoring trailing separators). Creates `PATH` if missing.
pub fn prepend_path(env: &mut BTreeMap<String, String>, dir: &str) {
    let norm = |s: &str| s.trim_end_matches(['\\', '/']).to_ascii_lowercase();
    let want = norm(dir);
    let mut parts = vec![dir.to_string()];
    if let Some(old) = get_var(env, "PATH") {
        parts.extend(
            old.split(';')
                .filter(|p| !p.is_empty() && norm(p) != want)
                .map(str::to_string),
        );
    }
    let key = env
        .keys()
        .find(|k| k.eq_ignore_ascii_case("PATH"))
        .cloned()
        .unwrap_or_else(|| "Path".to_string());
    env.insert(key, parts.join(";"));
}

/// Look up a variable case-insensitively.
pub fn get_var<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Names of the filter drivers with a program in `git config -z
/// --get-regexp '^filter\..*\.(clean|smudge|process)$'` output
/// (`key\nvalue\0`…).
pub fn parse_filter_drivers(s: &str) -> Vec<String> {
    let mut names = Vec::new();
    for entry in s.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        push_driver(&mut names, key, value);
    }
    names
}

fn push_driver(names: &mut Vec<String>, key: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    let Some((name, field)) = key
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("filter."))
        .and_then(|_| key[7..].rsplit_once('.'))
    else {
        return;
    };
    let field = field.to_ascii_lowercase();
    if matches!(field.as_str(), "clean" | "smudge" | "process") && !names.iter().any(|n| n == name)
    {
        names.push(name.to_string());
    }
}

/// The command-line git config (`GIT_CONFIG_COUNT/KEY_n/VALUE_n`) already
/// in `env`, in order. A missing or bad count is none.
pub fn git_config_entries(env: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let n = get_var(env, "GIT_CONFIG_COUNT")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    (0..n)
        .map(|i| {
            let k = get_var(env, &format!("GIT_CONFIG_KEY_{i}")).unwrap_or_default();
            let v = get_var(env, &format!("GIT_CONFIG_VALUE_{i}")).unwrap_or_default();
            (k.to_string(), v.to_string())
        })
        .collect()
}

/// `GIT_CONFIG_PARAMETERS` (how `git -c` hands its config to child gits):
/// `'key'='value'` entries (or the older `'key=value'`), single-quoted with
/// `'\''` / `'\!'` escapes, separated by spaces. Stops at the first entry
/// it cannot read.
pub fn parse_config_parameters(s: &str) -> Vec<(String, String)> {
    fn quoted(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
        if it.next() != Some('\'') {
            return None;
        }
        let mut s = String::new();
        loop {
            match it.next()? {
                '\'' if it.peek() == Some(&'\\') => {
                    it.next();
                    s.push(it.next()?);
                    if it.next() != Some('\'') {
                        return None;
                    }
                }
                '\'' => return Some(s),
                c => s.push(c),
            }
        }
    }
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        if it.peek().is_none() {
            break;
        }
        let Some(key) = quoted(&mut it) else { break };
        if it.peek() == Some(&'=') {
            it.next();
            let Some(value) = quoted(&mut it) else { break };
            out.push((key, value));
        } else if let Some((k, v)) = key.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        } else {
            out.push((key, String::new()));
        }
    }
    out
}

/// `s` single-quoted the way git writes `GIT_CONFIG_PARAMETERS`.
fn sq_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''").replace('!', "'\\!'"))
}

/// docs/19 §18.5, §18.6: the guard for an orchestration worker's own git
/// (and the git its agent program runs outside its sandbox): every filter
/// driver in `drivers` or set in `base`'s command-line config
/// (`GIT_CONFIG_COUNT` entries and `GIT_CONFIG_PARAMETERS`) emptied and
/// not required, fsmonitor off, hooks from `no_hooks` (an empty folder).
/// Returned as environment overrides that append to the command-line
/// config `base` already has (the user's entries keep their places). git
/// applies `GIT_CONFIG_PARAMETERS` after the counted entries, so when
/// `base` has one the guard is appended there too and still comes last.
/// Drivers configured after the worker started are not in it (the policy
/// is in docs/19 §18.6).
pub fn worker_git_guard(
    base: &BTreeMap<String, String>,
    drivers: &[String],
    no_hooks: &str,
) -> BTreeMap<String, String> {
    let user = git_config_entries(base);
    let params = get_var(base, "GIT_CONFIG_PARAMETERS").filter(|p| !p.trim().is_empty());
    let mut drivers = drivers.to_vec();
    for (k, v) in user
        .iter()
        .chain(&params.map(parse_config_parameters).unwrap_or_default())
    {
        push_driver(&mut drivers, k, v);
    }
    let mut cfg: Vec<(String, String)> = vec![
        ("core.fsmonitor".into(), "false".into()),
        ("core.hooksPath".into(), no_hooks.replace('\\', "/")),
    ];
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
    let first = user.len();
    let mut out = BTreeMap::new();
    out.insert(
        "GIT_CONFIG_COUNT".to_string(),
        (first + cfg.len()).to_string(),
    );
    if let Some(p) = params {
        let mut all = p.trim_end().to_string();
        for (k, v) in &cfg {
            all.push_str(&format!(" {}={}", sq_quote(k), sq_quote(v)));
        }
        out.insert("GIT_CONFIG_PARAMETERS".to_string(), all);
    }
    for (i, (k, v)) in cfg.into_iter().enumerate() {
        out.insert(format!("GIT_CONFIG_KEY_{}", first + i), k);
        out.insert(format!("GIT_CONFIG_VALUE_{}", first + i), v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §18.5: the worker guard appends after the user's own command-line
    /// config, blanks repository and user filter drivers, and turns off
    /// fsmonitor and hooks.
    #[test]
    fn worker_git_guard_appends_and_blanks_filters() {
        let drivers = parse_filter_drivers(
            "filter.lfs.clean\ngit-lfs clean -- %f\0filter.lfs.process\ngit-lfs filter-process\0\
             filter.a.b.smudge\nsh x.sh\0filter.off.clean\n\0filter.x.required\ntrue\0",
        );
        assert_eq!(drivers, ["lfs", "a.b"]);
        let base: BTreeMap<String, String> = [
            ("git_config_count", "2"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "Me"),
            ("GIT_CONFIG_KEY_1", "Filter.Mine.Clean"),
            ("GIT_CONFIG_VALUE_1", "mine.exe"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let g = worker_git_guard(&base, &drivers, r"C:\s\orch\no-hooks");
        let mut env = base.clone();
        for (k, v) in g {
            set_var(&mut env, &k, v);
        }
        let all = git_config_entries(&env);
        assert_eq!(all[0], ("user.name".into(), "Me".into()), "user's kept");
        assert_eq!(all[1].0, "Filter.Mine.Clean");
        let has = |k: &str, v: &str| all.iter().any(|(a, b)| a == k && b == v);
        assert!(has("core.fsmonitor", "false"));
        assert!(has("core.hooksPath", "C:/s/orch/no-hooks"));
        for d in ["lfs", "a.b", "Mine"] {
            for f in ["clean", "smudge", "process"] {
                assert!(has(&format!("filter.{d}.{f}"), ""), "{d}.{f}: {all:?}");
            }
            assert!(has(&format!("filter.{d}.required"), "false"));
        }
        assert_eq!(all.len(), 2 + 2 + 3 * 4);
        // Nothing set before: starts at 0; a bad count counts as none.
        let g = worker_git_guard(&BTreeMap::new(), &[], "h");
        assert_eq!(g["GIT_CONFIG_COUNT"], "2");
        assert_eq!(g["GIT_CONFIG_KEY_0"], "core.fsmonitor");
        let bad: BTreeMap<_, _> = [("GIT_CONFIG_COUNT".to_string(), "x".to_string())].into();
        assert_eq!(worker_git_guard(&bad, &[], "h")["GIT_CONFIG_COUNT"], "2");
    }

    #[test]
    fn config_parameters_parse_like_git() {
        assert_eq!(
            parse_config_parameters(
                "'core.fsmonitor'='false' 'old.style=1'  'a.b'='it'\\''s'\\!'' 'bare.key' 'x.y'"
            ),
            [
                ("core.fsmonitor".to_string(), "false".to_string()),
                ("old.style".into(), "1".into()),
                ("a.b".into(), "it's!".into()),
                ("bare.key".into(), String::new()),
                ("x.y".into(), String::new()),
            ]
        );
        assert_eq!(parse_config_parameters("'broken"), []);
        assert_eq!(sq_quote("it's!"), "'it'\\''s'\\!''");
        assert_eq!(
            parse_config_parameters(&format!("{}={}", sq_quote("k.k"), sq_quote("it's!")))[0].1,
            "it's!"
        );
    }

    /// TM-21 (docs/19 §18.6): `GIT_CONFIG_PARAMETERS` in the worker's base
    /// environment wins over `GIT_CONFIG_COUNT`, so the guard is appended
    /// there as well; its drivers are emptied. Checked with git itself.
    #[test]
    fn worker_git_guard_wins_over_inherited_config_parameters() {
        let base: BTreeMap<String, String> = [(
            "GIT_CONFIG_PARAMETERS".to_string(),
            "'core.hookspath'='C:/evil/hooks' 'filter.p.clean'='prog %f'".to_string(),
        )]
        .into();
        let g = worker_git_guard(&base, &[], r"C:\s\orch\no-hooks");
        let params = parse_config_parameters(&g["GIT_CONFIG_PARAMETERS"]);
        assert_eq!(
            params[0],
            ("core.hookspath".into(), "C:/evil/hooks".into()),
            "user's first"
        );
        assert!(
            params.contains(&("filter.p.clean".into(), String::new())),
            "{params:?}"
        );
        let mut env = base.clone();
        for (k, v) in &g {
            set_var(&mut env, k, v.clone());
        }
        let get = |key: &str| {
            let o = std::process::Command::new("git")
                .args(["config", "--get", key])
                .env_clear()
                .envs(std::env::vars().filter(|(k, _)| !k.to_ascii_uppercase().starts_with("GIT_")))
                .envs(&env)
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        assert_eq!(get("core.hooksPath"), "C:/s/orch/no-hooks");
        assert_eq!(get("filter.p.clean"), "");
        assert_eq!(get("core.fsmonitor"), "false");
        // Without inherited parameters nothing about them is added.
        assert!(
            !worker_git_guard(&BTreeMap::new(), &[], "h").contains_key("GIT_CONFIG_PARAMETERS")
        );
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn drops_launcher_markers_and_keeps_user_config() {
        let out = sanitize(env(&[
            ("PATH", r"C:\bin"),
            ("NO_COLOR", "1"),
            ("no_color", "1"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
            ("CLAUDE_CODE_SESSION_ID", "x"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_PID", "42"),
            ("CODEX_SANDBOX", "seatbelt"),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
            ("ORCA_TERMINAL_HANDLE", "term_x"),
            ("ORCA_AGENT_HOOK_TOKEN", "secret"),
            ("TERM_PROGRAM", "vscode"),
            ("TERM", "dumb"),
            ("WT_SESSION", "guid"),
            ("POD_TERMINAL_ID", "t9"),
            ("=C:", r"C:\"),
            ("CODEX_HOME", r"C:\codex"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CONFIG_DIR", r"C:\cfg"),
            ("USERPROFILE", r"C:\Users\me"),
        ]));
        let keys: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CONFIG_DIR",
                "CODEX_HOME",
                "PATH",
                "USERPROFILE"
            ]
        );
    }

    /// docs/19 §20: what a daemon started by `pod` never inherits.
    #[test]
    fn daemon_env_leaks() {
        for n in [
            "POD_ATTEMPT_ID",
            "pod_attempt_token",
            "POD_SOCKET",
            "POD_TERMINAL_ID",
            "POD_HOOK_TOKEN",
            "POD_RUN_ID",
            "POD_GIT",
            "POD_DAEMON",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "git_dir",
            "GIT_CONFIG_PARAMETERS",
        ] {
            assert!(is_daemon_env_leak(n), "{n}");
        }
        for n in [
            "POD_HOLDER",
            "POD_WORKTREES_ROOT",
            "PATH",
            "USERPROFILE",
            "GITHUB_TOKEN",
        ] {
            assert!(!is_daemon_env_leak(n), "{n}");
        }
    }

    #[test]
    fn prepend_path_puts_the_pod_folder_first_once() {
        let mut e = sanitize(env(&[("Path", r"C:\a;C:\Pod\bin\;C:\b")]));
        prepend_path(&mut e, r"C:\pod\BIN");
        assert_eq!(get_var(&e, "PATH"), Some(r"C:\pod\BIN;C:\a;C:\b"));
        assert!(
            e.contains_key("Path") && e.len() == 1,
            "keeps the key spelling"
        );
        let mut empty = BTreeMap::new();
        prepend_path(&mut empty, r"C:\x");
        assert_eq!(get_var(&empty, "path"), Some(r"C:\x"));
    }

    #[test]
    fn set_var_is_case_insensitive() {
        let mut e = sanitize(env(&[("Path", "a")]));
        set_var(&mut e, "PATH", "b");
        assert_eq!(e.len(), 1);
        assert_eq!(get_var(&e, "path"), Some("b"));
    }
}
