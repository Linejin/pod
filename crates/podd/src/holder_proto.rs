//! Daemon <-> holder channel.
//!
//! One holder process per terminal serves a private named pipe
//! (`\\.\pipe\pod-holder-<daemon key>-<id>`, current-user DACL). The daemon
//! is its only client; a new connection replaces the previous one, which is
//! how a restarted daemon takes over.
//!
//! Binary frames, little endian: `u32 len` (bytes after this field), `u8 kind`,
//! body. Raw PTY bytes travel as-is (no base64 on this hop).
//!
//! On connect the holder sends, atomically with respect to PTY output:
//! `Hello`, `Snapshot { seq }`, then (if the child already exited) `Exited`,
//! then live `Output`/`Resized`/`Exited` frames whose `seq` continue exactly
//! where the snapshot ends.

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Holder protocol spoken by this build.
/// - 1: the original frames (0x01..0x05, 0x81..0x84).
/// - 2: + `DaemonHello` (0x85, daemon -> holder, sent only to holders >= 2) and
///   the optional `parent_terminal`/`parent_task` in the spec and `Hello`.
/// - 3: + `InputAck` (0x06, holder -> daemon, sent only after a 3+
///   `DaemonHello`): input bytes of this connection the holder has consumed, so the
///   daemon can keep its input in flight under a byte budget (daemon-01).
pub const HOLDER_PROTOCOL: u32 = 3;
/// Oldest holder protocol this daemon can drive.
pub const HOLDER_PROTOCOL_MIN: u32 = 1;

/// Version check done by the daemon on a holder's `Hello`: `Err(reason)` when
/// the holder cannot be driven by this daemon.
pub fn check_holder_version(version: u32) -> Result<(), String> {
    if (HOLDER_PROTOCOL_MIN..=HOLDER_PROTOCOL).contains(&version) {
        Ok(())
    } else {
        Err(format!(
            "holder protocol {version}, this podd supports {HOLDER_PROTOCOL_MIN}..={HOLDER_PROTOCOL}"
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HolderHello {
    pub version: u32,
    pub id: String,
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub created_ms: u64,
    pub holder_pid: u32,
    pub child_pid: u32,
    pub cols: u16,
    pub rows: u16,
    /// ConPTY implementation: `bundled:<path>` or `inbox`.
    #[serde(default)]
    pub conpty: String,
    /// v2: the terminal's parent, from the spec (kept by the holder so it
    /// survives daemon restarts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
}

/// Daemon side of the version exchange (`DaemonHello`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonHello {
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExitInfo {
    pub seq: u64,
    pub exit_code: Option<i32>,
    pub killed: bool,
    pub exited_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    // holder -> daemon
    Hello(HolderHello),
    /// A hello this build cannot parse (another holder protocol): the raw JSON,
    /// so the daemon can still report the version and ids.
    ForeignHello(serde_json::Value),
    /// ANSI that reproduces the holder's terminal after `seq` output bytes.
    Snapshot {
        seq: u64,
        data: Vec<u8>,
    },
    /// PTY output bytes `[seq, seq + data.len())`.
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    /// The PTY was resized; applies to output from `seq` on.
    Resized {
        seq: u64,
        cols: u16,
        rows: u16,
    },
    Exited(ExitInfo),
    /// v3: total input bytes received on this connection that the holder has
    /// consumed (written to the PTY, or dropped over its hard cap).
    InputAck {
        written: u64,
    },
    // daemon -> holder
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Terminate the job (whole process tree).
    Kill,
    /// The terminal is exited and forgotten: remove the record and exit.
    Release,
    /// v2: the daemon's protocol version, sent right after it accepted the
    /// holder's `Hello` (only to holders >= 2; older ones reject unknown kinds).
    DaemonHello(DaemonHello),
}

mod kind {
    pub const HELLO: u8 = 0x01;
    pub const SNAPSHOT: u8 = 0x02;
    pub const OUTPUT: u8 = 0x03;
    pub const RESIZED: u8 = 0x04;
    pub const EXITED: u8 = 0x05;
    pub const INPUT_ACK: u8 = 0x06;
    pub const INPUT: u8 = 0x81;
    pub const RESIZE: u8 = 0x82;
    pub const KILL: u8 = 0x83;
    pub const RELEASE: u8 = 0x84;
    pub const DAEMON_HELLO: u8 = 0x85;
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let k = match self {
            Frame::Hello(h) => {
                body = serde_json::to_vec(h).expect("hello json");
                kind::HELLO
            }
            Frame::ForeignHello(v) => {
                body = serde_json::to_vec(v).expect("hello json");
                kind::HELLO
            }
            Frame::Snapshot { seq, data } => {
                body.extend(seq.to_le_bytes());
                body.extend(data);
                kind::SNAPSHOT
            }
            Frame::Output { seq, data } => {
                body.extend(seq.to_le_bytes());
                body.extend(data);
                kind::OUTPUT
            }
            Frame::Resized { seq, cols, rows } => {
                body.extend(seq.to_le_bytes());
                body.extend(cols.to_le_bytes());
                body.extend(rows.to_le_bytes());
                kind::RESIZED
            }
            Frame::Exited(e) => {
                body = serde_json::to_vec(e).expect("exit json");
                kind::EXITED
            }
            Frame::InputAck { written } => {
                body.extend(written.to_le_bytes());
                kind::INPUT_ACK
            }
            Frame::Input(data) => {
                body.extend(data);
                kind::INPUT
            }
            Frame::Resize { cols, rows } => {
                body.extend(cols.to_le_bytes());
                body.extend(rows.to_le_bytes());
                kind::RESIZE
            }
            Frame::Kill => kind::KILL,
            Frame::Release => kind::RELEASE,
            Frame::DaemonHello(h) => {
                body = serde_json::to_vec(h).expect("daemon hello json");
                kind::DAEMON_HELLO
            }
        };
        let mut out = Vec::with_capacity(5 + body.len());
        out.extend(((body.len() + 1) as u32).to_le_bytes());
        out.push(k);
        out.extend(body);
        out
    }

    pub fn decode(k: u8, body: &[u8]) -> io::Result<Frame> {
        let bad =
            |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("bad holder frame: {m}"));
        let seq = |b: &[u8]| -> io::Result<u64> {
            b.get(..8)
                .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
                .ok_or_else(|| bad("short seq"))
        };
        let u16_at = |b: &[u8], at: usize| -> io::Result<u16> {
            b.get(at..at + 2)
                .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
                .ok_or_else(|| bad("short size"))
        };
        Ok(match k {
            kind::HELLO => match serde_json::from_slice::<HolderHello>(body) {
                Ok(h) => Frame::Hello(h),
                Err(e) => Frame::ForeignHello(
                    serde_json::from_slice(body).map_err(|_| bad(&e.to_string()))?,
                ),
            },
            kind::SNAPSHOT => Frame::Snapshot {
                seq: seq(body)?,
                data: body[8..].to_vec(),
            },
            kind::OUTPUT => Frame::Output {
                seq: seq(body)?,
                data: body[8..].to_vec(),
            },
            kind::RESIZED => Frame::Resized {
                seq: seq(body)?,
                cols: u16_at(body, 8)?,
                rows: u16_at(body, 10)?,
            },
            kind::EXITED => {
                Frame::Exited(serde_json::from_slice(body).map_err(|e| bad(&e.to_string()))?)
            }
            kind::INPUT_ACK => Frame::InputAck {
                written: seq(body)?,
            },
            kind::INPUT => Frame::Input(body.to_vec()),
            kind::RESIZE => Frame::Resize {
                cols: u16_at(body, 0)?,
                rows: u16_at(body, 2)?,
            },
            kind::KILL => Frame::Kill,
            kind::RELEASE => Frame::Release,
            kind::DAEMON_HELLO => {
                Frame::DaemonHello(serde_json::from_slice(body).map_err(|e| bad(&e.to_string()))?)
            }
            other => return Err(bad(&format!("unknown kind {other:#x}"))),
        })
    }
}

/// Read one frame; `Ok(None)` on a clean end of stream.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.raw_os_error() == Some(109) => return Ok(None), // ERROR_BROKEN_PIPE
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad holder frame length {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Frame::decode(buf[0], &buf[1..]).map(Some)
}

/// Written by the daemon before it starts a holder (`pod-holder --spec <file>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolderSpec {
    pub id: String,
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    /// Complete child environment (already sanitized by the daemon).
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub pipe: String,
    /// Where the holder writes its [`HolderRecord`]; removed on release.
    pub record_path: String,
    pub created_ms: u64,
    /// v2: parent terminal / task (see `TerminalInfo.parent_terminal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
}

/// Written by the holder once it serves its pipe; the daemon scans these on start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolderRecord {
    pub id: String,
    pub name: Option<String>,
    pub pipe: String,
    pub holder_pid: u32,
    pub child_pid: u32,
    pub created_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_roundtrip() {
        let frames = vec![
            Frame::Hello(HolderHello {
                version: 1,
                id: "t1".into(),
                name: Some("n".into()),
                argv: vec!["cmd".into()],
                cwd: "C:\\".into(),
                created_ms: 5,
                holder_pid: 1,
                child_pid: 2,
                cols: 80,
                rows: 24,
                conpty: "inbox".into(),
                parent_terminal: Some("t0".into()),
                parent_task: None,
            }),
            Frame::Snapshot {
                seq: 7,
                data: b"\x1b[0mhi".to_vec(),
            },
            Frame::Output {
                seq: 9,
                data: vec![0, 255, 10],
            },
            Frame::Resized {
                seq: 12,
                cols: 100,
                rows: 40,
            },
            Frame::Exited(ExitInfo {
                seq: 12,
                exit_code: Some(3),
                killed: false,
                exited_ms: 9,
            }),
            Frame::InputAck { written: 1 << 40 },
            Frame::Input(b"ls\r".to_vec()),
            Frame::Resize { cols: 1, rows: 2 },
            Frame::Kill,
            Frame::Release,
            Frame::DaemonHello(DaemonHello { version: 2 }),
        ];
        let bytes: Vec<u8> = frames.iter().flat_map(Frame::encode).collect();
        let mut r = &bytes[..];
        for f in &frames {
            assert_eq!(read_frame(&mut r).await.unwrap().as_ref(), Some(f));
        }
        assert_eq!(read_frame(&mut r).await.unwrap(), None);
    }

    /// A hello from another holder protocol (different fields) still decodes,
    /// as `ForeignHello`, so the daemon can report it; versions are checked.
    #[tokio::test]
    async fn foreign_hello_and_version_check() {
        let v = serde_json::json!({"version": 9, "id": "t4", "pid": 77});
        let bytes = Frame::ForeignHello(v.clone()).encode();
        let f = read_frame(&mut &bytes[..]).await.unwrap().unwrap();
        assert_eq!(f, Frame::ForeignHello(v));
        // A v1 hello (no parent fields) is still a normal Hello.
        let v1 = serde_json::json!({"version": 1, "id": "t1", "name": null, "argv": [],
            "cwd": "C:\\", "created_ms": 1, "holder_pid": 1, "child_pid": 2, "cols": 80, "rows": 24});
        let bytes = Frame::ForeignHello(v1).encode();
        assert!(matches!(
            read_frame(&mut &bytes[..]).await.unwrap().unwrap(),
            Frame::Hello(HolderHello {
                version: 1,
                parent_terminal: None,
                ..
            })
        ));
        assert!(check_holder_version(1).is_ok());
        assert!(check_holder_version(HOLDER_PROTOCOL).is_ok());
        assert!(
            check_holder_version(0)
                .unwrap_err()
                .contains("holder protocol 0")
        );
        assert!(check_holder_version(HOLDER_PROTOCOL + 1).is_err());
    }
}
