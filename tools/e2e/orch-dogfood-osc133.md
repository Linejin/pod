You coordinate a small real change in this repository (a throwaway clone of Pod; it has no remote, never push).

Change: podd's default task shell (`crates/podd/src/server/tasks.rs`, `powershell -NoLogo`) is not shell-integrated. Add OSC 133 prompt marks the same way `crates/pod-app/src/layouts/shell_integration.rs` does for Pod's own shells, with a unit test.

1. Read `pod guide orchestration --lang en` and follow it.
2. Start two workers with `pod orch worker start` (short specs):
   - A, `--agent codex`: implements the change in podd. Tell A to ask you one question with `pod orch ask` before writing code (for example where the shared code should live).
   - B, `--agent claude`: writes or extends the unit test, and reviews A's diff (`pod task diff <A's task>`) once A is done; B reports findings in its done report.
3. Answer questions (`pod orch check`, `pod orch reply`). End your turn while waiting; do not poll.
4. When both reported done, merge both branches into your worktree (`git merge --no-edit pod/<name>`), run only the touched test, and report the result.

Build rules for everyone (put them in both specs): never build or test the whole workspace. Only `cargo test -p podd --lib <filter>`, run in PowerShell as:
`Remove-Item Env:CARGO_TARGET_DIR -EA 0; $env:CARGO_BUILD_JOBS='2'; $env:RUSTC_WRAPPER='C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'; & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1; cargo test -p podd --lib <filter>`
Do not push, do not edit files outside your worktree.
