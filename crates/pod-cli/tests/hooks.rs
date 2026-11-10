//! `pod hooks install|uninstall|status` against a temporary home only: never
//! the real `%USERPROFILE%\.claude` / `.codex`.

#[path = "../../../tools/test-support/require_tools.rs"]
mod require_tools;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const POD: &str = env!("CARGO_BIN_EXE_pod");

fn tmp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("pod-hooks-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Another tool's existing hook configuration plus unrelated keys.
fn seed(home: &Path) -> (String, String) {
    let claude = r#"{
  "model": "opus",
  "permissions": {"allow": ["Bash(git:*)"]},
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "C:/Users/x/.other-ide/hooks/foreign-hook.exe claude", "timeout": 10}]}],
    "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "C:/Users/x/.other-ide/hooks/foreign-hook.exe claude", "timeout": 10}]}],
    "TeammateIdle": [{"hooks": [{"type": "command", "command": "C:/Users/x/.other-ide/hooks/foreign-hook.exe claude"}]}]
  },
  "statusLine": {"type": "command", "command": "C:/Users/x/.other-ide/hooks/statusline.cmd"}
}
"#;
    let codex = r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "C:/Users/x/.other-ide/hooks/codex-hook.cmd", "timeout": 10}]}],
 "Stop": [{"hooks": [{"type": "command", "command": "C:/Users/x/.other-ide/hooks/codex-hook.cmd", "timeout": 10}]}]}}
"#;
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".claude/settings.json"), claude).unwrap();
    std::fs::write(home.join(".codex/hooks.json"), codex).unwrap();
    (claude.into(), codex.into())
}

fn pod(args: &[&str], home: &Path, bin: &Path) -> Value {
    let out = Command::new(POD)
        .arg("--json")
        .args(args)
        .arg("--home")
        .arg(home)
        .arg("--bin-dir")
        .arg(bin)
        .arg("--source")
        .arg(POD) // any file stands in for pod-hook.exe here
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "pod {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `canonicalize` without the verbatim prefix: the long, on-disk spelling.
fn long_spelling(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap();
    PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"))
}

/// The 8.3 short spelling of an existing path, `None` when the volume has
/// no short names.
fn short_spelling(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated; `buf` is as long as the size passed.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    let n = n as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let s = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    s.to_string_lossy().contains('~').then_some(s)
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn backups(dir: &Path, name: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{name}.pod-backup-"))
        })
        .collect()
}

fn commands(groups: &Value) -> Vec<String> {
    groups
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array().unwrap().iter())
        .map(|h| h["command"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn install_reinstall_status_uninstall_keep_foreign_entries() {
    let home = tmp("home");
    let bin = tmp("bin");
    let (claude0, codex0) = seed(&home);
    let settings = home.join(".claude/settings.json");
    let hooks = home.join(".codex/hooks.json");

    // Dry run writes nothing.
    let d = pod(&["hooks", "install", "--dry-run"], &home, &bin);
    assert_eq!(d["dry_run"], json!(true));
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), claude0);
    assert!(!bin.join("pod-hook.exe").exists());

    let r = pod(&["hooks", "install", "--no-codex-trust"], &home, &bin);
    assert_eq!(r["binary_copied"], json!(true));
    let exe = bin.join("pod-hook.exe");
    assert_eq!(std::fs::read(&exe).unwrap(), std::fs::read(POD).unwrap());
    for f in r["files"].as_array().unwrap() {
        assert_eq!(f["action"], json!("updated"));
        let b = PathBuf::from(f["backup"].as_str().unwrap());
        assert!(b.exists());
    }
    // Backups hold the original bytes.
    let cb = backups(&home.join(".claude"), "settings.json");
    assert_eq!(cb.len(), 1);
    assert_eq!(std::fs::read_to_string(&cb[0]).unwrap(), claude0);
    assert_eq!(
        std::fs::read_to_string(&backups(&home.join(".codex"), "hooks.json")[0]).unwrap(),
        codex0
    );

    // Merged: other keys kept (in their order), the other tool's groups first, Pod's appended.
    let s = read_json(&settings);
    let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "permissions", "hooks", "statusLine"]);
    assert_eq!(s["model"], json!("opus"));
    let ss = commands(&s["hooks"]["SessionStart"]);
    assert_eq!(ss.len(), 2);
    assert!(ss[0].contains("foreign-hook"));
    // Configs name the long spelling (TEMP may be an 8.3 short name).
    let hook_cmd = long_spelling(&bin)
        .join("pod-hook.exe")
        .display()
        .to_string()
        .replace('\\', "/");
    assert!(
        ss[1].contains(&hook_cmd) && ss[1].ends_with("claude SessionStart"),
        "{ss:?}"
    );
    assert_eq!(
        commands(&s["hooks"]["TeammateIdle"]).len(),
        1,
        "other-tool-only event kept"
    );
    assert_eq!(s["hooks"]["PreToolUse"][1]["matcher"], json!("*"));
    let c = read_json(&hooks);
    let stop = commands(&c["hooks"]["Stop"]);
    assert!(stop[0].contains("codex-hook.cmd") && stop[1].ends_with("codex Stop"));

    // Re-install: idempotent, no new backups, no duplicates.
    let r2 = pod(&["hooks", "install", "--no-codex-trust"], &home, &bin);
    assert_eq!(r2["binary_copied"], json!(false));
    for f in r2["files"].as_array().unwrap() {
        assert_eq!(f["action"], json!("unchanged"));
    }
    assert_eq!(read_json(&settings), s);
    assert_eq!(backups(&home.join(".claude"), "settings.json").len(), 1);

    // Status.
    let st = pod(&["hooks", "status"], &home, &bin);
    assert_eq!(st["bin_exists"], json!(true));
    assert_eq!(st["bin_current"], json!(true));
    let a = &st["agents"][0];
    assert_eq!(a["agent"], json!("claude"));
    assert_eq!(a["pod_events"].as_array().unwrap().len(), 13);
    assert_eq!(a["foreign_groups"], json!(3));
    assert_eq!(a["hook_exes"][0][1], json!(true), "hook exe exists");
    assert_eq!(st["agents"][1]["pod_events"].as_array().unwrap().len(), 9);

    // Uninstall: back to the original content, other-tool entries intact.
    let u = pod(&["hooks", "uninstall"], &home, &bin);
    for f in u["files"].as_array().unwrap() {
        assert_eq!(f["action"], json!("updated"));
    }
    let orig_claude: Value = serde_json::from_str(&claude0).unwrap();
    let orig_codex: Value = serde_json::from_str(&codex0).unwrap();
    assert_eq!(read_json(&settings), orig_claude);
    assert_eq!(read_json(&hooks), orig_codex);
    assert_eq!(backups(&home.join(".claude"), "settings.json").len(), 2);
    let st = pod(&["hooks", "status"], &home, &bin);
    assert_eq!(st["agents"][0]["pod_events"].as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}

/// Without codex: trust entries Pod wrote earlier (as `config/batchWrite`
/// would) are removed on uninstall from a temp copy; the other tool's entry and other
/// settings stay; `--dry-run` reports without writing.
#[test]
fn uninstall_removes_pod_codex_trust_entries_from_temp_config() {
    let home = tmp("untrust");
    let bin = tmp("bin4");
    seed(&home);
    pod(
        &["hooks", "install", "--no-codex-trust", "--agent", "codex"],
        &home,
        &bin,
    );
    let hooks = home.join(".codex").join("hooks.json");
    let key = |ev: &str, g: usize| format!("{}:{ev}:{g}:0", hooks.display());
    // Other tool: index 0 of SessionStart/Stop; Pod: index 1 there, 0 elsewhere.
    let config = format!(
        "model = \"x\"\n\n[hooks.state.'{}']\ntrusted_hash = \"sha256:other-tool\"\n\n\
         [hooks.state.'{}']\ntrusted_hash = \"sha256:pod1\"\n\n\
         [hooks.state.'{}']\ntrusted_hash = \"sha256:pod2\"\n\n\
         [hooks.state.'{}']\ntrusted_hash = \"sha256:pod3\"\n",
        key("stop", 0),
        key("stop", 1),
        key("session_start", 1),
        key("pre_tool_use", 0),
    );
    let cfg_path = home.join(".codex").join("config.toml");
    std::fs::write(&cfg_path, &config).unwrap();

    let d = pod(
        &["hooks", "uninstall", "--agent", "codex", "--dry-run"],
        &home,
        &bin,
    );
    assert_eq!(d["codex_untrust"]["removed"], json!(3), "{d:#}");
    assert_eq!(
        std::fs::read_to_string(&cfg_path).unwrap(),
        config,
        "dry run writes nothing"
    );

    let u = pod(&["hooks", "uninstall", "--agent", "codex"], &home, &bin);
    assert_eq!(u["codex_untrust"]["removed"], json!(3), "{u:#}");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        cfg.contains("sha256:other-tool") && cfg.contains("model = \"x\""),
        "{cfg}"
    );
    assert!(!cfg.contains("sha256:pod"), "{cfg}");
    let b = PathBuf::from(u["codex_untrust"]["backup"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(b).unwrap(), config);
    // Claude was not part of this run.
    assert!(
        u["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["agent"] == json!("codex"))
    );

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}

/// `hooks status --json` names the installed binary and the source with versions.
#[test]
fn status_reports_binary_paths_and_versions() {
    let home = tmp("ver");
    let bin = tmp("bin5");
    // The real pod-hook next to pod.exe when the workspace built it.
    let hook = Path::new(POD).with_file_name("pod-hook.exe");
    let source = if hook.is_file() {
        hook.clone()
    } else {
        PathBuf::from(POD)
    };
    let run = |args: &[&str]| -> Value {
        let out = Command::new(POD)
            .arg("--json")
            .args(args)
            .arg("--home")
            .arg(&home)
            .arg("--bin-dir")
            .arg(&bin)
            .arg("--source")
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let st = run(&["hooks", "status"]);
    // Pod reports (and writes into agent configs) the long spelling of the
    // bin dir; TEMP may be an 8.3 short name (`RUNNER~1`).
    assert_eq!(st["bin"], json!(long_spelling(&bin).join("pod-hook.exe")));
    assert_eq!(st["bin_exists"], json!(false));
    assert!(st.get("bin_version").is_none());
    assert_eq!(st["source"], json!(source));
    run(&["hooks", "install", "--no-codex-trust", "--agent", "claude"]);
    let st = run(&["hooks", "status"]);
    assert_eq!(st["bin_current"], json!(true));
    if hook.is_file() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(st["bin_version"], json!(v), "{st:#}");
        assert_eq!(st["source_version"], json!(v));
    } else {
        require_tools::missing("pod-hook.exe next to pod.exe (cargo build -p pod-hook)");
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}

/// Home and bin dir named through 8.3 short names (GitHub runners'
/// `RUNNER~1`): agent configs get the long spelling, which keeps working
/// where short names are off or stripped, and uninstall finds Pod's hooks.
#[test]
fn short_spelled_dirs_reach_agent_configs_in_long_spelling() {
    let home = tmp("short-home");
    let bin = tmp("short-bin");
    seed(&home);
    let (Some(short_home), Some(short_bin)) = (short_spelling(&home), short_spelling(&bin)) else {
        eprintln!("skipped: no 8.3 short names on this volume");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&bin);
        return;
    };
    let r = pod(
        &["hooks", "install", "--no-codex-trust"],
        &short_home,
        &short_bin,
    );
    let exe = long_spelling(&bin).join("pod-hook.exe");
    assert_eq!(r["hook_exe"], json!(exe), "{r:#}");
    let exe_cmd = exe.display().to_string().replace('\\', "/");
    for cfg in [".claude/settings.json", ".codex/hooks.json"] {
        let text = std::fs::read_to_string(home.join(cfg)).unwrap();
        assert!(text.contains(&exe_cmd), "{cfg}: {text}");
        assert!(!text.contains('~'), "{cfg}: {text}");
    }
    let u = pod(&["hooks", "uninstall"], &short_home, &short_bin);
    for f in u["files"].as_array().unwrap() {
        assert_eq!(f["action"], json!("updated"), "{u:#}");
    }
    for cfg in [".claude/settings.json", ".codex/hooks.json"] {
        let text = std::fs::read_to_string(home.join(cfg)).unwrap();
        assert!(!text.contains("pod-hook"), "{cfg}: {text}");
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
fn install_creates_missing_files_and_uninstall_does_not() {
    let home = tmp("empty");
    let bin = tmp("bin2");
    let u = pod(&["hooks", "uninstall"], &home, &bin);
    assert!(u["files"].as_array().unwrap().is_empty());
    assert!(!home.join(".claude").exists());
    let r = pod(
        &["hooks", "install", "--no-codex-trust", "--agent", "claude"],
        &home,
        &bin,
    );
    assert_eq!(r["files"][0]["action"], json!("created"));
    assert!(r["files"][0].get("backup").is_none());
    assert!(home.join(".claude/settings.json").exists());
    assert!(!home.join(".codex/hooks.json").exists());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}

/// Codex hook trust through the real `codex app-server`, with CODEX_HOME in
/// the temporary home (no model turn, no prompt). Skipped without codex.
#[test]
fn codex_hooks_trusted_via_app_server_in_temp_home() {
    if Command::new("codex").arg("--version").output().is_err() {
        require_tools::missing("codex on PATH");
        return;
    }
    let real = std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".codex").join("config.toml"));
    let real_before = real.as_ref().and_then(|p| std::fs::read(p).ok());

    let home = tmp("codex");
    let bin = tmp("bin3");
    seed(&home);
    std::fs::write(home.join(".codex/config.toml"), "model = \"x\"\n").unwrap();
    let r = pod(&["hooks", "install", "--agent", "codex"], &home, &bin);
    let t = &r["codex_trust"];
    assert_eq!(t["pod_hooks"], json!(9), "{r:#}");
    assert_eq!(t["newly_trusted"], json!(9));
    assert_eq!(t["untrusted"], json!(0));
    let cfg = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(cfg.contains("model = \"x\""));
    assert!(cfg.contains("trusted_hash"), "{cfg}");
    assert!(!backups(&home.join(".codex"), "config.toml").is_empty());
    // Foreign hooks are not trusted by Pod.
    // (The other tool's groups are at index 0 of SessionStart and Stop; Pod's at 1.)
    assert!(
        cfg.contains("hooks.json:session_start:1:0")
            && !cfg.contains("hooks.json:session_start:0:0"),
        "{cfg}"
    );
    assert!(
        cfg.contains("hooks.json:stop:1:0") && !cfg.contains("hooks.json:stop:0:0"),
        "{cfg}"
    );

    // Again: nothing new to trust.
    let r = pod(&["hooks", "install", "--agent", "codex"], &home, &bin);
    assert_eq!(r["codex_trust"]["newly_trusted"], json!(0));
    assert_eq!(r["codex_trust"]["untrusted"], json!(0));

    // Uninstall removes exactly Pod's trust entries (the other tool's hooks stay at
    // their positions, nothing to re-key), keeping the rest of config.toml.
    let before = backups(&home.join(".codex"), "config.toml").len();
    let u = pod(&["hooks", "uninstall", "--agent", "codex"], &home, &bin);
    assert_eq!(u["codex_untrust"]["removed"], json!(9), "{u:#}");
    assert_eq!(u["codex_untrust"]["moved"], json!(0));
    let cfg = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(cfg.contains("model = \"x\""), "{cfg}");
    assert!(!cfg.contains("trusted_hash"), "{cfg}");
    assert_eq!(
        backups(&home.join(".codex"), "config.toml").len(),
        before + 1
    );

    // The real configuration was not touched.
    if let Some(p) = &real {
        assert_eq!(
            std::fs::read(p).ok(),
            real_before,
            "real config.toml changed"
        );
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&bin);
}
