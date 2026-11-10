//! Remembers the sequences a VT grid cannot hold — the latest OSC 7501 status
//! report per record (`id=`, or the root) and the latest window title (OSC
//! 0/2) — so snapshots can replay them.
//!
//! Why: the status engine reads OSC 7501 from the byte stream, but a daemon
//! (re)connecting to a holder only gets a grid snapshot plus later output.
//! Reports written before the connection (a program that reports right at
//! start, or everything before a daemon restart) would otherwise be lost.
//! Appending them to the snapshot lets the daemon's engine re-derive the
//! status, and lets attach clients get the window title back.

/// Longest OSC body kept (OSC 7501 payloads are ≤ 4087 bytes).
const MAX_OSC: usize = 4096;
/// Most records remembered (oldest dropped first).
const MAX_ENTRIES: usize = 64;

#[derive(Default)]
pub struct OscReplay {
    state: St,
    body: Vec<u8>,
    overflow: bool,
    /// (key, complete sequence) in the order last seen.
    entries: Vec<(String, Vec<u8>)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum St {
    #[default]
    Ground,
    Esc,
    Body,
    BodyEsc,
}

impl OscReplay {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// The remembered sequences, oldest first.
    pub fn bytes(&self) -> Vec<u8> {
        self.entries.iter().flat_map(|(_, s)| s.clone()).collect()
    }

    fn step(&mut self, b: u8) {
        match self.state {
            St::Ground => {
                if b == 0x1b {
                    self.state = St::Esc;
                }
            }
            St::Esc => {
                self.state = match b {
                    b']' => {
                        self.body.clear();
                        self.overflow = false;
                        St::Body
                    }
                    0x1b => St::Esc,
                    _ => St::Ground,
                }
            }
            St::Body => match b {
                0x07 => self.finish(),
                0x1b => self.state = St::BodyEsc,
                0x18 | 0x1a => self.state = St::Ground,
                _ => {
                    if self.body.len() < MAX_OSC {
                        self.body.push(b);
                    } else {
                        self.overflow = true;
                    }
                }
            },
            St::BodyEsc => {
                if b == b'\\' {
                    self.finish();
                } else {
                    self.state = St::Ground;
                    self.step(b);
                }
            }
        }
    }

    fn finish(&mut self) {
        self.state = St::Ground;
        if self.overflow {
            return;
        }
        let body = std::mem::take(&mut self.body);
        let key = if let Some(p) = body.strip_prefix(b"7501;") {
            if p.first() == Some(&b'?') {
                return; // query or a nested terminal's reply
            }
            let id = p
                .split(|&c| c == b':')
                .find_map(|f| f.strip_prefix(b"id="))
                .map(|v| String::from_utf8_lossy(v).into_owned());
            match id {
                Some(id) => format!("7501/{id}"),
                None => "7501".to_string(),
            }
        } else if body.starts_with(b"0;") || body.starts_with(b"2;") {
            "title".to_string()
        } else {
            return;
        };
        let mut seq = Vec::with_capacity(body.len() + 3);
        seq.extend_from_slice(b"\x1b]");
        seq.extend_from_slice(&body);
        seq.push(0x07);
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, seq));
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_latest_per_record_and_title() {
        let mut r = OscReplay::default();
        let data = b"x\x1b]7501;state=working\x07\x1b]0;one\x1b\\\x1b]7501;?\x07\
\x1b]7501;state=working:id=sub\x07\x1b]8;;http://a\x07\x1b]7501;state=done\x07\x1b]2;two\x07";
        // Any chunking gives the same result.
        for cut in 0..data.len() {
            let mut r2 = OscReplay::default();
            r2.feed(&data[..cut]);
            r2.feed(&data[cut..]);
            assert_eq!(
                r2.bytes(),
                b"\x1b]7501;state=working:id=sub\x07\x1b]7501;state=done\x07\x1b]2;two\x07"
            );
        }
        r.feed(data);
        assert!(!r.bytes().is_empty());
    }
}
