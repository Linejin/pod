//! Where the daemon listens: one named pipe per user.

pub const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Environment variable naming the daemon endpoint. Set by podd inside every
/// terminal it starts, and honored by clients as an override.
pub const ENV_SOCKET: &str = "POD_SOCKET";
/// Environment variable carrying the terminal's own id inside each terminal.
pub const ENV_TERMINAL_ID: &str = "POD_TERMINAL_ID";

/// `\\.\pipe\podd-<username>`, unless `POD_SOCKET` overrides it.
pub fn default_pipe() -> String {
    if let Ok(p) = std::env::var(ENV_SOCKET)
        && !p.trim().is_empty()
    {
        return pipe_path(&p);
    }
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    pipe_path(&format!("podd-{}", sanitize(&user)))
}

/// Suffix of the restricted worker endpoint's pipe (docs/19 §20).
pub const WORKER_PIPE_SUFFIX: &str = "-worker";

/// The restricted worker endpoint of the daemon on `pipe`:
/// `\\.\pipe\podd-alice-worker`. Same-user DACL like the full pipe, but a
/// connection there acts for one Pod worker only (its attempt token, said
/// in `connection.hello`) and reaches a method allowlist. The full pipe is
/// the OS user: whoever can open it can do everything.
pub fn worker_pipe(pipe: &str) -> String {
    format!("{}{WORKER_PIPE_SUFFIX}", pipe_path(pipe))
}

/// Is `pipe` a worker endpoint (never a daemon's own pipe)?
pub fn is_worker_pipe(pipe: &str) -> bool {
    let p = pipe_path(pipe);
    p.len() > PIPE_PREFIX.len() + WORKER_PIPE_SUFFIX.len()
        && p.to_ascii_lowercase()
            .ends_with(&WORKER_PIPE_SUFFIX.to_ascii_lowercase())
}

/// Accepts a bare name (`podd-test`) or a full path (`\\.\pipe\podd-test`).
pub fn pipe_path(name_or_path: &str) -> String {
    if name_or_path.starts_with(r"\\") {
        name_or_path.to_string()
    } else {
        format!("{PIPE_PREFIX}{name_or_path}")
    }
}

/// File-name-safe key of a daemon pipe (`\\.\pipe\podd-alice` -> `podd-alice`).
pub fn pipe_key(pipe: &str) -> String {
    sanitize(pipe.strip_prefix(PIPE_PREFIX).unwrap_or(pipe))
}

/// Holder records of a daemon: `%LOCALAPPDATA%\pod\state\<pipe key>`.
pub fn default_state_dir(pipe: &str) -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("pod").join("state").join(pipe_key(pipe))
}

/// Private pipe of the holder of terminal `id` under daemon `pipe`.
pub fn holder_pipe(pipe: &str, id: &str) -> String {
    format!("{PIPE_PREFIX}pod-holder-{}-{id}", pipe_key(pipe))
}

pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(pipe_path("podd-x"), r"\\.\pipe\podd-x");
        assert_eq!(pipe_path(r"\\.\pipe\podd-x"), r"\\.\pipe\podd-x");
        assert_eq!(sanitize("a b\\c"), "a_b_c");
    }

    #[test]
    fn worker_pipes() {
        assert_eq!(worker_pipe("podd-x"), r"\\.\pipe\podd-x-worker");
        assert_eq!(worker_pipe(r"\\.\pipe\podd-x"), r"\\.\pipe\podd-x-worker");
        assert!(is_worker_pipe(r"\\.\pipe\podd-x-worker"));
        assert!(is_worker_pipe("podd-x-WORKER"));
        assert!(!is_worker_pipe(r"\\.\pipe\podd-x"));
        assert!(!is_worker_pipe("-worker"));
        // Its own holder pipes stay apart.
        assert_ne!(
            pipe_key(&worker_pipe("podd-x")),
            pipe_key(&holder_pipe("podd-x", "t1"))
        );
    }
}
