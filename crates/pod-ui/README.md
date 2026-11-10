# pod-ui

Apache-2.0 design system for the Pod GPUI application. This crate contains no IPC,
PTY, repository, user-settings or real-work fixture loader.

```rust,no_run
use pod_ui::{apply_theme, Mode, StatusBadge};
// Inside the application's startup closure, after gpui_kit::init(cx):
// apply_theme(Mode::Dark, reduced_motion, cx);
// let badge = StatusBadge::from_snapshot(&snapshot);
// div().child(badge.render(Mode::Dark))
```

`apply_theme` reloads the matching component theme then uses `Theme::update` to
reconcile colors/background tokens, project Base tokens and refresh windows.
It also installs the PodModal key bindings (Tab, Shift+Tab, Escape).
Always use it after a theme change. Pod tokens remain the source of truth.
The font chooser selects installed Segoe UI Variable Text / Segoe UI and
Cascadia Mono / Consolas; GPUI/Windows supplies Korean and emoji glyph fallback.
No fonts are downloaded or bundled.

## Components

- `StatusBadge::from_snapshot`: preserves `display_state`, root, source,
  confidence, final state and held-open children. Set `exit_code` from the
  terminal lifecycle and `stale_since` from the client connection separately.
  Render repeated badges inside parents with stable row IDs.
  The latter never changes the domain state. Low confidence means below 0.8.
  `detail()` exposes full provenance; render it in the inspector.
- `StatusSummary::from_terminals`: unique terminal IDs only; do not pass embedded
  child records again. UI priority is error > blocked > working > unknown > idle,
  followed by all-done/all-exited. This intentionally differs from the engine's
  evidence aggregation. Mixed done/exited stays mixed. `response_count()` caps
  visual counts at 99+, while `label()` keeps exact counts.
- `TreeRow`: stable IDs, indentation, selected/hover/focus, long-label ellipsis,
  full-label tooltip, Enter activation. Host owns expansion and the tree model.
- `IconButton`: embedded Lucide SVG, 28px hit target, tooltip/accessible name,
  Enter/Space activation; disabled/loading suppress activation.
- `semantic_button`: semantic action colors with an added hover border and pressed
  inset; use StatusBadge for noninteractive indicators.
- `Frame` + `FrameKind::{Dialog, CommandPalette, List, Toast, Empty}`: responsive
  maximum widths, typography, loading/error/disabled descriptions and elevation.
  Supply content as `AnyElement`s; set individual controls' disabled state too.
- `FocusReturn`: call `enter` with the initial input/control handle before opening,
  then `leave` on every dismiss path. Nested owners return in reverse order.
  Apply `modal_keyboard` to the frame with its owner handle and enabled tab stops;
  it traps Tab/Shift+Tab and calls your close callback on Escape. Toasts never
  acquire focus automatically. Keep modal-owned notifications inside its layer.
- `ListSelection`: stable ID selection across filtered refreshes, arrow navigation,
  and activation suppression while IME composition is active or loading/disabled.
  Hosts must feed actual input composition state, not infer it from Enter.
- `toast_timeout`: 5s success / 7s information; hover/focus pauses. Attention/error
  are persistent. Hosts own elapsed time, deduplication, maximum-three stacking,
  and notification history; dismissing a toast never resolves a blocked event.

Library defaults retained: chart colors, base red/green/blue/yellow/magenta/cyan,
syntax highlighting, and other library-specific defaults not mapped in §4.5.
Never use those fields as Pod agent status or ANSI colors.

## Terminal boundary

`TerminalPalette::for_mode` provides independent default fg/bg/cursor/selection
and normal/bright ANSI 16 entries. `indexed` retains xterm's 16–255 definitions.
Explicit process truecolor and OSC overrides take precedence. Applying a UI theme
must not recolor already explicit VT output. `pod-terminal-view` currently owns
hardcoded defaults: D2 must add a palette injection boundary there before applying
this palette; importing this crate alone does not change the terminal renderer.

## Gallery

```
cargo run -p pod-ui --example gallery --release
cargo run -p pod-ui --example gallery --release -- --light
```

Default display: first non-primary, with primary fallback. `--primary` selects the
main display for 100% comparison without changing OS settings. `--duration N`
automatically exits after N seconds. Release uses the Windows GUI subsystem.
`--self-check --duration 6` writes the live GPUI focus check to `qa/live-check.txt`;
`--dialog` opens the dialog fixture for capture.
The gallery uses fixture data only. Theme switching, modal focus return, an actual
GPUI input for Korean IME, reduced motion and component states are available.

The vendored icons are from Lucide **0.468.0**, independently fetched upstream.
See `THIRD-PARTY.md` for ISC and Feather MIT notices. No other product's assets are used.
