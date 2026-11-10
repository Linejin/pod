# kana: wide glyph placement QA (2026-11-09)

Branch `SJin-Choi/kana`. Design and findings: [docs/09 §13.1a](../../../../docs/09-pod-app.md).
Release `pod-app --ui-fixture --display secondary`, 150 % secondary monitor, DPI-aware `PrintWindow`.
Fixture data only. All images were looked at.

| File | Shows |
|---|---|
| `before-/after-dark-editor-terminal-cjk-16px.png` | `tools/visual/cjk-compare.ps1` Pod captures (terminal font 16 px, cell 14.06 px) before/after the fix. Only the emoji change size; every other glyph is pixel-for-pixel the same. |
| `before-measure.txt`, `after-measure.txt` | `cjk-measure.py` on those captures: ink box per glyph relative to its slot. Diff = the three emoji rows only (😀 0.75→0.89, 👍 0.57→0.68, 🎉 0.71→0.89 of the slot). |
| `wt-measure.txt` | Same measurement on the Windows Terminal capture (default Cascadia Mono 12 pt, same 14.0 px cell; the screenshot itself is not tracked). Wide glyphs at natural size, left in the slot: kana 0.39–0.68, kanji/Hangul 0.64–0.79, emoji 0.82–0.93. |
| `dark/light-editor-terminal-cjk.png` | New scene at the default 14 px font: hiragana, katakana, half-width katakana, kanji, simplified/traditional Chinese, Hangul, emoji, box drawing next to wide glyphs. |
| `dark/light-editor-terminal-wide.png` | Existing scene: selection ending on the first half of 역 still covers the whole glyph, block cursor on 다 covers both cells with the glyph redrawn; emoji now at natural size. Compare with `../small-fixes/`. |
| `clippy.txt`, `deny.txt`, `tests-debug.txt` | Final runs. |

Visual regression (`capture.ps1` + compare, 28 scenes): terminal and editor scenes 0 changed pixels.
`palette`, `review`, `settings` (both themes) differ from the baseline because of UI added on master
after those baselines (PR tab, extensions entry); those scenes contain no terminal, so their
baselines were not changed here.
