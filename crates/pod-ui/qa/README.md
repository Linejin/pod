# D1 visual and interaction evidence

Captured with DPI awareness context -4 and PrintWindow(2), from the fixture-only gallery.
The capture files retain physical pixels and OS window borders. They are not resized mockups.

| Evidence | Files |
|---|---|
| Secondary, 150% (GetDpiForWindow=144) | `dark-144-secondary.png`, `light-144-secondary.png` |
| Scrolled controls, tree, semantic buttons, switches, ANSI16 | `dark-144-secondary-controls.png`, `light-144-secondary-controls.png` |
| Every status without color | `dark-144-secondary-grayscale.png`, `light-144-secondary-grayscale.png` |
| Primary, 100% (GetDpiForWindow=96) | `dark-96-primary.png`, `light-96-primary.png` |
| Dialog in both themes | `dark-144-secondary-dialog.png`, `light-144-secondary-dialog.png` |
| Native IME composition/commit (dark) | `ime-composing-150.png`, `ime-committed-150.png` |
| Native Tab/Escape focus return (dark) | `focus-return-dark-150.png` |

Visual inspection: 13px Korean text and long tree labels are readable, status shapes/labels remain
unique in grayscale, selected rows retain the accent edge, disabled rows are subdued, both switch
positions are distinct, and ANSI colors are independent of status colors. Reduced motion is enabled
in the baseline captures (static activity arc). No real user work is shown.

An optional light-theme native IME replay aborted before input because another window covered
the target. It is not claimed as passed; native Korean IME evidence is dark/150%.
Both themes also run the actual GPUI dispatch focus check (`*-focus-check.txt` and `*-focus.png`).

100% was checked on the primary display, not by changing the secondary display's Windows scale.
High contrast, Narrator, extended monitor transitions, 125/200% and animation frame pacing remain D3.

`verification.txt`: final cargo command tails and contrast values. `workspace-checks.txt`: earlier
workspace-wide validation. `live-check.txt`: actual GPUI key dispatch and focus ownership result.
`capture.log`: measured DPI and physical bounds. The native harness restores the prior IME mode
and refuses to send keys if the gallery loses foreground ownership.

Reproduce from PowerShell (each harness honors the coordinator CPU gate when installed):

```powershell
./capture.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe
./capture.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe -Primary
./capture.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe -Dialog
./capture.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe -SelfCheck
./native-input.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe
./native-input.ps1 -Exe C:/ai/pod-target/release/examples/gallery.exe -Light
```
