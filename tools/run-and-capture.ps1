param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\pod-spike.exe"),
  [string[]]$AppArgs = @("--terminals","16","--stress","--duration","30"),
  [int[]]$ShotsAt = @(6, 20),
  [string]$OutDir = (Join-Path $PSScriptRoot "..\target\captures")
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
"@
# Per-monitor-v2 DPI awareness so window rects are physical pixels on 150% displays.
[void][W]::SetProcessDpiAwarenessContext([IntPtr](-4))
New-Item -ItemType Directory -Force $OutDir | Out-Null
$out = Join-Path $OutDir "run.out"
$p = Start-Process -FilePath $Exe -ArgumentList $AppArgs -RedirectStandardOutput $out -PassThru
$t0 = Get-Date
foreach ($s in $ShotsAt) {
  while (((Get-Date) - $t0).TotalSeconds -lt $s) { Start-Sleep -Milliseconds 200 }
  $p.Refresh(); $h = $p.MainWindowHandle
  if ($h -eq [IntPtr]::Zero) { "t=$s no window"; continue }
  $fg = ([W]::GetForegroundWindow() -eq $h)
  "t=$s foreground=$fg iconic=$([W]::IsIconic($h))"
  # PrintWindow(PW_RENDERFULLCONTENT) captures only this window, even when covered.
  $r = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$r)
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][W]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
  $file = Join-Path $OutDir "shot-$s.png"; $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
  "saved $file"
}
$null = $p.WaitForExit(120000); if (-not $p.HasExited) { $p.Kill(); "killed" }
Get-Content $out | Select-String "stats|summary"
