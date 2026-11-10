# fu-app follow-ups: QA evidence (2026-11-09)

Branch `SJin-Choi/fu-app` (master through `683f0bc` + fu-icon merged). Design: [docs/09 §12](../../../../docs/09-pod-app.md).

Captures: `tools/visual/capture.ps1` (release `pod-app --ui-fixture --display secondary`, 1440×960 window
on the 150 % secondary monitor, DPI-aware `PrintWindow`, UIA tree next to each image). Fixture data only;
no daemon, no real Claude/Codex, the hook scenes use the scripted `MockHookCli` (the real
`pod hooks install` was never run on this machine). All images were looked at.

| File | Shows |
|---|---|
| `dark/light-editor-terminal-hover.png` | Compiler output in Worker 0; the Ctrl-hovered `src/geometry.rs:4:9` is underlined. `missing/file.rs:9` is not a link (no such file). |
| `dark/light-editor-terminal-links.png` | Palette `터미널에서 링크 열기`: 3 file links (`/`, `\`, MSVC `(3,1)`) and the URL, each labelled 파일/주소. (`₩` is how the Korean font draws `\`.) |
| `dark/light-hooks-prompt.png` | Non-blocking banner `에이전트 훅이 설치되어 있지 않습니다` with `훅 설치` / `나중에`. |
| `dark/light-hooks-preview.png` | After `훅 설치`: the dry-run preview (pod-hook copy, Claude settings.json, Codex hooks.json, Codex config.toml trust, backup note), `취소` / `설치`. |
| `dark/light-editor-lsp-log.png` | Status-bar LSP item `언어 서버 꺼짐` (a button with 다시 시작/끄기/로그 보기) after 끄기, and the 로그 보기 dialog. |
| `visual/` | Full visual regression run (28 scenes) and `compare.json`: **28/28 pass** after intentional baseline updates. |

Intentional baseline updates (`tools/visual/baseline/`):
- `palette` (dark/light): three new commands (Ctrl+P 보내기, 터미널에서 링크 열기, 에이전트 훅 설치) change the result count.
- `shortcuts` (dark/light): two new rows (Ctrl+., Ctrl+클릭) and a two-column layout — the single column was already cutting off the last rows (F5 was not visible in the old baseline).
- `editor` (dark/light): the visual fixture runs with LSP disabled, which now shows the `언어 서버 꺼짐` status item (an off state stays visible so it can be turned back on).

Command results (this worktree's own `target`, sccache, 4 jobs, CPU gate before each run):
- `cargo build --workspace --release`: ok.
- `cargo clippy -p pod-app -p pod-terminal-view -p pod-editor --all-targets -- -D warnings`: 0 warnings (`clippy.txt`).
- `cargo test --workspace -- --test-threads=4` (dev profile, once): 374 passed, 0 failed, 2 ignored (`tests.txt`). The combined
  run then exited 255 at the doctest step without a failure line; `cargo test --workspace --doc` alone passed (0 failed, 1 ignored).
  Native QA needs the dev profile: GPUI compiles `debug_selector` out in release, so bounds-based native-QA tests (also the existing
  `d_ime_*`/`e_dpi_*`) fail under `--release`.
- `cargo deny check`: `advisories ok, bans ok, licenses ok, sources ok` (`deny.txt`; the existing unused-ISC note only).

Not verified here: real rust-analyzer code actions/resolve on screen; Narrator; a real ShellExecute browser launch (tests record the URL instead).
