# Pod app identity

Copyright (c) 2026 SJin-Choi. Apache-2.0, like the repository.
All artwork here is original geometry; no third-party product artwork (including Zed or Lucide) is used.
Lucide remains limited to the separately licensed in-app UI glyphs.

Selected: **Terminal bays** (`pod.svg`). A prompt and two vertically stacked agent
cells sit inside a protective pod. Blue #89BAFF, slate #14181E, pale #E8EDF5 and
mint #70D6B2 come from Pod design-language v1. This is a fixed brand mark, not a
live success/connection indicator. It remains recognizable in monochrome.

Alternatives in `concepts/`: agent cluster is friendly but loses the terminal
meaning; pod monogram is compact but reads too abstractly at 16 px. The chosen
asymmetric prompt/cell silhouette retains both terminal and parallel-agent ideas.
See `qa/contact-sheet.png` for all three studies and every native size on both themes.

Reproduce using Python 3 and Pillow (HPND; development artwork tool, not shipped):
`python assets/icon/generate.py`. No network assets or font downloads are used.
`GRID` defines independently tuned pixel geometry for each of 16, 20, 24, 32, 40,
48, 64, 128 and 256 px. Sizes 48 and above are anti-aliased from their own
optical grid; sizes 16 through 40 are drawn directly at native resolution. Especially at 16/20 px the one-pixel border, widened prompt
aperture and separated cells replace literal scaling of the SVG. Each PNG is
packed directly into `pod.ico`; no ICO writer resampling is allowed. The SVG is
the large-size vector master; optical PNGs are intentional size-specific variants.

Windows resource group ID is 1. Each executable has its own VERSIONINFO and icon,
linked only to that binary. The GUI loads group 1 into its HWND and window class,
with large/small sizes selected for that window's DPI; size-keyed cached handles live for the
process lifetime. No manifest or subsystem flags are replaced.


Installer welcome and header BMPs are derived from the same original mark by the
same generator. `verify.py` checks all nine embedded images in all five release
binaries byte-for-byte. `capture.ps1` captures the fixture/taskbar and the real
Explorer Details tab on the secondary monitor with per-monitor DPI awareness.
