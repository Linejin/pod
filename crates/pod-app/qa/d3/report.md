# D3 finishing pass — 2026-11-09

Branch: `SJin-Choi/d3-finish`. Master through `8b34bc3` was merged before final
verification. This branch is not merged back or pushed.

## Changes

Compact sidebar rows keep terminal names and quieter status badges on one line.
Agents show executable basenames, with complete commands in offset tooltips.
The palette has an opaque clipped surface. Settings and inspector panels now
restore across restart. Named dock tabs, Close/menu controls, settings choices,
tree roles and explicit focus rings improve the accessibility tree.

The palette includes the documented shortcut table. Reduced motion covers legacy
dialog slide/focus delay and cursor blinking. App-local high-contrast preview
does not alter Windows settings. Light muted text was darkened to meet AA on
selected/hover surfaces. The Apache-2.0 gpui-component 0.9.0 source is vendored
with a small documented accessibility/motion patch; no new dependency was added.

Coordinator follow-ups: Review keeps at least 320 logical pixels for the diff
and scrolls columns horizontally when needed; request cards show relative time
and disappear when resolved or their terminal exits; registered repositories
receive task-oriented empty guidance; pending exit codes display “종료 중…”.

## Evidence

- [Full checklist and manual steps](../../../../docs/10-ui-design.md#d3-checklist-and-evidence).
- [Capture/update recipe](../../../../tools/visual/README.md).
- [26-screen baseline](../../../../tools/visual/baseline/manifest.json): dark/light
  workspace, palette, Settings, Agents, inspector, repository/task/terminal/group
  dialogs, Gates, notifications, Review error state and shortcut help.
- [Independent capture comparison](current/compare.json): all 26 passed with
  zero pixels above RGB tolerance 8. Every baseline and current screenshot was
  inspected. Successful dimmed diff images are omitted from the commit; compare
  regenerates them. [Comparator self-test](comparator.txt) verifies an actual
  one-pixel failure and diff output.
- [Restoration](restoration/result.txt): UIA invocation, close/relaunch, saved
  Settings and inspector restored; [before](restoration/before.png) and
  [after](restoration/after.png) screenshots inspected.
- [Mixed DPI](mixed-dpi/manifest.json): actual secondary 144 DPI → primary 96 DPI
  → secondary 144 DPI, one nonactivating window, 1024×700 logical size. All three
  captures inspected; no system scale changes.
- [High-contrast preview](highcontrast/manifest.json): four dark/light Settings
  and palette captures inspected; [contrast audit](contrast.json) passes 72/72
  default token pairs (4.5 text, 3 control/focus outlines).
- [Tooltip](tooltip/command-hover.png): targeted WM_MOUSEMOVE only; complete
  command stays below its source row. No pointer movement or foreground takeover.
- [Native attempt](native-desktop.txt): foreground HWND zero, input desktop error
  5, UIA focus unsuccessful. Guarded SendInput refused before injecting keys.

UIA trees contain named Close buttons, tabs and settings controls. No unnamed
button/tab/edit was found in the 26 sampled trees. This is semantic evidence,
not a Narrator speech recording or a complete audit of every dynamic state.

## Final verification

All cargo commands used this worktree's local target, sccache and four jobs, with
the CPU gate before each invocation. Full workspace tests ran once at the end.
No real Claude/Codex prompt, external LSP tool, scaling change or stress run was
used. The workspace's existing real rust-analyzer smoke test remained ignored.

| Command | Result |
|---|---|
| `cargo check -p pod-app` | PASS ([log](check.txt)) |
| `cargo build --workspace --release` | PASS ([log](release.txt)) |
| `cargo clippy -p pod-app -p pod-ui -p pod-terminal-view --all-targets -- -D warnings` | PASS, zero warnings ([log](clippy.txt)) |
| `cargo test --workspace --release -- --test-threads=4` | PASS: 321 passed, 0 failed, 2 existing ignored ([log](tests.txt)) |
| `cargo deny check` | PASS: `advisories ok, bans ok, licenses ok, sources ok` ([log](deny.txt)) |

Cargo deny reports the existing unused ISC allow-list warning, not a license
failure. The exact final log lines are collected in `verification.txt`.
The isolated native daemon stopped with `killed 0 running terminal(s)`; all
fixture windows closed, and no Pod app/daemon remained after final verification.

## Remaining manual work

The coordinator explicitly instructed not to take over the user's desktop.
Native keyboard-only flow and every modal's focus return, actual Narrator speech,
Korean IME composition, absent D2Coding, 125%/175% scaling, Windows contrast-theme
integration and normal-motion timing remain MANUAL. The checklist gives concrete
steps for a later unlocked interactive session. Read-only Review/Gates baselines
do not prove populated review/comments or live cancellation/exit integration;
those follow-up UI corrections need the isolated interactive regression check.

The implementation and background verification are complete; the full native
accessibility acceptance gate is not claimed to have passed.
