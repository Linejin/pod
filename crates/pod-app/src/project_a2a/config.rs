//! Project-scoped A2A peer definitions: `<app config dir>/a2a-peers.json`.
//!
//! The file is the user's own app configuration, never repository content: a
//! project cannot add a peer by committing a file. Peers are listed per
//! registered repository name; a peer of one project is never offered for
//! another. Example:
//!
//! ```json
//! {
//!   "version": 1,
//!   "projects": {
//!     "sample": {
//!       "repo_path": "C:/src/sample",
//!       "peers": [
//!         { "id": "reviewer", "label": "Review agent", "peer": "https://agent.example.com" },
//!         { "id": "local", "card_url": "http://127.0.0.1:8000/.well-known/agent-card.json",
//!           "allow_http_loopback": true, "bearer_token_env": "LOCAL_AGENT_TOKEN" }
//!       ]
//!     }
//!   }
//! }
//! ```
//!
//! Unknown fields are refused (a misspelt switch must not silently fall back
//! to a default). URLs are checked with the client's own endpoint policy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use pod_a2a::target::{Endpoint, TargetPolicy};

pub const FILE_NAME: &str = "a2a-peers.json";
/// Larger files are not read.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Peers listed per project.
pub const MAX_PEERS: usize = 32;
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 80;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, ProjectEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectEntry {
    /// When set, the peers apply only while the registered repository with
    /// this name lives at this path (a re-registered name elsewhere gets none).
    #[serde(default)]
    repo_path: Option<String>,
    #[serde(default)]
    peers: Vec<PeerDef>,
}

/// One configured peer (an Agent Card source plus client switches).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerDef {
    /// Stable id within the project: `[A-Za-z0-9._-]`, at most 64.
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Peer base URL: the card is `{peer}/.well-known/agent-card.json`.
    #[serde(default)]
    pub peer: Option<String>,
    /// Explicit Agent Card URL (instead of `peer`).
    #[serde(default)]
    pub card_url: Option<String>,
    /// An interface URL the card advertises on another origin (as `--endpoint`).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Plain http to loopback only (local development).
    #[serde(default)]
    pub allow_http_loopback: bool,
    /// HTTP Bearer token from this environment variable (read per request).
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    /// HTTP Bearer token from this file (read per request, at most 16 KiB).
    #[serde(default)]
    pub bearer_token_file: Option<PathBuf>,
}

impl PeerDef {
    /// What people see: the label, else the id.
    pub fn display(&self) -> String {
        match &self.label {
            Some(l) if !l.trim().is_empty() => {
                format!(
                    "{} ({})",
                    pod_a2a::sanitize_line(l.trim(), MAX_LABEL),
                    self.id
                )
            }
            _ => self.id.clone(),
        }
    }

    pub fn policy(&self) -> TargetPolicy {
        TargetPolicy {
            allow_http_loopback: self.allow_http_loopback,
        }
    }

    /// The card source as configured.
    pub fn source(&self) -> pod_a2a::CardSource {
        match (&self.card_url, &self.peer) {
            (Some(c), _) => pod_a2a::CardSource::CardUrl(c.clone()),
            (None, Some(p)) => pod_a2a::CardSource::Peer(p.clone()),
            (None, None) => pod_a2a::CardSource::Peer(String::new()),
        }
    }

    /// The configured URL (for the review and the card line).
    pub fn source_url(&self) -> &str {
        self.card_url
            .as_deref()
            .or(self.peer.as_deref())
            .unwrap_or_default()
    }

    pub fn has_token(&self) -> bool {
        self.bearer_token_env.is_some() || self.bearer_token_file.is_some()
    }

    /// Why this definition cannot be used, or `None`.
    fn problem(&self) -> Option<String> {
        if self.id.is_empty()
            || self.id.len() > MAX_ID
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Some(format!(
                "id는 영문·숫자·._- 1~{MAX_ID}자여야 합니다: {:?}",
                pod_a2a::sanitize_line(&self.id, 80)
            ));
        }
        let url = match (&self.peer, &self.card_url) {
            (Some(_), Some(_)) => return Some("peer와 card_url 중 하나만 적으세요".into()),
            (None, None) => return Some("peer 또는 card_url이 필요합니다".into()),
            (Some(u), None) | (None, Some(u)) => u,
        };
        if let Err(e) = Endpoint::parse(url, self.policy()) {
            return Some(e.to_string());
        }
        if let Some(e) = &self.endpoint
            && let Err(e) = Endpoint::parse(e, self.policy())
        {
            return Some(format!("endpoint: {e}"));
        }
        if self.bearer_token_env.is_some() && self.bearer_token_file.is_some() {
            return Some("bearer_token_env와 bearer_token_file 중 하나만 적으세요".into());
        }
        if self
            .bearer_token_env
            .as_deref()
            .is_some_and(|n| n.is_empty() || n.contains('='))
        {
            return Some("bearer_token_env 이름이 올바르지 않습니다".into());
        }
        None
    }
}

/// A configured peer and, when it cannot be used, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSlot {
    pub def: PeerDef,
    pub problem: Option<String>,
}

/// The peers of one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectPeers {
    /// No configuration file.
    NoFile,
    /// The file exists but cannot be used at all.
    Invalid(String),
    /// The file has no entry (or an empty list) for this project.
    NotConfigured,
    /// The entry is bound to another repository path.
    OtherPath {
        configured: String,
    },
    Ready(Vec<PeerSlot>),
}

impl ProjectPeers {
    pub fn slots(&self) -> &[PeerSlot] {
        match self {
            Self::Ready(s) => s,
            _ => &[],
        }
    }

    /// The usable definition `id` of this project.
    pub fn usable(&self, id: &str) -> Option<&PeerDef> {
        self.slots()
            .iter()
            .find(|s| s.def.id == id && s.problem.is_none())
            .map(|s| &s.def)
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads the peers of `repo` (registered at `repo_path`) from `dir`.
pub fn load(dir: &Path, repo: &str, repo_path: Option<&str>) -> ProjectPeers {
    let path = path_in(dir);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ProjectPeers::NoFile,
        Err(e) => return ProjectPeers::Invalid(format!("{}: {e}", path.display())),
    };
    if meta.len() > MAX_FILE_BYTES {
        return ProjectPeers::Invalid(format!(
            "{}: {MAX_FILE_BYTES}바이트보다 큽니다",
            path.display()
        ));
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, repo, repo_path),
        Err(e) => ProjectPeers::Invalid(format!("{}: {e}", path.display())),
    }
}

/// The peers of `repo` in the file text `text`.
pub fn parse(text: &str, repo: &str, repo_path: Option<&str>) -> ProjectPeers {
    let file: File = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => return ProjectPeers::Invalid(format!("{FILE_NAME}: {e}")),
    };
    if file.version != 1 {
        return ProjectPeers::Invalid(format!(
            "{FILE_NAME}: version {}은 지원하지 않습니다 (1만 지원)",
            file.version
        ));
    }
    let Some(entry) = file.projects.get(repo) else {
        return ProjectPeers::NotConfigured;
    };
    if let Some(want) = &entry.repo_path
        && repo_path.is_none_or(|have| !same_path(want, have))
    {
        return ProjectPeers::OtherPath {
            configured: want.clone(),
        };
    }
    if entry.peers.is_empty() {
        return ProjectPeers::NotConfigured;
    }
    if entry.peers.len() > MAX_PEERS {
        return ProjectPeers::Invalid(format!(
            "{FILE_NAME}: {repo}의 피어가 {MAX_PEERS}개보다 많습니다"
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let slots = entry
        .peers
        .iter()
        .map(|def| {
            let problem = def.problem().or_else(|| {
                (!seen.insert(def.id.clone())).then(|| format!("id {}가 중복됩니다", def.id))
            });
            PeerSlot {
                def: def.clone(),
                problem,
            }
        })
        .collect();
    ProjectPeers::Ready(slots)
}

/// Same repository path, ignoring separators, a trailing slash and (on
/// Windows) case.
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}
