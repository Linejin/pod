//! JSON golden of the extension wire types (refactor card L-L7,
//! docs/20-refactor/libs.md): `ModInfo`, `ModList`, `CommandInfo` and
//! `StopReason` as built by hand and as the mod thread reports them, plus the
//! persisted state file. Fixed before `manager.rs` was split; compared as
//! text, so field order, `null` against a missing key, enum strings and error
//! messages all count. Only the mods folder path and the QuickJS heap size
//! are normalized.

use pod_mods::*;

fn same(what: &str, actual: &str, golden: &str) {
    assert!(
        actual == golden,
        "{what} changed\n--- actual ---\n{actual}\n--- golden ---\n{golden}\n---"
    );
}

fn pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

#[test]
fn stop_reasons_are_snake_case_strings() {
    let all = [
        StopReason::Cpu,
        StopReason::Memory,
        StopReason::Errors,
        StopReason::Load,
    ];
    same(
        "StopReason",
        &serde_json::to_string(&all).unwrap(),
        r#"["cpu","memory","errors","load"]"#,
    );
    for r in all {
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<StopReason>(&text).unwrap(), r);
    }
    assert!(serde_json::from_str::<StopReason>(r#""Cpu""#).is_err());
}

#[test]
fn command_info_skips_a_missing_detail() {
    let with = CommandInfo {
        id: "hello".into(),
        title: "인사".into(),
        detail: Some("알림을 띄운다".into()),
    };
    let without = CommandInfo {
        id: "fail".into(),
        title: "Fail".into(),
        detail: None,
    };
    same(
        "CommandInfo",
        &serde_json::to_string(&[&with, &without]).unwrap(),
        r#"[{"id":"hello","title":"인사","detail":"알림을 띄운다"},{"id":"fail","title":"Fail"}]"#,
    );
    let back: CommandInfo = serde_json::from_str(r#"{"id":"fail","title":"Fail"}"#).unwrap();
    assert_eq!(back, without);
    let back: CommandInfo =
        serde_json::from_str(r#"{"id":"fail","title":"Fail","detail":null}"#).unwrap();
    assert_eq!(back, without);
}

fn sample_info() -> ModInfo {
    ModInfo {
        name: "alpha".into(),
        version: "1.2.0".into(),
        title: Some("Alpha".into()),
        description: None,
        enabled: true,
        state: ModState::Stopped,
        stop_reason: Some(StopReason::Errors),
        error: Some("Error: boom".into()),
        permissions: vec![
            PermissionInfo {
                name: Permission::Notify,
                requested: true,
                granted: true,
            },
            PermissionInfo {
                name: Permission::TerminalSend,
                requested: true,
                granted: false,
            },
        ],
        checks: vec![CheckInfo {
            name: "test".into(),
            argv: vec!["cargo".into(), "test".into()],
        }],
        commands: vec![CommandInfo {
            id: "go".into(),
            title: "Go".into(),
            detail: None,
        }],
        detectors: vec![DetectorInfo {
            names: vec!["aider".into()],
        }],
        layouts: Vec::new(),
        log_lines: 4,
        memory_kib: 0,
    }
}

const SAMPLE_LIST: &str = r#"{
  "root": "C:\\mods",
  "mods": [
    {
      "name": "alpha",
      "version": "1.2.0",
      "title": "Alpha",
      "enabled": true,
      "state": "stopped",
      "stop_reason": "errors",
      "error": "Error: boom",
      "permissions": [
        {
          "name": "notify",
          "requested": true,
          "granted": true
        },
        {
          "name": "terminal.send",
          "requested": true,
          "granted": false
        }
      ],
      "checks": [
        {
          "name": "test",
          "argv": [
            "cargo",
            "test"
          ]
        }
      ],
      "commands": [
        {
          "id": "go",
          "title": "Go"
        }
      ],
      "detectors": [
        {
          "match": [
            "aider"
          ]
        }
      ],
      "layouts": [],
      "log_lines": 4,
      "memory_kib": 0
    }
  ],
  "dropped_events": 7
}"#;

#[test]
fn mod_list_by_hand() {
    let list = ModList {
        root: r"C:\mods".into(),
        mods: vec![sample_info()],
        dropped_events: 7,
    };
    same("ModList", &pretty(&list), SAMPLE_LIST);
    let back: ModList = serde_json::from_str(SAMPLE_LIST).unwrap();
    assert_eq!(back, list);
}

#[test]
fn mod_info_defaults_for_older_peers() {
    // The optional and defaulted fields may be missing; the rest may not.
    let min = r#"{"name":"m","version":"","enabled":false,"state":"disabled","permissions":[],"log_lines":0,"memory_kib":0}"#;
    let info: ModInfo = serde_json::from_str(min).unwrap();
    same(
        "ModInfo (minimal)",
        &serde_json::to_string(&info).unwrap(),
        r#"{"name":"m","version":"","enabled":false,"state":"disabled","permissions":[],"checks":[],"commands":[],"detectors":[],"layouts":[],"log_lines":0,"memory_kib":0}"#,
    );
    let err = serde_json::from_str::<ModInfo>(&min.replace(r#","log_lines":0"#, ""))
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("missing field `log_lines`"), "{err}");
    let err = serde_json::from_str::<ModInfo>(&min.replace("disabled", "paused"))
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("unknown variant `paused`"), "{err}");
    let list: ModList =
        serde_json::from_str(r#"{"root":"","mods":[],"dropped_events":0}"#).unwrap();
    assert_eq!(list, ModList::default());
}

/// The same types as the mod thread fills them, from fixed mods.
#[cfg(feature = "runtime")]
mod from_the_manager {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    struct NoHost;

    impl Host for NoHost {
        fn terminals(&self) -> Vec<TerminalFacts> {
            Vec::new()
        }
        fn tasks(&self) -> Vec<TaskFacts> {
            Vec::new()
        }
        fn groups(&self) -> Vec<GroupFacts> {
            Vec::new()
        }
        fn notify(&self, _: Notice) {}
        fn send_text(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn set_status(&self, _: &str, _: &str, _: Option<StatusHint>) -> Result<(), String> {
            Ok(())
        }
        fn clear_status(&self, _: &str) {}
        fn run_check(&self, _: &str, _: &str, _: &str, _: Vec<String>) -> Result<(), String> {
            Ok(())
        }
        fn request_gate(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, String> {
            Ok("g1".into())
        }
        fn mod_changed(&self, _: &ModInfo) {}
    }

    fn write(root: &Path, folder: &str, manifest: serde_json::Value, js: &str) {
        let d = root.join(folder);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mod.json"), manifest.to_string()).unwrap();
        std::fs::write(d.join("main.js"), js).unwrap();
    }

    fn fixture(root: &Path) {
        write(
            root,
            "alpha",
            json!({"pod": 1, "name": "alpha", "version": "1.2.0", "title": "Alpha",
                "description": "Golden fixture", "entry": "main.js",
                "permissions": ["notify", "commands", "status", "terminal.send", "check"],
                "checks": {"test": ["cargo", "test"], "lint": ["cargo", "clippy", "-q"]},
                "layouts": ["layouts/two.json"]}),
            r#"
            pod.on("task.changed", () => {});
            pod.on("*", () => {});
            pod.command({ id: "hello", title: "인사", detail: "알림을 띄운다" }, () => {});
            pod.command({ id: "long", title: "A very long command title that goes well past sixty characters" }, () => {});
            pod.detector({ match: ["C:\\Tools\\Aider.EXE", "claude"], onScreen: () => null });
            pod.log("line one\nline two");
            "#,
        );
        let lay = root.join("alpha").join("layouts");
        std::fs::create_dir_all(&lay).unwrap();
        std::fs::write(
            lay.join("two.json"),
            r#"{"id":"two","title":"둘","center":{"split":"h","children":[{"panel":"terminal","index":0,"size":0.5},{"tabs":[{"panel":"review"},{"panel":"editor"}]}]}}"#,
        )
        .unwrap();
        write(
            root,
            "boom",
            json!({"pod": 1, "name": "boom", "version": "0.1.0", "entry": "main.js"}),
            r#"throw new Error("boom at load");"#,
        );
        write(
            root,
            "spin",
            json!({"pod": 1, "name": "spin", "version": "0.1.0", "entry": "main.js"}),
            r#"pod.on("task.changed", () => { for (;;) {} });"#,
        );
        write(
            root,
            "flaky",
            json!({"pod": 1, "name": "flaky", "version": "0.1.0", "entry": "main.js"}),
            r#"let n = 0; pod.on("task.changed", () => { n++; throw new Error("fail " + n); });"#,
        );
        write(
            root,
            "quiet",
            json!({"pod": 1, "name": "quiet", "version": "3", "entry": "main.js",
                "permissions": ["gate", "orch"]}),
            "",
        );
        write(
            root,
            "Bad Name",
            json!({"pod": 1, "name": "x", "version": "1"}),
            "",
        );
    }

    /// `list()` with the root and the heap sizes normalized (running mods
    /// must report a heap, the others none).
    fn normalized(h: &ModHandle) -> ModList {
        let mut list = h.list().unwrap();
        list.root = "<root>".into();
        for m in &mut list.mods {
            if m.state == ModState::Running && m.name != "quiet" {
                assert!(m.memory_kib > 0, "{}: no heap while running", m.name);
            } else if m.state != ModState::Running {
                assert_eq!(m.memory_kib, 0, "{}", m.name);
            }
            m.memory_kib = 0;
        }
        list
    }

    #[test]
    fn mod_list_from_the_mod_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fixture(root);
        let cfg = ModConfig {
            root: root.to_path_buf(),
            log_dir: None,
            limits: Limits {
                load_ms: 500,
                call_ms: 150,
                ..Limits::default()
            },
        };
        let h = ModHandle::start(cfg, Arc::new(NoHost));
        same(
            "ModList (discovered)",
            &pretty(&normalized(&h)),
            GOLDEN_DISCOVERED,
        );

        for m in ["alpha", "boom", "spin", "flaky", "quiet"] {
            h.set_enabled(m, true).unwrap();
        }
        h.grant("alpha", Permission::TerminalSend, true).unwrap();
        h.grant("quiet", Permission::Orch, true).unwrap();
        for _ in 0..3 {
            h.post_event("task.changed", Scope::default(), &json!({"task": "repo/a"}));
        }
        // Both stopped mods report through the same list round trip.
        let stopped = pod_test_support::wait::until(
            Duration::from_secs(10),
            Duration::from_millis(10),
            || {
                let l = h.list().unwrap();
                let done = ["spin", "flaky"].iter().all(|n| {
                    l.mods
                        .iter()
                        .any(|m| m.name == *n && m.state == ModState::Stopped)
                });
                if done { Ok(()) } else { Err(l) }
            },
        );
        stopped.unwrap();
        same(
            "ModList (enabled)",
            &pretty(&normalized(&h)),
            GOLDEN_ENABLED,
        );
        same(
            "state file",
            &std::fs::read_to_string(root.join(STATE_FILE)).unwrap(),
            GOLDEN_STATE_FILE,
        );
        h.shutdown();
    }

    const GOLDEN_DISCOVERED: &str = r#"{
  "root": "<root>",
  "mods": [
    {
      "name": "Bad Name",
      "version": "",
      "enabled": false,
      "state": "invalid",
      "error": "folder name \"Bad Name\" is not a valid mod name",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "alpha",
      "version": "1.2.0",
      "title": "Alpha",
      "description": "Golden fixture",
      "enabled": false,
      "state": "disabled",
      "permissions": [
        {
          "name": "notify",
          "requested": true,
          "granted": false
        },
        {
          "name": "commands",
          "requested": true,
          "granted": false
        },
        {
          "name": "status",
          "requested": true,
          "granted": false
        },
        {
          "name": "terminal.send",
          "requested": true,
          "granted": false
        },
        {
          "name": "check",
          "requested": true,
          "granted": false
        }
      ],
      "checks": [
        {
          "name": "lint",
          "argv": [
            "cargo",
            "clippy",
            "-q"
          ]
        },
        {
          "name": "test",
          "argv": [
            "cargo",
            "test"
          ]
        }
      ],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "boom",
      "version": "0.1.0",
      "enabled": false,
      "state": "disabled",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "flaky",
      "version": "0.1.0",
      "enabled": false,
      "state": "disabled",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "quiet",
      "version": "3",
      "enabled": false,
      "state": "disabled",
      "permissions": [
        {
          "name": "gate",
          "requested": true,
          "granted": false
        },
        {
          "name": "orch",
          "requested": true,
          "granted": false
        }
      ],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "spin",
      "version": "0.1.0",
      "enabled": false,
      "state": "disabled",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    }
  ],
  "dropped_events": 0
}"#;
    const GOLDEN_ENABLED: &str = r#"{
  "root": "<root>",
  "mods": [
    {
      "name": "Bad Name",
      "version": "",
      "enabled": false,
      "state": "invalid",
      "error": "folder name \"Bad Name\" is not a valid mod name",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 0,
      "memory_kib": 0
    },
    {
      "name": "alpha",
      "version": "1.2.0",
      "title": "Alpha",
      "description": "Golden fixture",
      "enabled": true,
      "state": "running",
      "permissions": [
        {
          "name": "notify",
          "requested": true,
          "granted": true
        },
        {
          "name": "commands",
          "requested": true,
          "granted": true
        },
        {
          "name": "status",
          "requested": true,
          "granted": true
        },
        {
          "name": "terminal.send",
          "requested": true,
          "granted": true
        },
        {
          "name": "check",
          "requested": true,
          "granted": false
        }
      ],
      "checks": [
        {
          "name": "lint",
          "argv": [
            "cargo",
            "clippy",
            "-q"
          ]
        },
        {
          "name": "test",
          "argv": [
            "cargo",
            "test"
          ]
        }
      ],
      "commands": [
        {
          "id": "hello",
          "title": "인사",
          "detail": "알림을 띄운다"
        },
        {
          "id": "long",
          "title": "A very long command title that goes well past sixty charact…"
        }
      ],
      "detectors": [
        {
          "match": [
            "aider",
            "claude"
          ]
        }
      ],
      "layouts": [
        {
          "id": "two",
          "title": "둘",
          "center": {
            "split": "h",
            "children": [
              {
                "panel": "terminal",
                "index": 0,
                "size": 0.5
              },
              {
                "tabs": [
                  {
                    "panel": "review"
                  },
                  {
                    "panel": "editor"
                  }
                ]
              }
            ]
          }
        }
      ],
      "log_lines": 8,
      "memory_kib": 0
    },
    {
      "name": "boom",
      "version": "0.1.0",
      "enabled": true,
      "state": "stopped",
      "stop_reason": "load",
      "error": "Error: boom at load\n    at <eval> (main.js:1:11)",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 3,
      "memory_kib": 0
    },
    {
      "name": "flaky",
      "version": "0.1.0",
      "enabled": true,
      "state": "stopped",
      "stop_reason": "errors",
      "error": "Error: fail 3\n    at <anonymous> (main.js:1:74)\n    at dispatch (eval_script:128:52)",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 14,
      "memory_kib": 0
    },
    {
      "name": "quiet",
      "version": "3",
      "enabled": true,
      "state": "running",
      "permissions": [
        {
          "name": "gate",
          "requested": true,
          "granted": true
        },
        {
          "name": "orch",
          "requested": true,
          "granted": true
        }
      ],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 4,
      "memory_kib": 0
    },
    {
      "name": "spin",
      "version": "0.1.0",
      "enabled": true,
      "state": "stopped",
      "stop_reason": "cpu",
      "error": "CPU time limit exceeded",
      "permissions": [],
      "checks": [],
      "commands": [],
      "detectors": [],
      "layouts": [],
      "log_lines": 4,
      "memory_kib": 0
    }
  ],
  "dropped_events": 0
}"#;
    const GOLDEN_STATE_FILE: &str = r#"{
  "mods": {
    "Bad Name": {
      "enabled": false,
      "granted": []
    },
    "alpha": {
      "enabled": true,
      "granted": [
        "notify",
        "commands",
        "status",
        "terminal.send"
      ]
    },
    "boom": {
      "enabled": true,
      "granted": []
    },
    "flaky": {
      "enabled": true,
      "granted": []
    },
    "quiet": {
      "enabled": true,
      "granted": [
        "gate",
        "orch"
      ]
    },
    "spin": {
      "enabled": true,
      "granted": []
    }
  }
}"#;
}
