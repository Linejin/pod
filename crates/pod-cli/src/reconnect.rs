//! Long waits that survive a daemon restart (docs/12 B6).
//!
//! `pod gate request --wait`, `pod wait --gate`, `pod wait --status/--turn-end`
//! and `pod wait <terminal>` each block in one RPC. When podd restarts (crash,
//! update, the app restarting it) the connection closes; the condition they
//! wait for is persisted by the daemon (gates, terminals and their status
//! survive), so the wait reconnects with backoff and asks again with the
//! remaining timeout. It never starts a daemon itself (`pod server stop`
//! must stay stopped); if none comes back within the reconnect window
//! (`POD_RECONNECT_TIMEOUT` seconds, default 60) the wait fails with a clear
//! error.

use std::time::{Duration, Instant};

use anyhow::bail;
use podd::client::{Client, ClientError};

pub const ENV_RECONNECT_TIMEOUT: &str = "POD_RECONNECT_TIMEOUT";
const DEFAULT_WINDOW: Duration = Duration::from_secs(60);
const FIRST_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// The connection to the daemon is gone (as opposed to an answer).
pub fn is_disconnect(e: &ClientError) -> bool {
    match e {
        ClientError::NotRunning | ClientError::Io(_) => true,
        ClientError::Protocol(m) => m == "daemon closed the connection",
        // Refused by a daemon that is shutting down: it is about to go away.
        ClientError::Rpc(_) => e.is_stopping(),
    }
}

/// How long to keep trying to reconnect.
pub fn window() -> Duration {
    window_from(std::env::var(ENV_RECONNECT_TIMEOUT).ok().as_deref())
}

fn window_from(v: Option<&str>) -> Duration {
    v.and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_WINDOW)
}

/// Backoff before reconnect attempt `n` (0-based): 100 ms doubling, 2 s max.
pub fn backoff(n: u32) -> Duration {
    FIRST_BACKOFF
        .saturating_mul(1u32 << n.min(16))
        .min(MAX_BACKOFF)
}

/// Remaining part of a wait of `timeout_ms` started at `start`.
pub fn remaining(timeout_ms: Option<u64>, start: Instant) -> Option<u64> {
    timeout_ms.map(|t| t.saturating_sub(start.elapsed().as_millis() as u64))
}

fn reconnect(pipe: &str, what: &str) -> anyhow::Result<Client> {
    let window = window();
    let deadline = Instant::now() + window;
    let mut n = 0;
    loop {
        match Client::connect(pipe) {
            Ok(c) => return Ok(c),
            Err(e) if is_disconnect(&e) && Instant::now() < deadline => {
                std::thread::sleep(
                    backoff(n).min(deadline.saturating_duration_since(Instant::now())),
                );
                n += 1;
            }
            Err(e) if is_disconnect(&e) => bail!(
                "podd on {pipe} did not come back within {}s ({e}); \
                 {what} was not resolved — wait for it again once podd runs",
                window.as_secs_f64()
            ),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Run a blocking wait `call(client, remaining_timeout_ms)`; on a lost
/// connection, reconnect to the same pipe and call again for what is left
/// of `timeout_ms`.
pub fn wait<R>(
    c: &mut Client,
    timeout_ms: Option<u64>,
    what: &str,
    mut call: impl FnMut(&mut Client, Option<u64>) -> Result<R, ClientError>,
) -> anyhow::Result<R> {
    let start = Instant::now();
    loop {
        match call(c, remaining(timeout_ms, start)) {
            Ok(r) => return Ok(r),
            Err(e) if is_disconnect(&e) && pod_remote::is_remote_label(c.pipe()) => {
                bail!(
                    "{}와의 연결이 끊겨 {what}을(를) 더 기다리지 못했습니다 ({e}).                      원격 재접속은 아직 없습니다(docs/17 R4); 다시 실행하세요",
                    c.pipe()
                );
            }
            Err(e) if is_disconnect(&e) => {
                let pipe = c.pipe().to_string();
                eprintln!(
                    "pod: lost the connection to podd while waiting for {what} ({e}); reconnecting"
                );
                *c = reconnect(&pipe, what)?;
                eprintln!("pod: reconnected to podd; still waiting for {what}");
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnects_and_backoff() {
        assert!(is_disconnect(&ClientError::NotRunning));
        assert!(is_disconnect(&ClientError::Protocol(
            "daemon closed the connection".into()
        )));
        assert!(is_disconnect(&ClientError::Io(std::io::Error::other(
            "broken pipe"
        ))));
        assert!(!is_disconnect(&ClientError::Protocol(
            "bad response".into()
        )));
        assert!(!is_disconnect(&ClientError::Rpc(
            podd::protocol::RpcError::new(-32018, "no gate")
        )));
        assert!(is_disconnect(&ClientError::Rpc(
            podd::protocol::RpcError::new(podd::protocol::codes::SERVER_STOPPING, "stopping")
        )));
        let b: Vec<u64> = (0..7).map(|n| backoff(n).as_millis() as u64).collect();
        assert_eq!(b, [100, 200, 400, 800, 1600, 2000, 2000]);
        assert_eq!(backoff(60), MAX_BACKOFF);
        assert_eq!(window_from(None), DEFAULT_WINDOW);
        assert_eq!(window_from(Some("2.5")), Duration::from_millis(2_500));
        assert_eq!(window_from(Some("nope")), DEFAULT_WINDOW);
        let start = Instant::now() - Duration::from_millis(1_500);
        let r = remaining(Some(1_000), start).unwrap();
        assert_eq!(r, 0);
        assert!(remaining(Some(10_000), start).unwrap() <= 8_500);
        assert_eq!(remaining(None, start), None);
    }
}
