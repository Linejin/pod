//! The usage monitor's data, independent of GPUI: quota snapshots as the
//! providers report them, provider states, and what the status bar and the
//! details say about them at a given time ([`ServiceView`]).
//!
//! Rules (docs/usage/ai-usage-monitor.md):
//! - remaining = 100 − reported used percentage; nothing is estimated;
//! - a countdown is computed locally from the reported reset time, and when
//!   it passes the window is "reset time passed, waiting for a new report",
//!   never "reset";
//! - unlike quotas (5 h, weekly, per-model buckets, services) are never summed;
//!   the compact aggregate is the single most constrained window, named.

use super::copy;

/// The services with a documented machine-readable quota source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceId {
    Codex,
    Claude,
}

impl ServiceId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    pub fn mark(self) -> pod_ui::AgentMark {
        match self {
            Self::Codex => pod_ui::AgentMark::Codex,
            Self::Claude => pod_ui::AgentMark::Claude,
        }
    }
}

/// Which window a percentage belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    FiveHour,
    Weekly,
    /// A spend limit (Claude apps gateway) and its period, when reported.
    Spend(Option<String>),
    /// Any other reported window length (minutes), or none.
    Other(Option<u32>),
}

impl WindowKind {
    pub fn from_minutes(mins: Option<u32>) -> Self {
        match mins {
            Some(300) => Self::FiveHour,
            Some(10_080) => Self::Weekly,
            m => Self::Other(m),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::FiveHour => copy::WINDOW_5H.into(),
            Self::Weekly => copy::WINDOW_WEEKLY.into(),
            Self::Spend(p) => match p.as_deref() {
                Some("daily") => copy::SPEND_DAILY.into(),
                Some("weekly") => copy::SPEND_WEEKLY.into(),
                Some("monthly") => copy::SPEND_MONTHLY.into(),
                _ => copy::SPEND.into(),
            },
            Self::Other(Some(m)) => duration_label(u64::from(*m) * 60),
            Self::Other(None) => copy::WINDOW_UNKNOWN.into(),
        }
    }

    /// Shorter windows first when two are equally constrained.
    fn order(&self) -> u32 {
        match self {
            Self::FiveHour => 300,
            Self::Weekly => 10_080,
            Self::Spend(_) => u32::MAX - 1,
            Self::Other(Some(m)) => *m,
            Self::Other(None) => u32::MAX,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    pub kind: WindowKind,
    /// As reported: 0–100 (a spend limit may pass 100).
    pub used_percent: f64,
    /// Unix seconds, as reported.
    pub resets_at: Option<i64>,
}

impl QuotaWindow {
    pub fn remaining(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

/// One metered bucket (Codex `limitId`; Claude has one).
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    /// `None`: the service's main bucket.
    pub name: Option<String>,
    pub windows: Vec<QuotaWindow>,
    /// The provider says a limit in this bucket is reached.
    pub reached: bool,
}

/// Where a snapshot came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    CodexAppServer,
    ClaudeStatusLine,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CodexAppServer => copy::SOURCE_CODEX,
            Self::ClaudeStatusLine => copy::SOURCE_CLAUDE,
        }
    }
}

/// Allowed fields only: no token, email, account id, credit balance.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub service: ServiceId,
    pub source: SourceKind,
    /// A separate profile (Claude `CLAUDE_CONFIG_DIR` name), when not the default.
    pub profile: Option<String>,
    /// When the provider reported these numbers (Unix ms).
    pub observed_ms: u64,
    pub plan: Option<String>,
    pub buckets: Vec<Bucket>,
}

/// Why a service shows no numbers, or what it shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Provider {
    /// Not read yet (start-up).
    Pending,
    /// The CLI is not installed / not found.
    NotInstalled,
    /// Installed but not signed in / the collector is not connected.
    NotConnected,
    /// The account or auth mode has no quota source (API key, Bedrock…).
    Unsupported(String),
    /// Connected, waiting for the first sample (Claude: first API response).
    AwaitingFirst,
    /// The source reports but without limits (Claude: not a subscriber yet,
    /// or before the session's first response).
    NoLimits,
    Ready(Snapshot),
    /// The last read failed; `last` is the previous snapshot of the same account.
    Failed {
        error: String,
        last: Option<Snapshot>,
        at_ms: u64,
    },
}

impl Provider {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        match self {
            Self::Ready(s) => Some(s),
            Self::Failed { last, .. } => last.as_ref(),
            _ => None,
        }
    }
}

/// Colour role of a text (always paired with words).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Low,
    Exhausted,
    Muted,
    Error,
}

impl Tone {
    pub fn color(self, t: &pod_ui::Tokens) -> u32 {
        match self {
            Self::Normal => t.text_secondary,
            Self::Low => t.warning_fg,
            Self::Exhausted => t.danger_fg,
            Self::Muted => t.text_muted,
            Self::Error => t.warning_fg,
        }
    }
}

/// A window's state at `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Low,
    Exhausted,
    /// Reset time passed; the next report decides.
    ResetElapsed,
}

pub const LOW_REMAINING: f64 = 20.0;

#[derive(Clone, Debug, PartialEq)]
pub struct WindowView {
    /// `5시간`, `주간`, `Codex Spark · 5시간`.
    pub label: String,
    pub remaining: f64,
    /// `76% 남음`, `소진`, `이전 76%`.
    pub value: String,
    /// `2시간 13분 후 초기화 예정`, `초기화 시각 지남 · 새 보고 대기`, `초기화 시각 미보고`.
    pub reset: String,
    /// Compact countdown: `2시간 13분`, `지남`.
    pub countdown: Option<String>,
    pub state: WindowState,
    pub tone: Tone,
    pub kind: WindowKind,
    /// From the main bucket.
    pub main: bool,
}

/// What the monitor says about one service at `now`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceView {
    pub service: ServiceId,
    /// `정상`, `연결 안 됨`, `오래된 데이터`, …
    pub status: String,
    pub tone: Tone,
    pub windows: Vec<WindowView>,
    /// Index into `windows`: the most constrained live window.
    pub representative: Option<usize>,
    /// `3분 전 동기화`, `12분 전 보고`.
    pub synced: Option<String>,
    pub source: Option<&'static str>,
    pub profile: Option<String>,
    pub plan: Option<String>,
    /// A sentence for states without numbers (what to do).
    pub hint: Option<String>,
    pub stale: bool,
}

/// When a snapshot counts as old.
pub fn stale_after_ms(source: SourceKind) -> u64 {
    match source {
        // Polled every few minutes: three missed polls.
        SourceKind::CodexAppServer => 20 * 60_000,
        // Reported while Claude Code is used; quiet sessions report nothing.
        SourceKind::ClaudeStatusLine => 60 * 60_000,
    }
}

impl ServiceView {
    pub fn build(service: ServiceId, p: &Provider, now_ms: u64) -> Self {
        let mut v = Self {
            service,
            status: String::new(),
            tone: Tone::Muted,
            windows: Vec::new(),
            representative: None,
            synced: None,
            source: None,
            profile: None,
            plan: None,
            hint: None,
            stale: false,
        };
        let (status, hint) = match p {
            Provider::Pending => (copy::ST_PENDING, None),
            Provider::NotInstalled => (copy::ST_NOT_INSTALLED, Some(not_installed_hint(service))),
            Provider::NotConnected => (copy::ST_NOT_CONNECTED, Some(not_connected_hint(service))),
            Provider::Unsupported(why) => {
                v.status = copy::ST_UNSUPPORTED.into();
                v.hint = Some(why.clone());
                return v;
            }
            Provider::AwaitingFirst => (copy::ST_AWAITING, Some(copy::HINT_CLAUDE_AWAITING.into())),
            Provider::NoLimits => (copy::ST_NO_LIMITS, Some(copy::HINT_CLAUDE_NO_LIMITS.into())),
            Provider::Ready(_) | Provider::Failed { .. } => ("", None),
        };
        if !status.is_empty() {
            v.status = status.into();
            v.hint = hint;
            return v;
        }
        let Some(snap) = p.snapshot() else {
            // Failed without an earlier snapshot.
            if let Provider::Failed { error, .. } = p {
                v.status = copy::ST_ERROR.into();
                v.tone = Tone::Error;
                v.hint = Some(error.clone());
            }
            return v;
        };
        let now_s = (now_ms / 1000) as i64;
        v.source = Some(snap.source.label());
        v.profile = snap.profile.clone();
        v.plan = snap.plan.clone();
        let age = now_ms.saturating_sub(snap.observed_ms);
        v.synced = Some(match snap.source {
            SourceKind::CodexAppServer => copy::synced_ago(&ago_label(age)),
            SourceKind::ClaudeStatusLine => copy::reported_ago(&ago_label(age)),
        });
        v.stale = age > stale_after_ms(snap.source);
        for (bi, b) in snap.buckets.iter().enumerate() {
            for w in &b.windows {
                let label = match &b.name {
                    Some(n) => format!("{n} · {}", w.kind.label()),
                    None => w.kind.label(),
                };
                v.windows
                    .push(window_view(label, w, b.reached, bi == 0, now_s));
            }
        }
        v.representative = representative(&v.windows);
        let failed = matches!(p, Provider::Failed { .. });
        let rep = v.representative.map(|i| &v.windows[i]);
        (v.status, v.tone) = if failed {
            (copy::ST_ERROR_STALE.into(), Tone::Error)
        } else if v.windows.is_empty() {
            (copy::ST_NO_WINDOWS.into(), Tone::Muted)
        } else if rep.is_some_and(|w| w.state == WindowState::Exhausted) {
            (copy::ST_EXHAUSTED.into(), Tone::Exhausted)
        } else if rep.is_none() {
            // Every window passed its reset time.
            (copy::ST_RESET_ELAPSED.into(), Tone::Muted)
        } else if v.stale {
            (copy::ST_STALE.into(), Tone::Muted)
        } else if rep.is_some_and(|w| w.state == WindowState::Low) {
            (copy::ST_LOW.into(), Tone::Low)
        } else {
            (copy::ST_OK.into(), Tone::Normal)
        };
        if let Provider::Failed { error, .. } = p {
            v.hint = Some(error.clone());
        }
        v
    }

    /// The compact text: `5시간 76% · 2시간 13분`, or the status alone.
    pub fn compact(&self) -> String {
        match self.representative.map(|i| &self.windows[i]) {
            Some(w) => match &w.countdown {
                Some(c) => format!("{} {}% · {c}", w.label_short(), w.remaining.round()),
                None => format!("{} {}%", w.label_short(), w.remaining.round()),
            },
            None => self.status.clone(),
        }
    }

    /// The narrow text: `76%` or a short status.
    pub fn narrow(&self) -> String {
        match self.representative.map(|i| &self.windows[i]) {
            Some(w) if w.state == WindowState::Exhausted => copy::EXHAUSTED_SHORT.into(),
            Some(w) => format!("{}%", w.remaining.round()),
            None => self.status.clone(),
        }
    }

    /// Accessible sentence: service, status, representative window.
    pub fn sentence(&self) -> String {
        let mut s = format!("{} {}", self.service.name(), self.status);
        if let Some(w) = self.representative.map(|i| &self.windows[i]) {
            s.push_str(&format!(", {} {}, {}", w.label, w.value, w.reset));
        }
        if let Some(p) = &self.profile {
            s.push_str(&format!(", {} {p}", copy::PROFILE));
        }
        s
    }

    /// The tone the compact entry uses (stale numbers are muted).
    pub fn entry_tone(&self) -> Tone {
        match self.representative.map(|i| &self.windows[i]) {
            Some(_) if self.stale => Tone::Muted,
            Some(w) => w.tone,
            None => self.tone,
        }
    }

    pub fn has_numbers(&self) -> bool {
        self.representative.is_some()
    }
}

impl WindowView {
    /// `5h`, `주간`, a bucket's own label: short enough for the bar.
    pub fn label_short(&self) -> String {
        let base = match &self.kind {
            WindowKind::FiveHour => copy::WINDOW_5H_SHORT.to_string(),
            WindowKind::Weekly => copy::WINDOW_WEEKLY.to_string(),
            k => k.label(),
        };
        if self.main { base } else { self.label.clone() }
    }
}

fn not_installed_hint(s: ServiceId) -> String {
    match s {
        ServiceId::Codex => copy::HINT_CODEX_NOT_INSTALLED.into(),
        ServiceId::Claude => copy::HINT_CLAUDE_NOT_CONNECTED.into(),
    }
}

fn not_connected_hint(s: ServiceId) -> String {
    match s {
        ServiceId::Codex => copy::HINT_CODEX_LOGIN.into(),
        ServiceId::Claude => copy::HINT_CLAUDE_NOT_CONNECTED.into(),
    }
}

fn window_view(
    label: String,
    w: &QuotaWindow,
    reached: bool,
    main: bool,
    now_s: i64,
) -> WindowView {
    let remaining = w.remaining();
    let elapsed = w.resets_at.is_some_and(|r| r <= now_s);
    let state = if elapsed {
        WindowState::ResetElapsed
    } else if remaining <= 0.0 || (reached && remaining < 1.0) {
        WindowState::Exhausted
    } else if remaining <= LOW_REMAINING {
        WindowState::Low
    } else {
        WindowState::Normal
    };
    let pct = remaining.round();
    let value = match state {
        WindowState::ResetElapsed => copy::previous_pct(pct),
        WindowState::Exhausted => copy::EXHAUSTED.into(),
        _ if matches!(w.kind, WindowKind::Spend(_)) => copy::spend_used_pct(w.used_percent.round()),
        _ => copy::left_pct(pct),
    };
    let (reset, countdown) = match w.resets_at {
        Some(_) if elapsed => (
            copy::RESET_ELAPSED.to_string(),
            Some(copy::ELAPSED_SHORT.into()),
        ),
        Some(r) => {
            let c = countdown(now_s, r);
            (copy::resets_in(&c), Some(c))
        }
        None => (copy::RESET_UNKNOWN.to_string(), None),
    };
    let tone = match state {
        WindowState::Normal => Tone::Normal,
        WindowState::Low => Tone::Low,
        WindowState::Exhausted => Tone::Exhausted,
        WindowState::ResetElapsed => Tone::Muted,
    };
    WindowView {
        label,
        remaining,
        value,
        reset,
        countdown,
        state,
        tone,
        kind: w.kind.clone(),
        main,
    }
}

/// The most constrained window that has not passed its reset: least
/// remaining, then the main bucket, then the shorter window.
pub fn representative(ws: &[WindowView]) -> Option<usize> {
    ws.iter()
        .enumerate()
        .filter(|(_, w)| w.state != WindowState::ResetElapsed)
        .min_by(|(_, a), (_, b)| {
            a.remaining
                .total_cmp(&b.remaining)
                .then(b.main.cmp(&a.main))
                .then(a.kind.order().cmp(&b.kind.order()))
        })
        .map(|(i, _)| i)
}

/// `2일 4시간`, `2시간 13분`, `45분`, `1분 미만` until `resets_at` (seconds).
/// Rounded up to the minute so it never reads 0 before the time passes.
pub fn countdown(now_s: i64, resets_at: i64) -> String {
    let left = resets_at - now_s;
    if left <= 0 {
        return copy::ELAPSED_SHORT.into();
    }
    if left < 60 {
        return copy::UNDER_A_MINUTE.into();
    }
    let mins = (left + 59) / 60;
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    if d > 0 {
        if h > 0 {
            format!("{d}일 {h}시간")
        } else {
            format!("{d}일")
        }
    } else if h > 0 {
        if m > 0 {
            format!("{h}시간 {m}분")
        } else {
            format!("{h}시간")
        }
    } else {
        format!("{m}분")
    }
}

/// `방금`, `3분`, `2시간`, `1일`.
fn ago_label(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        copy::JUST_NOW.into()
    } else if s < 3600 {
        format!("{}분", s / 60)
    } else if s < 86_400 {
        format!("{}시간", s / 3600)
    } else {
        format!("{}일", s / 86_400)
    }
}

/// `5시간`, `7일`, `30분` for a window length in seconds.
pub fn duration_label(secs: u64) -> String {
    let m = secs / 60;
    if m >= 1440 && m.is_multiple_of(1440) {
        format!("{}일", m / 1440)
    } else if m >= 60 && m.is_multiple_of(60) {
        format!("{}시간", m / 60)
    } else {
        format!("{m}분")
    }
}

/// When the visible text can next change (Unix ms): the next minute
/// boundary of any countdown or "ago" label. The monitor repaints then, not
/// every second.
pub fn next_repaint_ms(now_ms: u64) -> u64 {
    (now_ms / 60_000 + 1) * 60_000
}

/// What the status bar shows for all services.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregate {
    /// The service with the most constrained window that has numbers.
    pub service: Option<ServiceId>,
    pub text: String,
    pub tone: Tone,
    pub sentence: String,
}

/// The compact aggregate: the single most constrained live window across
/// services (never a sum), or a status when no service has numbers.
pub fn aggregate(views: &[ServiceView]) -> Aggregate {
    let best = views.iter().filter(|v| v.has_numbers()).min_by(|a, b| {
        let ra = a.windows[a.representative.unwrap_or(0)].remaining;
        let rb = b.windows[b.representative.unwrap_or(0)].remaining;
        ra.total_cmp(&rb)
    });
    let sentence = views
        .iter()
        .map(ServiceView::sentence)
        .collect::<Vec<_>>()
        .join(". ");
    match best {
        Some(v) => Aggregate {
            service: Some(v.service),
            text: v.narrow(),
            tone: v.entry_tone(),
            sentence,
        },
        None => Aggregate {
            service: None,
            text: copy::NO_DATA_SHORT.into(),
            tone: Tone::Muted,
            sentence,
        },
    }
}
