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

/// docs/19 §18.5: the guard for an orchestration worker's own git (and
/// the git its agent program runs outside its sandbox): every filter
/// driver in `drivers` or set in `base`'s command-line config emptied and
/// not required, fsmonitor off, hooks from `no_hooks` (an empty folder).
/// Returned as environment overrides that append to the command-line
/// config `base` already has (the user's entries keep their places).
pub fn worker_git_guard(
    base: &BTreeMap<String, String>,
    drivers: &[String],
    no_hooks: &str,
) -> BTreeMap<String, String> {
    let user = git_config_entries(base);
    let mut drivers = drivers.to_vec();
    for (k, v) in &user {
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
