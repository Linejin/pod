//! Codex usage limits through the documented app-server method
//! `account/rateLimits/read` (<https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt>).
//!
//! One read = one short-lived `codex app-server` (stdio JSON-RPC):
//! `initialize` → `initialized` → `account/read {refreshToken:false}` →
//! `account/rateLimits/read {excludeResetCreditDetails:true}` (skipped when
//! `account/read` reports no sign-in or a non-ChatGPT login; an
//! `account/read` error does not block it).
//! Nothing else is called: no thread or turn (no model prompt), no login,
//! logout, reset-credit or email method.
//!
//! - **Executable.** [`resolve`] picks one native `codex.exe`, deterministically:
//!   `POD_CODEX` when it is an absolute existing `.exe`, else the first
//!   absolute PATH directory with `codex.exe`, or with the npm shim and the
//!   platform binary the shim itself would launch. `.cmd`/`.ps1` shims are
//!   never run (no shell, no `node`).
//! - **Process.** Hidden, in a kill-on-close job ([`pod_process::bounded::LineSession`]),
//!   working directory a Pod-owned empty folder (no repository's `.codex`
//!   config applies), one deadline for the whole read, stdout lines and
//!   total output capped. The session is dropped (tree killed) on return.
//! - **Data.** The user's existing Codex profile (`CODEX_HOME`) is reused as
//!   it is. From the replies only percentages, window lengths, reset times,
//!   limit names, the plan type and the reached state are kept
//!   ([`parse_rate_limits`]); the e-mail, ids and credit balance are dropped.
//!   Account identity is compared through an in-memory hash only.

use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pod_process::bounded::{LineLimits, LineSession, RecvError};
use serde_json::{Value, json};

use super::copy;
use super::model::{Bucket, QuotaWindow, ServiceId, Snapshot, SourceKind, WindowKind};

/// The whole read (start, three requests, network round trip).
pub const READ_DEADLINE: Duration = Duration::from_secs(25);
const LIMITS: LineLimits = LineLimits {
    max_line: 256 * 1024,
    max_total: 4 * 1024 * 1024,
};
/// Lines that are not the awaited reply (notifications) before giving up.
const MAX_SKIPPED_LINES: usize = 256;
const MAX_BUCKETS: usize = 8;
const MAX_NAME: usize = 40;
const RESETS_AT_RANGE: std::ops::RangeInclusive<i64> = 1_000_000_000..=4_102_444_800;

/// The executable to run and, for an npm install, the package root the shim
/// would announce to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexExe {
    pub path: PathBuf,
    /// Arguments before `app-server` (empty for Codex; the tests' mock server
    /// is a script run by an interpreter).
    pub pre_args: Vec<OsString>,
    pub npm_root: Option<PathBuf>,
}

fn target() -> (&'static str, &'static str) {
    if cfg!(target_arch = "aarch64") {
        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
    } else {
        ("codex-win32-x64", "x86_64-pc-windows-msvc")
    }
}

fn is_exe(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// Pick the native Codex executable (see the module docs). `exists` is the
/// file test (tests pass a fake file system).
pub fn resolve(
    override_exe: Option<OsString>,
    path_var: Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> Option<CodexExe> {
    if let Some(o) = override_exe.filter(|o| !o.is_empty()) {
        let p = PathBuf::from(o);
        if p.is_absolute() && is_exe(&p) && exists(&p) {
            return Some(CodexExe {
                path: p,
                pre_args: Vec::new(),
                npm_root: None,
            });
        }
        // An unusable override is not silently replaced by something else.
        return None;
    }
    let (pkg, triple) = target();
    for dir in std::env::split_paths(&path_var.unwrap_or_default()) {
        if !dir.is_absolute() {
            continue;
        }
        let direct = dir.join("codex.exe");
        if exists(&direct) {
            return Some(CodexExe {
                path: direct,
                pre_args: Vec::new(),
                npm_root: None,
            });
        }
        if exists(&dir.join("codex.cmd")) || exists(&dir.join("codex")) {
            let root = dir.join("node_modules").join("@openai").join("codex");
            for vendor in [
                root.join("node_modules")
                    .join("@openai")
                    .join(pkg)
                    .join("vendor"),
                root.join("vendor"),
            ] {
                let exe = vendor.join(triple).join("bin").join("codex.exe");
                if exists(&exe) {
                    return Some(CodexExe {
                        path: exe,
                        pre_args: Vec::new(),
                        npm_root: Some(root),
                    });
                }
            }
        }
    }
    None
}

/// What one read found.
#[derive(Clone, Debug, PartialEq)]
pub enum CodexRead {
    NotInstalled,
    /// No account signed in (`account/read` returned none).
    NotLoggedIn,
    /// Signed in another way; the sentence says which.
    Unsupported(String),
    Ok {
        snapshot: Snapshot,
        /// In-memory hash of the account (user id), for switch detection.
        account: Option<u64>,
    },
    /// The read failed. `account`: the account seen before the failure, if any.
    Failed {
        error: String,
        account: Option<u64>,
    },
}

/// The account kind from `account/read`; the e-mail is never read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Account {
    ChatGpt { plan: Option<String> },
    ApiKey,
    Other,
    None,
}

fn id_hash(s: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `account/read` result → kind and an account hash (from `userId`).
pub fn parse_account(result: &Value) -> (Account, Option<u64>) {
    let user = result
        .get("userId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(id_hash);
    let Some(acc) = result.get("account").filter(|a| a.is_object()) else {
        return (Account::None, None);
    };
    let kind = match acc.get("type").and_then(Value::as_str) {
        Some("chatgpt") => Account::ChatGpt {
            plan: acc
                .get("planType")
                .and_then(Value::as_str)
                .and_then(plan_label),
        },
        Some("apiKey") => Account::ApiKey,
        _ => Account::Other,
    };
    (kind, user)
}

/// Display names of the documented plan types; unknown codes are not shown.
pub fn plan_label(code: &str) -> Option<String> {
    Some(
        match code {
            "free" => "Free",
            "go" => "Go",
            "plus" => "Plus",
            "pro" => "Pro",
            "prolite" => "Pro Lite",
            "promax" => "Pro Max",
            "team" => "Team",
            "business" | "self_serve_business_prolite" | "self_serve_business_usage_based" => {
                "Business"
            }
            "enterprise" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" => {
                "Enterprise"
            }
            "edu" | "edu_plus" | "edu_pro" => "Edu",
            _ => return None,
        }
        .to_string(),
    )
}

fn clean_name(s: &str) -> Option<String> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn window(v: Option<&Value>) -> Option<QuotaWindow> {
    let w = v?.as_object()?;
    let used = w.get("usedPercent")?.as_f64()?;
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    let mins = w
        .get("windowDurationMins")
        .and_then(Value::as_u64)
        .filter(|m| (1..=527_040).contains(m))
        .map(|m| m as u32);
    let resets_at = w
        .get("resetsAt")
        .and_then(|r| r.as_i64().or_else(|| r.as_f64().map(|f| f as i64)))
        .filter(|r| RESETS_AT_RANGE.contains(r));
    Some(QuotaWindow {
        kind: WindowKind::from_minutes(mins),
        used_percent: used,
        resets_at,
    })
}

fn bucket(snap: &Value, main: bool) -> Option<Bucket> {
    let o = snap.as_object()?;
    let windows: Vec<QuotaWindow> = [o.get("primary"), o.get("secondary")]
        .into_iter()
        .filter_map(window)
        .collect();
    let name = if main {
        None
    } else {
        o.get("limitName")
            .and_then(Value::as_str)
            .and_then(clean_name)
            .or_else(|| {
                o.get("limitId")
                    .and_then(Value::as_str)
                    .and_then(clean_name)
            })
    };
    let reached = o
        .get("rateLimitReachedType")
        .and_then(Value::as_str)
        .is_some();
    Some(Bucket {
        name,
        windows,
        reached,
    })
}

/// `account/rateLimits/read` result → snapshot (allowed fields only) and an
/// account hash (from `accountId`). `Err` when the reply has no usable view.
pub fn parse_rate_limits(
    result: &Value,
    plan: Option<String>,
    now_ms: u64,
) -> Result<(Snapshot, Option<u64>), String> {
    let mut buckets = Vec::new();
    let by_id = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty());
    match by_id {
        Some(map) => {
            // The `codex` bucket is the main one; the others follow by id.
            let mut ids: Vec<&String> = map.keys().collect();
            ids.sort_by_key(|id| (id.as_str() != "codex", id.as_str()));
            for (i, id) in ids.into_iter().take(MAX_BUCKETS).enumerate() {
                if let Some(b) = map.get(id).and_then(|s| bucket(s, i == 0)) {
                    buckets.push(b);
                }
            }
        }
        None => {
            let single = result
                .get("rateLimits")
                .and_then(|s| bucket(s, true))
                .ok_or_else(|| "rateLimits 없음".to_string())?;
            buckets.push(single);
        }
    }
    buckets.retain(|b| !b.windows.is_empty() || b.reached);
    let plan = plan.or_else(|| {
        result
            .get("rateLimits")
            .and_then(|r| r.get("planType"))
            .and_then(Value::as_str)
            .and_then(plan_label)
    });
    let account = result
        .get("accountId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(id_hash);
    Ok((
        Snapshot {
            service: ServiceId::Codex,
            source: SourceKind::CodexAppServer,
            profile: None,
            observed_ms: now_ms,
            plan,
            buckets,
        },
        account,
    ))
}

/// A short, safe description of a JSON-RPC error (no payload dump).
fn rpc_error(e: &Value) -> String {
    let msg = e.get("message").and_then(Value::as_str).unwrap_or("error");
    let msg: String = msg.chars().filter(|c| !c.is_control()).take(120).collect();
    msg
}

/// A JSON-RPC client over one [`LineSession`] with a shared deadline.
struct Rpc {
    s: LineSession,
    deadline: Instant,
    next: u64,
}

impl Rpc {
    fn notify(&mut self, method: &str) -> Result<(), String> {
        let line = json!({ "method": method }).to_string();
        self.s
            .send_line(&line)
            .map_err(|_| copy::read_failed("app-server 입력 실패"))
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let line = json!({ "id": id, "method": method, "params": params }).to_string();
        self.s
            .send_line(&line)
            .map_err(|_| copy::read_failed("app-server 입력 실패"))?;
        for _ in 0..MAX_SKIPPED_LINES {
            let line = self.s.recv_line(self.deadline).map_err(|e| {
                copy::read_failed(match e {
                    RecvError::TimedOut => "응답 시간 초과",
                    RecvError::Closed => "app-server가 종료됨",
                    RecvError::TooLarge => "응답이 너무 큼",
                })
            })?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // notifications and other replies
            }
            if let Some(e) = v.get("error") {
                return Err(copy::read_failed(&format!("{method}: {}", rpc_error(e))));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(copy::read_failed("응답을 찾지 못함"))
    }
}

/// Run one read with `exe`, in `cwd`, against the user's own profile.
pub fn read_with(exe: &CodexExe, cwd: &Path, now_ms: impl Fn() -> u64) -> CodexRead {
    let failed = |error: String| CodexRead::Failed {
        error,
        account: None,
    };
    if std::fs::create_dir_all(cwd).is_err() {
        return failed(copy::read_failed("작업 폴더를 만들 수 없음"));
    }
    let mut cmd = Command::new(&exe.path);
    cmd.args(&exe.pre_args).arg("app-server").current_dir(cwd);
    if let Some(root) = &exe.npm_root {
        // What the npm shim tells the binary (update hints only).
        cmd.env("CODEX_MANAGED_BY_NPM", "1")
            .env("CODEX_MANAGED_PACKAGE_ROOT", root);
    }
    let session = match LineSession::spawn(&mut cmd, LIMITS) {
        Ok(s) => s,
        Err(_) => return failed(copy::read_failed("Codex를 시작하지 못함")),
    };
    let mut rpc = Rpc {
        s: session,
        deadline: Instant::now() + READ_DEADLINE,
        next: 1,
    };
    let init = rpc.call(
        "initialize",
        json!({
            "clientInfo": {"name": "pod_usage_monitor", "title": "Pod", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": null
        }),
    );
    if let Err(e) = init.and_then(|_| rpc.notify("initialized")) {
        return failed(e);
    }
    // Account status is optional: it tells a sign-out or an API-key login
    // apart before the limits read, but a failing `account/read` (e.g. a
    // transient workspace lookup) does not block the documented limits read.
    let (plan, user) = match rpc.call("account/read", json!({ "refreshToken": false })) {
        Ok(r) => match parse_account(&r) {
            (Account::None, _) => return CodexRead::NotLoggedIn,
            (Account::ApiKey, _) => {
                return CodexRead::Unsupported(copy::HINT_CODEX_API_KEY.into());
            }
            (Account::Other, _) => {
                return CodexRead::Unsupported(copy::HINT_CODEX_OTHER_AUTH.into());
            }
            (Account::ChatGpt { plan }, user) => (plan, user),
        },
        Err(_) => (None, None),
    };
    match rpc.call(
        "account/rateLimits/read",
        json!({ "excludeResetCreditDetails": true }),
    ) {
        Ok(r) => match parse_rate_limits(&r, plan, now_ms()) {
            Ok((snapshot, acc)) => CodexRead::Ok {
                snapshot,
                account: user.or(acc),
            },
            Err(e) => CodexRead::Failed {
                error: copy::read_failed(&e),
                account: user,
            },
        },
        Err(error) => CodexRead::Failed {
            error,
            account: user,
        },
    }
}

/// The installed Codex, read by the documented method.
pub struct RealCodex {
    /// Pod-owned empty working directory.
    pub cwd: PathBuf,
    pub clock: super::Clock,
}

impl super::CodexProbe for RealCodex {
    fn read(&self) -> CodexRead {
        let exe = resolve(
            std::env::var_os("POD_CODEX"),
            std::env::var_os("PATH"),
            Path::is_file,
        );
        match exe {
            None => CodexRead::NotInstalled,
            Some(exe) => read_with(&exe, &self.cwd, || (self.clock)()),
        }
    }
}

/// The working directory Pod runs `codex app-server` in.
pub fn default_cwd() -> PathBuf {
    pod_status::usage::usage_dir().join("codex-cwd")
}
