//! Mobile approval, daemon side (docs/17 §4, step M1). **No network
//! listener**: this is the model a future LAN/relay channel will sit on.
//!
//! - [`registry`]: paired devices, per-device per-task capabilities
//!   (deny by default), persisted in `<state_dir>\mobile\devices.json`.
//! - [`session`]: in-memory sessions (8 h / 15 min idle, revoke = epoch
//!   bump), the clock (fake in tests) and rate limits.
//! - [`audit`]: bounded append-only JSONL log.
//! - [`message`]: validation and framing of short text messages.
//! - [`api`]: the closed set of operations a phone can call.
//!
//! The PC manages devices through the `mobile.*` pipe methods here
//! (`pod mobile ...`). Pairing is M2/M4; until then a device can only be
//! created by the pairing stub, available in tests and in debug builds with
//! `POD_MOBILE_TEST_HOOKS=1` (pipe methods `mobile.test.*`).

pub mod api;
pub mod audit;
pub mod message;
pub mod registry;
pub mod session;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{Daemon, log, params, to_value};
use crate::protocol::*;
use audit::AuditLog;
use registry::Registry;
use session::{Clock, Limits, Sessions};

/// Test-only pipe methods (debug builds + `POD_MOBILE_TEST_HOOKS=1`).
pub const TEST_PAIR: &str = "mobile.test.pair";
pub const TEST_SESSION: &str = "mobile.test.session";
pub const TEST_CALL: &str = "mobile.test.call";

pub struct MobileState {
    pub registry: Registry,
    pub sessions: Sessions,
    pub limits: Limits,
}

pub struct Mobile {
    pub state: Mutex<MobileState>,
    pub audit: AuditLog,
    pub clock: Arc<dyn Clock>,
    state_dir: PathBuf,
    test_hooks: bool,
}

impl Mobile {
    pub fn load(state_dir: &Path, clock: Arc<dyn Clock>) -> Self {
        let boot = clock.now_ms();
        Self {
            state: Mutex::new(MobileState {
                registry: Registry::load(state_dir),
                sessions: Sessions::new(boot),
                limits: Limits::default(),
            }),
            audit: AuditLog::new(state_dir),
            clock,
            state_dir: state_dir.to_path_buf(),
            test_hooks: cfg!(debug_assertions)
                && std::env::var("POD_MOBILE_TEST_HOOKS").is_ok_and(|v| v == "1"),
        }
    }

    /// An audit entry stamped now.
    pub fn entry(
        &self,
        actor: &str,
        action: &str,
        result: Result<&str, &RpcError>,
    ) -> MobileAuditEntry {
        let (result, code, detail) = match result {
            Ok(r) => (r.to_string(), None, None),
            Err(e)
                if e.code == mobile_codes::MOBILE_DENIED
                    || e.code == mobile_codes::MOBILE_AUTH
                    || e.code == mobile_codes::RATE_LIMITED =>
            {
                ("denied".to_string(), Some(e.code), Some(e.message.clone()))
            }
            Err(e) => ("error".to_string(), Some(e.code), Some(e.message.clone())),
        };
        MobileAuditEntry {
            ts_ms: self.clock.now_ms(),
            actor: actor.to_string(),
            action: action.chars().take(64).collect(),
            result,
            code,
            detail,
            ..Default::default()
        }
    }

    /// Append, logging (not failing) on error. For records that must exist
    /// before acting, call `audit.append` and refuse on error instead.
    pub fn record(&self, e: MobileAuditEntry) {
        if let Err(io) = self.audit.append(&e) {
            log(&format!("mobile audit: cannot write ({io}): {e:?}"));
        }
    }

    fn save(&self, st: &mut MobileState) -> Result<(), RpcError> {
        st.registry.save(&self.state_dir)
    }
}

pub fn handles(method: &str) -> bool {
    MOBILE_METHODS.contains(&method) || method.starts_with("mobile.test.")
}

pub async fn dispatch(daemon: Arc<Daemon>, method: &str, p: Value) -> Result<Value, RpcError> {
    match method {
        MobileDevices::NAME => {
            let _: MobileDevicesParams = params(p)?;
            to_value(daemon.mobile_devices())
        }
        MobileRevoke::NAME => to_value(daemon.mobile_revoke(params(p)?)?),
        MobilePolicy::NAME => to_value(daemon.mobile_policy(params(p)?)?),
        MobileAudit::NAME => {
            let p: MobileAuditParams = params(p)?;
            to_value(MobileAuditResult {
                entries: daemon
                    .mobile
                    .audit
                    .read(p.limit.unwrap_or(100), p.device.as_deref()),
            })
        }
        MobileSettingsMethod::NAME => to_value(daemon.mobile_settings(params(p)?)?),
        m if daemon.mobile.test_hooks => test_hooks::dispatch(&daemon, m, p).await,
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}

impl Daemon {
    fn mobile_devices(&self) -> MobileDevicesResult {
        let st = self.mobile.state.lock().unwrap();
        MobileDevicesResult {
            listener: "off".into(),
            devices: st
                .registry
                .devices
                .iter()
                .map(|d| d.info(st.sessions.count(&d.id)))
                .collect(),
            settings: st.registry.settings.clone(),
        }
    }

    /// Revoke one or all devices. Sessions end at once; the revocation also
    /// stays in effect in memory when it cannot be saved (fail safe), and
    /// that is reported as an error.
    fn mobile_revoke(&self, p: MobileRevokeParams) -> Result<MobileRevokeResult, RpcError> {
        let now = self.mobile.clock.now_ms();
        let mut st = self.mobile.state.lock().unwrap();
        let ids: Vec<String> = match (&p.device, p.all) {
            (Some(d), false) => vec![d.clone()],
            (None, true) => st
                .registry
                .devices
                .iter()
                .filter(|d| d.active())
                .map(|d| d.id.clone())
                .collect(),
            _ => {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "give either device or all",
                ));
            }
        };
        let mut revoked = Vec::new();
        for id in &ids {
            if st.registry.revoke(id, now)? {
                revoked.push(id.clone());
            }
            st.sessions.drop_device(id);
        }
        let saved = self.mobile.save(&mut st);
        drop(st);
        for id in &revoked {
            let mut e = self
                .mobile
                .entry("local", "revoke", saved.as_ref().map(|()| "ok"));
            e.detail = Some(format!("device {id}"));
            self.mobile.record(e);
            log(&format!("mobile: device {id} revoked"));
        }
        saved.map_err(|e| {
            RpcError::new(
                e.code,
                format!("revoked for this daemon run, but {}", e.message),
            )
        })?;
        Ok(MobileRevokeResult { revoked })
    }

    fn mobile_policy(&self, p: MobilePolicyParams) -> Result<MobileDeviceInfo, RpcError> {
        let task = self.task_record(&p.task)?.id;
        let mut st = self.mobile.state.lock().unwrap();
        let before = st.registry.device(&p.device).cloned();
        let d = st
            .registry
            .set_policy(&p.device, &task, &p.allow, &p.deny, p.clear)?;
        if let Err(e) = self.mobile.save(&mut st) {
            if let (Some(b), Ok(slot)) = (before, st.registry.device_mut(&p.device)) {
                *slot = b;
            }
            return Err(e);
        }
        let info = d.info(st.sessions.count(&d.id));
        drop(st);
        let caps: Vec<&str> = info
            .policy
            .get(&task)
            .map(|c| c.iter().map(|c| c.as_str()).collect())
            .unwrap_or_default();
        let mut e = self.mobile.entry("local", "policy", Ok("ok"));
        e.task = Some(task.clone());
        e.detail = Some(format!("device {}: [{}]", p.device, caps.join(",")));
        self.mobile.record(e);
        Ok(info)
    }

    fn mobile_settings(&self, p: MobileSettingsParams) -> Result<MobileSettings, RpcError> {
        let mut st = self.mobile.state.lock().unwrap();
        let before = st.registry.settings.clone();
        if let Some(b) = p.audit_message_bodies {
            st.registry.settings.audit_message_bodies = b;
        }
        if let Err(e) = self.mobile.save(&mut st) {
            st.registry.settings = before;
            return Err(e);
        }
        let s = st.registry.settings.clone();
        drop(st);
        if s != before {
            let mut e = self.mobile.entry("local", "settings", Ok("ok"));
            e.detail = Some(format!("audit_message_bodies = {}", s.audit_message_bodies));
            self.mobile.record(e);
        }
        Ok(s)
    }
}

#[cfg(debug_assertions)]
mod test_hooks {
    use super::*;

    #[derive(serde::Deserialize)]
    struct PairParams {
        name: String,
        #[serde(default)]
        public_key: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SessionParams {
        device: String,
    }

    #[derive(serde::Deserialize)]
    struct CallParams {
        session: String,
        method: String,
        #[serde(default)]
        params: Value,
        #[serde(default = "yes")]
        user_initiated: bool,
    }

    fn yes() -> bool {
        true
    }

    pub async fn dispatch(daemon: &Arc<Daemon>, method: &str, p: Value) -> Result<Value, RpcError> {
        let m = &daemon.mobile;
        match method {
            TEST_PAIR => {
                let p: PairParams = params(p)?;
                let now = m.clock.now_ms();
                let mut st = m.state.lock().unwrap();
                let d = st.registry.pair_stub(
                    &p.name,
                    p.public_key.as_deref().unwrap_or("placeholder"),
                    now,
                )?;
                if let Err(e) = m.save(&mut st) {
                    st.registry.devices.pop();
                    return Err(e);
                }
                drop(st);
                let mut e = m.entry("local", "pair", Ok("ok"));
                e.detail = Some(format!("device {} ({}) via the test stub", d.id, d.name));
                m.record(e);
                to_value(d.info(0))
            }
            TEST_SESSION => {
                let p: SessionParams = params(p)?;
                let now = m.clock.now_ms();
                let mut st = m.state.lock().unwrap();
                let s = &mut *st;
                let id = s.sessions.open(&s.registry, &p.device, now)?;
                Ok(serde_json::json!({ "session": id }))
            }
            TEST_CALL => {
                let p: CallParams = params(p)?;
                let ctx = api::MobileCtx {
                    session: p.session,
                    user_initiated: p.user_initiated,
                };
                daemon.mobile_call(&ctx, &p.method, p.params).await
            }
            other => Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("unknown method {other:?}"),
            )),
        }
    }
}

#[cfg(not(debug_assertions))]
mod test_hooks {
    use super::*;

    pub async fn dispatch(_: &Arc<Daemon>, method: &str, _: Value) -> Result<Value, RpcError> {
        Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {method:?}"),
        ))
    }
}
