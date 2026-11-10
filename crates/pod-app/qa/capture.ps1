param([Parameter(Mandatory=$true)][int]$AppPid, [Parameter(Mandatory=$true)][string]$Name)
$ErrorActionPreference = 'Stop'
# Capture only this fixture app window, at its native physical pixel dimensions.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class D2Capture {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
}
"@
[void][D2Capture]::SetProcessDpiAwarenessContext([IntPtr](-4))
$p=Get-Process -Id $AppPid -ErrorAction Stop
$h=$p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw 'Fixture app has no window' }
$r=New-Object D2Capture+RECT
[void][D2Capture]::GetWindowRect($h,[ref]$r)
$dpi=[D2Capture]::GetDpiForWindow($h)
$bmp=New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
$g=[System.Drawing.Graphics]::FromImage($bmp)
$dc=$g.GetHdc();$ok=[D2Capture]::PrintWindow($h,$dc,2);$g.ReleaseHdc($dc);$g.Dispose()
if (-not $ok) {$bmp.Dispose();throw 'PrintWindow failed'}
$path=Join-Path $PSScriptRoot "$Name.png"
$bmp.Save($path);$bmp.Dispose()
$record = "$Name dpi=$dpi rect=$($r.L),$($r.T),$($r.R),$($r.B) PrintWindow=$ok"
$record | Add-Content -Encoding UTF8 -LiteralPath (Join-Path $PSScriptRoot capture.log)
$record
$path
