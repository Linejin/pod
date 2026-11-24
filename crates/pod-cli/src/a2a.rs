//! `pod a2a card|send|task show|list|cancel`: a standard Agent2Agent 1.0
//! client for one explicitly named peer agent (docs/a2a-client.md).
//!
//! Runs on this PC only and never touches podd or orch mail. The peer's
//! Agent Card, messages and artifacts are printed as data: control
//! characters are escaped, URLs are not fetched, raw bytes are not saved,
//! nothing is executed. Requests are never repeated: when a send or cancel
//! may have reached the peer without a valid answer, the exit code is 4 and
//! the message id is printed so the user decides what to do.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};
use pod_a2a::model::{
    AgentCard, Artifact, ListTasksRequest, Message, Part, PartContent, SendMessageConfiguration,
    SendMessageResponse, StateClass, Task, TaskState,
};
use pod_a2a::{
    A2aClient, A2aError, BearerToken, CardSource, ClientConfig, Discovery, SelectedInterface,
    discover, sanitize_line, sanitize_text,
};
use serde_json::{Value, json};

/// Exit code: the request may have taken effect at the peer, but no valid
/// answer came back. Not retried.
const EXIT_OUTCOME_UNKNOWN: u8 = 4;
const EXIT_ERROR: u8 = 2;
/// Largest message text read from argv, a file or stdin.
const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Text-mode cut-off for one text part (all of it is in --json).
const MAX_PART_CHARS: usize = 64 * 1024;
const MAX_TIMEOUT_SECS: u64 = 3600;

#[derive(Subcommand)]
pub(crate) enum A2aCmd {
    /// Fetch and check a peer's Agent Card and show the interface Pod would
    /// use (no credential is sent).
    Card {
        #[command(flatten)]
        peer: PeerArgs,
    },
    /// Send a message (SendMessage) and print the Task or Message reply.
    /// Continue a task with --task-id (and --context-id) from an earlier reply.
    Send {
        #[command(flatten)]
        peer: PeerArgs,
        #[command(flatten)]
        auth: AuthArgs,
        /// The message text.
        #[arg(
            long,
            value_name = "TEXT",
            required_unless_present = "body_file",
            conflicts_with = "body_file"
        )]
        text: Option<String>,
        /// Read the message text from this UTF-8 file (`-`: stdin), at most 1 MiB.
        #[arg(long, value_name = "PATH")]
        body_file: Option<PathBuf>,
        /// messageId to use (default: a new UUID). Reuse one only to resend
        /// the same message after an unknown outcome.
        #[arg(long, value_name = "ID")]
        message_id: Option<String>,
        /// Continue this existing task (input-required, auth-required, ...).
        #[arg(long, value_name = "ID")]
        task_id: Option<String>,
        /// Conversation context from an earlier reply.
        #[arg(long, value_name = "ID")]
        context_id: Option<String>,
        /// Related task for additional context (repeatable).
        #[arg(long = "reference-task-id", value_name = "ID")]
        reference_task_ids: Vec<String>,
        /// Return once the task is created instead of waiting for it to
        /// finish or pause (returnImmediately).
        #[arg(long)]
        return_immediately: bool,
        /// At most this many history messages in the reply.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(i32).range(0..))]
        history_length: Option<i32>,
        /// Accepted output media type (repeatable), e.g. text/plain.
        #[arg(long = "accept-output", value_name = "MEDIA_TYPE")]
        accepted_output_modes: Vec<String>,
    },
    /// Tasks at the peer.
    Task {
        #[command(subcommand)]
        cmd: A2aTaskCmd,
    },
}

#[derive(Subcommand)]
pub(crate) enum A2aTaskCmd {
    /// Show one task (GetTask).
    Show {
        #[command(flatten)]
        peer: PeerArgs,
        #[command(flatten)]
        auth: AuthArgs,
        task_id: String,
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(i32).range(0..))]
        history_length: Option<i32>,
    },
    /// List tasks, newest first (ListTasks, one page).
    #[command(alias = "ls")]
    List {
        #[command(flatten)]
        peer: PeerArgs,
        #[command(flatten)]
        auth: AuthArgs,
        #[arg(long, value_name = "ID")]
        context_id: Option<String>,
        /// Only tasks in this state: submitted, working, completed, failed,
        /// canceled, input-required, rejected, auth-required (or TASK_STATE_*).
        #[arg(long, value_name = "STATE", value_parser = parse_state)]
        status: Option<TaskState>,
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(i32).range(1..=100))]
        page_size: Option<i32>,
        #[arg(long, value_name = "TOKEN")]
        page_token: Option<String>,
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(i32).range(0..))]
        history_length: Option<i32>,
        #[arg(long)]
        include_artifacts: bool,
    },
    /// Ask the peer to cancel a task (CancelTask).
    Cancel {
        #[command(flatten)]
        peer: PeerArgs,
        #[command(flatten)]
        auth: AuthArgs,
        task_id: String,
    },
}

#[derive(Args, Clone)]
pub(crate) struct PeerArgs {
    /// Peer base URL (https). Its Agent Card is read from
    /// <URL>/.well-known/agent-card.json.
    #[arg(
        long,
        value_name = "URL",
        required_unless_present = "card_url",
        conflicts_with = "card_url"
    )]
    peer: Option<String>,
    /// Explicit Agent Card URL instead of the well-known location.
    #[arg(long, value_name = "URL")]
    card_url: Option<String>,
    /// Use this advertised JSONRPC interface URL. Needed when the card's
    /// interface is on another origin than the card itself.
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,
    /// Allow plain http to a loopback address (local development and tests only).
    #[arg(long)]
    allow_http_loopback: bool,
    /// Seconds to wait for the response (default 120 for send, 30 otherwise; at most 3600).
    #[arg(long, value_name = "S", value_parser = clap::value_parser!(u64).range(1..=MAX_TIMEOUT_SECS))]
    timeout: Option<u64>,
}

#[derive(Args, Clone)]
pub(crate) struct AuthArgs {
    /// Read an HTTP Bearer token from this file. Sent only to the selected
    /// interface, and only if the Agent Card declares an HTTP Bearer scheme.
    #[arg(long, value_name = "PATH", conflicts_with = "token_env")]
    token_file: Option<PathBuf>,
    /// Read an HTTP Bearer token from this environment variable (same rules).
    #[arg(long, value_name = "NAME")]
    token_env: Option<String>,
}

fn parse_state(s: &str) -> Result<TaskState, String> {
    let upper = s.trim().to_ascii_uppercase().replace('-', "_");
    let name = if upper.starts_with("TASK_STATE_") {
        upper
    } else {
        format!("TASK_STATE_{upper}")
    };
    match TaskState::from_proto_name(&name) {
        Some(TaskState::Unspecified) | None => Err(format!("unknown task state {s:?}")),
        Some(st) => Ok(st),
    }
}

/// A failure with what the user needs to know about it.
struct Failure {
    error: Box<A2aError>,
    /// The request may have taken effect (send, cancel).
    outcome_unknown: bool,
    message_id: Option<String>,
}

impl From<A2aError> for Failure {
    fn from(error: A2aError) -> Self {
        Self {
            error: Box::new(error),
            outcome_unknown: false,
            message_id: None,
        }
    }
}

pub(crate) fn run(cmd: A2aCmd, json: bool) -> anyhow::Result<ExitCode> {
    match execute(cmd, json) {
        Ok(code) => Ok(code),
        Err(f) => Ok(report_failure(&f, json)),
    }
}

fn report_failure(f: &Failure, json: bool) -> ExitCode {
    let e = &f.error;
    if json {
        let mut err = json!({
            "code": e.code(),
            "message": e.to_string(),
            "outcomeUnknown": f.outcome_unknown,
        });
        if let Some(id) = &f.message_id {
            err["messageId"] = json!(id);
        }
        if let A2aError::Rpc(rpc) = e.as_ref() {
            err["rpc"] = serde_json::to_value(rpc).unwrap_or(Value::Null);
        }
        crate::print_json(&json!({ "error": err }));
    }
    eprintln!("pod a2a: {}", sanitize_line(&e.to_string(), 600));
    if f.outcome_unknown {
        eprintln!(
            "pod a2a: the outcome is unknown: the peer may have received and acted on the request. \
             It was NOT retried."
        );
        if let Some(id) = &f.message_id {
            eprintln!(
                "pod a2a: messageId {id}: check with `pod a2a task list`, or resend deliberately \
                 with --message-id {id} so the peer can recognise the duplicate."
            );
        }
        ExitCode::from(EXIT_OUTCOME_UNKNOWN)
    } else {
        ExitCode::from(EXIT_ERROR)
    }
}

fn config(peer: &PeerArgs, default_timeout: u64) -> ClientConfig {
    ClientConfig {
        policy: pod_a2a::target::TargetPolicy {
            allow_http_loopback: peer.allow_http_loopback,
        },
        request_timeout: Duration::from_secs(peer.timeout.unwrap_or(default_timeout)),
        ..ClientConfig::default()
    }
}

fn source(peer: &PeerArgs) -> CardSource {
    match (&peer.peer, &peer.card_url) {
        (_, Some(card)) => CardSource::CardUrl(card.clone()),
        (Some(p), None) => CardSource::Peer(p.clone()),
        (None, None) => unreachable!("clap requires --peer or --card-url"),
    }
}

/// The peer flags again, for the "next step" hints (never a token).
fn peer_flags(peer: &PeerArgs) -> String {
    let mut s = match (&peer.peer, &peer.card_url) {
        (_, Some(c)) => format!("--card-url {c}"),
        (Some(p), None) => format!("--peer {p}"),
        (None, None) => String::new(),
    };
    if let Some(e) = &peer.endpoint {
        s.push_str(&format!(" --endpoint {e}"));
    }
    if peer.allow_http_loopback {
        s.push_str(" --allow-http-loopback");
    }
    s
}

/// A peer-chosen id as a shell word in a hint, or a placeholder when it
/// has characters that would need quoting.
fn hint_id(id: &str) -> String {
    if !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    {
        id.to_string()
    } else {
        "<id from --json>".into()
    }
}

fn read_token(auth: &AuthArgs) -> Result<Option<BearerToken>, Failure> {
    let raw = if let Some(path) = &auth.token_file {
        let meta = std::fs::metadata(path).map_err(|e| {
            A2aError::Invalid(format!("cannot read --token-file {}: {e}", path.display()))
        })?;
        if meta.len() > 16 * 1024 {
            return Err(A2aError::Invalid("--token-file is larger than 16 KiB".into()).into());
        }
        std::fs::read_to_string(path).map_err(|e| {
            A2aError::Invalid(format!("cannot read --token-file {}: {e}", path.display()))
        })?
    } else if let Some(name) = &auth.token_env {
        std::env::var(name)
            .map_err(|_| A2aError::Invalid(format!("environment variable {name} is not set")))?
    } else {
        return Ok(None);
    };
    Ok(Some(BearerToken::new(&raw)?))
}

/// Discovery, interface selection and the token check shared by the
/// commands that call the peer.
fn connect(
    peer: &PeerArgs,
    auth: &AuthArgs,
    default_timeout: u64,
) -> Result<(Discovery, A2aClient), Failure> {
    let cfg = config(peer, default_timeout);
    let token = read_token(auth)?;
    let disc = discover(&source(peer), &cfg)?;
    let iface = disc.select(peer.endpoint.as_deref(), cfg.policy)?;
    if token.is_some() && !card_accepts_bearer(&disc.card) {
        return Err(A2aError::Invalid(
            "the agent card declares no HTTP Bearer security scheme, so the token was not sent"
                .into(),
        )
        .into());
    }
    let client = A2aClient::new(iface, token, cfg)?;
    Ok((disc, client))
}

fn card_accepts_bearer(card: &AgentCard) -> bool {
    card.security_schemes.values().any(|s| s.is_http_bearer())
}

fn execute(cmd: A2aCmd, json: bool) -> Result<ExitCode, Failure> {
    match cmd {
        A2aCmd::Card { peer } => card(&peer, json),
        A2aCmd::Send {
            peer,
            auth,
            text,
            body_file,
            message_id,
            task_id,
            context_id,
            reference_task_ids,
            return_immediately,
            history_length,
            accepted_output_modes,
        } => {
            let text = read_text(text, body_file)?;
            let message_id = match message_id {
                Some(id) if id.trim().is_empty() => {
                    return Err(A2aError::Invalid("--message-id is empty".into()).into());
                }
                Some(id) => id,
                None => Message::new_id(),
            };
            let mut message = Message::user_text(message_id.clone(), text);
            message.task_id = task_id.unwrap_or_default();
            message.context_id = context_id.unwrap_or_default();
            message.reference_task_ids = reference_task_ids;
            let configuration = (return_immediately
                || history_length.is_some()
                || !accepted_output_modes.is_empty())
            .then_some(SendMessageConfiguration {
                accepted_output_modes,
                history_length,
                return_immediately,
            });
            let (_disc, client) = connect(&peer, &auth, 120)?;
            let sent_task = message.task_id.clone();
            let sent_context = message.context_id.clone();
            let reply = client
                .send_message(message, configuration)
                .map_err(|error| Failure {
                    outcome_unknown: error.outcome_unknown(),
                    message_id: Some(message_id.clone()),
                    error: Box::new(error),
                })?;
            let mut warnings = Vec::new();
            if let SendMessageResponse::Task(t) = &reply {
                if !sent_task.is_empty() && t.id != sent_task {
                    warnings.push(format!(
                        "sent to task {sent_task} but the peer answered with task {}",
                        sanitize_line(&t.id, 100)
                    ));
                }
                if !sent_context.is_empty()
                    && !t.context_id.is_empty()
                    && t.context_id != sent_context
                {
                    warnings.push(format!(
                        "sent in context {sent_context} but the peer answered in context {}",
                        sanitize_line(&t.context_id, 100)
                    ));
                }
            }
            print_send(
                &peer,
                client.interface(),
                &message_id,
                &reply,
                &warnings,
                json,
            );
            Ok(ExitCode::SUCCESS)
        }
        A2aCmd::Task { cmd } => match cmd {
            A2aTaskCmd::Show {
                peer,
                auth,
                task_id,
                history_length,
            } => {
                let (_d, client) = connect(&peer, &auth, 30)?;
                let task = client.get_task(&task_id, history_length)?;
                if json {
                    crate::print_json(&task_json(&task, client.interface(), None));
                } else {
                    print_task(&task);
                    print_next(&peer, &task);
                }
                Ok(ExitCode::SUCCESS)
            }
            A2aTaskCmd::List {
                peer,
                auth,
                context_id,
                status,
                page_size,
                page_token,
                history_length,
                include_artifacts,
            } => {
                let (_d, client) = connect(&peer, &auth, 30)?;
                let resp = client.list_tasks(ListTasksRequest {
                    context_id: context_id.unwrap_or_default(),
                    status,
                    page_size,
                    page_token: page_token.unwrap_or_default(),
                    history_length,
                    include_artifacts: include_artifacts.then_some(true),
                    ..ListTasksRequest::default()
                })?;
                if json {
                    crate::print_json(&json!({
                        "interface": client.interface(),
                        "tasks": resp.tasks,
                        "nextPageToken": resp.next_page_token,
                        "pageSize": resp.page_size,
                        "totalSize": resp.total_size,
                    }));
                } else {
                    let rows = resp
                        .tasks
                        .iter()
                        .map(|t| {
                            vec![
                                sanitize_line(&t.id, 80),
                                t.status.state.word().to_string(),
                                sanitize_line(&t.context_id, 80),
                                t.status
                                    .timestamp
                                    .as_deref()
                                    .map(|s| sanitize_line(s, 40))
                                    .unwrap_or_default(),
                            ]
                        })
                        .collect();
                    crate::print_rows(&["TASK", "STATE", "CONTEXT", "UPDATED"], rows);
                    println!(
                        "{} of {} task(s){}",
                        resp.tasks.len(),
                        resp.total_size,
                        if resp.next_page_token.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "; next page: --page-token {}",
                                hint_id(&resp.next_page_token)
                            )
                        }
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            A2aTaskCmd::Cancel {
                peer,
                auth,
                task_id,
            } => {
                let (_d, client) = connect(&peer, &auth, 30)?;
                let task = client.cancel_task(&task_id).map_err(|error| Failure {
                    outcome_unknown: error.outcome_unknown(),
                    message_id: None,
                    error: Box::new(error),
                })?;
                if json {
                    crate::print_json(&task_json(&task, client.interface(), None));
                } else {
                    print_task(&task);
                    if task.status.state != TaskState::Canceled {
                        println!(
                            "note: the peer answered but the task is {}, not canceled",
                            task.status.state.word()
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
    }
}

fn read_text(text: Option<String>, body_file: Option<PathBuf>) -> Result<String, Failure> {
    let text = match (text, body_file) {
        (Some(t), _) => t,
        (None, Some(p)) => {
            let mut buf = Vec::new();
            let limit = MAX_TEXT_BYTES as u64 + 1;
            let read = if p.as_os_str() == "-" {
                std::io::stdin().lock().take(limit).read_to_end(&mut buf)
            } else {
                std::fs::File::open(&p).and_then(|f| f.take(limit).read_to_end(&mut buf))
            };
            read.map_err(|e| A2aError::Invalid(format!("cannot read {}: {e}", p.display())))?;
            if buf.len() > MAX_TEXT_BYTES {
                return Err(
                    A2aError::Invalid("the message text is larger than 1 MiB".into()).into(),
                );
            }
            String::from_utf8(buf)
                .map_err(|_| A2aError::Invalid(format!("{} is not UTF-8 text", p.display())))?
        }
        (None, None) => unreachable!("clap requires --text or --body-file"),
    };
    if text.len() > MAX_TEXT_BYTES {
        return Err(A2aError::Invalid("the message text is larger than 1 MiB".into()).into());
    }
    if text.is_empty() {
        return Err(A2aError::Invalid("the message text is empty".into()).into());
    }
    Ok(text)
}

// ---------------------------------------------------------------- card

fn card(peer: &PeerArgs, json: bool) -> Result<ExitCode, Failure> {
    let cfg = config(peer, 30);
    let disc = discover(&source(peer), &cfg)?;
    let selected = disc.select(peer.endpoint.as_deref(), cfg.policy);
    let notes = card_notes(&disc.card);
    if json {
        let (sel, err) = match &selected {
            Ok(s) => (serde_json::to_value(s).unwrap_or(Value::Null), Value::Null),
            Err(e) => (
                Value::Null,
                json!({ "code": e.code(), "message": e.to_string() }),
            ),
        };
        crate::print_json(&json!({
            "cardUrl": disc.card_url.to_string(),
            "card": disc.card,
            "selectedInterface": sel,
            "selectionError": err,
            "notes": notes,
        }));
    } else {
        print_card(&disc, selected.as_ref().ok(), &notes);
    }
    match selected {
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            if !json {
                eprintln!("pod a2a: {}", sanitize_line(&e.to_string(), 600));
            }
            Ok(ExitCode::from(EXIT_ERROR))
        }
    }
}

fn card_notes(card: &AgentCard) -> Vec<String> {
    let mut notes = Vec::new();
    if card.capabilities.streaming == Some(true) {
        notes.push("the peer offers streaming; this client uses SendMessage only".into());
    }
    for ext in card.capabilities.extensions.iter().filter(|e| e.required) {
        notes.push(format!(
            "the peer requires extension {} which this client does not support",
            sanitize_line(&ext.uri, 200)
        ));
    }
    if !card.security_requirements.is_empty() || !card.security_schemes.is_empty() {
        if card_accepts_bearer(card) {
            notes.push("the peer declares HTTP Bearer: pass --token-file or --token-env".into());
        } else {
            notes.push(
                "the peer declares security schemes this client cannot satisfy (only HTTP Bearer is supported)"
                    .into(),
            );
        }
    }
    if !card.signatures.is_empty() {
        notes.push("the card is signed; Pod does not verify card signatures".into());
    }
    notes
}

fn yes_no(b: Option<bool>) -> &'static str {
    if b == Some(true) { "yes" } else { "no" }
}

fn print_card(disc: &Discovery, selected: Option<&SelectedInterface>, notes: &[String]) {
    let c = &disc.card;
    println!(
        "agent       {}  (version {})",
        sanitize_line(&c.name, 200),
        sanitize_line(&c.version, 60)
    );
    println!("card        {}", disc.card_url);
    if !c.description.is_empty() {
        println!("about       {}", sanitize_line(&c.description, 400));
    }
    if let Some(p) = &c.provider {
        println!(
            "provider    {} ({})",
            sanitize_line(&p.organization, 200),
            sanitize_line(&p.url, 200)
        );
    }
    println!("interfaces");
    for (i, iface) in c.supported_interfaces.iter().enumerate() {
        let mark = if selected.is_some_and(|s| s.index == i) {
            "  <- selected"
        } else if !Discovery::is_compatible(&iface.protocol_binding, &iface.protocol_version) {
            "  (not supported by this client)"
        } else {
            ""
        };
        println!(
            "  [{i}] {} {} {}{}{mark}",
            sanitize_line(&iface.protocol_binding, 60),
            sanitize_line(&iface.protocol_version, 20),
            sanitize_line(&iface.url, 200),
            if iface.tenant.is_empty() {
                String::new()
            } else {
                format!(" tenant={}", sanitize_line(&iface.tenant, 100))
            }
        );
    }
    println!(
        "capabilities streaming={} pushNotifications={} extendedAgentCard={}",
        yes_no(c.capabilities.streaming),
        yes_no(c.capabilities.push_notifications),
        yes_no(c.capabilities.extended_agent_card)
    );
    if c.security_schemes.is_empty() {
        println!("security    none declared");
    } else {
        for (name, s) in &c.security_schemes {
            println!(
                "security    {}: {}",
                sanitize_line(name, 80),
                sanitize_line(&s.describe(), 200)
            );
        }
    }
    println!(
        "modes       in: {}  out: {}",
        sanitize_line(&c.default_input_modes.join(", "), 200),
        sanitize_line(&c.default_output_modes.join(", "), 200)
    );
    for s in &c.skills {
        println!(
            "skill       {}: {} - {}",
            sanitize_line(&s.id, 80),
            sanitize_line(&s.name, 120),
            sanitize_line(&s.description, 300)
        );
    }
    for n in notes {
        println!("note: {n}");
    }
}

// ---------------------------------------------------------------- results

fn task_json(task: &Task, iface: &SelectedInterface, message_id: Option<&str>) -> Value {
    let mut v = json!({
        "result": "task",
        "interface": iface,
        "state": task.status.state,
        "stateClass": task.status.state.class(),
        "task": task,
    });
    if let Some(id) = message_id {
        v["messageId"] = json!(id);
    }
    v
}

fn print_send(
    peer: &PeerArgs,
    iface: &SelectedInterface,
    message_id: &str,
    reply: &SendMessageResponse,
    warnings: &[String],
    json: bool,
) {
    if json {
        let mut v = match reply {
            SendMessageResponse::Task(t) => task_json(t, iface, Some(message_id)),
            SendMessageResponse::Message(m) => json!({
                "result": "message",
                "interface": iface,
                "messageId": message_id,
                "message": m,
            }),
        };
        if !warnings.is_empty() {
            v["warnings"] = json!(warnings);
        }
        crate::print_json(&v);
        return;
    }
    println!("sent        messageId {message_id}");
    match reply {
        SendMessageResponse::Message(m) => {
            println!("reply       message (no task was created)");
            print_message("  ", m);
            if !m.context_id.is_empty() {
                println!(
                    "next: continue the conversation with --context-id {}",
                    hint_id(&m.context_id)
                );
            }
        }
        SendMessageResponse::Task(t) => {
            println!("reply       task");
            print_task(t);
            print_next(peer, t);
        }
    }
    for w in warnings {
        println!("warning: {w}");
    }
}

fn print_task(t: &Task) {
    let st = &t.status.state;
    println!("task        {}", sanitize_line(&t.id, 200));
    if !t.context_id.is_empty() {
        println!("context     {}", sanitize_line(&t.context_id, 200));
    }
    let class = match st.class() {
        StateClass::Active => "active",
        StateClass::Interrupted => "interrupted",
        StateClass::Terminal => "terminal",
        StateClass::Unknown => "unknown",
    };
    println!(
        "state       {} ({}, {class})",
        st.word(),
        sanitize_line(st.as_proto_name(), 60)
    );
    if let Some(ts) = &t.status.timestamp {
        println!("updated     {}", sanitize_line(ts, 60));
    }
    if let Some(m) = &t.status.message {
        println!("status message");
        print_message("  ", m);
    }
    match &t.artifacts {
        None => {}
        Some(a) if a.is_empty() => println!("artifacts   none"),
        Some(a) => {
            println!("artifacts   {}", a.len());
            for art in a {
                print_artifact(art);
            }
        }
    }
    if !t.history.is_empty() {
        println!("history     {} message(s) (see --json)", t.history.len());
    }
}

fn print_next(peer: &PeerArgs, t: &Task) {
    let flags = peer_flags(peer);
    let id = hint_id(&t.id);
    match t.status.state {
        TaskState::InputRequired => {
            let ctx = if t.context_id.is_empty() {
                String::new()
            } else {
                format!(" --context-id {}", hint_id(&t.context_id))
            };
            println!(
                "next: the agent needs input. Reply with:\n  pod a2a send {flags} --task-id {id}{ctx} --text \"...\""
            );
        }
        TaskState::AuthRequired => println!(
            "next: the agent needs authorization that Pod does not provide or forward. \
             Resolve it out of band as its message says, then check with:\n  pod a2a task show {flags} {id}"
        ),
        TaskState::Submitted | TaskState::Working => println!(
            "next: still running at the peer. Check with:\n  pod a2a task show {flags} {id}\n  (cancel: pod a2a task cancel {flags} {id})"
        ),
        _ => {}
    }
}

fn print_message(indent: &str, m: &Message) {
    println!(
        "{indent}{} {}",
        match m.role {
            pod_a2a::model::Role::Agent => "agent:",
            pod_a2a::model::Role::User => "user:",
            _ => "unknown role:",
        },
        if m.message_id.is_empty() {
            String::new()
        } else {
            format!("(messageId {})", sanitize_line(&m.message_id, 120))
        }
    );
    for p in &m.parts {
        print_part(&format!("{indent}  "), p);
    }
}

fn print_artifact(a: &Artifact) {
    println!(
        "  - {}  (artifactId {})",
        if a.name.is_empty() {
            "(unnamed)".to_string()
        } else {
            sanitize_line(&a.name, 200)
        },
        sanitize_line(&a.artifact_id, 120)
    );
    if !a.description.is_empty() {
        println!("    {}", sanitize_line(&a.description, 400));
    }
    for p in &a.parts {
        print_part("    ", p);
    }
}

fn print_part(indent: &str, p: &Part) {
    let mut tags = Vec::new();
    if !p.media_type.is_empty() {
        tags.push(sanitize_line(&p.media_type, 80));
    }
    if !p.filename.is_empty() {
        tags.push(format!("file {}", sanitize_line(&p.filename, 120)));
    }
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tags.join(", "))
    };
    match &p.content {
        PartContent::Text(t) => {
            let shown = sanitize_text(t, MAX_PART_CHARS);
            let mut lines = shown.lines();
            println!("{indent}text{tags}: {}", lines.next().unwrap_or(""));
            for l in lines {
                println!("{indent}  {l}");
            }
        }
        PartContent::Data(d) => {
            let s = serde_json::to_string(d).unwrap_or_default();
            println!("{indent}data{tags}: {}", sanitize_line(&s, 2000));
        }
        PartContent::Url(u) => {
            println!("{indent}url{tags}: {} (not fetched)", sanitize_line(u, 400));
        }
        PartContent::Raw(r) => println!(
            "{indent}raw{tags}: {} bytes (not saved; see --json)",
            Part::raw_len(r).unwrap_or(0)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_filter_accepts_words_and_proto_names() {
        assert_eq!(parse_state("working"), Ok(TaskState::Working));
        assert_eq!(parse_state("input-required"), Ok(TaskState::InputRequired));
        assert_eq!(
            parse_state("TASK_STATE_AUTH_REQUIRED"),
            Ok(TaskState::AuthRequired)
        );
        assert!(parse_state("unspecified").is_err());
        assert!(parse_state("bogus").is_err());
    }

    #[test]
    fn hints_never_print_ids_that_need_quoting() {
        assert_eq!(hint_id("task-1_a.b:c"), "task-1_a.b:c");
        assert_eq!(hint_id("$(rm -rf x)"), "<id from --json>");
        assert_eq!(hint_id("a b"), "<id from --json>");
        assert_eq!(hint_id(""), "<id from --json>");
    }
}
