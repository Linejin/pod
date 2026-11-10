//! `pod machine add|list|remove|test` and the `--machine` connection (docs/17 R3).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use pod_remote::{Machine, MachineStore};
use podd::client::Client;

#[derive(Subcommand)]
pub enum MachineCmd {
    /// Add an SSH machine profile (host/user/port/key/known_hosts only).
    Add {
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Private key file (absolute path). Default: ssh-agent / ssh's default keys.
        #[arg(long, value_name = "PATH")]
        identity_file: Option<PathBuf>,
        /// known_hosts file (absolute path). Default: ~/.ssh/known_hosts.
        #[arg(long, value_name = "PATH")]
        known_hosts: Option<PathBuf>,
    },
    /// List machine profiles.
    #[command(alias = "ls")]
    List,
    /// Remove a machine profile (nothing on the remote side changes).
    #[command(alias = "rm")]
    Remove { name: String },
    /// Connect over SSH and run the v12 handshake; prints the remote podd.
    Test { name: String },
}

fn err(e: String) -> anyhow::Error {
    anyhow::anyhow!(e)
}

pub fn run(cmd: MachineCmd, json: bool) -> anyhow::Result<ExitCode> {
    let store = MachineStore::open_default();
    match cmd {
        MachineCmd::Add {
            name,
            host,
            user,
            port,
            identity_file,
            known_hosts,
        } => {
            let m = Machine {
                name,
                host,
                user,
                port,
                identity_file,
                known_hosts,
            };
            store.add(m.clone()).map_err(err)?;
            if json {
                crate::print_json(&m);
            } else {
                println!("{} 추가됨: {}", m.name, m.target());
            }
        }
        MachineCmd::List => {
            let all = store.load().map_err(err)?;
            if json {
                crate::print_json(&serde_json::json!({ "machines": all }));
            } else if all.is_empty() {
                println!("머신 없음 (pod machine add <name> --host <host>)");
            } else {
                let rows = all
                    .iter()
                    .map(|m| {
                        let p = |o: &Option<PathBuf>| {
                            o.as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "-".into())
                        };
                        vec![
                            m.name.clone(),
                            m.target(),
                            p(&m.identity_file),
                            p(&m.known_hosts),
                        ]
                    })
                    .collect();
                crate::print_rows(&["NAME", "TARGET", "KEY", "KNOWN_HOSTS"], rows);
            }
        }
        MachineCmd::Remove { name } => {
            if !store.remove(&name).map_err(err)? {
                anyhow::bail!("머신 {name:?}이(가) 없습니다");
            }
            if json {
                crate::print_json(&serde_json::json!({ "removed": name }));
            } else {
                println!("{name} 삭제됨");
            }
        }
        MachineCmd::Test { name } => {
            let m = store.get(&name).map_err(err)?;
            match pod_remote::connect(&m) {
                Ok(conn) => {
                    let h = conn.hello.clone();
                    let _ = conn.close();
                    if json {
                        crate::print_json(&serde_json::json!({
                            "ok": true, "machine": name, "hello": h }));
                    } else {
                        println!(
                            "{name} 연결됨 ({}): podd {}, protocol {}, os {}, machine_id {}, boot_id {}",
                            m.target(),
                            h.version,
                            h.protocol,
                            h.os,
                            h.machine_id,
                            h.boot_id
                        );
                    }
                }
                Err(e) => {
                    if json {
                        crate::print_json(&serde_json::json!({
                            "ok": false,
                            "machine": name,
                            "kind": e.kind.code(),
                            "message": e.kind.summary(),
                            "detail": e.detail,
                            "stderr": e.stderr,
                            "exit_code": e.exit_code,
                        }));
                    }
                    eprintln!("pod: {e}");
                    return Ok(ExitCode::from(2));
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A client on machine `name` over SSH (after the v12 handshake).
pub fn connect(name: &str) -> anyhow::Result<Client> {
    let m = MachineStore::open_default().get(name).map_err(err)?;
    let conn = pod_remote::connect(&m)?;
    Ok(conn.into_client())
}
