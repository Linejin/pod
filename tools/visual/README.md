# Pod visual regression (Windows)

**빌드**: 캡처는 이 worktree의 `target/release/pod-app.exe`만 띄우므로 `cargo build -p pod-app --release`만
빌드한다(workspace 전체 빌드 불필요). `CARGO_TARGET_DIR`을 지워 공유 target 디렉터리를 쓰지 않고, sccache와
`CARGO_BUILD_JOBS=2`를 쓴다. 빌드와 캡처 실행 전마다 `tools/cpu-gate.ps1 -MaxPct 70 -MaxCargo 1`을 돌리고
여유 물리 메모리가 8 GB 이상일 때 시작한다. Captures launch only `--ui-fixture`; no daemon, user repository
or real agent is contacted.

From the repository root:

```powershell
powershell -NoProfile -File tools/visual/capture.ps1
powershell -NoProfile -File tools/visual/capture.ps1 -Mode compare
```

The capture pass uses dark and light themes, Malgun Gothic, reduced motion,
a fresh isolated config per screen and a fixed 1440×960 **physical pixel** window
on the secondary monitor. `manifest.json` records actual HWND bounds and DPI.
Use the same monitor scale, font installation and renderer for comparisons.
The script closes each fixture app even on failure. UIA trees are saved with PNGs.
**UIA 트리**: 덤프가 가끔 제목줄만 돌려준다. 장면별 필수 노드(`uia-required.ps1`)가 나올 때까지
1초 간격으로 최대 `-UiaAttempts`(6)번 다시 받고, 끝내 없으면 빠진 노드 이름으로 캡처를 실패시킨다.
장면 UI 문구를 바꾸면 그 표도 고친다. 자체 시험: `tools/visual/uia-selftest.ps1`.
필수 노드는 레이아웃별 표다(`-Layout`, 작업대 외 6종 포함). 창은 앱이 스스로 최대화한 뒤에 배치한다(그 전에 옮기면 약 50장에 1장꼴로 11px 아래로 찍혔다).
Snapshots are visual evidence, not proof of native keyboard or Narrator behavior.

Compare uses maximum absolute RGB channel distance per pixel (default tolerance 8).
Any pixel above tolerance fails the comparison; there is no percentage allowance
or ignored region. Magenta pixels in `*.diff.png` show failures, with a dimmed
actual image underneath. Missing images or differing dimensions also fail.

To update baselines after reviewing **every** changed image:

```powershell
powershell -NoProfile -File tools/visual/capture.ps1 -Output tools/visual/baseline
```

`-Scenes`, `-Themes` and `-SettleSeconds` (default 2) narrow a run or give slow
scenes time (the editor LSP scenes use 40). The `editor` scene (P3) opens the
files panel of `sample/task-000` on a generated fixture git repository with
language servers off, so it is deterministic; other `editor-*` scenes are QA
evidence, not baselines (`crates/pod-app/qa/editor/run-editor-qa.ps1`).

Commit PNGs, UIA trees and the manifest together with the source change. Never
accept a baseline merely to hide a failure.

**병합 전 확인**: UI를 바꾸는 브랜치는 병합 전에 `-Mode compare`를 돌린다. 화면 전체에
보이는 변경(에이전트 표시, 상태 표시줄, 공통 컨트롤)은 기준 이미지 갱신을 같은 브랜치에
넣는다. 2026-11-10 리뷰에서 기준 이미지가 에이전트 표시 병합보다 앞서 36/36이 실패한 채
남아 있었다(ui-01). 같은 날 fix-ui 브랜치에서 36장을 모두 직접 보고 다시 찍었다
(보조 모니터 144 DPI, 1440×960 physical). Keep QA captures under
`crates/pod-app/qa/d3/`. `-Width 1024 -Height 700` exercises a small physical window.
Changing that size is a separate matrix, not comparable to the default baseline.

`native.ps1 -AppPid PID -Keys 17,75` takes foreground focus with AttachThreadInput
and sends Ctrl+K through SendInput. Input is refused if another HWND owns focus.
`-Text` uses Unicode SendInput, which **does not verify IME composition**.
Only use this helper against an isolated fixture window while keyboard takeover
is appropriate. `contrast.py` audits default tokens against WCAG AA thresholds.

Wide glyphs (Japanese, Korean, Chinese, emoji, box drawing) against Windows Terminal:
`cjk-compare.ps1` captures the `editor-terminal-cjk` scene (terminal font 16 px =
Windows Terminal's default 12 pt) and the same lines in Windows Terminal (its own
defaults, settings untouched) to `target/cjk/` (not tracked: no third-party UI
screenshots in the repository). `python -I tools/visual/cjk-measure.py <png> --box x0,y0,x1,y1`
prints each glyph's ink box relative to its cell slot. See docs/09 §13.1a.

Additional background-only checks (no keyboard or pointer takeover):

```powershell
& tools/visual/capture.ps1 -HighContrast -Scenes @('settings','palette') -Output crates/pod-app/qa/d3/highcontrast
& tools/visual/mixed-dpi.ps1
& tools/visual/restore-panels.ps1
& tools/visual/compare-selftest.ps1
& tools/visual/uia-selftest.ps1
```

Mixed-DPI captures use one window moved secondary → primary → secondary,
measuring actual HWND DPI at every step and sizing it to 1024×700 logical units.
The restore check invokes the inspector through UIA without acquiring focus,
then restarts with the same isolated config and checks both named tabs.
These tests do not change any Windows settings.

The read-only fixture has 100 tasks and 300 terminals. Review and Gates capture
their disconnected/empty states, because the fixture intentionally has no daemon
or real worktree. Populated review comments, live terminal rendering, actual IME,
and full request lifecycles need the separate manual integration checklist in
`docs/10-ui-design.md`; these baselines must not be represented as that coverage.
