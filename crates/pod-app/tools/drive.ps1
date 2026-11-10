# Helpers to drive pod-app with real input and capture its window (DPI aware).
# Dot-source from the repo root:  . crates/pod-app/tools/drive.ps1
#
# Uses an isolated daemon and config so a developer's own podd is untouched:
#   POD_SOCKET=podd-podapp-demo, POD_APP_CONFIG_DIR=target/podapp-demo/config,
#   POD_WORKTREES_ROOT=target/podapp-demo/worktrees
# Functions: Start-PodApp, Stop-PodApp, Shot, Click, Key, Type-Text, At, Pod
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
# Binaries: $env:CARGO_TARGET_DIR\release when a shared target dir is used.
$Rel = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $Root "target\release" }
$Demo = Join-Path $Root "target\podapp-demo"
$Captures = Join-Path $Root "target\captures\pod-app"
New-Item -ItemType Directory -Force $Captures, $Demo | Out-Null
$env:POD_SOCKET = "podd-podapp-demo"
$env:POD_APP_CONFIG_DIR = Join-Path $Demo "config"
$env:POD_WORKTREES_ROOT = Join-Path $Demo "worktrees"
Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

if (-not ("PA" -as [type])) {
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class PA {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort vk; public ushort scan; public uint flags; public uint time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
  public static void Unicode(string s) {
    foreach (char c in s) {
      var i = new INPUT[2];
      i[0].type = 1; i[0].ki.scan = c; i[0].ki.flags = 4;
      i[1].type = 1; i[1].ki.scan = c; i[1].ki.flags = 4 | 2;
      SendInput(2, i, Marshal.SizeOf(typeof(INPUT)));
      System.Threading.Thread.Sleep(5);
    }
  }
}
"@
}
[void][PA]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Pod { & (Join-Path $Rel "pod.exe") @args }

function App { Get-Process pod-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $Rel "pod-app.exe") } | Select-Object -First 1 }

function Start-PodApp([string[]]$AppArgs = @("--display", "secondary"), [string]$Log = "app.out") {
  if (-not $AppArgs -or $AppArgs.Count -eq 0) { $AppArgs = @("--display", "secondary") }
  $p = Start-Process -FilePath (Join-Path $Rel "pod-app.exe") -ArgumentList $AppArgs -PassThru -RedirectStandardOutput (Join-Path $Captures $Log)
  for ($i = 0; $i -lt 100 -and $p.MainWindowHandle -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 100; $p.Refresh() }
  Start-Sleep -Milliseconds 2500
  return $p
}

function Stop-PodApp {
  $a = App
  if ($a) { $a.CloseMainWindow() | Out-Null; [void]$a.WaitForExit(3000) }
  $a = App
  if ($a) { try { $a.Kill() } catch {} ; Start-Sleep -Milliseconds 500 }
}

function Hwnd { $a = App; if (-not $a) { throw "pod-app not running" }; $a.Refresh(); return $a.MainWindowHandle }
function Rect { $r = New-Object PA+RECT; [void][PA]::GetWindowRect((Hwnd), [ref]$r); return $r }
function Scale { return [PA]::GetDpiForWindow((Hwnd)) / 96.0 }

# Point from logical (GPUI px) offsets inside the window; negative = from right/bottom.
function At([double]$x, [double]$y) {
  $r = Rect; $s = Scale
  $px = if ($x -lt 0) { $r.R + $x * $s } else { $r.L + $x * $s }
  $py = if ($y -lt 0) { $r.B + $y * $s } else { $r.T + $y * $s }
  return @([int]$px, [int]$py)
}
function Click($pt) {
  # Move first and give GPUI a WM_MOUSEMOVE to hit-test against; a press
  # that arrives in the same instant as the move is sometimes dropped.
  [void][PA]::SetCursorPos($pt[0], $pt[1]); Start-Sleep -Milliseconds 60
  [PA]::mouse_event(1, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 120
  [PA]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
  [PA]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 300
}
function Key([byte]$vk, [byte[]]$mods = @()) {
  foreach ($m in $mods) { [PA]::keybd_event($m, 0, 0, [UIntPtr]::Zero) }
  [PA]::keybd_event($vk, 0, 0, [UIntPtr]::Zero); [PA]::keybd_event($vk, 0, 2, [UIntPtr]::Zero)
  foreach ($m in $mods) { [PA]::keybd_event($m, 0, 2, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 150
}
function Type-Text([string]$s) { [PA]::Unicode($s); Start-Sleep -Milliseconds 150 }

function Shot([string]$name) {
  $h = Hwnd; $r = Rect
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][PA]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
  $file = Join-Path $Captures "$name.png"; $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
  return $file
}
