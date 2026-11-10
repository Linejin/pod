//! The real Windows OpenSSH client, without any SSH server (docs/17 R3).
//!
//! Only client-side behavior is checked: `ssh -G` (print the resolved
//! config, no connection) accepts the generated config and the enforced
//! options, and a connection to a closed localhost port is classified as
//! unreachable. Skipped when no system ssh exists. A real login (host key,
//! authentication, remote `pod`) is **not** covered here; see docs/17 §6 R3.

#[path = "../../../tools/test-support/require_tools.rs"]
mod require_tools;

use std::process::Command;

use pod_remote::ssh::{self, ErrorKind};
use pod_remote::{Machine, connect};

#[test]
fn system_ssh_accepts_generated_config_and_reports_refused_port() {
    let Ok(exe) = ssh::locate_ssh() else {
        require_tools::missing("system ssh");
        return;
    };
    let root = std::env::temp_dir().join(format!("pod-remote-sysssh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The only test in this binary: no other thread reads the environment.
    // SAFETY: single-threaded at this point of the test.
    unsafe { std::env::set_var("LOCALAPPDATA", root.join("local")) };

    let kh = root.join("known %hosts");
    let m = Machine {
        name: "sys".into(),
        host: "127.0.0.1".into(),
        user: Some("alice".into()),
        port: Some(2222),
        identity_file: Some(root.join("A B").join("id_test")),
        known_hosts: Some(kh.clone()),
    };
    let cfg = root.join("c.conf");
    std::fs::write(&cfg, ssh::config_text(&m)).unwrap();
    let mut args = vec!["-G".into()];
    args.extend(ssh::ssh_args(&cfg));
    let out = Command::new(&exe).args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "ssh -G failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let fwd = |p: &std::path::Path| p.display().to_string().replace('\\', "/").to_lowercase();
    for want in [
        "hostname 127.0.0.1".to_string(),
        "user alice".into(),
        "port 2222".into(),
        "batchmode yes".into(),
        "stricthostkeychecking true".into(),
        "passwordauthentication no".into(),
        "kbdinteractiveauthentication no".into(),
        "forwardagent no".into(),
        "forwardx11 no".into(),
        "clearallforwardings yes".into(),
        "requesttty false".into(),
        "permitlocalcommand no".into(),
        "updatehostkeys false".into(),
        "identitiesonly yes".into(),
        format!("identityfile {}", fwd(m.identity_file.as_ref().unwrap())),
        format!("userknownhostsfile {}", fwd(&kh)),
    ] {
        assert!(
            text.lines().any(|l| l.trim() == want),
            "missing {want:?} in ssh -G output:\n{text}"
        );
    }

    // A port nothing listens on: real ssh fails before any host key or login.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let refused = Machine {
        port: Some(port),
        identity_file: None,
        ..m
    };
    let e = match connect(&refused) {
        Ok(_) => panic!("connected to a closed port"),
        Err(e) => e,
    };
    assert_eq!(e.kind, ErrorKind::Unreachable, "{e}");
    assert_eq!(e.exit_code, Some(255));
    assert!(e.to_string().contains("호스트에 연결할 수 없습니다"), "{e}");
    // The generated config was removed; known_hosts was never created.
    let left: Vec<_> = std::fs::read_dir(root.join("local").join("pod").join("ssh"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "config left behind: {left:?}");
    assert!(!kh.exists());
    let _ = std::fs::remove_dir_all(&root);
}
