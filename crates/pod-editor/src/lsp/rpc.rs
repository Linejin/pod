//! JSON-RPC 2.0 over the LSP base protocol (`Content-Length` framing).

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

/// Writes one framed message.
pub fn write_message(w: &mut dyn Write, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one framed message. `Ok(None)` on a clean EOF between messages.
pub fn read_message(r: &mut dyn BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    let mut saw_header = false;
    loop {
        line.clear();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return if saw_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF inside headers",
                ))
            } else {
                Ok(None)
            };
        }
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            if saw_header {
                break;
            }
            // Tolerate stray blank lines before a header block.
            continue;
        }
        saw_header = true;
        if let Some((k, v)) = t.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad Content-Length: {v}"),
                )
            })?);
        }
    }
    let len =
        len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}
impl std::error::Error for RpcError {}

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const REQUEST_CANCELLED: i64 = -32800;
/// Our own code for "the server process went away".
pub const SERVER_GONE: i64 = -32099;

impl Message {
    pub fn parse(v: Value) -> Option<Message> {
        let method = v.get("method").and_then(|m| m.as_str()).map(str::to_string);
        let id = v.get("id").cloned().filter(|i| !i.is_null());
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            (Some(method), Some(id)) => Some(Message::Request { id, method, params }),
            (Some(method), None) => Some(Message::Notification { method, params }),
            (None, Some(id)) => {
                let result = match v.get("error") {
                    Some(e) if !e.is_null() => Err(RpcError {
                        code: e.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                        message: e
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string(),
                    }),
                    _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                Some(Message::Response { id, result })
            }
            (None, None) => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Message::Request { id, method, params } => {
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            }
            Message::Notification { method, params } => {
                json!({"jsonrpc":"2.0","method":method,"params":params})
            }
            Message::Response { id, result: Ok(r) } => json!({"jsonrpc":"2.0","id":id,"result":r}),
            Message::Response { id, result: Err(e) } => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":e.code,"message":e.message}})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_round_trip_with_unicode() {
        let mut buf = Vec::new();
        let a = json!({"jsonrpc":"2.0","method":"x","params":{"t":"한글 😀"}});
        let b = json!({"jsonrpc":"2.0","id":7,"result":null});
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        let s = String::from_utf8(buf.clone()).unwrap();
        // Content-Length counts bytes, not chars.
        let body_len = serde_json::to_vec(&a).unwrap().len();
        assert!(s.starts_with(&format!("Content-Length: {body_len}\r\n\r\n")));
        let mut r = Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Some(a));
        assert_eq!(read_message(&mut r).unwrap(), Some(b));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn extra_headers_and_case() {
        let body = br#"{"jsonrpc":"2.0","id":"a","error":{"code":-32601,"message":"nope"}}"#;
        let mut raw = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body);
        let v = read_message(&mut Cursor::new(raw)).unwrap().unwrap();
        match Message::parse(v).unwrap() {
            Message::Response { id, result: Err(e) } => {
                assert_eq!(id, json!("a"));
                assert_eq!(e.code, METHOD_NOT_FOUND);
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn truncated_and_bad_frames_error() {
        assert!(read_message(&mut Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec())).is_err());
        assert!(read_message(&mut Cursor::new(b"X: 1\r\n\r\n{}".to_vec())).is_err());
        assert!(read_message(&mut Cursor::new(b"Content-Length: 2\r\n".to_vec())).is_err());
    }

    #[test]
    fn classify_messages() {
        let req = Message::parse(
            json!({"jsonrpc":"2.0","id":1,"method":"workspace/configuration","params":{}}),
        )
        .unwrap();
        assert!(matches!(req, Message::Request { .. }));
        let n = Message::parse(json!({"jsonrpc":"2.0","method":"$/progress"})).unwrap();
        assert_eq!(
            n,
            Message::Notification {
                method: "$/progress".into(),
                params: Value::Null
            }
        );
        let back = Message::parse(req.to_value()).unwrap();
        assert_eq!(back, req);
    }
}
