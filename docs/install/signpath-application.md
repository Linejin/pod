# SignPath Foundation application — unsubmitted draft

Prepared from the repository and maintainer-provided facts. This is an application
content draft, not a copy of the current web form. No submission or acceptance
has occurred. Replace every TBD and confirm public evidence before submitting at
[the official application page](https://signpath.org/apply).

| Field | Draft / required input |
|---|---|
| Project | Pod |
| Source and proposed homepage | https://github.com/Linejin/pod — maintainer to confirm canonical URL |
| License | Apache License 2.0; dependency and bundled-component notices must accompany the binaries |
| Contact name / email / legal entity / country | **TBD — supplied privately by the applicant, not inferred from Git history** |
| Existing Windows binary release and download page | **TBD — no GitHub release reported by the maintainer; no Pod installer was built on this PC** |
| Maintenance and verifiable reputation | **TBD — provide public release/use/contribution evidence; do not invent adoption or download counts** |
| Committers / reviewers / signing approvers | **TBD — actual public team/member links and responsibilities** |
| GitHub and SignPath MFA | **TBD — administrator confirmation; no secrets or recovery codes in the application draft** |

## Proposed project description

Pod is a Windows-first native desktop IDE for running and supervising terminal
coding agents in separate Git worktrees. It provides persistent terminal sessions,
agent status, diff review, orchestration and an editor. A daemon and terminal
holder keep sessions alive across UI restarts. It is currently pre-alpha, written
in Rust, with Apache-2.0 project source. Claude Code, Codex and other external
agents are user-installed tools and are not included in the Pod package.

## Proposed signing request

We intend to request OSS support for project-owned Windows x64 executables
`pod-app.exe`, `podd.exe`, `pod-holder.exe`, `pod.exe` and `pod-hook.exe`, and to
agree on the correct managed signing flow for our Inno Setup installer and its
uninstaller/setup runtime copies. We understand that the certificate belongs to
SignPath Foundation, approval is discretionary, and source/build provenance and
manual approval are required. We do not currently have Foundation approval.

ConPTY and OpenConsole are pinned upstream MIT components and retain verified
Microsoft signatures. We will not sign upstream components with the project
identity. Proposed artifact rules name only the five Pod payload files and
enforce their product name and version. All production requests are proposed to
require manual release-approver approval; role assignments have not yet been made.

## Proposed build and provenance description

We have prepared a manual GitHub-hosted Windows build with pinned Rust, Inno Setup
and cargo-deny versions, Cargo.lock builds, dependency policy checks, upstream
hash/signature verification, unsigned EXE/ZIP generation and a separate artifact
containing only the five project PE files. It has **not** been run in this work.
After approval, our proposed integration uses the SignPath GitHub connector and
the workflow artifact ID, rather than arbitrary uploaded local files. Signed Inno
packaging still needs the staged internal-runtime and final-installer integration
described in the application plan and Foundation review.

## Privacy and system changes

The linked [privacy draft](../../PRIVACY.md) describes local state/terminal logs,
user-selected external agents, Git/GitHub operations, automatic refresh of configured
open PRs, SSH sessions and browser links. We do not assert zero network transfer.
Maintainers must finalize contact details, third-party policy links and release
network checks, and implement installer disclosure/disable controls if required.

The installer is per-user, offers PATH/shortcut options, and offers optional agent
hook configuration with confirmation and backups. Uninstallation and optional
local data cleanup are documented in [installation instructions](../14-install.md).
The [Code signing policy](../../CODE_SIGNING_POLICY.md) is explicitly a draft with
unassigned roles and unverified MFA. Support acknowledgments/badges will only be
activated after approval.

## Before sending

Proposed download-page text (fill in the actual version, commit, asset URLs and
hashes after building): “Pod [VERSION] is a Windows 10/11 x64 pre-alpha IDE for
managing terminal coding agents, Git worktrees and reviews. Choose the per-user
Inno Setup EXE or the portable ZIP. This release is **UNSIGNED** and is not a
Smart App Control-compatible signed release. Do not disable SAC to use it. The
installer writes to the current user's Programs folder, offers PATH/shortcut
options, and separately offers agent hooks with confirmation. Remove Pod from
Windows Settings or its installed uninstaller; terminal sessions must be stopped
for removal. Check SHA256SUMS against the downloaded files. See **Code signing
policy**, privacy/network disclosure and installation instructions.” Link these
documents at the actual release tag before publishing; do not leave placeholder
links or advertise a file that has not been generated.

Do not send this as a completed application until an actual downloadable release,
public functionality description, verifiable reputation evidence, final policy,
team roles/MFA confirmation and privacy review are available. The local Rust
toolchain is blocked by Windows error 4551; this preparation has not produced the
requested installer or validated signed installation on SAC-enabled Windows.
