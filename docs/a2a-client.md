# A2A client (`pod a2a`)

`pod a2a` is a client for the open **Agent2Agent (A2A) protocol, version 1.0**.
It talks to one peer agent that you name explicitly. Use it instead of Pod's
internal orchestration mail (`pod orch`) when the other agent speaks the
standard. Pod runs no A2A server. It opens no listener, starts no background
agent, and never relays work through podd.

Standard followed: [A2A specification v1.0.1](https://a2a-protocol.org/v1.0.1/specification/)
and its release-tagged `specification/a2a.proto` (tag `v1.0.1`, commit
`3303592588e388e62e0f69f701af531d2f4e3991`).

## What is supported

| Area | Supported | Not supported |
| --- | --- | --- |
| Discovery | `/.well-known/agent-card.json` under the peer URL, or an explicit card URL | registries; card signature verification (signatures are shown, not checked) |
| Binding | JSON-RPC 2.0 over HTTPS (`protocolBinding: JSONRPC`, `protocolVersion: 1.0`) | gRPC, HTTP+JSON/REST, A2A 0.3 cards |
| Methods | `SendMessage`, `GetTask`, `ListTasks`, `CancelTask` | `SendStreamingMessage`, `SubscribeToTask`, push notification configs, `GetExtendedAgentCard` |
| Content | text, `data` (JSON), `url` and `raw` parts in replies (shown only) | sending files; downloading `url` parts |
| Auth | HTTP Bearer token from a file or an environment variable | OAuth flows, OpenID Connect, API keys, mTLS |

Every request carries `A2A-Version: 1.0`. Payloads are canonical ProtoJSON:
camelCase fields, enums such as `TASK_STATE_COMPLETED` and `ROLE_USER`, and
exactly one member set in each `oneof`.

## Commands

```text
pod a2a card        PEER
pod a2a send        PEER (--text TEXT | --body-file PATH|-)
                    [--message-id ID] [--task-id ID] [--context-id ID]
                    [--reference-task-id ID]... [--return-immediately]
                    [--history-length N] [--accept-output MEDIA_TYPE]...
pod a2a task show   PEER TASK_ID [--history-length N]
pod a2a task list   PEER [--context-id ID] [--status STATE]
                    [--page-size N] [--page-token TOKEN]
                    [--history-length N] [--include-artifacts]
pod a2a task cancel PEER TASK_ID
```

Replace `PEER` with exactly one of `--peer URL` and `--card-url URL`.
Every command except `card` also takes `--token-file PATH` or
`--token-env NAME`. All commands take `--endpoint URL`,
`--allow-http-loopback` and `--timeout S`. Add the global `--json` flag for
machine-readable output.

### 1. Look at the peer

```text
pod a2a card --peer https://agent.example.com
```

This prints the card: name, version, interfaces, capabilities, security
schemes, skills and notes. It also marks the interface Pod would use. Pod
picks the first `supportedInterfaces` entry with binding `JSONRPC` and
protocol version `1.0`; a patch suffix such as `1.0.1` also matches. That
interface's URL must be on the same origin as the card: same scheme, host
and port. A card is peer data, not permission. If its interface lives
elsewhere, Pod refuses, and you must name that URL yourself with
`--endpoint URL`. The URL must be one the card advertises.

### 2. Send a message

```text
pod a2a send --peer https://agent.example.com --text "Summarise the release notes"
pod a2a send --peer https://agent.example.com --body-file request.txt
```

A message is one UTF-8 text part, up to 1 MiB. The reply is one of two kinds:

- **Message**: a direct answer. No task was created. To keep talking in the
  same conversation, pass the printed `--context-id`.
- **Task**: shows the task id, context id, state, status message, artifacts
  (text, `data` as JSON, `url` marked *not fetched*, `raw` as a byte count
  marked *not saved*), and the history length.

By default the peer answers when the task finishes or pauses. With
`--return-immediately`, it answers as soon as the task exists. Then check
on it with `pod a2a task show`.

### 3. Task states and what to do next

| State | Class | Next step Pod prints |
| --- | --- | --- |
| `submitted`, `working` | active | `pod a2a task show …` or `pod a2a task cancel …` |
| `input-required` | interrupted | `pod a2a send … --task-id T --context-id C --text "..."` |
| `auth-required` | interrupted | Authorize out of band, as the agent's message says, then `task show`. Pod never supplies or forwards credentials for the agent. |
| `completed`, `failed`, `canceled`, `rejected` | terminal | none: the peer rejects more messages to the task (`UnsupportedOperationError`) |
| anything else | unknown | reported as is |

With `--json`, `state` holds the proto enum name and `stateClass` holds
`active`, `interrupted`, `terminal` or `unknown`. `result` is `task` or
`message`. `messageId` is the id Pod sent.

### 4. Tasks

```text
pod a2a task list   --peer https://agent.example.com --status working
pod a2a task show   --peer https://agent.example.com 9f0c…
pod a2a task cancel --peer https://agent.example.com 9f0c…
```

`list` fetches one page, newest first. If more pages exist, it prints the
`--page-token` for the next one.

## Exit codes and errors

| Code | Meaning |
| --- | --- |
| 0 | The peer gave a valid answer. Check the task state: `failed` is still exit 0. |
| 2 | An error with a known outcome: a refused URL, a card problem, a JSON-RPC error such as `TaskNotFoundError (-32001)` or `VersionNotSupportedError (-32009)`, HTTP 401/403 (`auth-required`), a redirect, or a connection that failed before anything was sent. |
| 4 | **Outcome unknown** (`send` and `task cancel` only). The request may have reached the peer and taken effect, but no valid answer came back: a timeout, a dropped connection, an oversized or invalid reply, or HTTP 5xx. |

Pod makes a single attempt. After exit 4 from `send`, check the peer's
tasks with `pod a2a task list`. If you choose to resend the same message,
use the printed `messageId` as `--message-id` so the peer can spot the
duplicate (spec §3.3.1). After exit 4 from `task cancel`, check the task
with `pod a2a task show` and its task ID.

With `--json`, errors print `{"error": {"code", "message", "outcomeUnknown",
"messageId"?, "rpc"?}}`. The `rpc` object holds the JSON-RPC `code`, the A2A
error `kind`, the `message` and the `data` details.

## Safety rules

- **HTTPS only.** TLS uses rustls and trusts the operating system's
  certificate store. If `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, it replaces
  that store. Revocation is not checked.
- **Plain HTTP for local development only.** `http://` is accepted only for
  127.0.0.0/8, `::1` or `localhost`, which connects to loopback addresses
  only, and only with `--allow-http-loopback`. An https card can never lead
  to an http interface.
- **No redirects.** A 3xx answer is reported, never followed, so a request
  or token cannot be moved to another URL or downgraded to http.
- **No credentials in URLs.** URLs with `user:password@`, fragments,
  whitespace or non-ASCII hosts are refused. Use punycode for such hosts.
- **Tokens are opt-in and bound to one endpoint.** `--token-file` or
  `--token-env` supplies an HTTP Bearer token. Pod sends it only if the card
  declares an HTTP Bearer security scheme, and only on JSON-RPC requests to
  the selected interface, never on the card fetch. The token is never
  printed or logged, and Pod never sends its own credentials.
- **Bounded.** The card is read up to 512 KiB, a response up to 8 MiB, and a
  request up to 2 MiB. Connecting times out after 10 s. A response times out
  after 120 s for `send` and 30 s otherwise; set it with `--timeout`, up to
  3600 s. Each call uses one fresh connection: no proxy, no cookies, no
  connection reuse.
- **Peer content is data.** Control characters, ANSI escapes and bidi
  overrides in peer text are printed escaped (`\u{1b}`). Pod never runs
  returned content, opens or downloads `url` parts, or writes `raw` parts to
  disk. Every reply part is available unchanged under `--json`.
- **Local only.** `pod a2a` refuses `--machine`, and it does not use podd,
  `pod orch` mail or worker permissions.

## Local development against a loopback agent

```text
pod a2a card --peer http://127.0.0.1:8000 --allow-http-loopback
pod a2a send --peer http://127.0.0.1:8000 --allow-http-loopback --text "hello"
```

## For code: the `pod-a2a` crate

The CLI uses the client library in `crates/pod-a2a`.

```rust
let cfg = pod_a2a::ClientConfig::default();
let disc = pod_a2a::discover(&pod_a2a::CardSource::Peer(url), &cfg)?;
let iface = disc.select(None, cfg.policy)?;            // or Some(explicit_endpoint)
let client = pod_a2a::A2aClient::new(iface, None, cfg)?;
let reply = client.send_message(pod_a2a::model::Message::user_text(id, text), None)?;
```

The API is blocking and runs each client on its own current-thread runtime,
so call it from a background thread, not the UI thread. Every request is a
single exchange. When `A2aError::outcome_unknown()` returns true, the caller
must not resend the request automatically.
