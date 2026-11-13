//! Shared helpers: an isolated key store per test (deleted on drop) and a
//! loopback TCP pair. Nothing listens beyond 127.0.0.1.

#![allow(dead_code)]

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

use pod_mobile::crypto;
use pod_mobile::identity::KeyStore;

/// A key store whose keys are removed when the test ends (also on panic).
/// Windows: the current user's software KSP under a test-only prefix.
/// Linux: a fresh directory under the temp dir.
pub struct TestStore {
    pub store: KeyStore,
    #[cfg(target_os = "linux")]
    dir: std::path::PathBuf,
}

impl TestStore {
    pub fn new(tag: &str) -> TestStore {
        let mut id = [0u8; 4];
        crypto::random_bytes(&mut id).unwrap();
        let id: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let prefix = format!("PodMobileTest-{tag}-{id}-");
        #[cfg(windows)]
        {
            TestStore {
                store: KeyStore::current_user(&prefix).unwrap(),
            }
        }
        #[cfg(target_os = "linux")]
        {
            let dir = std::env::temp_dir().join(format!("pod-mobile-test-{tag}-{id}"));
            TestStore {
                store: KeyStore::directory(&dir, &prefix).unwrap(),
                dir,
            }
        }
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        if let Ok(keys) = self.store.keys() {
            for k in keys {
                let _ = self.store.delete(&k);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A connected loopback TCP pair `(server side, client side)` with 10 s
/// read/write timeouts.
pub fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for s in [&server, &client] {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        s.set_nodelay(true).unwrap();
    }
    (server, client)
}

/// Hex to bytes (test vectors).
pub fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
