# Pod Windows identity QA

Date: 2026-11-09. Branch: `SJin-Choi/fu-icon`; base: `bf21c27`.

## Identity

Selected **Terminal bays**: an original terminal prompt and two agent cells inside
a blue pod. The dark interior works on light taskbars and the pale blue outline
works on dark taskbars. The agent-cluster option loses the terminal meaning; the
monogram is less legible at 16 px. All artwork is Apache-2.0, copyright SJin-Choi;
no third-party product artwork (including Zed or Lucide) is incorporated.

- Master: `../pod.svg`; nine optical PNGs: `../pod-{16,20,24,32,40,48,64,128,256}.png`.
- Multi-image Windows bundle: `../pod.ico`; generator: `../generate.py`.
- Three concepts and all sizes, dark/light: [contact sheet](contact-sheet.png).
- Alternative originals: `../concepts/`.

## Implementation

All five executables have group-icon resource 1 and individual VERSIONINFO:
Pod, SJin-Choi, workspace version 0.1.0 (fixed Windows tuple 0.1.0.0), Apache-2.0
file descriptions and copyright. `embed-resource` 3.0.12 is MIT and was already
in Cargo.lock; `raw-window-handle` 0.6.2 is MIT/Apache-2.0 and was also already
present. No dependency versions were added or changed.

Resources are linked per binary, never through podd's client rlib. The existing
ConPTY staging build script, manifests/DPI behavior and GUI subsystem attributes
remain intact. The GUI sets both HWND and class icons from the embedded resource,
choosing the initial window's DPI sizes and caching native handles by size.

Cross-worker source edits are limited to build dependencies in pod-app/podd/
pod-cli/pod-hook Cargo.toml, two additional Windows API features plus the handle
trait dependency in pod-app, and four lines in pod-app/src/main.rs (module and
icon-install call). Runtime implementation lives in new app_icon.rs. No feature
worker's editor/terminal/daemon logic was changed.

The installer uses the same ICO, explicit shortcut icon index 0, uninstall icon,
and original welcome/header bitmaps. Installer test cleanup uses finally and
validates temporary deletion boundaries; the shared CPU gate can be selected
with POD_CPU_GATE.

## Validation

- Final release build via `tools/installer/build.ps1 -SkipDeny`: passed;
  license check had already passed separately. [Build log](installer-build.txt).
- Full workspace test invocation ran once: **339 passed, 0 failed, 2 ignored**.
  [Log](workspace-tests.txt). Existing integration tests invoke their own serial
  prerequisite builds while the outer cargo test waits; no extra stress/perf run.
- Clippy for all targets of pod-app/podd/pod-cli/pod-hook, `-D warnings`: **passed,
  zero warnings**. [Log](clippy.txt).
- `cargo deny check`: **passed**. Existing policy has an unused ISC allowance
  warning; no dependency/license exception was added. [Log](cargo-deny.txt).

```text
advisories ok, bans ok, licenses ok, sources ok
```

- All 45 embedded PNG resource payloads match their optical sources byte-for-byte:
  [verification](embedded-icons.txt), reproducible with `python assets/icon/verify.py`.
- Installer install/upgrade/uninstall: **63 passed, 0 failed** (previous 53 plus
  metadata/shortcut/uninstall/native window-icon checks). [Test log](install-test/test-log.md).
- Screenshot install/upgrade/uninstall also completed: **0 processes**, no install
  directory/data/uninstall registration, **PATH restored**.
  [Capture log](installer-pages-capture.txt).
- No Pod installation or test processes remain. No system settings were changed.

Logs preserve tool output with PowerShell's redundant NativeCommandError wrapper
removed from successful stderr output and trailing whitespace trimmed; exit
codes were checked independently.

## Native screenshots

App and Properties windows, plus every installer page, were placed on the
secondary monitor. The app reports **144 DPI / DISPLAY2**, physical monitor bounds
`2560,235,1920,1200`: [capture metadata](capture-info.txt).

- [App window](app-window.png) and [installed fixture](install-test/app-installed-fixture.png).
- Actual taskbar icon: checked on the primary taskbar (Windows has **no secondary
  taskbar window** in this session) while Pod runs on the secondary monitor. The
  capture showed the developer's whole taskbar, so it is not published.
  Taskbar settings were not changed.
- [Explorer Properties / Details](explorer-properties-details.png) and
  [General](explorer-properties-general.png), both showing the new icon.
- Installer [welcome](installer-pages/01-welcome.png),
  [tasks](installer-pages/03-tasks.png), [finished](installer-pages/05-finished.png).
  All 15 install/upgrade/uninstall pages are in `installer-pages/`.

Desktop CopyFromScreen is unavailable in this session. These are genuine native
PrintWindow captures. For the system-DPI-aware Properties/installer dialogs,
DWM frame and native WM_PRINT client capture are combined to avoid stale/omitted
controls, then the unused physical-DPI margin is cropped; no UI content is mocked.

## Distribution

`dist/Pod-Setup-0.1.0-x64.exe` (12.2 MB), SHA-256:
`6c7178c64f7d7cb331e8c90fa91d6a055f4a8b2639f8fda55cce179c596b977f`.

`dist/Pod-0.1.0-x64-portable.zip` (16.9 MB), SHA-256:
`d0dfd5da1fbb42f3e6fa12a2edbbda23c20edb7f69bc00c295a6f62203688ed7`.

Distribution files stay git-ignored. Source, original assets, QA evidence and docs
are committed on this worktree's own branch; nothing is merged or pushed.
