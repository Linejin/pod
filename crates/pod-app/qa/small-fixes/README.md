# small-fixes: QA evidence (2026-11-09)

Branch `SJin-Choi/small-fixes`. Design and causes: [docs/09 §13](../../../../docs/09-pod-app.md), editor: [docs/13 §7.1–7.2](../../../../docs/13-editor-and-lsp.md).

Captures: `tools/visual/capture.ps1` (release `pod-app --ui-fixture --display secondary`, 1440×960 window on the
150 % secondary monitor, DPI-aware `PrintWindow`, UIA tree next to each image). Fixture data only, no daemon, no
real Claude/Codex. All images were looked at.

| File | Shows |
|---|---|
| `before-dark/light-editor-terminal-hover.png` | Before (copied from `qa/followups/`, master build): Korean syllables drawn at the fallback font's natural 0.92 em in a 1.17 em two-cell slot, so every syllable has a gap after it ("문 서 :", "형 식  경 로"). |
| `dark/light-editor-terminal-hover.png` | After, same scene: wide glyphs fill their two cells, Korean reads at normal density; links, colors and the Ctrl-hover underline unchanged. |
| `dark/light-editor-terminal-wide.png` | New scene `--fixture-scene editor-terminal-wide`: Hangul, kanji, kana, full-width letters and emoji; a selection dragged to the *first half* of 역 covers the whole glyph (`영역`); the block cursor on 다 covers both cells with the glyph redrawn inside it. |
| `wt-compare.ps1` | Opens the same lines in Windows Terminal on the secondary monitor and captures it to `target\wt-reference.png` (not tracked: no third-party UI screenshots in the public repo). Result, by eye: Windows Terminal draws Hangul/kana/kanji filling their two cells; the after-images match that density. Emoji are small inside their cells in both. |
| `rust-analyzer-code-actions.txt` | `cargo test -p pod-editor --test lsp_lifecycle rust_analyzer_code_actions -- --ignored --nocapture` against real rust-analyzer on the fixture repo: `Import std::collections::HashMap` (edit via `codeAction/resolve`) applied; `BTreeSet` completion's `use` from `completionItem/resolve`; `word_count(${1:text})$0` → stops `[11..15, 16..16]`. No rust-analyzer process left afterwards. |
| `visual/` | Full visual regression (28 scenes) and `compare.json`: **28/28 pass, 0 changed pixels**, no baseline changed (the baseline scenes have no wide characters in terminals). Diff images (all empty) not kept. |
| `clippy.txt`, `deny.txt` | Final lint and license runs. |

Command results (this worktree's own `target`, sccache, 4 jobs, CPU gate before each run; final numbers after merging master are in docs/09 §13):
- After merging master `6a61c3d` (final): debug and release `cargo test --workspace` each **424 passed, 0 failed, 3 ignored, exit 0** (`tests-debug.txt`, `tests-release.txt`); clippy and deny as below.
- Before that merge: `cargo test --workspace -- --test-threads=4`: 384 passed, 0 failed, 3 ignored, exit 0 (run twice: background, and foreground through PowerShell `2>&1 | Tee-Object`, 107 s with a warm build).
- Before that merge: `cargo test --workspace --release -- --test-threads=4`: 384 passed, 0 failed, 3 ignored, exit 0 (native QA 19/19 in release).
- `cargo clippy -p pod-app -p pod-terminal-view -p pod-editor --all-targets -- -D warnings`: 0 warnings; `cargo clippy -p gpui-component --lib -- -D warnings` (vendor patch): 0 warnings (its own test targets need assets that are not vendored).
- `cargo deny check`: `advisories ok, bans ok, licenses ok, sources ok` (the existing unused-ISC note only).

The doctest exit 255 seen in fu-app did not reproduce here; see docs/09 §13.3 for why it is judged an external kill.
