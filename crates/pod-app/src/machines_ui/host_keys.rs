//! Host key fingerprints of a profile as known_hosts has them: the keys a
//! successful connection was checked against (`StrictHostKeyChecking=yes`).
//!
//! Read-only: `ssh-keygen -l -F <host> -f <file>` from the same OpenSSH
//! installation as the ssh Pod runs (it matches hashed entries and
//! `[host]:port` exactly like ssh). Pod never adds, replaces or removes a
//! known_hosts entry.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pod_remote::Machine;

/// How the app learns the fingerprints (tests use fixed ones).
pub trait HostKeys: Send + Sync {
    /// `ED25519 SHA256:…` lines for the profile's host, sorted; empty when
    /// known_hosts has none. `Err` when they could not be read at all.
    fn lookup(&self, m: &Machine) -> Result<Vec<String>, String>;
}

/// The name known_hosts files the host under: `host`, or `[host]:port` for
/// a port other than 22 (OpenSSH's rule).
pub fn search_name(m: &Machine) -> String {
    match m.port {
        None | Some(22) => m.host.clone(),
        Some(p) => format!("[{}]:{p}", m.host),
    }
}

/// The files ssh reads for this profile (`-F` config: the profile's
/// `UserKnownHostsFile`, else the user's default files, plus the global one).
pub fn known_hosts_files(m: &Machine) -> Vec<PathBuf> {
    if let Some(p) = &m.known_hosts {
        return vec![p.clone()];
    }
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
        let ssh = PathBuf::from(home).join(".ssh");
        out.push(ssh.join("known_hosts"));
        out.push(ssh.join("known_hosts2"));
    }
    if let Some(pd) = std::env::var_os("ProgramData").filter(|h| !h.is_empty()) {
        out.push(PathBuf::from(pd).join("ssh").join("ssh_known_hosts"));
    }
    out
}

/// `TYPE SHA256:…` from `ssh-keygen -l -F` output (`host TYPE SHA256:…`
/// per match; `#` lines are headers). Marker lines (`@revoked`,
/// `@cert-authority`) are not host keys and are left out.
pub fn parse_keygen(out: &str) -> Vec<String> {
    let mut keys: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('@'))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            let fp = parts.iter().position(|p| p.starts_with("SHA256:"))?;
            let kind = fp.checked_sub(1).map(|i| parts[i]).unwrap_or("?");
            Some(format!("{kind} {}", parts[fp]))
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// The fingerprint a refusing ssh printed for the key the remote host sent
/// (`HostKeyChanged` stderr: "…key sent by the remote host is" then
/// `SHA256:….`).
pub fn sent_fingerprint(stderr: &[String]) -> Option<String> {
    stderr.iter().find_map(|l| {
        l.split_whitespace()
            .find(|w| w.starts_with("SHA256:"))
            .map(|w| w.trim_end_matches('.').to_string())
    })
}

/// The real lookup through `ssh-keygen` next to the ssh Pod uses.
pub struct SshKeygen;

impl SshKeygen {
    fn exe() -> Result<PathBuf, String> {
        let ssh = pod_remote::ssh::locate_ssh()?;
        let dir = ssh.parent().unwrap_or(Path::new("."));
        let k = dir.join("ssh-keygen.exe");
        if k.is_file() {
            Ok(k)
        } else {
            Err(format!("{}이(가) 없습니다", k.display()))
        }
    }
}

impl HostKeys for SshKeygen {
    fn lookup(&self, m: &Machine) -> Result<Vec<String>, String> {
        let exe = Self::exe()?;
        let name = search_name(m);
        let mut keys = Vec::new();
        for file in known_hosts_files(m).into_iter().filter(|f| f.is_file()) {
            let out = run_bounded(
                Command::new(&exe)
                    .arg("-l")
                    .arg("-F")
                    .arg(&name)
                    .arg("-f")
                    .arg(&file),
                Duration::from_secs(5),
            )?;
            keys.extend(parse_keygen(&out));
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }
}

/// Run with no console window, stdout captured (bounded), killed after `limit`.
fn run_bounded(cmd: &mut Command, limit: Duration) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ssh-keygen 실행 실패: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = (&mut stdout).take(256 * 1024).read_to_end(&mut buf);
        buf
    });
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("ssh-keygen이 시간 안에 끝나지 않았습니다".into());
            }
        }
    }
    let buf = reader.join().unwrap_or_default();
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Fixed answers (fixtures and tests).
pub struct FixedKeys(pub std::sync::Mutex<Result<Vec<String>, String>>);

impl FixedKeys {
    pub fn new(keys: &[&str]) -> Self {
        Self(std::sync::Mutex::new(Ok(keys
            .iter()
            .map(|k| k.to_string())
            .collect())))
    }

    /// What later lookups return (a known_hosts edit between connections).
    #[cfg(test)]
    pub fn set(&self, keys: &[&str]) {
        *self.0.lock().unwrap() = Ok(keys.iter().map(|k| k.to_string()).collect());
    }
}

impl HostKeys for FixedKeys {
    fn lookup(&self, _: &Machine) -> Result<Vec<String>, String> {
        self.0.lock().unwrap().clone()
    }
}
