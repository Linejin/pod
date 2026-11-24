//! Usage monitor: provider payloads, states, countdown, scheduling, the
//! Claude setup, the status-bar entry at small/normal widths across themes
//! and layouts, and a smoke run against a mock `codex app-server` process.

use core::prelude::v1::test;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::claude::{self, ClaudeRead, Collector};
use super::codex::{self, Account, CodexExe, CodexRead};
use super::model::{self, Provider, ServiceId, ServiceView, Tone, WindowKind, WindowState};
use super::samples;
use super::view::{EntryPlan, entry_segments};
use super::{Cadence, CodexState, Schedule, UsageBackend};

const NOW_MS: u64 = 1_780_000_000_000;
const NOW_S: i64 = 1_780_000_000;

// ─────────────────────────── Codex payloads ───────────────────────────

/// The documented example of section 6, plus fields Pod must drop.
fn documented_reply() -> serde_json::Value {
    json!({
        "rateLimits": {
            "limitId": "codex", "limitName": null,
            "primary": {"usedPercent": 25, "windowDurationMins": 15, "resetsAt": 1_730_947_200},
            "secondary": null, "rateLimitReachedType": null
        },
        "rateLimitsByLimitId": {
            "codex": {
                "limitId": "codex", "limitName": null,
                "primary": {"usedPercent": 24, "windowDurationMins": 300, "resetsAt": NOW_S + 7980},
                "secondary": {"usedPercent": 61, "windowDurationMins": 10080, "resetsAt": NOW_S + 288_000},
                "rateLimitReachedType": null,
                "credits": {"hasCredits": true, "unlimited": false, "balance": "123.45"},
                "planType": "plus"
            },
            "codex_other": {
                "limitId": "codex_other", "limitName": "GPT-5.3-Codex-Spark",
                "primary": {"usedPercent": 42, "windowDurationMins": 300, "resetsAt": NOW_S + 3600},
                "secondary": null, "rateLimitReachedType": null
            }
        },
        "rateLimitResetCredits": {"availableCount": 2, "credits": null},
        "accountId": "acct-secret-123",
        "ordinaryUsageAllowed": true,
        "rateLimitUpsell": {"title": "buy more"}
    })
}

#[test]
fn codex_documented_reply_keeps_allowed_fields_only() {
    let (snap, account) = codex::parse_rate_limits(&documented_reply(), None, NOW_MS).unwrap();
    assert!(account.is_some());
    assert_eq!(snap.plan, None, "plan only from planType of the main view");
    assert_eq!(snap.buckets.len(), 2);
    let main = &snap.buckets[0];
    assert_eq!(main.name, None, "codex is the main bucket");
    assert_eq!(main.windows[0].kind, WindowKind::FiveHour);
    assert_eq!(main.windows[1].kind, WindowKind::Weekly);
    assert_eq!(main.windows[0].resets_at, Some(NOW_S + 7980));
    assert_eq!(snap.buckets[1].name.as_deref(), Some("GPT-5.3-Codex-Spark"));
    let dump = format!("{snap:?}");
    for leak in ["acct-secret-123", "123.45", "buy more", "availableCount"] {
        assert!(!dump.contains(leak), "{leak} kept: {dump}");
    }
}

#[test]
fn codex_single_view_absent_and_malformed_windows() {
    // No multi-bucket view: the single one.
    let r = json!({"rateLimits": {"primary": {"usedPercent": 10, "windowDurationMins": 300, "resetsAt": NOW_S + 60}, "planType": "pro"}});
    let (s, acc) = codex::parse_rate_limits(&r, None, NOW_MS).unwrap();
    assert_eq!(
        (s.buckets.len(), s.plan.as_deref(), acc),
        (1, Some("Pro"), None)
    );
    // Nothing usable.
    assert!(codex::parse_rate_limits(&json!({}), None, NOW_MS).is_err());
    assert!(codex::parse_rate_limits(&json!({"rateLimits": 5}), None, NOW_MS).is_err());
    // Malformed windows are dropped, not guessed; a bucket without windows goes.
    let r = json!({"rateLimitsByLimitId": {"codex": {
        "primary": {"usedPercent": "24", "windowDurationMins": 300},
        "secondary": {"usedPercent": 140.0, "resetsAt": NOW_S}
    }, "x": {"primary": {"usedPercent": 5, "windowDurationMins": -3, "resetsAt": 7}}}});
    let (s, _) = codex::parse_rate_limits(&r, None, NOW_MS).unwrap();
    assert_eq!(s.buckets.len(), 1, "{s:?}");
    let w = &s.buckets[0].windows[0];
    assert_eq!((&w.kind, w.resets_at), (&WindowKind::Other(None), None));
    // Unknown reached type still counts as reached; unknown plan is hidden.
    let r = json!({"rateLimits": {"primary": {"usedPercent": 100, "windowDurationMins": 300}, "rateLimitReachedType": "new_kind", "planType": "ent26_internal"}});
    let (s, _) = codex::parse_rate_limits(&r, None, NOW_MS).unwrap();
    assert!(s.buckets[0].reached);
    assert_eq!(s.plan, None);
}

#[test]
fn codex_account_kinds_never_read_the_email() {
    let chat = json!({"account": {"type": "chatgpt", "email": "me@example.com", "planType": "plus", "workspaceName": "W"}, "userId": "u-1", "requiresOpenaiAuth": true});
    let (k, u) = codex::parse_account(&chat);
    assert_eq!(
        k,
        Account::ChatGpt {
            plan: Some("Plus".into())
        }
    );
    assert!(u.is_some());
    assert!(!format!("{k:?}").contains("example.com"));
    assert_eq!(
        codex::parse_account(&json!({"account": {"type": "apiKey"}})).0,
        Account::ApiKey
    );
    assert_eq!(
        codex::parse_account(&json!({"account": {"type": "gcp"}})).0,
        Account::Other
    );
    assert_eq!(
        codex::parse_account(&json!({"account": null, "requiresOpenaiAuth": true})).0,
        Account::None
    );
    // Different users hash differently; the same user the same.
    let other = json!({"account": {"type": "chatgpt", "planType": "pro"}, "userId": "u-2"});
    assert_ne!(codex::parse_account(&other).1, u);
    assert_eq!(codex::parse_account(&chat).1, u);
}

// ─────────────────────────── executable choice ───────────────────────────

fn fs(files: &[&str]) -> impl Fn(&Path) -> bool {
    let set: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    move |p: &Path| set.iter().any(|f| f == p)
}

#[test]
fn codex_executable_is_native_and_deterministic() {
    let npm = r"C:\Users\u\AppData\Roaming\npm";
    let native = format!(
        r"{npm}\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe"
    );
    let path = OsString::from(format!(r"relative\dir;{npm};C:\Tools"));
    // The npm shim resolves to the binary the shim launches, never the .cmd.
    let e = codex::resolve(
        None,
        Some(path.clone()),
        fs(&[&format!(r"{npm}\codex.cmd"), &native]),
    )
    .unwrap();
    assert_eq!(e.path, PathBuf::from(&native));
    assert_eq!(
        e.npm_root,
        Some(PathBuf::from(format!(r"{npm}\node_modules\@openai\codex")))
    );
    // A shim without its binary is skipped; a later codex.exe wins.
    let e = codex::resolve(
        None,
        Some(path.clone()),
        fs(&[&format!(r"{npm}\codex.cmd"), r"C:\Tools\codex.exe"]),
    )
    .unwrap();
    assert_eq!(e.path, PathBuf::from(r"C:\Tools\codex.exe"));
    // Relative PATH entries are never searched.
    assert_eq!(
        codex::resolve(None, Some(path.clone()), fs(&[r"relative\dir\codex.exe"])),
        None
    );
    // POD_CODEX: absolute .exe only; anything else is refused, not replaced.
    let o = r"D:\codex\codex.exe";
    assert_eq!(
        codex::resolve(Some(o.into()), Some(path.clone()), fs(&[o]))
            .unwrap()
            .path,
        PathBuf::from(o)
    );
    assert_eq!(
        codex::resolve(
            Some(r"D:\codex\codex.cmd".into()),
            Some(path.clone()),
            fs(&[r"D:\codex\codex.cmd", r"C:\Tools\codex.exe"])
        ),
        None
    );
    assert_eq!(
        codex::resolve(
            Some("codex.exe".into()),
            Some(path),
            fs(&[r"C:\Tools\codex.exe"])
        ),
        None
    );
    assert_eq!(codex::resolve(None, None, fs(&[])), None);
}

// ─────────────────────────── states and text ───────────────────────────

#[test]
fn countdown_rounds_up_and_never_claims_a_reset() {
    assert_eq!(model::countdown(0, 3 * 86_400 + 4 * 3600), "3일 4시간");
    assert_eq!(model::countdown(0, 2 * 86_400), "2일");
    assert_eq!(model::countdown(0, 2 * 3600 + 13 * 60), "2시간 13분");
    assert_eq!(model::countdown(0, 2 * 3600 + 12 * 60 + 1), "2시간 13분");
    assert_eq!(model::countdown(0, 3600), "1시간");
    assert_eq!(model::countdown(0, 61), "2분");
    assert_eq!(model::countdown(0, 60), "1분");
    assert_eq!(model::countdown(0, 59), "1분 미만");
    assert_eq!(model::countdown(10, 10), "지남");
    assert_eq!(model::countdown(10, 5), "지남");
    assert_eq!(
        model::next_repaint_ms(NOW_MS + 1),
        NOW_MS + 60_000 - (NOW_MS % 60_000)
    );
    assert_eq!(model::duration_label(15 * 60), "15분");
    assert_eq!(model::duration_label(30 * 86_400), "30일");
}

fn ready(now: u64) -> Provider {
    match samples::codex_ok(now) {
        CodexRead::Ok { snapshot, .. } => Provider::Ready(snapshot),
        _ => unreachable!(),
    }
}

#[test]
fn a_fresh_snapshot_names_its_most_constrained_window() {
    let v = ServiceView::build(ServiceId::Codex, &ready(NOW_MS), NOW_MS);
    assert_eq!(v.status, "정상");
    let rep = &v.windows[v.representative.unwrap()];
    assert_eq!(
        (rep.kind.clone(), rep.remaining),
        (WindowKind::Weekly, 39.0)
    );
    assert_eq!(v.compact(), "주간 39% · 3일 4시간");
    assert_eq!(v.windows[0].value, "76% 남음");
    assert_eq!(v.windows[0].reset, "2시간 13분 후 초기화 예정");
    assert_eq!(v.synced.as_deref(), Some("방금 동기화"));
    assert!(
        v.sentence()
            .contains("Codex 정상, 주간 39% 남음, 3일 4시간 후 초기화 예정")
    );
}

#[test]
fn states_distinguish_exhausted_elapsed_stale_and_errors() {
    let mut snap = ready(NOW_MS).snapshot().unwrap().clone();
    // Exhausted 5h window.
    snap.buckets[0].windows[0].used_percent = 100.0;
    let v = ServiceView::build(ServiceId::Codex, &Provider::Ready(snap.clone()), NOW_MS);
    assert_eq!((v.status.as_str(), v.tone), ("소진", Tone::Exhausted));
    assert_eq!(v.narrow(), "소진");
    // 3 hours later: the 5h reset time passed. It is not "reset": the old
    // value is shown as previous and the weekly window becomes representative.
    let later = NOW_MS + 3 * 3_600_000;
    let v = ServiceView::build(ServiceId::Codex, &Provider::Ready(snap.clone()), later);
    assert_eq!(v.windows[0].state, WindowState::ResetElapsed);
    assert_eq!(v.windows[0].value, "이전 0% 남음");
    assert_eq!(v.windows[0].reset, "초기화 시각 지남 · 새 보고 대기");
    assert_eq!(
        v.windows[v.representative.unwrap()].kind,
        WindowKind::Weekly
    );
    assert!(!format!("{v:?}").contains("초기화됨"));
    // Every window elapsed.
    let much_later = NOW_MS + 10 * 86_400_000;
    let v = ServiceView::build(ServiceId::Codex, &Provider::Ready(snap.clone()), much_later);
    assert_eq!(v.status, "초기화 시각 지남 · 새 보고 대기");
    assert_eq!(v.representative, None);
    // Stale: numbers stay, muted.
    let mut s2 = ready(NOW_MS).snapshot().unwrap().clone();
    s2.observed_ms = NOW_MS - 30 * 60_000;
    let v = ServiceView::build(ServiceId::Codex, &Provider::Ready(s2.clone()), NOW_MS);
    assert_eq!(
        (v.status.as_str(), v.entry_tone(), v.stale),
        ("오래된 데이터", Tone::Muted, true)
    );
    assert_eq!(v.synced.as_deref(), Some("30분 전 동기화"));
    // Error with and without earlier numbers.
    let f = Provider::Failed {
        error: "x 실패".into(),
        last: Some(s2),
        at_ms: NOW_MS,
    };
    let v = ServiceView::build(ServiceId::Codex, &f, NOW_MS);
    assert_eq!(
        (v.status.as_str(), v.has_numbers()),
        ("읽기 실패 · 이전 값", true)
    );
    let f = Provider::Failed {
        error: "x 실패".into(),
        last: None,
        at_ms: NOW_MS,
    };
    let v = ServiceView::build(ServiceId::Codex, &f, NOW_MS);
    assert_eq!(
        (v.status.as_str(), v.hint.as_deref()),
        ("읽기 실패", Some("x 실패"))
    );
    // No numbers: each state says what it is.
    for (p, status) in [
        (Provider::Pending, "확인 중"),
        (Provider::NotInstalled, "설치 안 됨"),
        (Provider::NotConnected, "연결 안 됨"),
        (Provider::Unsupported("API 키".into()), "지원 안 됨"),
        (Provider::AwaitingFirst, "첫 보고 대기"),
        (Provider::NoLimits, "한도 정보 없음"),
    ] {
        let v = ServiceView::build(ServiceId::Claude, &p, NOW_MS);
        assert_eq!(
            (v.status.as_str(), v.has_numbers()),
            (status, false),
            "{p:?}"
        );
        assert!(
            v.hint.is_some() || p == Provider::Pending,
            "{p:?} explains itself"
        );
    }
}

#[test]
fn low_remaining_is_flagged_and_spend_shows_used() {
    let mut snap = ready(NOW_MS).snapshot().unwrap().clone();
    snap.buckets[0].windows[1].used_percent = 85.0;
    snap.buckets[0].windows.push(model::QuotaWindow {
        kind: WindowKind::Spend(Some("monthly".into())),
        used_percent: 120.0,
        resets_at: None,
    });
    let v = ServiceView::build(ServiceId::Claude, &Provider::Ready(snap), NOW_MS);
    let spend = v
        .windows
        .iter()
        .find(|w| matches!(w.kind, WindowKind::Spend(_)))
        .unwrap();
    assert_eq!(
        (spend.label.as_str(), spend.value.as_str()),
        ("지출 한도(월)", "소진")
    );
    assert_eq!(spend.reset, "초기화 시각 미보고");
    assert_eq!(v.status, "소진", "spend over its limit is the constraint");
}

// ─────────────────────────── scheduling ───────────────────────────

#[test]
fn schedule_is_single_flight_with_backoff_and_a_manual_gap() {
    let c = Cadence::default();
    let mut s = Schedule::default();
    assert!(s.try_start(NOW_MS, false, c.manual_gap_ms));
    assert!(!s.try_start(NOW_MS, false, 0), "one read in flight");
    assert!(!s.try_start(NOW_MS, true, 0), "manual dedups too");
    s.finish(true, NOW_MS, c.codex_ms, c.max_backoff_ms);
    assert_eq!(s.next_ms, NOW_MS + 5 * 60_000);
    assert!(
        !s.try_start(NOW_MS + 1000, false, c.manual_gap_ms),
        "not due"
    );
    assert!(
        !s.try_start(NOW_MS + 1000, true, c.manual_gap_ms),
        "manual within 10 s"
    );
    assert!(
        s.try_start(NOW_MS + 10_000, true, c.manual_gap_ms),
        "manual reads at once"
    );
    // Failures back off: 10, 20, 40, then the 60 min cap.
    let mut t = NOW_MS;
    for want in [10, 20, 40, 60, 60] {
        s.finish(false, t, c.codex_ms, c.max_backoff_ms);
        assert_eq!(s.next_ms - t, want * 60_000);
        t = s.next_ms;
        assert!(s.try_start(t, false, 0));
    }
    s.finish(true, t, c.codex_ms, c.max_backoff_ms);
    assert_eq!((s.failures, s.next_ms - t), (0, 5 * 60_000));
}

#[test]
fn an_account_switch_or_sign_out_never_shows_the_old_account() {
    let c = Cadence::default();
    let mut st = CodexState::default();
    st.apply(samples::codex_ok(NOW_MS), NOW_MS, &c);
    assert_eq!(st.account, Some(1));
    // Same account, read failed: old numbers stay, marked.
    st.apply(
        CodexRead::Failed {
            error: "e".into(),
            account: Some(1),
        },
        NOW_MS,
        &c,
    );
    assert!(matches!(
        &st.provider,
        Provider::Failed { last: Some(_), .. }
    ));
    // Unknown account (failed before account/read): still the same profile.
    st.apply(
        CodexRead::Failed {
            error: "e".into(),
            account: None,
        },
        NOW_MS,
        &c,
    );
    assert!(st.provider.snapshot().is_some());
    // Another account signed in, its read failed: the old numbers go.
    st.apply(
        CodexRead::Failed {
            error: "e".into(),
            account: Some(2),
        },
        NOW_MS,
        &c,
    );
    assert_eq!(st.provider.snapshot(), None);
    assert_eq!(st.account, None);
    // Sign-out and API-key login drop numbers too.
    st.apply(samples::codex_ok(NOW_MS), NOW_MS, &c);
    st.apply(CodexRead::NotLoggedIn, NOW_MS, &c);
    assert_eq!(st.provider, Provider::NotConnected);
    st.apply(samples::codex_ok(NOW_MS), NOW_MS, &c);
    st.apply(CodexRead::Unsupported("API".into()), NOW_MS, &c);
    assert_eq!(st.provider.snapshot(), None);
    assert_eq!(st.schedule.failures, 0, "not errors");
}

// ─────────────────────────── Claude ───────────────────────────

#[test]
fn claude_settings_collector_detection_and_composition() {
    assert_eq!(claude::collector_from_settings(None), Collector::None);
    assert_eq!(
        claude::collector_from_settings(Some("{")),
        Collector::Unreadable
    );
    assert_eq!(
        claude::collector_from_settings(Some(r#"{"model":"x"}"#)),
        Collector::None
    );
    let pod = r#"{"statusLine":{"type":"command","command":"C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe statusline"}}"#;
    assert_eq!(claude::collector_from_settings(Some(pod)), Collector::Pod);
    let other = r#"{"env":{"ANTHROPIC_API_KEY":"sk-secret"},"statusLine":{"type":"command","command":"~/.claude/statusline.sh","padding":2}}"#;
    assert_eq!(
        claude::collector_from_settings(Some(other)),
        Collector::Other("~/.claude/statusline.sh".into())
    );
    let exe = Path::new(r"C:\Users\u\AppData\Local\Pod\bin\pod-hook.exe");
    assert_eq!(
        claude::setup_command(exe, None),
        "C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe statusline"
    );
    assert_eq!(
        claude::setup_command(exe, Some("~/.claude/statusline.sh")),
        "C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe statusline --passthrough | ~/.claude/statusline.sh"
    );
    let already = "C:/x/pod-hook.exe statusline --passthrough | foo";
    assert_eq!(
        claude::setup_command(exe, Some(already)),
        already,
        "never doubled"
    );
    assert_eq!(
        claude::setup_command(Path::new(r"C:\Program Files\Pod\pod-hook.exe"), None),
        "\"C:/Program Files/Pod/pod-hook.exe\" statusline"
    );
    let json: serde_json::Value = serde_json::from_str(&claude::setup_json("a b")).unwrap();
    assert_eq!(
        json,
        json!({"statusLine": {"type": "command", "command": "a b"}})
    );
}

fn record(profile: &str, label: Option<&str>, five: f64) -> pod_status::usage::ClaudeUsageRecord {
    let mut r = samples::claude_record(NOW_MS, five, 10.0);
    r.profile = profile.into();
    r.profile_label = label.map(str::to_string);
    r
}

#[test]
fn claude_providers_follow_the_collector_and_keep_profiles_apart() {
    let read = |records, collector| ClaudeRead {
        records,
        collector,
        setup: None,
    };
    // Not connected and nothing recent: old numbers are hidden.
    let mut old = record("default", None, 50.0);
    old.seen_at_ms = NOW_MS - 2 * 3_600_000;
    assert_eq!(
        claude::providers(&read(vec![old.clone()], Collector::None), NOW_MS),
        [Provider::NotConnected]
    );
    assert_eq!(
        claude::providers(&read(vec![], Collector::Other("x".into())), NOW_MS),
        [Provider::NotConnected]
    );
    // Connected, nothing yet.
    assert_eq!(
        claude::providers(&read(vec![], Collector::Pod), NOW_MS),
        [Provider::AwaitingFirst]
    );
    // Samples without limits.
    let mut none = record("default", None, 0.0);
    none.limits_at_ms = None;
    assert_eq!(
        claude::providers(&read(vec![none], Collector::Pod), NOW_MS),
        [Provider::NoLimits]
    );
    // A recent sample counts even when the user settings do not show it.
    let recent = record("default", None, 20.0);
    assert!(matches!(
        claude::providers(&read(vec![recent.clone()], Collector::None), NOW_MS)[..],
        [Provider::Ready(_)]
    ));
    // Two profiles: two views, never summed; the aggregate is the tighter one.
    let work = record("cfg-00000000000000ab", Some(".claude-work"), 90.0);
    let ps = claude::providers(&read(vec![recent, work], Collector::Pod), NOW_MS);
    let views: Vec<ServiceView> = ps
        .iter()
        .map(|p| ServiceView::build(ServiceId::Claude, p, NOW_MS))
        .collect();
    assert_eq!(views.len(), 2);
    assert_eq!(views[1].profile.as_deref(), Some(".claude-work"));
    let agg = model::aggregate(&views);
    assert_eq!(agg.text, "10%");
    assert!(agg.sentence.contains("프로필 .claude-work"));
}

// ─────────────────────────── entry widths ───────────────────────────

fn two_services() -> Vec<ServiceView> {
    let claude = samples::claude_ok(NOW_MS);
    let mut v = vec![ServiceView::build(ServiceId::Codex, &ready(NOW_MS), NOW_MS)];
    v.extend(
        claude::providers(&claude, NOW_MS)
            .iter()
            .map(|p| ServiceView::build(ServiceId::Claude, p, NOW_MS)),
    );
    v
}

#[test]
fn entry_text_per_width() {
    let views = two_services();
    assert_eq!(EntryPlan::for_width(1600.), EntryPlan::Full);
    assert_eq!(EntryPlan::for_width(1200.), EntryPlan::Medium);
    assert_eq!(EntryPlan::for_width(800.), EntryPlan::Compact);
    let full = entry_segments(&views, EntryPlan::Full);
    assert_eq!(
        full.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(),
        ["주간 39% · 3일 4시간", "주간 65% · 5일"]
    );
    let medium = entry_segments(&views, EntryPlan::Medium);
    assert_eq!(medium[0].1, "39% · 3일 4시간");
    let compact = entry_segments(&views, EntryPlan::Compact);
    assert_eq!(compact.len(), 1, "one aggregate entry");
    assert_eq!(
        (compact[0].0, compact[0].1.as_str()),
        (ServiceId::Codex, "39%")
    );
    // Nothing connected anywhere: a status, not a number.
    let none = vec![ServiceView::build(
        ServiceId::Codex,
        &Provider::NotInstalled,
        NOW_MS,
    )];
    assert_eq!(entry_segments(&none, EntryPlan::Compact)[0].1, "한도 없음");
    assert_eq!(entry_segments(&none, EntryPlan::Full)[0].1, "설치 안 됨");
}

// ─────────────────────────── the shell ───────────────────────────

use crate::native_qa::{Harness, seed_two_tasks};
use crate::workspace::StartOptions;
use gpui_kit::{px, size};

fn clock(ms: Arc<std::sync::atomic::AtomicU64>) -> super::Clock {
    Arc::new(move || ms.load(std::sync::atomic::Ordering::SeqCst))
}

fn harness(name: &'static str, w: f32, backend: UsageBackend) -> Harness {
    let options = StartOptions {
        usage: Some(backend),
        ..Default::default()
    };
    let mut h = Harness::with_options(name, options, seed_two_tasks);
    h.cx.simulate_resize(size(px(w), px(800.)));
    h.settle();
    h
}

fn entry_name(h: &mut Harness) -> String {
    h.cx.update(|_, cx| {
        let m = super::monitor(cx).expect("monitor");
        let m = m.read(cx);
        super::copy::entry_name(&model::aggregate(&m.views(m.now())).sentence)
    })
}

/// The entry, 상태 and 알림 are all inside the window at `w` px.
fn assert_reachable(h: &mut Harness, w: f32) {
    for id in ["status-usage", "status-inspect", "status-notifications"] {
        let b = h.bounds(id).unwrap_or_else(|| panic!("{id} drawn at {w}"));
        assert!(
            f32::from(b.origin.x) >= 0. && f32::from(b.origin.x + b.size.width) <= w + 0.5,
            "{id} inside the window at {w}: {b:?}"
        );
    }
    let usage = h.bounds("status-usage").unwrap();
    let inspect = h.bounds("status-inspect").unwrap();
    assert!(
        usage.origin.x + usage.size.width <= inspect.origin.x,
        "no overlap"
    );
}

#[test]
fn the_entry_shows_real_snapshots_at_small_and_normal_widths() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(NOW_MS));
    let b = UsageBackend::fixed(
        Some(samples::codex_ok(NOW_MS)),
        Some(samples::claude_ok(NOW_MS)),
        clock(now.clone()),
    );
    let mut h = harness("usage-widths", 1600., b);
    assert_reachable(&mut h, 1600.);
    assert!(
        h.bounds("status-usage-segment-1").is_some(),
        "both services at full width"
    );
    let name = entry_name(&mut h);
    assert!(
        name.starts_with("AI 사용 한도: Codex 정상, 주간 39% 남음"),
        "{name}"
    );
    assert!(name.contains("Claude 정상"), "{name}");
    // 800 px minimum: one aggregate entry, all three actions reachable.
    h.cx.simulate_resize(size(px(800.), px(700.)));
    h.settle();
    assert_reachable(&mut h, 800.);
    assert!(
        h.bounds("status-usage-segment-1").is_none(),
        "compact aggregate only"
    );
    // Time passes locally: the countdown moves, the entry width does not.
    let before = h.bounds("status-usage").unwrap();
    now.store(NOW_MS + 61 * 60_000, std::sync::atomic::Ordering::SeqCst);
    h.cx.update(|_, cx| super::monitor(cx).unwrap().update(cx, |_, cx| cx.notify()));
    h.settle();
    assert_eq!(
        h.bounds("status-usage").unwrap().size,
        before.size,
        "no re-layout per tick"
    );
    let name = entry_name(&mut h);
    assert!(
        name.contains("주간 39% 남음, 3일 2시간 후 초기화 예정"),
        "{name}"
    );
    assert!(
        name.contains("Codex 오래된 데이터"),
        "61 min without a sync: {name}"
    );
}

#[test]
fn details_open_by_keyboard_and_show_states_and_setup() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(NOW_MS));
    let claude = ClaudeRead {
        records: vec![],
        collector: Collector::Other("~/.claude/statusline.sh".into()),
        setup: Some(claude::setup_json(
            "x/pod-hook.exe statusline --passthrough | ~/.claude/statusline.sh",
        )),
    };
    let b = UsageBackend::fixed(
        Some(CodexRead::Unsupported(
            super::copy::HINT_CODEX_API_KEY.into(),
        )),
        Some(claude),
        clock(now),
    );
    let mut h = harness("usage-details", 1200., b);
    // Tab to the entry (the popover trigger) and open it with Enter.
    let mut found = false;
    for _ in 0..60 {
        h.keys("tab");
        if h.context_path().ends_with("Popover") || h.context_path().contains("> Popover") {
            found = true;
            break;
        }
    }
    assert!(found, "the entry is a tab stop: {}", h.context_path());
    assert!(h.bounds("usage-details").is_none());
    h.press("enter");
    assert!(
        h.bounds("usage-details").is_some(),
        "Enter opens the details"
    );
    assert!(h.bounds("usage-card-0").is_some() && h.bounds("usage-card-1").is_some());
    h.cx.update(|_, cx| {
        let m = super::monitor(cx).unwrap();
        let m = m.read(cx);
        let v = m.views(m.now());
        assert_eq!(
            (v[0].status.as_str(), v[1].status.as_str()),
            ("지원 안 됨", "연결 안 됨")
        );
        assert!(
            m.claude_setup()
                .unwrap()
                .contains("--passthrough | ~/.claude/statusline.sh")
        );
    });
    h.keys("escape");
    assert!(h.bounds("usage-details").is_none(), "Escape closes");
}

#[test]
fn the_entry_stays_across_layouts_and_themes() {
    use crate::layout_mode::LayoutMode;
    let now = Arc::new(std::sync::atomic::AtomicU64::new(NOW_MS));
    let b = UsageBackend::fixed(Some(samples::codex_ok(NOW_MS)), None, clock(now));
    let mut h = harness("usage-layouts", 1000., b);
    let modes = [
        LayoutMode::FocusRail,
        LayoutMode::Queue,
        LayoutMode::Kanban,
        LayoutMode::BlockTerminal,
        LayoutMode::Review,
        LayoutMode::DenseDetails,
        LayoutMode::Workbench,
    ];
    for (theme, contrast) in [("light", false), ("dark", false), ("dark", true)] {
        for mode in modes {
            let ws = h.ws.clone();
            h.cx.update(|_, cx| {
                let s = crate::theme::Settings {
                    theme: theme.into(),
                    high_contrast: contrast,
                    layout: mode.id().into(),
                    ..cx.global::<crate::theme::Settings>().clone()
                };
                s.apply_with(false, cx);
                ws.update(cx, |_, cx| cx.notify());
            });
            h.settle();
            h.settle();
            assert_reachable(&mut h, 1000.);
            assert!(
                h.bounds("status-usage-segment-0").is_some(),
                "{theme}/{contrast}/{mode:?}"
            );
        }
    }
}

#[test]
fn without_a_backend_there_is_no_entry() {
    let mut h = Harness::new("usage-none", seed_two_tasks);
    // Other harness tests may have installed the app-wide monitor on this
    // thread's app; a fresh harness has its own app.
    assert!(h.bounds("status-usage").is_none());
}

// ─────────────────────────── mock app-server smoke ───────────────────────────

/// A PowerShell stand-in for `codex app-server`: answers the JSON-RPC
/// methods from `replies` and logs each method and its working directory.
fn mock_server(dir: &Path, replies: serde_json::Value) -> CodexExe {
    std::fs::create_dir_all(dir).unwrap();
    let replies_path = dir.join("replies.json");
    std::fs::write(&replies_path, replies.to_string()).unwrap();
    let log = dir.join("calls.log");
    let script = dir.join("mock-app-server.ps1");
    let body = format!(
        r#"$ErrorActionPreference = 'Stop'
$replies = Get-Content -Raw -LiteralPath '{replies}' | ConvertFrom-Json
Add-Content -LiteralPath '{log}' -Value ("args " + ($args -join ' '))
Add-Content -LiteralPath '{log}' -Value ("cwd " + (Get-Location).Path)
$in = [Console]::In
while (($line = $in.ReadLine()) -ne $null) {{
  $m = $line | ConvertFrom-Json
  Add-Content -LiteralPath '{log}' -Value ("method " + $m.method)
  if ($m.id -eq $null) {{ continue }}
  [Console]::Out.WriteLine('{{"method":"account/rateLimits/updated","params":{{}}}}')
  $r = $replies.($m.method)
  if ($r -eq $null) {{ $out = '{{"id":' + $m.id + ',"error":{{"message":"unknown method"}}}}' }}
  else {{ $out = '{{"id":' + $m.id + ',"result":' + ($r | ConvertTo-Json -Depth 20 -Compress) + '}}' }}
  [Console]::Out.WriteLine($out)
  [Console]::Out.Flush()
}}
"#,
        replies = replies_path.display(),
        log = log.display()
    );
    std::fs::write(&script, body).unwrap();
    CodexExe {
        path: PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
        pre_args: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.into_os_string(),
        ],
        npm_root: None,
    }
}

fn smoke_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pod-usage-smoke-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn smoke_mock_app_server_documented_flow_only() {
    let d = smoke_dir("ok");
    let exe = mock_server(
        &d,
        json!({
            "initialize": {"userAgent": "mock", "codexHome": "C:/x", "platformFamily": "windows", "platformOs": "windows"},
            "account/read": {"account": {"type": "chatgpt", "email": "me@example.com", "planType": "pro"}, "userId": "u-9", "requiresOpenaiAuth": true},
            "account/rateLimits/read": documented_reply(),
        }),
    );
    let cwd = d.join("neutral-cwd");
    let r = codex::read_with(&exe, &cwd, || NOW_MS);
    let CodexRead::Ok { snapshot, account } = r else {
        panic!("{r:?}")
    };
    assert!(account.is_some());
    assert_eq!(snapshot.plan.as_deref(), Some("Pro"));
    assert_eq!(snapshot.buckets.len(), 2);
    assert!(!format!("{snapshot:?}").contains("example.com"));
    let log = std::fs::read_to_string(d.join("calls.log")).unwrap();
    let methods: Vec<&str> = log
        .lines()
        .filter_map(|l| l.strip_prefix("method "))
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "account/read",
            "account/rateLimits/read"
        ],
        "documented read-only methods, in order"
    );
    assert!(log.lines().any(|l| l == "args app-server"), "{log}");
    let cwd_line = log.lines().find_map(|l| l.strip_prefix("cwd ")).unwrap();
    assert_eq!(Path::new(cwd_line), cwd.as_path(), "Pod-owned neutral cwd");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn smoke_mock_app_server_api_key_and_errors() {
    let d = smoke_dir("apikey");
    let exe = mock_server(
        &d,
        json!({
            "initialize": {},
            "account/read": {"account": {"type": "apiKey"}, "userId": null, "requiresOpenaiAuth": false},
        }),
    );
    assert_eq!(
        codex::read_with(&exe, &d.join("cwd"), || NOW_MS),
        CodexRead::Unsupported(super::copy::HINT_CODEX_API_KEY.into())
    );
    let log = std::fs::read_to_string(d.join("calls.log")).unwrap();
    assert!(!log.contains("rateLimits"), "no limits read for an API key");
    let _ = std::fs::remove_dir_all(&d);

    let d = smoke_dir("nologin");
    let exe = mock_server(
        &d,
        json!({"initialize": {}, "account/read": {"account": null, "requiresOpenaiAuth": true}}),
    );
    assert_eq!(
        codex::read_with(&exe, &d.join("cwd"), || NOW_MS),
        CodexRead::NotLoggedIn
    );
    let _ = std::fs::remove_dir_all(&d);

    // account/read failing (seen live: "workspace routing discovery
    // failed") does not block the documented limits read.
    let d = smoke_dir("acctfail");
    let exe = mock_server(
        &d,
        json!({"initialize": {}, "account/rateLimits/read": documented_reply()}),
    );
    match codex::read_with(&exe, &d.join("cwd"), || NOW_MS) {
        CodexRead::Ok { snapshot, account } => {
            assert_eq!(snapshot.buckets.len(), 2);
            assert!(account.is_some(), "from accountId, hashed");
        }
        r => panic!("{r:?}"),
    }
    let _ = std::fs::remove_dir_all(&d);

    let d = smoke_dir("err");
    let exe = mock_server(
        &d,
        json!({"initialize": {}, "account/read": {"account": {"type": "chatgpt", "planType": "plus"}, "userId": "u"}}),
    );
    match codex::read_with(&exe, &d.join("cwd"), || NOW_MS) {
        CodexRead::Failed { error, account } => {
            assert!(error.contains("unknown method"), "{error}");
            assert!(account.is_some(), "the account seen before the failure");
        }
        r => panic!("{r:?}"),
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Real installed Codex, read-only, normalized output only. Not run by
/// default (needs a signed-in Codex and the network): the coordinator runs
/// it with `--ignored` when a live check is wanted.
#[test]
#[ignore]
fn live_codex_read_normalized_only() {
    use super::CodexProbe as _;
    let probe = codex::RealCodex {
        cwd: codex::default_cwd(),
        clock: Arc::new(pod_status::usage::now_ms),
    };
    let now = pod_status::usage::now_ms();
    let mut st = CodexState::default();
    st.apply(probe.read(), now, &Cadence::default());
    let v = ServiceView::build(ServiceId::Codex, &st.provider, now);
    println!(
        "status={} compact={} windows={}",
        v.status,
        v.compact(),
        v.windows.len()
    );
    for w in &v.windows {
        println!("  {} | {} | {}", w.label, w.value, w.reset);
    }
}
