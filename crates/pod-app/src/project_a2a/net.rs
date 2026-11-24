//! The blocking A2A calls of the section, run on a background thread.
//!
//! Thin over `pod-a2a`: discovery + interface selection, SendMessage and
//! GetTask. Nothing here retries, caches on disk or logs. The bearer token
//! is read per call, sent only when the card declares HTTP Bearer, and only
//! to the selected interface (the client's rule).

use std::time::Duration;

use pod_a2a::model::{Message, SendMessageConfiguration, SendMessageResponse, Task};
use pod_a2a::{A2aClient, A2aError, BearerToken, ClientConfig, discover};

use super::config::PeerDef;
use super::model::{Resolved, Review};

/// SendMessage asks for `returnImmediately`, so its answer is quick; a peer
/// that ignores the flag still gets this long.
const SEND_TIMEOUT: Duration = Duration::from_secs(120);
const GET_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOKEN_FILE: u64 = 16 * 1024;

fn config(def: &PeerDef, timeout: Duration) -> ClientConfig {
    ClientConfig {
        policy: def.policy(),
        request_timeout: timeout,
        ..ClientConfig::default()
    }
}

/// Fetches the card and selects the interface (no credential is sent).
pub fn resolve(def: &PeerDef) -> Result<Resolved, A2aError> {
    let cfg = config(def, GET_TIMEOUT);
    let disc = discover(&def.source(), &cfg)?;
    let interface = disc.select(def.endpoint.as_deref(), cfg.policy)?;
    let bearer_declared = disc
        .card
        .security_schemes
        .values()
        .any(|s| s.is_http_bearer());
    Ok(Resolved {
        card_url: disc.card_url.to_string(),
        card: disc.card,
        interface,
        bearer_declared,
    })
}

fn token(def: &PeerDef) -> Result<Option<BearerToken>, A2aError> {
    let raw = if let Some(name) = &def.bearer_token_env {
        std::env::var(name).map_err(|_| {
            A2aError::Invalid(format!("환경 변수 {name}이(가) 설정되어 있지 않습니다"))
        })?
    } else if let Some(path) = &def.bearer_token_file {
        let meta = std::fs::metadata(path)
            .map_err(|e| A2aError::Invalid(format!("토큰 파일을 읽을 수 없습니다: {e}")))?;
        if meta.len() > MAX_TOKEN_FILE {
            return Err(A2aError::Invalid("토큰 파일이 16 KiB보다 큽니다".into()));
        }
        std::fs::read_to_string(path)
            .map_err(|e| A2aError::Invalid(format!("토큰 파일을 읽을 수 없습니다: {e}")))?
    } else {
        return Ok(None);
    };
    BearerToken::new(&raw).map(Some)
}

/// A client for `def` on a freshly resolved card.
fn client(def: &PeerDef, timeout: Duration) -> Result<(Resolved, A2aClient), A2aError> {
    let r = resolve(def)?;
    let token = token(def)?;
    if token.is_some() && !r.bearer_declared {
        return Err(A2aError::Invalid(
            "에이전트 카드가 HTTP Bearer를 선언하지 않아 토큰을 보내지 않았습니다".into(),
        ));
    }
    let c = A2aClient::new(r.interface.clone(), token, config(def, timeout))?;
    Ok((r, c))
}

/// What a send produced.
pub enum SendOutcome {
    /// The peer's answer, or the client error (with its delivery outcome).
    Done(Box<Result<SendMessageResponse, A2aError>>),
    /// Refused before sending: the card or interface changed since review.
    Changed(String),
}

/// Sends exactly `review` (re-resolving the card first).
pub fn send(def: &PeerDef, review: &Review) -> SendOutcome {
    let (r, c) = match client(def, SEND_TIMEOUT) {
        Ok(x) => x,
        Err(e) => return SendOutcome::Done(Box::new(Err(e))),
    };
    if r.card_url != review.card_url || r.interface.url != review.interface_url {
        return SendOutcome::Changed(format!(
            "검토 뒤 피어의 카드나 전송 주소가 바뀌었습니다 ({} → {}). 다시 검토하세요.",
            review.interface_url, r.interface.url
        ));
    }
    if review.bearer != (def.has_token() && r.bearer_declared) {
        return SendOutcome::Changed("검토 뒤 인증 설정이 바뀌었습니다. 다시 검토하세요.".into());
    }
    let mut message = Message::user_text(review.message_id.clone(), review.text.clone());
    message.task_id = review.task_id.clone();
    message.context_id = review.context_id.clone();
    let configuration = SendMessageConfiguration {
        accepted_output_modes: review.accepted_output_modes.clone(),
        history_length: Some(0),
        return_immediately: true,
    };
    SendOutcome::Done(Box::new(c.send_message(message, Some(configuration))))
}

/// `GetTask` for `id` (history not requested).
pub fn get_task(def: &PeerDef, id: &str) -> Result<Task, A2aError> {
    let (_, c) = client(def, GET_TIMEOUT)?;
    c.get_task(id, Some(0))
}
