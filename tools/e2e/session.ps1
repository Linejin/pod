# SPDX-License-Identifier: Apache-2.0
# Dot-source this file for the isolated, real-agent verification session.
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$E2eRoot = 'C:\ai\pod-e2e\rerun-c9cf81'
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Evidence = Join-Path $RepoRoot 'docs\e2e\rerun'
$Rel = Join-Path $RepoRoot 'target\release'
$env:POD_SOCKET = 'pod-e2e-rerun-c9cf81'
$env:POD_WORKTREES_ROOT = Join-Path $E2eRoot 'worktrees'
$env:POD_APP_CONFIG_DIR = Join-Path $E2eRoot 'config'
New-Item -ItemType Directory -Force $Evidence,$E2eRoot | Out-Null
function Pod {
  $ErrorActionPreference = 'Continue'
  $stamp = [DateTimeOffset]::UtcNow.ToString('o')
  $output = & (Join-Path $Rel 'pod.exe') @args 2>&1
  $code = $LASTEXITCODE
  Add-Content -Encoding UTF8 (Join-Path $Evidence 'cli.log') ("`n[$stamp] pod " + ($args -join ' ') + "`n" + ($output -join "`n") + "`nexit=$code")
  $output
  $global:LASTEXITCODE = $code
}
function App { Get-Process pod-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $Rel 'pod-app.exe') } | Select-Object -First 1 }
function Start-E2eApp {
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
  Start-Process -FilePath (Join-Path $Rel 'pod-app.exe') -ArgumentList (@('--display','secondary','--no-sound') + $args) -RedirectStandardOutput (Join-Path $E2eRoot 'app.out') -RedirectStandardError (Join-Path $E2eRoot 'app.err') -PassThru
}
# Same per-monitor-v2 PrintWindow capture as tools/run-and-capture.ps1.
# PostMessage input from the existing review demo avoids moving the real cursor.
if (-not ('E2eWindow' -as [type])) {
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class E2eWindow {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h,ref POINT p);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code,uint type);
}
'@
}
[void][E2eWindow]::SetProcessDpiAwarenessContext([IntPtr](-4))
function Shot([string]$Name) {
 $a=App; if (!$a) { throw 'Isolated pod-app not running' }; $h=$a.MainWindowHandle
 $r=New-Object E2eWindow+RECT; [void][E2eWindow]::GetWindowRect($h,[ref]$r)
 $b=New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
 $g=[System.Drawing.Graphics]::FromImage($b); $dc=$g.GetHdc()
 try { if (![E2eWindow]::PrintWindow($h,$dc,2)) { throw 'PrintWindow failed' } } finally { $g.ReleaseHdc($dc) }
 $path=Join-Path $Evidence ($Name+'.png'); $b.Save($path); $g.Dispose(); $b.Dispose()
 "capture=$path rect=$($r.L),$($r.T),$($r.R),$($r.B) dpi=$([E2eWindow]::GetDpiForWindow($h))"
}
# Coordinates are physical pixels in the latest full-resolution Shot.
function PClick([int]$X,[int]$Y) {
 $h=(App).MainWindowHandle; $r=New-Object E2eWindow+RECT; [void][E2eWindow]::GetWindowRect($h,[ref]$r)
 $p=New-Object E2eWindow+POINT; $p.X=$r.L+$X; $p.Y=$r.T+$Y; [void][E2eWindow]::ScreenToClient($h,[ref]$p)
 $l=[IntPtr](($p.Y -shl 16) -bor ($p.X -band 65535))
 [void][E2eWindow]::PostMessage($h,0x200,[IntPtr]0,$l); Start-Sleep -Milliseconds 100
 [void][E2eWindow]::PostMessage($h,0x201,[IntPtr]1,$l); Start-Sleep -Milliseconds 50
 [void][E2eWindow]::PostMessage($h,0x202,[IntPtr]0,$l); Start-Sleep -Milliseconds 350
}
function PType([string]$Text) { $h=(App).MainWindowHandle; foreach($c in $Text.ToCharArray()) { [void][E2eWindow]::PostMessage($h,0x102,[IntPtr][int]$c,[IntPtr]1); Start-Sleep -Milliseconds 8 }; Start-Sleep -Milliseconds 200 }
function PKey([int]$Key) {
 $h=(App).MainWindowHandle; $scan=[E2eWindow]::MapVirtualKey($Key,0)
 [void][E2eWindow]::PostMessage($h,0x100,[IntPtr]$Key,[IntPtr](1 -bor ($scan -shl 16))); Start-Sleep -Milliseconds 50
 [void][E2eWindow]::PostMessage($h,0x101,[IntPtr]$Key,[IntPtr](1 -bor ($scan -shl 16) -bor (3 -shl 30))); Start-Sleep -Milliseconds 200
}
