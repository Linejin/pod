# Native QA automation (D3 MANUAL rows) — 2026-11-09

Branch `SJin-Choi/native-qa`. The Windows session was **locked** for the whole
run (`desktop-probe.log`: `OpenInputDesktop` → error 5, `LogonUI` running), so
no real SendInput, IME or Narrator session was possible. The checks below prove
the behavior inside the app instead and say exactly what each one covers.

## 1. Headless interaction tests — `crates/pod-app/src/native_qa.rs`

`cargo test -p pod-app native_qa -- --nocapture` (log: `tests-native-qa.txt`).

The tests build the real `Workspace` in GPUI's test platform with:

- the app's own key bindings and dock panel registry (`main.rs` `install()`),
- the real toolkit dialogs, inputs, dock and focus handling,
- the **real DirectWrite text system** (Malgun Gothic metrics, not the no-op
  test text system), dark theme, reduced motion,
- `TestBackend` (`source.rs`): an in-process podd double served on the test
  thread. Nothing is spawned or contacted; terminal panels attach to a pipe
  that does not exist.

Input is keyboard only. `keys()` sends keystrokes through GPUI's dispatch
(bindings, key handlers, text input); `press()` sends key-down + key-up (GPUI
clicks a focused button on key-up). IME composition calls the focused
control's own `EntityInputHandler` exactly as GPUI's Windows backend does for
`WM_IME_COMPOSITION` (GCS_COMPSTR → `replace_and_mark_text_in_range`,
GCS_RESULTSTR → `replace_text_in_range`, empty GCS_COMPSTR = cancel). Every
step logs the focus target (`[native-qa] … | focus: …`).

Not covered: Win32 `WM_KEYDOWN` translation, the Microsoft IME and its
candidate window, Narrator speech, the compositor, and pixel output at
125%/175% (the test platform has no renderer on Windows).

| Test | What it proves |
|---|---|
| `a_keyboard_only_repo_task_terminal_blocked_work_done` | Launch focus → Ctrl+K "저장소 열기" → path + Enter (`repo.add`) → Ctrl+N name + Enter (`task.create`, Shell) → new terminal focused, typing reaches it → Ctrl+Shift+B, Tab, Enter on the blocked notice → b1 → palette "응답할 요청 찾기" → b1 → back to work → b1 turns done (toast, no ack) → jump to b1 sends `terminal.ack`. Focus asserted after every step. |
| `a_shortcuts_work_at_launch_and_after_the_focused_panel_closes` | Ctrl+K works with no click at launch; Esc returns to the workspace root; focus recovers when the focused terminal panel closes. |
| `a_enter_in_dialogs_goes_to_the_focused_control` | Enter with an empty name keeps New Task open; Enter on a focused option button selects it instead of closing; Enter in the name field creates. |
| `b_escape_returns_focus_to_the_opener` | Palette, New Task, New Terminal, Notifications, New Group, shortcut help, Open Repository and Task details: each opened by keyboard from terminal w1 and closed with Esc returns focus to w1 (typing reaches it); Ctrl+N / Ctrl+K / Ctrl+Shift+B from the sidebar tree return to the tree; Ctrl+, and Ctrl+Shift+S move focus into Settings / inspector. |
| `c_tab_order_cycles_and_reverses_on_main_screens` | Tab cycles through the workspace (title bar, sidebar tree, header buttons, filter, status bar), Settings (25 choices) and the New Task dialog (focus trapped); Shift+Tab retraces each cycle exactly. Agents panel (focused by Ctrl+Shift+E) is symmetric too; Ctrl+Shift+I focuses the approvals inbox. |
| `c_shift_tab_from_the_agents_tree_reverses_tab` | Shift+Tab from the agents tree goes to its own header menu, and six Shift+Tabs back into Settings are retraced exactly by Tab. Fails with the old wiring, where the dock frame and the tree shared one tab-stop handle (registered twice per frame). |
| `c_review_controls_are_reachable_by_keyboard` | Ctrl+Shift+R focuses Review on a real fixture git diff; 37 tab stops, symmetric; Alt+PageDown then Ctrl+Shift+C opens a comment draft with focus in it. |
| `d_ime_terminal_sends_bytes_only_on_commit` | 한 → 한글 in the terminal: no bytes while composing, preedit kept for drawing, IME anchor = cursor cell, bytes sent on each commit; Esc during composition sends no ESC; cancel clears the preedit. |
| `d_ime_palette_sidebar_filter_new_task_name` | Palette: Enter while composing does not activate; committed "리뷰 봇" opens b1; cancel leaves the value. Sidebar filter: Tab-reachable, preedit/commit, filtering applies on commit. New Task name: cancel drops the syllable, no create while composing, commit + Enter creates "로그인수정". |
| `d_ime_review_comment_editor` | Review draft: 좋아요 composed/committed; nothing saved while composing; Korean-IME order (commit, then Enter) saves exactly once with the full text. |
| `e_dpi_scales_keep_chrome_rows_dialogs_inside_the_window` | Scale factors 1.0/1.25/1.5/1.75 with the logical viewport a maximized window gets on 1920×1080 at that scale (down to 1097×590): title-bar actions, sidebar header buttons, sidebar rows, status-bar buttons and five dialogs lie fully inside their containers and the window. |
| `theme::tests::app_switches_tokens_when_the_os_reports_high_contrast` | With the OS query mocked: contrast theme on → high-contrast tokens; unchanged flag → no rebuild; off → normal tokens; the in-app preview setting is untouched. |

## 2. UI Automation tree — `uia-dump.ps1`, `uia/`

The real release `pod-app --ui-fixture` per scene, dumped with .NET UI
Automation (ControlViewWalker): Name, ControlType, keyboard focusability,
AcceleratorKey/AccessKey and description. No input, no focus change. This is
what Narrator reads; it is **not** a recording of Narrator speech.

## 3. Desktop probe — `desktop-probe.ps1`, `desktop-probe.log`

Read-only check at the start and about every 20 minutes. The real SendInput
keyboard flow was to run once if the input desktop became available.
