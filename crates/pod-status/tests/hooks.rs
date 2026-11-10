//! Hook event mapping (Claude / Codex) with representative stdin payloads,
//! wire message round trip, and the installer against a temporary directory.

use pod_status::hook::{
    CLAUDE_EVENTS, CODEX_EVENTS, ChildEffect, HOOK_MESSAGE_VERSION, RootEffect, map_hook,
};
use pod_status::install::{
    ApplyOptions, HookCommandSpec, InstallScope, apply_plan, claude_hooks_block, codex_hooks_block,
    is_user_agent_config, merge_hooks, plan_install, plan_uninstall, remove_pod_hooks,
};
use pod_status::{AgentState, BlockedKind, HookAgent, HookMessage, Outcome};
use serde_json::{Value, json};

use AgentState::*;

fn root(agent: HookAgent, ev: &str, p: Value) -> Option<AgentState> {
    match map_hook(agent, ev, &p).root {
        RootEffect::Set { state, .. } => Some(state),
        _ => None,
    }
}

fn claude_payload(event: &str, extra: Value) -> Value {
    let mut base = json!({
        "session_id": "0b6e3c1c-5d1e-4c3e-9a51-1f2d3c4b5a69",
        "transcript_path": "C:\\Users\\u\\.claude\\projects\\x\\0b6e.jsonl",
        "cwd": "C:\\work\\repo",
        "permission_mode": "default",
        "hook_event_name": event,
    });
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

#[test]
fn claude_mapping_table() {
    use HookAgent::Claude as C;
    let p = |ev: &str, extra: Value| claude_payload(ev, extra);
    assert_eq!(
        root(
            C,
            "SessionStart",
            p("SessionStart", json!({"source":"startup","model":"x"}))
        ),
        Some(Idle)
    );
    assert_eq!(
        root(
            C,
            "SessionStart",
            p("SessionStart", json!({"source":"resume"}))
        ),
        Some(Idle)
    );
    assert_eq!(
        root(
            C,
            "SessionStart",
            p("SessionStart", json!({"source":"compact"}))
        ),
        None,
        "compact restart ignored"
    );
    assert_eq!(
        root(
            C,
            "UserPromptSubmit",
            p("UserPromptSubmit", json!({"prompt":"fix it"}))
        ),
        Some(Working)
    );
    let pre = p(
        "PreToolUse",
        json!({"tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"toolu_1"}),
    );
    assert_eq!(root(C, "PreToolUse", pre), Some(Working));
    let ask = p(
        "PreToolUse",
        json!({"tool_name":"AskUserQuestion","tool_input":{"questions":[]}}),
    );
    assert_eq!(
        root(C, "PreToolUse", ask),
        Some(Blocked(BlockedKind::Question))
    );
    assert_eq!(
        root(
            C,
            "PostToolUse",
            p(
                "PostToolUse",
                json!({"tool_name":"Bash","tool_response":{}})
            )
        ),
        Some(Working)
    );
    assert_eq!(
        root(
            C,
            "PostToolUseFailure",
            p(
                "PostToolUseFailure",
                json!({"tool_name":"Bash","error":"x"})
            )
        ),
        Some(Working)
    );
    assert_eq!(
        root(
            C,
            "PermissionRequest",
            p(
                "PermissionRequest",
                json!({"tool_name":"Edit","permission_suggestions":[]})
            )
        ),
        Some(Blocked(BlockedKind::Permission))
    );
    assert_eq!(
        root(
            C,
            "Elicitation",
            p(
                "Elicitation",
                json!({"mcp_server_name":"gh","message":"Token?"})
            )
        ),
        Some(Blocked(BlockedKind::Question))
    );
    assert_eq!(
        map_hook(C, "ElicitationResult", &p("ElicitationResult", json!({}))).root,
        RootEffect::RestoreFromWait
    );
    let stop = map_hook(C, "Stop", &p("Stop", json!({"stop_hook_active": false})));
    assert_eq!(
        (stop.root.clone(), stop.outcome),
        (
            RootEffect::Set {
                state: Done,
                msg: None
            },
            Some(Outcome::Completed)
        )
    );
    let intr = map_hook(C, "Stop", &p("Stop", json!({"is_interrupt": true})));
    assert_eq!(intr.outcome, Some(Outcome::Cancelled));
    let fail = map_hook(
        C,
        "StopFailure",
        &p("StopFailure", json!({"error":"overloaded"})),
    );
    assert_eq!(
        fail.root,
        RootEffect::Set {
            state: Error,
            msg: Some("overloaded".into())
        }
    );
    assert_eq!(fail.outcome, Some(Outcome::Failed));
    let sub = map_hook(
        C,
        "SubagentStart",
        &p(
            "SubagentStart",
            json!({"agent_id":"ag/1 x","agent_type":"Explore"}),
        ),
    );
    assert_eq!(sub.root, RootEffect::None);
    assert_eq!(
        sub.child,
        Some(ChildEffect::Set {
            id: "hook/ag-1-x".into(),
            state: Working,
            title: Some("Explore".into())
        })
    );
    let subs = map_hook(
        C,
        "SubagentStop",
        &p("SubagentStop", json!({"agent_id":"ag/1 x"})),
    );
    assert_eq!(
        subs.child,
        Some(ChildEffect::Remove {
            id: "hook/ag-1-x".into()
        })
    );
    assert_eq!(
        map_hook(C, "SessionEnd", &p("SessionEnd", json!({"reason":"exit"}))).root,
        RootEffect::EndSession
    );
    // events inside a subagent update the child, not the lead
    let inner = map_hook(
        C,
        "PreToolUse",
        &p("PreToolUse", json!({"agent_id":"a2","tool_name":"Read"})),
    );
    assert_eq!(inner.root, RootEffect::None);
    assert!(matches!(inner.child, Some(ChildEffect::Set { .. })));
    // unknown events are harmless
    assert_eq!(
        map_hook(C, "Notification", &p("Notification", json!({}))).root,
        RootEffect::None
    );
    // session id is extracted
    assert_eq!(
        map_hook(C, "Stop", &p("Stop", json!({})))
            .session_id
            .as_deref(),
        Some("0b6e3c1c-5d1e-4c3e-9a51-1f2d3c4b5a69")
    );
}

#[test]
fn codex_mapping_table() {
    use HookAgent::Codex as X;
    let p = |ev: &str, extra: Value| {
        let mut v = json!({"session_id":"s","turn_id":"t1","transcript_path":"r.jsonl","cwd":"C:\\w","hook_event_name":ev});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    };
    assert_eq!(
        root(X, "SessionStart", p("SessionStart", json!({}))),
        Some(Idle)
    );
    assert_eq!(
        root(
            X,
            "UserPromptSubmit",
            p("UserPromptSubmit", json!({"prompt":"hi"}))
        ),
        Some(Working)
    );
    assert_eq!(
        root(
            X,
            "PreToolUse",
            p("PreToolUse", json!({"tool_name":"shell"}))
        ),
        Some(Working)
    );
    assert_eq!(
        root(
            X,
            "PreToolUse",
            p("PreToolUse", json!({"tool_name":"request_user_input"}))
        ),
        Some(Blocked(BlockedKind::Question))
    );
    assert_eq!(
        root(
            X,
            "PermissionRequest",
            p("PermissionRequest", json!({"tool_name":"shell"}))
        ),
        Some(Blocked(BlockedKind::Permission))
    );
    assert_eq!(
        root(X, "PostToolUse", p("PostToolUse", json!({}))),
        Some(Working)
    );
    assert_eq!(root(X, "Stop", p("Stop", json!({}))), Some(Done));
    let i = map_hook(X, "Interrupt", &p("Interrupt", json!({})));
    assert_eq!(
        (i.root, i.outcome),
        (
            RootEffect::Set {
                state: Idle,
                msg: None
            },
            Some(Outcome::Cancelled)
        )
    );
    assert!(matches!(
        map_hook(
            X,
            "SubagentStart",
            &p("SubagentStart", json!({"agent_id":"c1"}))
        )
        .child,
        Some(ChildEffect::Set { .. })
    ));
    assert!(matches!(
        map_hook(
            X,
            "SubagentStop",
            &p("SubagentStop", json!({"agent_id":"c1"}))
        )
        .child,
        Some(ChildEffect::Remove { .. })
    ));
    // no agent_id → nothing
    assert_eq!(
        map_hook(X, "SubagentStart", &p("SubagentStart", json!({}))).child,
        None
    );
}

#[test]
fn every_installed_event_has_a_mapping() {
    for ev in CLAUDE_EVENTS {
        let p = claude_payload(ev, json!({"agent_id": "a"}));
        let e = map_hook(HookAgent::Claude, ev, &p);
        let does_something = e.root != RootEffect::None || e.child.is_some();
        assert!(does_something, "claude {ev}");
    }
    for ev in CODEX_EVENTS {
        let e = map_hook(HookAgent::Codex, ev, &json!({"agent_id": "a"}));
        assert!(
            e.root != RootEffect::None || e.child.is_some(),
            "codex {ev}"
        );
    }
}

#[test]
fn wire_message_round_trip() {
    let mut m = HookMessage::new("term-7", HookAgent::Codex, "Stop", json!({"turn_id":"t"}));
    m.token = Some("tok".into());
    let line = m.to_line();
    assert!(!line.contains('\n'));
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["v"], json!(HOOK_MESSAGE_VERSION));
    assert_eq!(v["agent"], json!("codex"));
    assert_eq!(v["terminal_id"], json!("term-7"));
    assert_eq!(HookMessage::from_line(&format!("{line}\n")).unwrap(), m);
    // event falls back to payload.hook_event_name
    let m2 = HookMessage::new(
        "t",
        HookAgent::Claude,
        "",
        json!({"hook_event_name":"Stop"}),
    );
    assert_eq!(m2.event_name(), "Stop");
    assert_eq!(HookAgent::parse("Claude"), Some(HookAgent::Claude));
    assert_eq!(HookAgent::parse("gemini"), None);
}

// ───────────── installer ─────────────

fn spec() -> HookCommandSpec {
    HookCommandSpec::new("C:\\Program Files\\Pod\\pod-hook.exe")
}

#[test]
fn claude_block_shape() {
    let b = claude_hooks_block(&spec());
    let hooks = b["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), CLAUDE_EVENTS.len());
    let pre = &hooks["PreToolUse"][0];
    assert_eq!(pre["matcher"], json!("*"));
    assert_eq!(pre["hooks"][0]["type"], json!("command"));
    assert_eq!(
        pre["hooks"][0]["command"],
        json!("\"C:/Program Files/Pod/pod-hook.exe\" claude PreToolUse")
    );
    assert_eq!(pre["hooks"][0]["timeout"], json!(10));
    assert!(hooks["SessionStart"][0].get("matcher").is_none());
}

#[test]
fn codex_block_shape() {
    let b = codex_hooks_block(&HookCommandSpec::new("C:/pod/pod-hook.exe"));
    let hooks = b["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), CODEX_EVENTS.len());
    assert_eq!(
        hooks["Stop"][0]["hooks"][0]["command"],
        json!("C:/pod/pod-hook.exe codex Stop")
    );
    assert_eq!(hooks["Interrupt"][0]["hooks"][0]["timeout"], json!(3));
}

#[test]
fn merge_keeps_foreign_hooks_and_is_idempotent() {
    let foreign = json!({
        "model": "opus",
        "hooks": {
            "Stop": [{"hooks": [{"type":"command","command":"C:/Users/u/.other-ide/hooks/foreign-hook.exe claude","timeout":10}]}],
            "Notification": [{"hooks": [{"type":"command","command":"notify.cmd"}]}]
        }
    });
    let block = claude_hooks_block(&spec());
    let once = merge_hooks(foreign.clone(), &block);
    let twice = merge_hooks(once.clone(), &block);
    assert_eq!(once, twice, "idempotent");
    assert_eq!(once["model"], json!("opus"));
    assert_eq!(
        once["hooks"]["Stop"].as_array().unwrap().len(),
        2,
        "foreign + pod"
    );
    assert_eq!(once["hooks"]["Notification"].as_array().unwrap().len(), 1);
    let removed = remove_pod_hooks(once);
    assert_eq!(removed, foreign, "uninstall restores the original");
}

#[test]
fn plan_and_apply_in_temp_dir_global_and_profile() {
    let tmp = tempfile::tempdir().unwrap();
    // global layout under a fake home
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/settings.json"), r#"{"theme":"dark"}"#).unwrap();
    let scope = InstallScope::Global { home: home.clone() };
    let plan = plan_install(&scope, &spec(), &[HookAgent::Claude, HookAgent::Codex]).unwrap();
    assert_eq!(plan.files.len(), 2);
    assert!(plan.files[0].existed && !plan.files[1].existed);
    assert!(plan.launch_env.is_empty());
    // planning writes nothing
    assert!(!home.join(".codex/hooks.json").exists());
    apply_plan(&plan, ApplyOptions::default()).unwrap();
    let claude: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(claude["theme"], json!("dark"));
    assert_eq!(
        claude["hooks"].as_object().unwrap().len(),
        CLAUDE_EVENTS.len()
    );
    let codex: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".codex/hooks.json")).unwrap())
            .unwrap();
    assert_eq!(
        codex["hooks"].as_object().unwrap().len(),
        CODEX_EVENTS.len()
    );
    // re-plan is unchanged
    let again = plan_install(&scope, &spec(), &[HookAgent::Claude, HookAgent::Codex]).unwrap();
    assert!(again.files.iter().all(|f| f.unchanged));
    // uninstall
    let un = plan_uninstall(&scope, &spec(), &[HookAgent::Claude]).unwrap();
    apply_plan(&un, ApplyOptions::default()).unwrap();
    let claude: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(claude, json!({"theme":"dark","hooks":{}}));

    // separate profile (CLAUDE_CONFIG_DIR / CODEX_HOME)
    let scope = InstallScope::Profile {
        claude_config_dir: tmp.path().join("pc"),
        codex_home: tmp.path().join("px"),
    };
    let plan = plan_install(&scope, &spec(), &[HookAgent::Claude, HookAgent::Codex]).unwrap();
    assert_eq!(
        plan.files[0].path,
        tmp.path().join("pc").join("settings.json")
    );
    assert_eq!(plan.files[1].path, tmp.path().join("px").join("hooks.json"));
    let keys: Vec<&str> = plan.launch_env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["CLAUDE_CONFIG_DIR", "CODEX_HOME"]);
    apply_plan(&plan, ApplyOptions::default()).unwrap();
    assert!(tmp.path().join("px/hooks.json").exists());
}

#[test]
fn apply_refuses_real_user_config() {
    let Some(home) = pod_status::install::user_home() else {
        return;
    };
    let scope = InstallScope::Global { home: home.clone() };
    assert!(is_user_agent_config(
        &home.join(".claude").join("settings.json")
    ));
    assert!(is_user_agent_config(
        &home.join(".codex").join("hooks.json")
    ));
    assert!(!is_user_agent_config(
        &home.join("pod-test").join("settings.json")
    ));
    // plan only reads; build a plan by hand so the test never depends on the real files
    let plan = pod_status::install::InstallPlan {
        scope,
        files: vec![pod_status::install::PlannedFile {
            agent: HookAgent::Claude,
            path: home.join(".claude").join("settings.json"),
            existed: true,
            content: "{}\n".into(),
            unchanged: false,
        }],
        launch_env: vec![],
        notes: vec![],
    };
    let err = apply_plan(&plan, ApplyOptions::default()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn malformed_existing_file_is_an_error_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
    let scope = InstallScope::Profile {
        claude_config_dir: tmp.path().into(),
        codex_home: tmp.path().join("x"),
    };
    assert!(plan_install(&scope, &spec(), &[HookAgent::Claude]).is_err());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("settings.json")).unwrap(),
        "{ not json"
    );
}
