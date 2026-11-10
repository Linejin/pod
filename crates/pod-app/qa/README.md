# D2 verification record

Branch: `SJin-Choi/d2-apply-ui`. Base includes master `7d63b74` (protocol v7,
Review, Groups, Gates and Agents). Only fixture repositories and shell scripts
were used; no prompts were entered into real Claude Code or Codex sessions.

## Screens and evidence

All PNGs in this directory use DPI-aware `PrintWindow` of the fixture app only.
`capture.log` records 144 DPI (150%) and window bounds on the secondary monitor.
The images were visually inspected. `capture.ps1 -AppPid PID -Name NAME` captures
the app without capturing another app over it. `input.ps1` sends target-window
messages without moving the user's pointer; this is synthetic input, **not proof
of the native keyboard-only gate**. UIAutomation semantic actions were verified
through the returned tree and screenshots.

| Screen | Evidence |
|---|---|
| Virtual repo/task/terminal sidebar, 100 tasks / 300 terminals | `dark-sidebar-final.png`, `light-sidebar-final.png` (final fixed slots); `dark-sidebar-300.png`, `light-sidebar-300.png` (earlier) |
| Command palette | `dark-palette.png` |
| Appearance settings | `dark-settings.png`, `light-settings.png` |
| New Task / validation | `light-new-task.png`, `light-task-validation.png` |
| Review diff and comments | `dark-review.png`, `dark-review-comment.png`, `light-review-comment.png` |
| Refused review delivery while blocked | `light-review-delivery.png` |
| Light terminal palette / permission request | `light-terminal-blocked.png` |
| Status source, confidence, freshness | `light-status-inspector.png` |
| Agent hierarchy | `light-agent-tree.png` |
| Group cards / successful check | `light-group.png`, `light-group-check.png` |
| Pick winner confirmation | `light-pick-winner.png` |
| Pending gate / persistent notification | `light-gates.png`, `dark-gates.png` |
| New Group | `dark-new-group.png` |
| Resumed/completed fixture output, dark terminal defaults | `light-terminal-resumed.png`, `dark-terminal.png` |
| Exit zero versus disconnected | `dark-terminal-exited.png`, `dark-disconnected.png` |

Historical before evidence is retained in `../screenshots/review/` and
`../screenshots/orch/`, especially `review/01-review-panel.png`. These are earlier
fixture captures, not a controlled same-data before/after pair. There was no light
theme before D2; a complete per-screen paired baseline has not been produced.

## Behavior exercised

The read-only fixture exposes all 100 tasks and 300 terminals without spawning
processes. Real integration used only `podd-d2-ui-qa`, a repository under
`target/d2-demo`, and a shell fixture that emits working/blocked/done OSC status.
Review comment creation succeeded; sending while blocked was visibly refused.
Group check `cmd.exe /c echo pass` returned pass for both members. Pick winner
opened the existing removal confirmation; it was cancelled. A pending gate was
rendered without approving any real request. Theme changes immediately updated
chrome and terminal default colors. New Task's empty-name validation stayed in
the open dialog. Palette Enter opens a destination dialog without the Enter
falling through and closing it again.

## Part 1 historical gaps (see D2b below for current status)

- Native keyboard-only repo-open through done-confirmation is not proven:
  Windows refused foreground keyboard input even with restore requested. Targeted
  synthetic messages and semantic button clicks are not an equivalent pass.
- Complete dark/light before/after coverage per screen is incomplete.
- The virtual sidebar was rendered with 100/300 data; no numeric performance
  claim is made and no stress/performance run was performed.
- Settings and status inspector currently use dialogs rather than the specified
  settings tab and right inspector. Settings offer font family, not font-size or
  a separate terminal-font category; shortcut changes require restart.
- Palette uses substring search, not fuzzy highlight or recents. Sidebar lacks
  the specified search/filter and branch secondary row. Final sidebar status
  slots prevent long labels displacing names;
  older captures show the bug that prompted the fix. Context-menu keyboard access
  needs native QA.
- Toast timeout pauses on hover, not keyboard focus; Narrator announcement is
  unverified. Loading uses explanatory text rather than a skeleton.
- Native caption drag/Snap, Korean composition, small-window layout and focus
  return across every modal still need live verification. D3 covers the broader
  DPI/high-contrast/Narrator matrix, but these D2 gaps must not be called passed.
- Protocol-v7 terminal parent fields are not yet used for loose terminals in
  Agents; existing task parent relations remain intact. `terminal.lagged` does
  trigger reattach in the app.

Build/test/license results are recorded in `verification.txt`. The coordinator
identified the lagged-attach failure as a known timing-sensitive podd test and
requested exactly one targeted rerun. That rerun passed (1 passed, 3 filtered,
36.82s); no podd code was changed, and the daemon owner is hardening the test.
The original full-suite failure remains recorded rather than rewritten as a
full-suite pass. The unverified UI gates above still mean D2 is **not accepted
as complete**.

Lifecycle check: a fixture `cmd /c exit 0` showed a neutral exited badge and `[exited 0]` banner. Stopping only the fixture daemon showed the amber reconnect banner and stale statuses, distinct from exit; auto-start was deliberately unavailable for this capture through an isolated POD_DAEMON override. The app was closed and the fixture daemon was restarted through `pod ls` then stopped with `server stop --kill`, releasing all four holders (three still running).


## D2b controlled screen comparison

Baseline is the actual app at `18b4129`, rebuilt in this worktree's private
target directory. After images use the D2b implementation with protocol-v8
master merged. Each side uses the same small fixture recipe: one blocked shell,
two comparison shells with a passing check, exit code 7, one review comment and
one pending gate. The fixture was recreated after the protocol update; paths,
daemon PIDs, commit hashes and elapsed times are diagnostic data, not copy.

All `d2b-*.png` images are target-window PrintWindow captures on the secondary
monitor at 144 DPI (150%), personally inspected. No real Claude/Codex prompt was
sent. `d2b-fixture.ps1`, `d2b-launch.ps1` and `d2b-screens.ps1` reproduce the
setup and semantic/synthetic navigation. A fresh config is required for each
capture pass. This is visual evidence, not native keyboard-only evidence.

Each screen has four paths: `d2b-before-dark-SCREEN.png`,
`d2b-after-dark-SCREEN.png`, `d2b-before-light-SCREEN.png`,
`d2b-after-light-SCREEN.png`.

| SCREEN | Change checked |
|---|---|
| review | Korean toolbar/comment/send copy; sidebar toast leaves send button clear |
| settings | Dialog becomes a center tab; UI/terminal font and size; immediate shortcut preference |
| palette | Korean commands/hints; recent ordering; Unicode fuzzy matching |
| repository | Korean path prompt and open action |
| new-task | Korean labels, hints and actions; identifiers stay unchanged |
| new-terminal | Korean program choices, name/path labels and actions |
| new-group | Korean comparison prompt and member controls |
| agents | Korean hierarchy guidance and summary labels; parent logic unchanged |
| gates | Korean pending/expiry/decision copy; no fixture request approved |
| notifications | Korean history; dedicated sidebar stack remains outside the work area |
| group | Korean comparison/check/member actions and summaries |
| terminal | Full useful sidebar names, branch secondary rows, themed terminal |
| inspector | Status dialog becomes right dock inspector with source/confidence/freshness |

Native keyboard-only navigation failed at the OS foreground-input boundary:
see `d2b-native-keyboard.txt` (`window_not_focused`, even with restore requested).
No native keyboard, Korean IME, caption drag/Snap, focus return or Narrator pass
is claimed. Broader mixed-DPI, high-contrast and small-window checks remain D3.
The library's dock/input menus and diff accessibility labels also use the host
Korean catalog through rust-i18n's extension hook. The library's hard-coded dialog
close accessibility label remains English; changing that requires a toolkit API
or upstream patch. Host copy is in `src/strings.rs`; status badge copy remains
owned by pod-ui.

The static loading skeleton respects reduced motion. Settings and inspector
tabs are not restored after restart; layout restoration belongs to the parallel
bugfix work. The latest master submit-confirmation presentation is merged: pending, confirmed,
unconfirmed and Enter actions now use the central Korean catalog. No real agent
submission was exercised in this design fixture.
The full D2 accessibility gate remains open; implementation completion does not
mean the unverified native gate passed.

D2b integration note: master through a35648d is merged locally (7a63fdb), including
the parallel worker's parent tree, OSC reply, focused-terminal toast and panel
restoration fixes. Those changes are inherited, not reimplemented by this task.
Review's empty/error/loading content now has an explicit full-width flex column.

Final results and clickable four-way per-screen comparison are in [d2b-report.md](d2b-report.md); exact commands/tails are in [d2b-verification.txt](d2b-verification.txt). Final source is 8d0f633. Full workspace tests: 270 passed, 0 failed, 1 existing ignored doctest; strict clippy has zero warnings; deny passes all four categories. Native keyboard-only remains blocked, not passed.

## D3 finishing pass

See [d3/report.md](d3/report.md) and the itemized PASS/MANUAL checklist in
[docs/10-ui-design.md](../../../docs/10-ui-design.md). Deterministic captures,
comparison/update commands and limitations are in
[tools/visual/README.md](../../../tools/visual/README.md).
The D3 capture and UIA checks never acquire foreground input. Native keyboard,
IME and actual Narrator speech remain manual under the coordinator's instruction.
