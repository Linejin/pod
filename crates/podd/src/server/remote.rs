//! v12 connection identity and `connection.hello` (docs/17 R2).

use std::path::Path;

use super::{Daemon, log, to_value};
use crate::protocol::*;
use crate::transport::REMOTE_MAX_LINE;

const MACHINE_ID_FILE: &str = "machine_id";

/// The daemon's persistent machine id: `<state dir>/machine_id` (128 random
/// bits as hex), created on first start. An unreadable or malformed file is
/// replaced. If it cannot be written the id still works for this start but
/// changes on the next one; remote clients then see a different machine,
/// which is the safe direction (they do not reuse old task links).
pub fn load_or_create_machine_id(state_dir: &Path) -> String {
    let path = state_dir.join(MACHINE_ID_FILE);
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if is_id(s) {
            return s.to_string();
        }
        log(&format!(
            "{}: malformed machine id, replacing it",
            path.display()
        ));
    }
    let id = new_id();
    let tmp = state_dir.join(format!("{MACHINE_ID_FILE}.tmp"));
    let res = std::fs::write(&tmp, format!("{id}\n")).and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = res {
        log(&format!(
            "cannot persist machine id to {}: {e}",
            path.display()
        ));
    }
    id
}

/// A fresh random id for this daemon start.
pub fn new_boot_id() -> String {
    new_id()
}

fn new_id() -> String {
    // The system RNG; a failure there is not survivable for ids.
    super::status::new_token().expect("system RNG")
}

fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Daemon {
    pub(super) fn hello(&self, p: HelloParams) -> Result<serde_json::Value, RpcError> {
        let range = server_range();
        let protocol = match negotiate(&p, range, CAPABILITIES) {
            Ok(v) => v,
            Err(e) => {
                log(&format!(
                    "connection.hello refused ({}): {}",
                    p.client.as_deref().unwrap_or("unknown client"),
                    e.message
                ));
                return Err(e);
            }
        };
        // v20 remote framing (docs/17 R4) only for a client that chose 20;
        // an older client keeps the values it was told before.
        let remote = protocol >= CHUNKED_ATTACH_PROTOCOL;
        to_value(HelloResult {
            protocol,
            min_protocol: range.0,
            max_protocol: range.1,
            capabilities: capabilities_for(protocol),
            machine_id: self.machine_id.clone(),
            boot_id: self.boot_id.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            limits: FrameLimits {
                max_line_bytes: REMOTE_MAX_LINE as u64,
                max_request_bytes: super::MAX_LINE as u64,
                max_output_chunk_bytes: if remote {
                    REMOTE_OUTPUT_CHUNK as u64
                } else {
                    crate::holder::OUTPUT_CHUNK as u64
                },
                attach_queue_bytes: super::connection::attach_queue_for(
                    remote,
                    self.config.attach_queue_limit,
                ) as u64,
                // v22 (docs/17 R5); dropped for older hellos (downgrade).
                max_read_bytes: Some(MAX_READ_BYTES),
                max_write_bytes: Some(MAX_WRITE_BYTES),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_persists_and_malformed_is_replaced() {
        let dir = std::env::temp_dir().join(format!(
            "podd-machine-id-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = load_or_create_machine_id(&dir);
        assert!(is_id(&a));
        assert_eq!(
            load_or_create_machine_id(&dir),
            a,
            "same id after a restart"
        );
        std::fs::write(dir.join(MACHINE_ID_FILE), "not an id").unwrap();
        let b = load_or_create_machine_id(&dir);
        assert!(is_id(&b));
        assert_ne!(a, b);
        assert_eq!(load_or_create_machine_id(&dir), b);
        assert_ne!(new_boot_id(), new_boot_id());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
