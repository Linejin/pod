//! `pod mobile`: paired phones, what each may do per task, and the audit
//! log (docs/17 M1). Pairing itself comes with the network listener (M2).

use std::process::ExitCode;

use clap::Subcommand;
use podd::client::Client;
use podd::protocol::*;

#[derive(Subcommand)]
pub enum MobileCmd {
    /// List paired devices, their per-task permissions and live sessions.
    Devices,
    /// Revoke a device (or all): its sessions end at once, its permissions are cleared.
    Revoke {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        device: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Set what a device may do with one task (nothing is allowed by default).
    Policy {
        device: String,
        task: String,
        /// Comma-separated: status, output, approve, message.
        #[arg(long, value_delimiter = ',', value_parser = parse_cap)]
        allow: Vec<MobileCap>,
        #[arg(long, value_delimiter = ',', value_parser = parse_cap)]
        deny: Vec<MobileCap>,
        /// Remove every permission for the task first.
        #[arg(long)]
        clear: bool,
    },
    /// Show the mobile audit log (newest last).
    Audit {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show or change mobile settings.
    Settings {
        /// Keep message text in the audit log (default off: length + SHA-256 only).
        #[arg(long, value_parser = parse_on_off, value_name = "on|off")]
        audit_bodies: Option<bool>,
    },
}

fn parse_cap(s: &str) -> Result<MobileCap, String> {
    MobileCap::parse(s)
        .ok_or_else(|| format!("unknown permission {s:?} (status, output, approve, message)"))
}

fn parse_on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(format!("expected on or off, got {s:?}")),
    }
}

fn caps(c: &[MobileCap]) -> String {
    c.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",")
}

fn print_device(d: &MobileDeviceInfo) {
    let state = match d.revoked_ms {
        Some(_) => "revoked".to_string(),
        None => format!("{} session(s)", d.sessions),
    };
    println!("{} {:?} ({state}, key {})", d.id, d.name, d.public_key);
    if d.policy.is_empty() && d.revoked_ms.is_none() {
        println!("  no tasks allowed");
    }
    for (task, c) in &d.policy {
        println!("  {task}: {}", caps(c));
    }
}

pub fn run(cmd: MobileCmd, c: &mut Client, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        MobileCmd::Devices => {
            let r = c.call::<MobileDevices>(&MobileDevicesParams {})?;
            if json {
                crate::print_json(&r);
            } else {
                println!("network listener: {}", r.listener);
                if r.devices.is_empty() {
                    println!("no paired devices");
                }
                for d in &r.devices {
                    print_device(d);
                }
            }
        }
        MobileCmd::Revoke { device, all } => {
            let r = c.call::<MobileRevoke>(&MobileRevokeParams { device, all })?;
            if json {
                crate::print_json(&r);
            } else if r.revoked.is_empty() {
                println!("nothing to revoke");
            } else {
                println!("revoked {}", r.revoked.join(", "));
            }
        }
        MobileCmd::Policy {
            device,
            task,
            allow,
            deny,
            clear,
        } => {
            let d = c.call::<MobilePolicy>(&MobilePolicyParams {
                device,
                task,
                allow,
                deny,
                clear,
            })?;
            if json {
                crate::print_json(&d);
            } else {
                print_device(&d);
            }
        }
        MobileCmd::Audit { device, limit } => {
            let r = c.call::<MobileAudit>(&MobileAuditParams {
                limit: Some(limit),
                device,
            })?;
            if json {
                crate::print_json(&r);
            } else if r.entries.is_empty() {
                println!("no audit entries");
            } else {
                for e in &r.entries {
                    let mut line = format!("{} {} {} {}", e.ts_ms, e.actor, e.action, e.result);
                    for (k, v) in [
                        ("task", &e.task),
                        ("gate", &e.gate),
                        ("op", &e.operation_id),
                    ] {
                        if let Some(v) = v {
                            line.push_str(&format!(" {k}={v}"));
                        }
                    }
                    if let (Some(n), Some(h)) = (e.msg_len, &e.msg_sha256) {
                        line.push_str(&format!(" msg={n} chars sha256={}", &h[..12.min(h.len())]));
                    }
                    if let Some(d) = &e.detail {
                        line.push_str(&format!(" ({d})"));
                    }
                    println!("{line}");
                }
            }
        }
        MobileCmd::Settings { audit_bodies } => {
            let s = c.call::<MobileSettingsMethod>(&MobileSettingsParams {
                audit_message_bodies: audit_bodies,
            })?;
            if json {
                crate::print_json(&s);
            } else {
                println!(
                    "audit message bodies: {}",
                    if s.audit_message_bodies {
                        "on"
                    } else {
                        "off (length + SHA-256)"
                    }
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
