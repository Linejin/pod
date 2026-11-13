# Code signing policy

**Draft — SignPath Foundation approval has not been obtained. No application has
been submitted by this worktree, and no Foundation-signed Pod release is available
from this work.** The project intends to apply for the Foundation's OSS program;
eligibility and acceptance are determined by the Foundation. Public source alone
does not establish approval or reputation.

Unsigned application/release candidates are clearly identified as **UNSIGNED**.
They are not represented as compatible with Windows Smart App Control. Checksums
identify bytes; they do not establish a trusted publisher.

## Proposed scope and process (pending maintainer and Foundation review)

Only Pod binaries built from the reviewed public project source may be submitted:
`pod-app.exe`, `podd.exe`, `pod-holder.exe`, `pod.exe`, and `pod-hook.exe`.
Installer, uninstaller and setup runtime signing requires the separately reviewed
Inno integration described in [the application plan](docs/install/signpath.md).
ConPTY and OpenConsole retain their upstream Microsoft signatures and are never
re-signed with the project signing identity.

Signing inputs must originate from an approved GitHub-hosted build, tied to a
reviewed commit/tag and the repository's build configuration. Each signing request
requires a trusted release approver's **manual** decision. A CI token only submits
requests; it must not authorize unattended production signing. Returned artifacts
must pass Authenticode, timestamp and complete payload checks before final
packaging and checksums. Source, build configuration and signing-policy changes
are subject to review.

## Team responsibilities — maintainers must fill these in

| Role | Responsible members / public team link | Current verification |
|---|---|---|
| Committers / authors | **TBD by project administrator** | Membership and repository rights unverified |
| Reviewers | **TBD by project administrator** | Review rules and branch protection unverified |
| Release signing approvers | **TBD by project administrator** | SignPath approval authority unconfigured |

All team members must use MFA for GitHub and SignPath access before activating
Foundation signing. This document does **not** claim that anyone's MFA or roles
have been checked. Administrators must verify these privately; never publish
recovery codes, tokens or personal identity documents.

## Privacy and third parties

See [Privacy and network behavior — draft](PRIVACY.md). Pod can invoke coding
agents, Git/GitHub tooling and SSH connections; it does not promise zero network
traffic. Configured PRs are polled in the background while an app is subscribed.
The draft records the source evidence, controls and remaining audit work.

## Activation and support acknowledgment

Do not add a SignPath support badge or claim free signing is currently provided
until the Foundation approves the project and the configured signing process has
been verified. After approval, replace this draft status, fill in actual role/team
links, confirm the privacy statement, and add the exact acknowledgment required by
the then-current [Foundation terms](https://signpath.org/terms.html), linking
SignPath.io and SignPath Foundation. Add a **Code signing policy** link to every
download/release page. Approval does not eliminate the need to verify each release
and test installation, execution and uninstallation on SAC-enabled Windows.

For suspected signing misuse or vulnerabilities, follow [SECURITY.md](SECURITY.md).
