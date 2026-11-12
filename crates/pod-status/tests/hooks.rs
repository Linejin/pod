//! Hook event mapping (Claude / Codex) with representative stdin payloads,
//! wire message round trip, and the installer against a temporary directory.

use pod_status::hook::{
    CLAUDE_EVENTS, CODEX_EVENTS, ChildEffect, HOOK_MESSAGE_VERSION, HookEffect, RootEffect,
    map_hook, map_message,
};
use pod_status::install::{
    ApplyOptions, HookCommandSpec, InstallScope, apply_plan, claude_hooks_block, codex_hooks_block,
    is_user_agent_config, merge_hooks, plan_install, plan_uninstall, remove_pod_hooks,
};
use std::path::Path;

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

/// Every public `hook::` path podd, pod-hook and the installer name, and the
/// installed event lists in order (refactor D-P7 keeps them on the facade).
#[test]
fn hook_public_paths_and_event_lists() {
    use pod_status::hook::{
        CLAUDE_MATCHER_EVENTS, ENV_HOOK_TOKEN, ENV_SOCKET, ENV_TERMINAL_ID, MAX_HOOK_LINE,
        PAYLOAD_FIELDS, QUESTION_TOOLS, TRUNCATED_KEY,
    };
    assert_eq!(
        (ENV_TERMINAL_ID, ENV_SOCKET, ENV_HOOK_TOKEN),
        ("POD_TERMINAL_ID", "POD_SOCKET", "POD_HOOK_TOKEN")
    );
    assert_eq!(
        (MAX_HOOK_LINE, TRUNCATED_KEY, HOOK_MESSAGE_VERSION),
        (4 * 1024 * 1024, "pod_truncated", 1)
    );
    assert_eq!(QUESTION_TOOLS, ["AskUserQuestion", "request_user_input"]);
    assert_eq!(PAYLOAD_FIELDS.len(), 22);
    assert_eq!(
        CLAUDE_EVENTS,
        [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "Elicitation",
            "ElicitationResult",
            "Stop",
            "StopFailure",
            "SubagentStart",
            "SubagentStop",
            "SessionEnd",
        ]
    );
    assert_eq!(
        CODEX_EVENTS,
        [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "Interrupt",
            "SubagentStart",
            "SubagentStop",
        ]
    );
    assert_eq!(
        CLAUDE_MATCHER_EVENTS,
        [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "Elicitation",
            "ElicitationResult",
        ]
    );
    let _: fn(HookAgent, &str, &Value) -> HookEffect = map_hook;
    let _: fn(&HookMessage) -> HookEffect = map_message;
    assert_eq!(HookAgent::parse("claude-code"), Some(HookAgent::Claude));
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

/// TM-22: only an absolute local `pod-hook.exe` without shell-expanded
/// characters is written into agent configs; the plan refuses anything
/// else (uninstall plans do not need an executable).
#[test]
fn hook_exe_must_be_absolute_plain_pod_hook() {
    use pod_status::install::check_hook_exe;
    for ok in [
        r"C:\Users\u\AppData\Local\Pod\bin\pod-hook.exe",
        "C:/Program Files (x86)/Pod & Co/pod-hook.exe",
        r"D:\사용자\Pod\bin\POD-HOOK.EXE",
    ] {
        assert!(check_hook_exe(Path::new(ok)).is_ok(), "{ok}");
    }
    for bad in [
        "pod-hook.exe",
        r"bin\pod-hook.exe",
        r"\\server\share\pod-hook.exe",
        "//server/share/pod-hook.exe",
        r"\\?\C:\pod\pod-hook.exe",
        r"C:\pod\evil.exe",
        r"C:\pod\pod-hook.exe.cmd",
        r"C:\100%\pod-hook.exe",
        r"C:\$HOME\pod-hook.exe",
        r"C:\a`b\pod-hook.exe",
        r"C:\a!b\pod-hook.exe",
        "C:\\a\nb\\pod-hook.exe",
    ] {
        assert!(check_hook_exe(Path::new(bad)).is_err(), "{bad:?}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let scope = InstallScope::Profile {
        claude_config_dir: tmp.path().join("c"),
        codex_home: tmp.path().join("x"),
    };
    let relative = HookCommandSpec::new("pod-hook.exe");
    let err = plan_install(&scope, &relative, &[HookAgent::Claude]).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{err}");
    assert!(plan_uninstall(&scope, &relative, &[HookAgent::Claude]).is_ok());
}

/// Paths with anything but letters, digits and `-_./:` are quoted (a `&`
/// or `(` would split the command in cmd/bash otherwise); plain ones stay
/// as they were, so existing installs stay unchanged.
#[test]
fn hook_command_quoting() {
    let cmd = |p: &str| HookCommandSpec::new(p).command(HookAgent::Claude, "Stop");
    assert_eq!(
        cmd(r"C:\Users\u\AppData\Local\Pod\bin\pod-hook.exe"),
        "C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe claude Stop"
    );
    assert_eq!(
        cmd(r"D:\사용자\bin\pod-hook.exe"),
        "D:/사용자/bin/pod-hook.exe claude Stop"
    );
    assert_eq!(
        cmd(r"C:\Tom&Jerry\pod-hook.exe"),
        "\"C:/Tom&Jerry/pod-hook.exe\" claude Stop"
    );
    assert_eq!(
        cmd(r"C:\Program Files\Pod\pod-hook.exe"),
        "\"C:/Program Files/Pod/pod-hook.exe\" claude Stop"
    );
}

/// A command is Pod's only in Pod's exact shape; another tool whose command
/// merely mentions `pod-hook` keeps its hook through install and uninstall.
#[test]
fn only_exact_pod_commands_are_pods() {
    use pod_status::install::is_pod_command;
    for pod in [
        "C:/Users/u/AppData/Local/Pod/bin/pod-hook.exe claude Stop",
        "\"C:/Program Files/Pod/pod-hook.exe\" codex PermissionRequest",
        r"C:\old\POD-HOOK.EXE claude SessionStart",
        "pod-hook claude Stop",
    ] {
        assert!(is_pod_command(pod), "{pod}");
    }
    for other in [
        "C:/tools/my-pod-hook-logger.exe claude Stop",
        "C:/tools/notify.cmd --after pod-hook",
        "C:/pod/pod-hook.exe claude Stop && evil.exe",
        "C:/pod/pod-hook.exe gemini Stop",
        "C:/pod/pod-hook.exe claude",
        "\"C:/pod/pod-hook.exe claude Stop",
        "C:/pod/pod-hook.exe.cmd claude Stop",
    ] {
        assert!(!is_pod_command(other), "{other}");
    }
    let foreign = json!({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "C:/tools/my-pod-hook-logger.exe claude Stop"}]},
        {"hooks": [{"type": "command", "command": "C:/tools/notify.cmd --after pod-hook"}]}
    ]}});
    let installed = merge_hooks(foreign.clone(), &claude_hooks_block(&spec()));
    assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 3);
    assert_eq!(remove_pod_hooks(installed), foreign);
}

// ───────────── pinned mapping matrix (refactor D-P7) ─────────────
//
// Every installed event of both agents (plus an unknown and an empty event
// name) against a set of payload variants, rendered field by field. The
// golden lines were taken from the mapping before it was split into
// `hook/{claude,codex}.rs`; any difference is a behaviour change.

fn render(eff: &HookEffect) -> String {
    let mut out = match &eff.root {
        RootEffect::None => "-".to_string(),
        RootEffect::Set { state, msg } => match msg {
            Some(m) => format!("set {state:?} {m:?}"),
            None => format!("set {state:?}"),
        },
        RootEffect::RestoreFromWait => "restore".into(),
        RootEffect::EndSession => "end".into(),
    };
    match &eff.child {
        None => {}
        Some(ChildEffect::Set { id, state, title }) => {
            out.push_str(&format!(" child=set {id} {state:?} {title:?}"));
        }
        Some(ChildEffect::Remove { id }) => out.push_str(&format!(" child=rm {id}")),
    }
    if eff.session_boundary {
        out.push_str(" boundary");
    }
    if let Some(o) = eff.outcome {
        out.push_str(&format!(" outcome={o:?}"));
    }
    if let Some(s) = &eff.session_id {
        out.push_str(&format!(" sid={s}"));
    }
    if let Some(t) = &eff.tool_name {
        out.push_str(&format!(" tool={t}"));
    }
    out
}

fn matrix_variants() -> Vec<(&'static str, Value)> {
    let lead = json!({
        "session_id": "s1",
        "tool_name": "Bash",
        "source": "startup",
        "message": "Pick one",
        "error": "overloaded",
        "agent_type": "Explore",
        "is_interrupt": false,
    });
    let mut sub = lead.clone();
    sub["agent_id"] = json!("a/1 x");
    vec![
        ("bare", json!({})),
        ("lead", lead),
        ("sub", sub),
        (
            "alt",
            json!({
                "session_id": "",
                "agent_id": "",
                "tool_name": "AskUserQuestion",
                "source": "compact",
                "is_interrupt": true,
                "error_message": "rate limit",
            }),
        ),
        (
            "rui",
            json!({"tool_name": "request_user_input", "agent_id": 7}),
        ),
    ]
}

fn mapping_matrix() -> Vec<String> {
    let mut events: Vec<&str> = CLAUDE_EVENTS.to_vec();
    for e in CODEX_EVENTS {
        if !events.contains(e) {
            events.push(e);
        }
    }
    events.extend(["Notification", ""]);
    let mut lines = Vec::new();
    for agent in [HookAgent::Claude, HookAgent::Codex] {
        for ev in &events {
            for (name, p) in matrix_variants() {
                let eff = map_hook(agent, ev, &p);
                lines.push(format!(
                    "{} {ev:?} {name}: {}",
                    agent.as_str(),
                    render(&eff)
                ));
            }
        }
        // `map_message` falls back to the payload's event name.
        let m = HookMessage::new("t1", agent, "", json!({"hook_event_name": "Stop"}));
        lines.push(format!(
            "{} message-fallback: {}",
            agent.as_str(),
            render(&map_message(&m))
        ));
    }
    lines
}

#[test]
fn pinned_mapping_matrix() {
    let actual = mapping_matrix();
    let expected: Vec<&str> = MAPPING_GOLDEN
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if actual != expected {
        let first = actual
            .iter()
            .zip(&expected)
            .position(|(a, e)| a != e)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "mapping differs at line {first}\n expected: {:?}\n   actual: {:?}\n\nactual matrix:\n{}",
            expected.get(first),
            actual.get(first),
            actual.join("\n")
        );
    }
}

const MAPPING_GOLDEN: &str = r##"
claude "SessionStart" bare: set Idle boundary
claude "SessionStart" lead: set Idle boundary sid=s1 tool=Bash
claude "SessionStart" sub: set Idle boundary sid=s1 tool=Bash
claude "SessionStart" alt: - tool=AskUserQuestion
claude "SessionStart" rui: set Idle boundary tool=request_user_input
claude "UserPromptSubmit" bare: set Working
claude "UserPromptSubmit" lead: set Working sid=s1 tool=Bash
claude "UserPromptSubmit" sub: set Working sid=s1 tool=Bash
claude "UserPromptSubmit" alt: set Working tool=AskUserQuestion
claude "UserPromptSubmit" rui: set Working tool=request_user_input
claude "PreToolUse" bare: set Working
claude "PreToolUse" lead: set Working sid=s1 tool=Bash
claude "PreToolUse" sub: - child=set hook/a-1-x Working None sid=s1 tool=Bash
claude "PreToolUse" alt: set Blocked(Question) "AskUserQuestion" tool=AskUserQuestion
claude "PreToolUse" rui: set Blocked(Question) "request_user_input" tool=request_user_input
claude "PostToolUse" bare: set Working
claude "PostToolUse" lead: set Working sid=s1 tool=Bash
claude "PostToolUse" sub: - child=set hook/a-1-x Working None sid=s1 tool=Bash
claude "PostToolUse" alt: set Working tool=AskUserQuestion
claude "PostToolUse" rui: set Working tool=request_user_input
claude "PostToolUseFailure" bare: set Working
claude "PostToolUseFailure" lead: set Working sid=s1 tool=Bash
claude "PostToolUseFailure" sub: - child=set hook/a-1-x Working None sid=s1 tool=Bash
claude "PostToolUseFailure" alt: set Working tool=AskUserQuestion
claude "PostToolUseFailure" rui: set Working tool=request_user_input
claude "PermissionRequest" bare: set Blocked(Permission)
claude "PermissionRequest" lead: set Blocked(Permission) "Bash" sid=s1 tool=Bash
claude "PermissionRequest" sub: set Blocked(Permission) "Bash" sid=s1 tool=Bash
claude "PermissionRequest" alt: set Blocked(Permission) "AskUserQuestion" tool=AskUserQuestion
claude "PermissionRequest" rui: set Blocked(Permission) "request_user_input" tool=request_user_input
claude "Elicitation" bare: set Blocked(Question)
claude "Elicitation" lead: set Blocked(Question) "Pick one" sid=s1 tool=Bash
claude "Elicitation" sub: set Blocked(Question) "Pick one" sid=s1 tool=Bash
claude "Elicitation" alt: set Blocked(Question) tool=AskUserQuestion
claude "Elicitation" rui: set Blocked(Question) tool=request_user_input
claude "ElicitationResult" bare: restore
claude "ElicitationResult" lead: restore sid=s1 tool=Bash
claude "ElicitationResult" sub: restore sid=s1 tool=Bash
claude "ElicitationResult" alt: restore tool=AskUserQuestion
claude "ElicitationResult" rui: restore tool=request_user_input
claude "Stop" bare: set Done outcome=Completed
claude "Stop" lead: set Done outcome=Completed sid=s1 tool=Bash
claude "Stop" sub: set Done outcome=Completed sid=s1 tool=Bash
claude "Stop" alt: set Idle outcome=Cancelled tool=AskUserQuestion
claude "Stop" rui: set Done outcome=Completed tool=request_user_input
claude "StopFailure" bare: set Error outcome=Failed
claude "StopFailure" lead: set Error "overloaded" outcome=Failed sid=s1 tool=Bash
claude "StopFailure" sub: set Error "overloaded" outcome=Failed sid=s1 tool=Bash
claude "StopFailure" alt: set Error "rate limit" outcome=Failed tool=AskUserQuestion
claude "StopFailure" rui: set Error outcome=Failed tool=request_user_input
claude "SubagentStart" bare: -
claude "SubagentStart" lead: - sid=s1 tool=Bash
claude "SubagentStart" sub: - child=set hook/a-1-x Working Some("Explore") sid=s1 tool=Bash
claude "SubagentStart" alt: - tool=AskUserQuestion
claude "SubagentStart" rui: - tool=request_user_input
claude "SubagentStop" bare: -
claude "SubagentStop" lead: - sid=s1 tool=Bash
claude "SubagentStop" sub: - child=rm hook/a-1-x sid=s1 tool=Bash
claude "SubagentStop" alt: - tool=AskUserQuestion
claude "SubagentStop" rui: - tool=request_user_input
claude "SessionEnd" bare: end
claude "SessionEnd" lead: end sid=s1 tool=Bash
claude "SessionEnd" sub: end sid=s1 tool=Bash
claude "SessionEnd" alt: end tool=AskUserQuestion
claude "SessionEnd" rui: end tool=request_user_input
claude "Interrupt" bare: -
claude "Interrupt" lead: - sid=s1 tool=Bash
claude "Interrupt" sub: - sid=s1 tool=Bash
claude "Interrupt" alt: - tool=AskUserQuestion
claude "Interrupt" rui: - tool=request_user_input
claude "Notification" bare: -
claude "Notification" lead: - sid=s1 tool=Bash
claude "Notification" sub: - sid=s1 tool=Bash
claude "Notification" alt: - tool=AskUserQuestion
claude "Notification" rui: - tool=request_user_input
claude "" bare: -
claude "" lead: - sid=s1 tool=Bash
claude "" sub: - sid=s1 tool=Bash
claude "" alt: - tool=AskUserQuestion
claude "" rui: - tool=request_user_input
claude message-fallback: set Done outcome=Completed
codex "SessionStart" bare: set Idle boundary
codex "SessionStart" lead: set Idle boundary sid=s1 tool=Bash
codex "SessionStart" sub: set Idle boundary sid=s1 tool=Bash
codex "SessionStart" alt: set Idle boundary tool=AskUserQuestion
codex "SessionStart" rui: set Idle boundary tool=request_user_input
codex "UserPromptSubmit" bare: set Working
codex "UserPromptSubmit" lead: set Working sid=s1 tool=Bash
codex "UserPromptSubmit" sub: set Working sid=s1 tool=Bash
codex "UserPromptSubmit" alt: set Working tool=AskUserQuestion
codex "UserPromptSubmit" rui: set Working tool=request_user_input
codex "PreToolUse" bare: set Working
codex "PreToolUse" lead: set Working sid=s1 tool=Bash
codex "PreToolUse" sub: set Working sid=s1 tool=Bash
codex "PreToolUse" alt: set Blocked(Question) "AskUserQuestion" tool=AskUserQuestion
codex "PreToolUse" rui: set Blocked(Question) "request_user_input" tool=request_user_input
codex "PostToolUse" bare: set Working
codex "PostToolUse" lead: set Working sid=s1 tool=Bash
codex "PostToolUse" sub: set Working sid=s1 tool=Bash
codex "PostToolUse" alt: set Working tool=AskUserQuestion
codex "PostToolUse" rui: set Working tool=request_user_input
codex "PostToolUseFailure" bare: -
codex "PostToolUseFailure" lead: - sid=s1 tool=Bash
codex "PostToolUseFailure" sub: - sid=s1 tool=Bash
codex "PostToolUseFailure" alt: - tool=AskUserQuestion
codex "PostToolUseFailure" rui: - tool=request_user_input
codex "PermissionRequest" bare: set Blocked(Permission)
codex "PermissionRequest" lead: set Blocked(Permission) "Bash" sid=s1 tool=Bash
codex "PermissionRequest" sub: set Blocked(Permission) "Bash" sid=s1 tool=Bash
codex "PermissionRequest" alt: set Blocked(Permission) "AskUserQuestion" tool=AskUserQuestion
codex "PermissionRequest" rui: set Blocked(Permission) "request_user_input" tool=request_user_input
codex "Elicitation" bare: -
codex "Elicitation" lead: - sid=s1 tool=Bash
codex "Elicitation" sub: - sid=s1 tool=Bash
codex "Elicitation" alt: - tool=AskUserQuestion
codex "Elicitation" rui: - tool=request_user_input
codex "ElicitationResult" bare: -
codex "ElicitationResult" lead: - sid=s1 tool=Bash
codex "ElicitationResult" sub: - sid=s1 tool=Bash
codex "ElicitationResult" alt: - tool=AskUserQuestion
codex "ElicitationResult" rui: - tool=request_user_input
codex "Stop" bare: set Done outcome=Completed
codex "Stop" lead: set Done outcome=Completed sid=s1 tool=Bash
codex "Stop" sub: set Done outcome=Completed sid=s1 tool=Bash
codex "Stop" alt: set Done outcome=Completed tool=AskUserQuestion
codex "Stop" rui: set Done outcome=Completed tool=request_user_input
codex "StopFailure" bare: -
codex "StopFailure" lead: - sid=s1 tool=Bash
codex "StopFailure" sub: - sid=s1 tool=Bash
codex "StopFailure" alt: - tool=AskUserQuestion
codex "StopFailure" rui: - tool=request_user_input
codex "SubagentStart" bare: -
codex "SubagentStart" lead: - sid=s1 tool=Bash
codex "SubagentStart" sub: - child=set hook/a-1-x Working Some("Explore") sid=s1 tool=Bash
codex "SubagentStart" alt: - tool=AskUserQuestion
codex "SubagentStart" rui: - tool=request_user_input
codex "SubagentStop" bare: -
codex "SubagentStop" lead: - sid=s1 tool=Bash
codex "SubagentStop" sub: - child=rm hook/a-1-x sid=s1 tool=Bash
codex "SubagentStop" alt: - tool=AskUserQuestion
codex "SubagentStop" rui: - tool=request_user_input
codex "SessionEnd" bare: -
codex "SessionEnd" lead: - sid=s1 tool=Bash
codex "SessionEnd" sub: - sid=s1 tool=Bash
codex "SessionEnd" alt: - tool=AskUserQuestion
codex "SessionEnd" rui: - tool=request_user_input
codex "Interrupt" bare: set Idle outcome=Cancelled
codex "Interrupt" lead: set Idle outcome=Cancelled sid=s1 tool=Bash
codex "Interrupt" sub: set Idle outcome=Cancelled sid=s1 tool=Bash
codex "Interrupt" alt: set Idle outcome=Cancelled tool=AskUserQuestion
codex "Interrupt" rui: set Idle outcome=Cancelled tool=request_user_input
codex "Notification" bare: -
codex "Notification" lead: - sid=s1 tool=Bash
codex "Notification" sub: - sid=s1 tool=Bash
codex "Notification" alt: - tool=AskUserQuestion
codex "Notification" rui: - tool=request_user_input
codex "" bare: -
codex "" lead: - sid=s1 tool=Bash
codex "" sub: - sid=s1 tool=Bash
codex "" alt: - tool=AskUserQuestion
codex "" rui: - tool=request_user_input
codex message-fallback: set Done outcome=Completed
"##;
