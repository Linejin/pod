Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'capture.ps1') -HelpersOnly
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class PodHover {
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
 [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h,ref POINT p);
}
'@
$out = Join-Path $root 'crates/pod-app/qa/d3/tooltip'
New-Item -ItemType Directory -Force $out | Out-Null
$config = Join-Path $root ('target/visual/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $config | Out-Null
$old = $env:POD_APP_CONFIG_DIR
$env:POD_APP_CONFIG_DIR = $config
[IO.File]::WriteAllText((Join-Path $config 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic"}')
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene agents' -PassThru
try {
 for ($i=0;$i -lt 100;$i++) { $app.Refresh(); if ($app.MainWindowHandle -ne 0) { break }; Start-Sleep -Milliseconds 100 }
 $h=$app.MainWindowHandle
 if ($h -eq 0) { throw 'No fixture window' }
 $monitor=[Windows.Forms.Screen]::AllScreens | Where-Object {-not $_.Primary} | Select-Object -First 1
 [void][PodCapture]::ShowWindow($h,4)
 [void][PodCapture]::SetWindowPos($h,[IntPtr]::Zero,$monitor.WorkingArea.X+24,$monitor.WorkingArea.Y+24,1440,960,0x14)
 Start-Sleep -Seconds 2
 $r=New-Object PodCapture+RECT
 [void][PodCapture]::GetWindowRect($h,[ref]$r)
 # Coordinates from the reviewed dark-agents baseline, within its command detail.
 # Targeted WM_MOUSEMOVE only: no click, foreground change, or system pointer move.
 $p=New-Object PodHover+POINT; $p.X=$r.L+1020; $p.Y=$r.T+204
 [void][PodHover]::ScreenToClient($h,[ref]$p)
 [void][PodHover]::PostMessage($h,0x200,[IntPtr]::Zero,[IntPtr](($p.Y -shl 16) -bor $p.X))
 Start-Sleep -Seconds 2
 $bitmap=New-Object Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
 $graphics=[Drawing.Graphics]::FromImage($bitmap); $dc=$graphics.GetHdc()
 try { $ok=[PodCapture]::PrintWindow($h,$dc,2) } finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
 if (-not $ok) { $bitmap.Dispose(); throw 'PrintWindow failed' }
 $bitmap.Save((Join-Path $out 'command-hover.png')); $bitmap.Dispose()
 'Synthetic targeted WM_MOUSEMOVE, not native pointer input; visual result requires inspection.' | Set-Content (Join-Path $out 'method.txt')
} finally {
 if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
 $env:POD_APP_CONFIG_DIR=$old
}
