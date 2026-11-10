# Save a DPI-aware PrintWindow capture of a running pod-app window (uses tools/visual helpers).
#   powershell -NoProfile -File crates/pod-app/qa/settings/snap.ps1 -AppPid <pid> -Path out.png
param([Parameter(Mandatory)][int]$AppPid, [Parameter(Mandatory)][string]$Path)
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
. (Join-Path $root 'tools/visual/capture.ps1') -HelpersOnly
$h = (Get-Process -Id $AppPid).MainWindowHandle
$r = New-Object PodCapture+RECT
[void][PodCapture]::GetWindowRect($h, [ref]$r)
$bitmap = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [Drawing.Graphics]::FromImage($bitmap)
$dc = $g.GetHdc()
try { [void][PodCapture]::PrintWindow($h, $dc, 2) } finally { $g.ReleaseHdc($dc); $g.Dispose() }
$bitmap.Save($Path); $bitmap.Dispose()
