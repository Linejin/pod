//! Machine profiles: `%APPDATA%\Pod\machines.json` (or `$POD_CONFIG_DIR\machines.json`).
//!
//! A profile holds **only** host, user, port, a key path and a known_hosts
//! path (docs/17 §2.1). No free-form SSH options, no ProxyCommand, no remote
//! command: unknown fields in the file are rejected, and every value is
//! validated again whenever it is used (the file can be edited by hand).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Overrides the Pod config directory (tests, portable setups).
pub const ENV_CONFIG_DIR: &str = "POD_CONFIG_DIR";
const FILE_NAME: &str = "machines.json";
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    /// Profile name (`pod --machine <name>`).
    pub name: String,
    /// Host name or IP address of the SSH server.
    pub host: String,
    /// Login user (default: ssh's default, the local user name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Private key file (else ssh-agent / ssh's default keys).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<PathBuf>,
    /// known_hosts file (else ssh's default `~/.ssh/known_hosts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_hosts: Option<PathBuf>,
}

/// Profile name: 1–64 of `A-Z a-z 0-9 . _ -`, starting with a letter or digit.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err(format!("머신 이름은 1–64자여야 합니다: {name:?}"));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "머신 이름은 영문자나 숫자로 시작해야 합니다: {name:?}"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "머신 이름에는 영문자·숫자·`.`·`_`·`-`만 쓸 수 있습니다: {name:?}"
        ));
    }
    Ok(())
}

/// Shared checks of host and user: never an option (`-…`), no control
/// characters, no whitespace.
fn plain_token(what: &str, v: &str, max: usize) -> Result<(), String> {
    if v.is_empty() || v.len() > max {
        return Err(format!("{what}은(는) 1–{max}바이트여야 합니다"));
    }
    if v.starts_with('-') {
        return Err(format!("{what}은(는) `-`로 시작할 수 없습니다: {v:?}"));
    }
    if v.chars().any(|c| c.is_control()) {
        return Err(format!("{what}에 제어 문자가 있습니다: {v:?}"));
    }
    if v.chars().any(char::is_whitespace) {
        return Err(format!("{what}에 공백이 있습니다: {v:?}"));
    }
    Ok(())
}

/// Host name, IPv4 or IPv6 address (no brackets; `%zone` allowed).
pub fn validate_host(host: &str) -> Result<(), String> {
    plain_token("호스트", host, 253)?;
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'))
    {
        return Err(format!(
            "호스트에는 영문자·숫자·`.`·`-`·`_`·`:`·`%`만 쓸 수 있습니다: {host:?}"
        ));
    }
    Ok(())
}

/// Login user: `A-Z a-z 0-9 . _ - @ $` (`user@domain` works; `DOMAIN\user` does not).
pub fn validate_user(user: &str) -> Result<(), String> {
    plain_token("사용자", user, 64)?;
    if !user
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '$'))
    {
        return Err(format!(
            "사용자에는 영문자·숫자·`.`·`_`·`-`·`@`·`$`만 쓸 수 있습니다: {user:?}"
        ));
    }
    Ok(())
}

/// Key / known_hosts path: absolute, no control characters or `"` (it is
/// written quoted into the generated ssh config).
pub fn validate_path(what: &str, p: &Path) -> Result<(), String> {
    let s = p.to_string_lossy();
    if s.chars().any(|c| c.is_control() || c == '"') {
        return Err(format!("{what} 경로에 제어 문자나 `\"`가 있습니다: {s:?}"));
    }
    if !p.is_absolute() {
        return Err(format!("{what} 경로는 절대 경로여야 합니다: {s:?}"));
    }
    Ok(())
}

impl Machine {
    pub fn validate(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        validate_host(&self.host)?;
        if let Some(u) = &self.user {
            validate_user(u)?;
        }
        if self.port == Some(0) {
            return Err("포트는 1–65535여야 합니다".into());
        }
        if let Some(p) = &self.identity_file {
            validate_path("키", p)?;
        }
        if let Some(p) = &self.known_hosts {
            validate_path("known_hosts", p)?;
        }
        Ok(())
    }

    /// `user@host:port` for messages.
    pub fn target(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        if self.host.contains(':') {
            s.push_str(&format!("[{}]", self.host));
        } else {
            s.push_str(&self.host);
        }
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFormat {
    version: u32,
    #[serde(default)]
    machines: Vec<Machine>,
}

/// The profiles file.
pub struct MachineStore {
    path: PathBuf,
}

impl MachineStore {
    /// `$POD_CONFIG_DIR`, else `%APPDATA%\Pod`.
    pub fn config_dir() -> PathBuf {
        if let Some(d) = std::env::var_os(ENV_CONFIG_DIR).filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Pod")
    }

    pub fn open_default() -> Self {
        Self::at(Self::config_dir().join(FILE_NAME))
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All profiles (none when the file does not exist). A file that does
    /// not parse is an error and is never overwritten.
    pub fn load(&self) -> Result<Vec<Machine>, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(format!(
                    "{}을(를) 읽을 수 없습니다: {e}",
                    self.path.display()
                ));
            }
        };
        let f: FileFormat = serde_json::from_slice(&bytes)
            .map_err(|e| format!("{}이(가) 올바르지 않습니다: {e}", self.path.display()))?;
        if f.version != FILE_VERSION {
            return Err(format!(
                "{}의 version {}을(를) 모릅니다 (이 Pod는 {FILE_VERSION})",
                self.path.display(),
                f.version
            ));
        }
        Ok(f.machines)
    }

    pub fn get(&self, name: &str) -> Result<Machine, String> {
        let m = self
            .load()?
            .into_iter()
            .find(|m| m.name == name)
            .ok_or_else(|| {
                format!("머신 {name:?}이(가) 없습니다 (`pod machine list`, `pod machine add`)")
            })?;
        m.validate()
            .map_err(|e| format!("머신 {name:?} 프로필이 올바르지 않습니다: {e}"))?;
        Ok(m)
    }

    pub fn add(&self, m: Machine) -> Result<(), String> {
        m.validate()?;
        let mut all = self.load()?;
        if all.iter().any(|x| x.name == m.name) {
            return Err(format!(
                "머신 {:?}이(가) 이미 있습니다 (먼저 `pod machine remove`)",
                m.name
            ));
        }
        all.push(m);
        self.save(all)
    }

    /// `false` when there was no such profile.
    pub fn remove(&self, name: &str) -> Result<bool, String> {
        let mut all = self.load()?;
        let before = all.len();
        all.retain(|m| m.name != name);
        if all.len() == before {
            return Ok(false);
        }
        self.save(all)?;
        Ok(true)
    }

    /// Write to a temp file next to it, then replace.
    fn save(&self, machines: Vec<Machine>) -> Result<(), String> {
        let f = FileFormat {
            version: FILE_VERSION,
            machines,
        };
        let text = serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?;
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("{}을(를) 만들 수 없습니다: {e}", dir.display()))?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text.as_bytes())
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("{}에 쓸 수 없습니다: {e}", self.path.display())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, host: &str) -> Machine {
        Machine {
            name: name.into(),
            host: host.into(),
            user: None,
            port: None,
            identity_file: None,
            known_hosts: None,
        }
    }

    #[test]
    fn host_user_and_name_validation() {
        for ok in ["example.com", "10.0.0.5", "fe80::1%eth0", "my_host-1"] {
            validate_host(ok).unwrap();
        }
        for bad in [
            "",
            "-oProxyCommand=calc",
            "-p",
            "host\nx",
            "host\u{7}",
            "a b",
            "a\tb",
            "user@host",
            "host;calc",
            "\"host\"",
            "host/x",
        ] {
            assert!(validate_host(bad).is_err(), "{bad:?}");
        }
        for ok in ["alice", "svc$", "a.b-c_d", "alice@corp.example"] {
            validate_user(ok).unwrap();
        }
        for bad in [
            "",
            "-l",
            "-oX=y",
            "a b",
            "a\u{1b}[31m",
            "DOM\\user",
            "a\"b",
            "a\r",
        ] {
            assert!(validate_user(bad).is_err(), "{bad:?}");
        }
        for ok in ["dev", "box-1", "a.b_c", "1"] {
            validate_name(ok).unwrap();
        }
        for bad in ["", "-x", ".x", "a/b", "a b", "한글", &"x".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        let mut x = m("dev", "h");
        x.port = Some(0);
        assert!(x.validate().is_err());
        x.port = Some(2222);
        x.identity_file = Some("relative\\key".into());
        assert!(x.validate().is_err());
        x.identity_file = Some("C:\\keys\\a\"b".into());
        assert!(x.validate().is_err());
        x.identity_file = Some("C:\\Users\\사용자 이름\\.ssh\\id_ed25519".into());
        x.validate().unwrap();
        assert_eq!(x.target(), "h:2222");
        x.user = Some("u".into());
        x.host = "fe80::1".into();
        assert_eq!(x.target(), "u@[fe80::1]:2222");
    }

    #[test]
    fn store_add_list_remove_and_unknown_fields() {
        let dir = std::env::temp_dir().join(format!(
            "pod-remote-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let s = MachineStore::at(dir.join("sub").join("machines.json"));
        assert!(s.load().unwrap().is_empty(), "missing file = no machines");
        let mut a = m("a", "host-a");
        a.user = Some("alice".into());
        a.port = Some(22);
        s.add(a.clone()).unwrap();
        s.add(m("b", "host-b")).unwrap();
        assert!(s.add(m("a", "other")).is_err(), "duplicate name");
        assert!(
            s.add(m("c", "-oProxyCommand=x")).is_err(),
            "validated on add"
        );
        assert_eq!(s.get("a").unwrap(), a);
        assert_eq!(s.load().unwrap().len(), 2);
        assert!(s.remove("a").unwrap());
        assert!(!s.remove("a").unwrap());
        assert!(s.get("a").is_err());
        // No tmp files left behind.
        let names: Vec<_> = std::fs::read_dir(dir.join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["machines.json"]);

        // A hand-added SSH option is refused, not ignored.
        std::fs::write(
            s.path(),
            r#"{"version":1,"machines":[{"name":"x","host":"h","proxy_command":"calc"}]}"#,
        )
        .unwrap();
        assert!(s.load().unwrap_err().contains("proxy_command"));
        // A hand-edited bad host is refused when used.
        std::fs::write(
            s.path(),
            r#"{"version":1,"machines":[{"name":"x","host":"-oProxyCommand=calc"}]}"#,
        )
        .unwrap();
        assert!(s.get("x").is_err());
        // Garbage is an error, and add() does not overwrite it.
        std::fs::write(s.path(), "not json").unwrap();
        assert!(s.add(m("y", "h")).is_err());
        assert_eq!(std::fs::read_to_string(s.path()).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
