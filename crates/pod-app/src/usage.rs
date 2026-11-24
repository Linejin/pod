//! AI usage monitor (docs/usage/ai-usage-monitor.md): how much of each connected
//! AI service's usage limit is left and when it is due to reset, always in
//! the status bar, with details in a popover.
//!
//! - Sources, documented and machine-readable only: Codex app-server
//!   `account/rateLimits/read` ([`codex`]) and Claude Code status-line
//!   samples collected by `pod-hook statusline` ([`claude`]). Other agents Pod
//!   recognises (Gemini, Aider, …) are listed as detected but unsupported.
//! - [`UsageMonitor`] is one app-wide entity ([`MonitorGlobal`]): every
//!   window's status bar shows it, and each provider has a single read in
//!   flight at a time. Reads run on background threads; the UI thread only
//!   applies their results.
//! - Cadence ([`Cadence`]): Codex every 5 minutes, backing off on errors up to
//!   an hour; the Claude cache (a local file) every 30 seconds. "새로고침"
//!   reads at once (at most every 10 s). The countdown is local: the monitor
//!   repaints at minute boundaries, never every second.
//! - Tests and the visual fixture use fake probes ([`UsageBackend::fixed`]);
//!   without a backend (`StartOptions::usage == None`) there is no monitor.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

pub mod claude;
pub mod codex;
pub(crate) mod copy;
pub mod model;
mod view;

#[cfg(test)]
mod tests;

use claude::ClaudeRead;
use codex::CodexRead;
use model::{Provider, ServiceId, ServiceView};

/// Unix milliseconds.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Reads Codex limits (blocking; runs on a background thread).
pub trait CodexProbe: Send + Sync {
    fn read(&self) -> CodexRead;
}

/// Reads the Claude status-line cache and settings (blocking).
pub trait ClaudeProbe: Send + Sync {
    fn read(&self) -> ClaudeRead;
    /// Remove Pod's own record of `profile`.
    fn forget(&self, profile: &str) -> Result<(), String>;
}

/// How often to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    pub codex_ms: u64,
    pub claude_ms: u64,
    pub max_backoff_ms: u64,
    /// Least time between two manual refreshes.
    pub manual_gap_ms: u64,
}

impl Default for Cadence {
    fn default() -> Self {
        Self {
            codex_ms: 5 * 60_000,
            claude_ms: 30_000,
            max_backoff_ms: 60 * 60_000,
            manual_gap_ms: 10_000,
        }
    }
}

/// Startup option: where the monitor reads from.
#[derive(Clone)]
pub struct UsageBackend {
    pub codex: Option<Arc<dyn CodexProbe>>,
    pub claude: Option<Arc<dyn ClaudeProbe>>,
    pub clock: Clock,
    pub cadence: Cadence,
}

impl std::fmt::Debug for UsageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageBackend")
            .field("codex", &self.codex.is_some())
            .field("claude", &self.claude.is_some())
            .finish()
    }
}

fn system_clock() -> Clock {
    Arc::new(pod_status::usage::now_ms)
}

impl UsageBackend {
    /// The installed CLIs and Pod's cache (the app).
    pub fn system() -> Self {
        let clock = system_clock();
        Self {
            codex: Some(Arc::new(codex::RealCodex {
                cwd: codex::default_cwd(),
                clock: clock.clone(),
            })),
            claude: Some(Arc::new(claude::RealClaude {
                usage_dir: pod_status::usage::usage_dir(),
            })),
            clock,
            cadence: Cadence::default(),
        }
    }

    /// Fixed answers, no process and no network (tests, `--ui-fixture`).
    pub fn fixed(codex: Option<CodexRead>, claude: Option<ClaudeRead>, clock: Clock) -> Self {
        Self {
            codex: codex.map(|r| Arc::new(Fixed(r)) as Arc<dyn CodexProbe>),
            claude: claude.map(|r| Arc::new(Fixed(r)) as Arc<dyn ClaudeProbe>),
            clock,
            cadence: Cadence::default(),
        }
    }

    /// The visual fixture (`--fixture-scene usage-*`): `usage-demo` shows
    /// sample numbers (clearly not live), any other scene nothing connected.
    pub fn fixture(scene: Option<&str>) -> Self {
        let clock = system_clock();
        let now = clock();
        match scene {
            Some("usage-demo") => Self::fixed(
                Some(samples::codex_ok(now)),
                Some(samples::claude_ok(now)),
                clock,
            ),
            _ => Self::fixed(
                Some(CodexRead::NotInstalled),
                Some(ClaudeRead {
                    records: Vec::new(),
                    collector: claude::Collector::None,
                    setup: None,
                }),
                clock,
            ),
        }
    }
}

struct Fixed<T>(T);

impl CodexProbe for Fixed<CodexRead> {
    fn read(&self) -> CodexRead {
        self.0.clone()
    }
}

impl ClaudeProbe for Fixed<ClaudeRead> {
    fn read(&self) -> ClaudeRead {
        self.0.clone()
    }
    fn forget(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// When to read next; one read in flight at a time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    pub next_ms: u64,
    pub failures: u32,
    pub in_flight: bool,
    pub last_start_ms: Option<u64>,
}

impl Schedule {
    /// Start a read now? Scheduled reads wait for `next_ms`; manual ones
    /// only for `gap` since the last start. Never two at once.
    pub fn try_start(&mut self, now: u64, manual: bool, gap: u64) -> bool {
        if self.in_flight {
            return false;
        }
        let ready = if manual {
            self.last_start_ms.is_none_or(|l| now >= l + gap)
        } else {
            now >= self.next_ms
        };
        if ready {
            self.in_flight = true;
            self.last_start_ms = Some(now);
        }
        ready
    }

    /// The read ended: next one after `base`, doubled per failure up to `max`.
    pub fn finish(&mut self, ok: bool, now: u64, base: u64, max: u64) {
        self.in_flight = false;
        if ok {
            self.failures = 0;
            self.next_ms = now + base;
        } else {
            self.failures = self.failures.saturating_add(1);
            let factor = 1u64 << self.failures.min(16);
            self.next_ms = now + base.saturating_mul(factor).min(max);
        }
    }
}

/// Codex: the provider state and the account it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct CodexState {
    pub provider: Provider,
    pub account: Option<u64>,
    pub schedule: Schedule,
}

impl Default for CodexState {
    fn default() -> Self {
        Self {
            provider: Provider::Pending,
            account: None,
            schedule: Schedule::default(),
        }
    }
}

impl CodexState {
    /// Apply one read. A failure keeps the previous numbers (marked) only
    /// when they belong to the same account; a sign-out, another account or
    /// an unsupported login drops them.
    pub fn apply(&mut self, read: CodexRead, now: u64, c: &Cadence) {
        let ok = !matches!(read, CodexRead::Failed { .. });
        match read {
            CodexRead::Ok { snapshot, account } => {
                self.account = account;
                self.provider = Provider::Ready(snapshot);
            }
            CodexRead::NotInstalled => {
                self.account = None;
                self.provider = Provider::NotInstalled;
            }
            CodexRead::NotLoggedIn => {
                self.account = None;
                self.provider = Provider::NotConnected;
            }
            CodexRead::Unsupported(why) => {
                self.account = None;
                self.provider = Provider::Unsupported(why);
            }
            CodexRead::Failed { error, account } => {
                let same = account.is_none() || account == self.account;
                let last = same.then(|| self.provider.snapshot().cloned()).flatten();
                if !same {
                    self.account = None;
                }
                self.provider = Provider::Failed {
                    error,
                    last,
                    at_ms: now,
                };
            }
        }
        self.schedule.finish(ok, now, c.codex_ms, c.max_backoff_ms);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeState {
    pub read: Option<ClaudeRead>,
    pub schedule: Schedule,
}

/// The app-wide monitor.
pub struct UsageMonitor {
    backend: UsageBackend,
    pub codex: CodexState,
    pub claude: ClaudeState,
    /// The minute last painted (repaint only when it changes).
    painted_minute: u64,
    _loop: Option<Task<()>>,
    _codex_read: Option<Task<()>>,
    _claude_read: Option<Task<()>>,
}

/// The app-wide monitor entity (set once, by the first workspace).
pub struct MonitorGlobal(pub Entity<UsageMonitor>);

impl Global for MonitorGlobal {}

/// Start the monitor once for the app (later windows reuse it).
pub fn ensure_started(backend: UsageBackend, cx: &mut App) {
    if cx.has_global::<MonitorGlobal>() {
        return;
    }
    let monitor = cx.new(|cx| {
        let mut m = UsageMonitor::new(backend);
        m.start(cx);
        m
    });
    cx.set_global(MonitorGlobal(monitor));
}

/// The monitor, when the app has one.
pub fn monitor(cx: &App) -> Option<Entity<UsageMonitor>> {
    cx.try_global::<MonitorGlobal>().map(|g| g.0.clone())
}

impl UsageMonitor {
    pub fn new(backend: UsageBackend) -> Self {
        Self {
            backend,
            codex: CodexState::default(),
            claude: ClaudeState::default(),
            painted_minute: 0,
            _loop: None,
            _codex_read: None,
            _claude_read: None,
        }
    }

    pub fn now(&self) -> u64 {
        (self.backend.clock)()
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self._loop = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(wait) = this.update(cx, |m, cx| m.tick(cx)) else {
                    return;
                };
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    /// Start due reads, repaint on a new minute; returns how long to sleep.
    fn tick(&mut self, cx: &mut Context<Self>) -> Duration {
        self.read_codex(false, cx);
        self.read_claude(false, cx);
        let now = self.now();
        let minute = now / 60_000;
        if minute != self.painted_minute {
            self.painted_minute = minute;
            cx.notify();
        }
        let mut wake = model::next_repaint_ms(now);
        if self.backend.codex.is_some() && !self.codex.schedule.in_flight {
            wake = wake.min(self.codex.schedule.next_ms);
        }
        if self.backend.claude.is_some() && !self.claude.schedule.in_flight {
            wake = wake.min(self.claude.schedule.next_ms);
        }
        Duration::from_millis(wake.saturating_sub(now).clamp(1_000, 60_000))
    }

    pub fn reading(&self) -> bool {
        self.codex.schedule.in_flight || self.claude.schedule.in_flight
    }

    /// "새로고침": read every source now (single-flight, rate-limited).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.read_codex(true, cx);
        self.read_claude(true, cx);
        cx.notify();
    }

    fn read_codex(&mut self, manual: bool, cx: &mut Context<Self>) {
        let Some(probe) = self.backend.codex.clone() else {
            return;
        };
        let (now, cad) = (self.now(), self.backend.cadence);
        if !self
            .codex
            .schedule
            .try_start(now, manual, cad.manual_gap_ms)
        {
            return;
        }
        self._codex_read = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_spawn(async move { probe.read() }).await;
            let _ = this.update(cx, |m, cx| {
                let now = m.now();
                m.codex.apply(read, now, &cad);
                cx.notify();
            });
        }));
    }

    fn read_claude(&mut self, manual: bool, cx: &mut Context<Self>) {
        let Some(probe) = self.backend.claude.clone() else {
            return;
        };
        let (now, cad) = (self.now(), self.backend.cadence);
        if !self
            .claude
            .schedule
            .try_start(now, manual, cad.manual_gap_ms.min(2_000))
        {
            return;
        }
        self._claude_read = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_spawn(async move { probe.read() }).await;
            let _ = this.update(cx, |m, cx| {
                let now = m.now();
                if m.claude.read.as_ref() != Some(&read) {
                    cx.notify();
                }
                m.claude.read = Some(read);
                m.claude
                    .schedule
                    .finish(true, now, cad.claude_ms, cad.max_backoff_ms);
            });
        }));
    }

    /// "기록 지우기": drop Pod's own Claude records, then re-read.
    pub fn forget_claude(&mut self, cx: &mut Context<Self>) {
        let Some(probe) = self.backend.claude.clone() else {
            return;
        };
        let profiles: Vec<String> = self
            .claude
            .read
            .iter()
            .flat_map(|r| r.records.iter().map(|rec| rec.profile.clone()))
            .collect();
        let task = cx.background_spawn(async move {
            for p in profiles {
                let _ = probe.forget(&p);
            }
        });
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |m, cx| {
                m.claude.schedule.last_start_ms = None;
                m.read_claude(true, cx);
            });
        })
        .detach();
    }

    /// Everything the monitor shows at `now`: Codex first, then each Claude
    /// profile (services that are not set up at all show their state).
    pub fn views(&self, now: u64) -> Vec<ServiceView> {
        let mut out = Vec::new();
        if self.backend.codex.is_some() {
            out.push(ServiceView::build(
                ServiceId::Codex,
                &self.codex.provider,
                now,
            ));
        }
        if self.backend.claude.is_some() {
            match &self.claude.read {
                None => out.push(ServiceView::build(
                    ServiceId::Claude,
                    &Provider::Pending,
                    now,
                )),
                Some(r) => out.extend(
                    claude::providers(r, now)
                        .iter()
                        .map(|p| ServiceView::build(ServiceId::Claude, p, now)),
                ),
            }
        }
        out
    }

    pub fn claude_setup(&self) -> Option<claude::Setup> {
        self.claude.read.as_ref().and_then(|r| r.setup.clone())
    }

    pub fn claude_collector(&self) -> Option<&claude::Collector> {
        self.claude.read.as_ref().map(|r| &r.collector)
    }

    pub fn has_claude_records(&self) -> bool {
        self.claude
            .read
            .as_ref()
            .is_some_and(|r| !r.records.is_empty())
    }
}

/// Sample answers for tests and the `usage-demo` fixture scene.
pub(crate) mod samples {
    use super::claude::{ClaudeRead, Collector, Setup};
    use super::codex::CodexRead;
    use super::model::{Bucket, QuotaWindow, ServiceId, Snapshot, SourceKind, WindowKind};
    use pod_status::usage::{
        ClaudeUsageRecord, LimitSample, RECORD_VERSION, SOURCE_CLAUDE_STATUSLINE,
    };

    pub fn codex_ok(now_ms: u64) -> CodexRead {
        let now_s = (now_ms / 1000) as i64;
        CodexRead::Ok {
            snapshot: Snapshot {
                service: ServiceId::Codex,
                source: SourceKind::CodexAppServer,
                profile: None,
                observed_ms: now_ms,
                plan: Some("Plus".into()),
                buckets: vec![Bucket {
                    name: None,
                    windows: vec![
                        QuotaWindow {
                            kind: WindowKind::FiveHour,
                            used_percent: 24.0,
                            resets_at: Some(now_s + 2 * 3600 + 13 * 60),
                        },
                        QuotaWindow {
                            kind: WindowKind::Weekly,
                            used_percent: 61.0,
                            resets_at: Some(now_s + 3 * 86_400 + 4 * 3600),
                        },
                    ],
                    reached: false,
                }],
            },
            account: Some(1),
        }
    }

    pub fn claude_record(now_ms: u64, five: f64, week: f64) -> ClaudeUsageRecord {
        let now_s = (now_ms / 1000) as i64;
        ClaudeUsageRecord {
            v: RECORD_VERSION,
            source: SOURCE_CLAUDE_STATUSLINE.into(),
            profile: "default".into(),
            profile_label: None,
            seen_at_ms: now_ms,
            limits_at_ms: Some(now_ms),
            five_hour: Some(LimitSample {
                used_percentage: five,
                resets_at: Some(now_s + 3600 + 5 * 60),
                period: None,
            }),
            seven_day: Some(LimitSample {
                used_percentage: week,
                resets_at: Some(now_s + 5 * 86_400),
                period: None,
            }),
            spend_limit: None,
        }
    }

    pub fn claude_ok(now_ms: u64) -> ClaudeRead {
        ClaudeRead {
            records: vec![claude_record(now_ms, 8.0, 35.0)],
            collector: Collector::Pod,
            setup: Some(Setup::new(
                std::path::Path::new(r"C:\Users\u\AppData\Local\Pod\bin\pod-hook.exe"),
                None,
            )),
        }
    }
}
