# Fixture-only native input QA, adapted from pod-spike's DPI-aware capture harness.
param([string]$Exe="$PSScriptRoot/../../../target/release/examples/gallery.exe", [switch]$Light)
$ErrorActionPreference="Stop"
$OutDir=$PSScriptRoot
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class D {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h,uint flags);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("imm32.dll")] public static extern IntPtr ImmGetDefaultIMEWnd(IntPtr h);


}
"@
[void][D]::SetProcessDpiAwarenessContext([IntPtr](-4))
[void][D]::SetThreadDpiAwarenessContext([IntPtr](-4))
New-Item -ItemType Directory -Force $OutDir | Out-Null
function Rect($h) { [void][D]::SetThreadDpiAwarenessContext([IntPtr](-4)); $r = New-Object D+RECT; [void][D]::GetWindowRect($h, [ref]$r); return $r }
# Point at fractions of the window (physical pixels).
function At($h, [double]$fx, [double]$fy) { $r = Rect $h; return @([int]($r.L + ($r.R - $r.L) * $fx), [int]($r.T + ($r.B - $r.T) * $fy)) }
function MoveTo($pt) { [void][D]::SetCursorPos($pt[0], $pt[1]); Start-Sleep -Milliseconds 30 }
function Click($pt, [int]$n = 1) {
  [void][D]::SetThreadDpiAwarenessContext([IntPtr](-4))
  $point=New-Object D+POINT;$point.X=$pt[0];$point.Y=$pt[1]
  $under=[D]::GetAncestor([D]::WindowFromPoint($point),2)
  if($under -ne $script:galleryHwnd){throw "Gallery is occluded at target; no mouse input sent"}
  MoveTo $pt
  for($i=0;$i -lt $n;$i++) {
    [D]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
    [D]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
  }
  Start-Sleep -Milliseconds 250
}
function Wheel($pt, [int]$notches) {
  $lp=[IntPtr](($pt[1] -shl 16) -bor ($pt[0] -band 65535))
  [void][D]::SendMessage($script:galleryHwnd,0x20A,[IntPtr](120*$notches*65536),$lp)
  Start-Sleep -Milliseconds 300
}
function Key([byte]$vk, [byte[]]$mods = @()) {
  if([D]::GetForegroundWindow() -ne $script:galleryHwnd){throw "Gallery lost focus; no input sent"}
  # Navigation keys need KEYEVENTF_EXTENDEDKEY, otherwise they are the numpad
  # keys and Windows drops Shift for them (NumLock on).
  $ext = if (@(0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E) -contains $vk) { 1 } else { 0 }
  foreach ($m in $mods) { [D]::keybd_event($m, 0, 0, [UIntPtr]::Zero) }
  [D]::keybd_event($vk, 0, $ext, [UIntPtr]::Zero); [D]::keybd_event($vk, 0, $ext -bor 2, [UIntPtr]::Zero)
  foreach ($m in $mods) { [D]::keybd_event($m, 0, 2, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 120
}
function Shot($h, [string]$name) {
  $r = Rect $h
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][D]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
  $file = Join-Path $OutDir "$name.png"; $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
  "saved $file"
}

if(Test-Path 'C:/ai/ide/tools/cpu-gate.ps1'){ & C:/ai/ide/tools/cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 }
$galleryArgs=@("--duration","30");if($Light){$galleryArgs+="--light"}
$theme=if($Light){"light"}else{"dark"}
$imeSuffix=if($Light){"-light"}else{""}
$p=Start-Process -FilePath $Exe -ArgumentList $galleryArgs -PassThru
try {
 for($i=0;$i -lt 100 -and $p.MainWindowHandle -eq [IntPtr]::Zero;$i++){Start-Sleep -Milliseconds 100;$p.Refresh()}
 Start-Sleep -Seconds 2
 $h=$p.MainWindowHandle;$script:galleryHwnd=$h
 [void][D]::SetWindowPos($h,[IntPtr]0,0,0,0,0,0x53)
 [void][D]::SetForegroundWindow($h)
 Start-Sleep -Milliseconds 250
 Click (At $h 0.70 0.334)
 [void][D]::SetForegroundWindow($h)
 $ime=[D]::ImmGetDefaultIMEWnd($h)
 if($ime -eq [IntPtr]::Zero){throw "No native IME window"}
 $oldOpen=[D]::SendMessage($ime,0x283,[IntPtr]5,[IntPtr]0)
 $oldMode=[D]::SendMessage($ime,0x283,[IntPtr]1,[IntPtr]0)
 try {
  [void][D]::SendMessage($ime,0x283,[IntPtr]6,[IntPtr]1)
  [void][D]::SendMessage($ime,0x283,[IntPtr]2,[IntPtr]1)
  foreach($vk in 0x47,0x4B,0x53,0x52,0x4D,0x46){Key ([byte]$vk)}
  Shot $h "ime-composing$imeSuffix-150"
  Key 0x20
  Shot $h "ime-committed$imeSuffix-150"
 } finally {
  [void][D]::SendMessage($ime,0x283,[IntPtr]2,$oldMode)
  [void][D]::SendMessage($ime,0x283,[IntPtr]6,$oldOpen)
 }
 # Input focus may scroll the gallery; restore the observed toolbar position first.
 Wheel (At $h 0.40 0.70) 20
 # Open dialog and test Escape in the same foreground session.
 Click (At $h 0.145 0.19)
 Shot $h "dialog-$theme-150"
 Key 0x09
 Key 0x1B
 Shot $h "focus-return-$theme-150"
 Wheel (At $h 0.40 0.70) -7
 Shot $h "controls-$theme-150"
 "Native input session completed; inspect composing/committed and focus screenshots." | Set-Content "$OutDir/native-input.log"
} finally {if(-not $p.HasExited){Stop-Process -Id $p.Id}}
