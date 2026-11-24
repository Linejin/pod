# Improvement workstream

This workstream implements the reviewed backlog in `docs/24-improvements.md`.
Execution and integration records belong in `docs/25-improvement-execution.md`.

- The integration branch is `improvements/stability-foundation`. Worker branches
  must start from its recorded baseline and use separate Orca worktrees.
- Do not merge, cherry-pick, rebase, push, or otherwise write these changes to
  `main` or `master`. Do not update those local or remote branch refs.
- Workers commit only their assigned changes on their own feature branches.
  The coordinator reviews and integrates into the integration branch only.
- Do not push, open pull requests, publish releases, or change GitHub settings
  without a separate user instruction. Local work and commits are authorized.
- Follow the assigned file ownership. Ask the coordinator before expanding
  scope or editing files owned by another worker. Avoid unrelated formatting.
- Add focused regression coverage for data loss, persistence, or concurrency
  fixes. Run relevant existing checks. Report the exact commands and outcomes;
  blocked or unexecuted tests are not passing tests.
- Apply the documentation policy in `CONTRIBUTING.md` to each product change.
  In the completion report, list the docs updated and the verification evidence,
  or give a specific reason the change has no documentation impact.
  Run `powershell -NoProfile -File tools/check-docs.ps1` after documentation edits.
- Windows Smart App Control may block Rust tools with error 4551. Do not disable
  security settings or silently substitute another test environment. Report the
  limitation and perform the static checks that remain available.
- Preserve command trust, worker authorization, and existing public protocol
  compatibility. Do not add dependencies without a demonstrated need.
- Workers leave central roadmap and execution documents to the coordinator and
  return commit hashes, changed files, affected document paths, validation
  evidence, and remaining risks. The coordinator updates the affected central
  docs at integration.
