# Privacy and network behavior — draft

This is a source-based disclosure for the current Windows pre-alpha, prepared for
maintainer review before a public binary release and SignPath application. It is
not a completed legal/privacy audit. The maintainer's privacy contact, final
third-party policy links, retention commitments and release-specific verification
are **not yet supplied**. Do not substitute a blanket “no data is transferred”
statement for this draft.

## Behavior found in the source

| Feature | Data / destination / trigger | Control and source evidence |
|---|---|---|
| Local terminals, daemon and hooks | Terminal input/output, working directories, agent status and task state are exchanged over local named pipes and stored on the PC. Logs/terminal output can include secrets printed by other programs. | [client transport](crates/pod-client/src/transport.rs), [installation/data locations](docs/14-install.md). Hooks are optional and modify agent configuration after confirmation. |
| Coding agents and commands | Pod launches user-selected terminal commands, including Claude Code and Codex. Those tools can send source, prompts, terminal context and credentials to their configured services. | Review the chosen tool's configuration and current privacy policy. These programs are not included in the installer; their independent behavior is not covered by a zero-network guarantee. |
| GitHub pull requests | Git and `gh` contact the repository's configured remote/GitHub service. Push/PR operations can transmit commits, branch names, PR text and review metadata. | [gh commands](crates/pod-forge/src/gh.rs), [PR service](crates/podd/src/server/pr.rs). Review pushes and remote permissions before use. |
| Background PR refresh | Open task PRs are refreshed every **120 seconds** while an app lifecycle subscriber is connected. This contacts the configured GitHub repository without a new click for every poll. | `POD_PR_POLL=off` disables polling; it does not disable explicitly requested Git/GitHub operations. See [PR service](crates/podd/src/server/pr.rs). |
| Remote machines | SSH sends Pod protocol messages and terminal/task data to a configured machine; selected identity files/ssh-agent authenticate the connection. Connected sessions can reconnect with backoff. | [SSH entry point](crates/pod-remote/src/ssh.rs), [enforced SSH options](crates/pod-remote/src/ssh/config.rs), [machine controller](crates/pod-app/src/machines_ui/controller.rs). Disconnect/remove the configured machine to stop this use. |
| Open terminal links | Clicking an HTTP(S) link invokes the default browser, which contacts the selected address under the browser's policies. | [terminal link handler](crates/pod-app/src/term_links.rs). |
| Extensions | The reviewed JavaScript sandbox does not expose `fetch`, WebSocket or arbitrary process/filesystem modules. This is a limited sandbox observation, not an audit of all dependencies or launched commands. | [sandbox tests](crates/pod-mods/src/runtime.rs). |

Local data locations include `%LOCALAPPDATA%\pod\state`, `podd.log`,
`%APPDATA%\pod`, and worktrees under `%USERPROFILE%\pod\worktrees` (or configured
paths). Uninstallation offers optional data cleanup; worktrees, external agent
profiles and optional retained hooks require separate handling. See the uninstall
documentation before sharing logs or removing data.

No dedicated Pod analytics/telemetry uploader or automatic update checker was
found in the inspected first-party source search. This is **not proof of no
outbound traffic**: framework dependencies, OS services, LSP servers, Git credential
helpers, hooks, extensions and user-launched processes require further audit.
The HTTP dependencies in Cargo.lock are not, by themselves, evidence of what is
sent. Mobile/LAN plans in docs are not evidence that a listener is shipped; the
release's actual enabled features must be checked.

## Required before application / release

Maintainers must confirm the intended disclosure and contact channel, audit the
release build and its dependencies, exercise default and configured network
behavior, supply the relevant GitHub/agent/remote-service privacy policy links,
and document data access/retention and available opt-outs. If data is collected
and sent to systems not specified by the user, the Foundation requires a privacy
policy shown during installation and installer options to disable that behavior.
The current installer has **not** been changed to claim those requirements are
met. Resolve any such findings before asking for approval.
