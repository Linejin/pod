# Real fixture UIA regression: activating/restoring a window must retain dock
# content and must not open the 상태 inspector. No real daemon/config is used.
param([string]$Theme = 'dark', [string]$Profile = 'release')
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '2'
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
. (Join-Path $root 'tools/visual/uia-required.ps1')
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class DockActivation {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int w, int height, uint flags);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
}
'@
[void][DockActivation]::SetProcessDpiAwarenessContext([IntPtr](-4))
$monitor = [Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
if (-not $monitor) { throw 'Secondary monitor required' }
$cfg = Join-Path $root ('target/ui-wave0/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $cfg | Out-Null
[IO.File]::WriteAllText((Join-Path $cfg 'settings.json'), (@{theme=$Theme;reduced_motion=$true;font='Malgun Gothic'} | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
$oldConfig = $env:POD_APP_CONFIG_DIR
$env:POD_APP_CONFIG_DIR = $cfg
$log = [Collections.Generic.List[string]]::new()
function Check-Tree([string]$phase) {
 $el = [Windows.Automation.AutomationElement]::FromHandle($script:h)
 $nodes = $el.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
 $rows = @($nodes | ForEach-Object { "$($_.Current.ControlType.ProgrammaticName) $($_.Current.Name)" })
 $text = $rows -join "`n"
 $text | Set-Content -Encoding utf8 (Join-Path $PSScriptRoot "$Theme-$phase.uia.txt")
 $problems = Test-PodDockActivationTree $text
 if ($problems.Count) { throw "$phase : $($problems -join '; ')" }
 $log.Add("PASS $phase : $($rows.Count) UIA nodes, settings content present, no 상태 tab")
}
try {
 & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
 while ((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory -lt 8GB/1KB) { Start-Sleep -Seconds 10 }
 $app = Start-Process (Join-Path $root "target/$Profile/pod-app.exe") -WindowStyle Hidden -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene settings' -PassThru
 try {
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
  $script:h = $app.MainWindowHandle
  if ($h -eq 0) { throw 'Fixture did not open' }
  [void][DockActivation]::ShowWindow($h, 4)
  [void][DockActivation]::SetWindowPos($h, [IntPtr]::Zero, $monitor.WorkingArea.X+24, $monitor.WorkingArea.Y+24, 1440, 960, 0x14)
  Start-Sleep -Seconds 2
  # Initial query activates AccessKit; allow its initial tree to arrive once.
  [void][Windows.Automation.AutomationElement]::FromHandle($h).FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
  Start-Sleep -Seconds 1
  Check-Tree 'before'
  # A fixture-owned focus target changes activation without a resize/full redraw
  # (minimize/restore alone can mask cached-subtree accessibility bugs).
  $other = New-Object Windows.Forms.Form
  $other.Text = 'Pod activation fixture'
  $other.StartPosition = 'Manual'
  $other.Location = New-Object Drawing.Point ($monitor.WorkingArea.X+48), ($monitor.WorkingArea.Y+48)
  $other.Size = New-Object Drawing.Size 240, 100
  try {
  foreach ($i in 1..3) {
   $other.Show()
   $other.Activate()
   [Windows.Forms.Application]::DoEvents()
   Start-Sleep -Milliseconds 200
   if ([DockActivation]::GetForegroundWindow() -eq $h) { throw 'Fixture focus target did not deactivate Pod' }
   [void][DockActivation]::SetForegroundWindow($h)
   Start-Sleep -Milliseconds 500
   if ([DockActivation]::GetForegroundWindow() -ne $h) { throw 'Foreground activation refused: native check NOT RUN' }
   Check-Tree "active-$i"
   $other.Hide()
  }
  } finally { $other.Dispose() }
  $r = New-Object DockActivation+RECT
  [void][DockActivation]::GetWindowRect($h, [ref]$r)
  if ($r.L -lt $monitor.Bounds.Left -or $r.R -gt $monitor.Bounds.Right) { throw 'Window escaped secondary monitor' }
  $bitmap = New-Object Drawing.Bitmap ($r.R-$r.L), ($r.B-$r.T)
  $g = [Drawing.Graphics]::FromImage($bitmap)
  $dc = $g.GetHdc()
  try { $ok = [DockActivation]::PrintWindow($h, $dc, 2) } finally { $g.ReleaseHdc($dc); $g.Dispose() }
  if (-not $ok) { throw 'PrintWindow failed' }
  $bitmap.Save((Join-Path $PSScriptRoot "$Theme-active.png")); $bitmap.Dispose()
  $log.Add("DPI=$([DockActivation]::GetDpiForWindow($h)); bounds=$($r.L),$($r.T),$($r.R),$($r.B); fixture only")
 } finally {
  if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
 }
} finally {
 $env:POD_APP_CONFIG_DIR = $oldConfig
 $log | Set-Content -Encoding utf8 (Join-Path $PSScriptRoot "$Theme-activation.txt")
 $log
}
