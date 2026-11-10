# Launch pod-spike, drive it with real mouse / keyboard / IME input, capture the window.
#
# Usage (from the repo root, PowerShell):
#   & crates/pod-spike/tools/drive-and-capture.ps1 -Scenario select
#   Scenarios: select | scroll | vim | ime | all
#
# Notes
# - Moves the real mouse pointer (on the secondary monitor, where pod-spike opens).
# - Per-monitor DPI aware: window rects and SetCursorPos use physical pixels.
# - Screenshots: target/captures/<scenario>-*.png ; clipboard checks are printed.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\..\..\target\release\pod-spike.exe"),
  [string]$Scenario = "all",
  [string]$OutDir = (Join-Path $PSScriptRoot "..\..\..\target\captures")
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class D {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("imm32.dll")] public static extern IntPtr ImmGetDefaultIMEWnd(IntPtr h);

  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort vk; public ushort scan; public uint flags; public uint time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
  public static void Unicode(string s) {
    foreach (char c in s) {
      var i = new INPUT[2];
      i[0].type = 1; i[0].ki.scan = c; i[0].ki.flags = 4;          // KEYEVENTF_UNICODE
      i[1].type = 1; i[1].ki.scan = c; i[1].ki.flags = 4 | 2;      // | KEYUP
      SendInput(2, i, Marshal.SizeOf(typeof(INPUT)));
      System.Threading.Thread.Sleep(5);
    }
  }
}
"@
[void][D]::SetProcessDpiAwarenessContext([IntPtr](-4))
New-Item -ItemType Directory -Force $OutDir | Out-Null
# Stale instances (e.g. from an aborted run) would steal the injected input.
$exeFull = (Resolve-Path $Exe).Path
Get-Process pod-spike -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exeFull } | Stop-Process -Force

function Start-Spike([string[]]$AppArgs) {
  $env:POD_TERM_DEBUG = "1"
  $p = Start-Process -FilePath $Exe -ArgumentList $AppArgs -PassThru -RedirectStandardOutput (Join-Path $OutDir "drive-$Scenario.out")
  for ($i = 0; $i -lt 100 -and $p.MainWindowHandle -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 100; $p.Refresh() }
  Start-Sleep -Milliseconds 1500
  $p.Refresh()
  return $p
}
function Rect($h) { $r = New-Object D+RECT; [void][D]::GetWindowRect($h, [ref]$r); return $r }
# Point at fractions of the window (physical pixels).
function At($h, [double]$fx, [double]$fy) { $r = Rect $h; return @([int]($r.L + ($r.R - $r.L) * $fx), [int]($r.T + ($r.B - $r.T) * $fy)) }
function MoveTo($pt) { [void][D]::SetCursorPos($pt[0], $pt[1]); Start-Sleep -Milliseconds 30 }
function Click($pt, [int]$n = 1) {
  MoveTo $pt
  for ($i = 0; $i -lt $n; $i++) { [D]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); [D]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
  Start-Sleep -Milliseconds 200
}
function Drag($a, $b) {
  MoveTo $a; [D]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50
  for ($s = 1; $s -le 10; $s++) { MoveTo @([int]($a[0] + ($b[0] - $a[0]) * $s / 10), [int]($a[1] + ($b[1] - $a[1]) * $s / 10)) }
  [D]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 250
}
function Wheel($pt, [int]$notches) { MoveTo $pt; [D]::mouse_event(0x0800, 0, 0, 120 * $notches, [UIntPtr]::Zero); Start-Sleep -Milliseconds 250 }
function Key([byte]$vk, [byte[]]$mods = @()) {
  # Navigation keys need KEYEVENTF_EXTENDEDKEY, otherwise they are the numpad
  # keys and Windows drops Shift for them (NumLock on).
  $ext = if (@(0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E) -contains $vk) { 1 } else { 0 }
  foreach ($m in $mods) { [D]::keybd_event($m, 0, 0, [UIntPtr]::Zero) }
  [D]::keybd_event($vk, 0, $ext, [UIntPtr]::Zero); [D]::keybd_event($vk, 0, $ext -bor 2, [UIntPtr]::Zero)
  foreach ($m in $mods) { [D]::keybd_event($m, 0, 2, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 120
}
function TypeText([string]$s) { [D]::Unicode($s); Start-Sleep -Milliseconds 150 }
function Shot($h, [string]$name) {
  $r = Rect $h
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][D]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
  $file = Join-Path $OutDir "$name.png"; $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
  "saved $file"
}
$VK_SHIFT = 0x10; $VK_CONTROL = 0x11; $VK_RETURN = 0x0D; $VK_PRIOR = 0x21; $VK_ESCAPE = 0x1B

function Scenario-Select {
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"powershell.exe -NoLogo -NoProfile`"")
  $h = $p.MainWindowHandle
  Click (At $h 0.5 0.5)
  TypeText "Get-ChildItem C:\Windows\System32\drivers\etc | Format-Table Name, Length -AutoSize"; Key $VK_RETURN
  TypeText "echo 'double-click: pod_terminal_view::selection'"; Key $VK_RETURN
  Start-Sleep -Milliseconds 800
  # Rows (physical px, 150%): row r center ~ 108 + 25.5 r ; col c center ~ 17 + 11.4 c (window 1942x1222).
  Drag (At $h 0.009 0.130) (At $h 0.106 0.255)
  Shot $h "select-drag"
  Key 0x43 @($VK_CONTROL, $VK_SHIFT)   # Ctrl+Shift+C
  "clipboard after drag+Ctrl+Shift+C:"; Get-Clipboard | ForEach-Object { "  |$_|" }
  $p.Kill()
}

function Scenario-SelectWord([double]$fx, [double]$fy) {
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"powershell.exe -NoLogo -NoProfile`"")
  $h = $p.MainWindowHandle
  Click (At $h 0.5 0.5)
  TypeText "echo 'double-click: pod_terminal_view::selection and triple-click line'"; Key $VK_RETURN
  Start-Sleep -Milliseconds 800
  Click (At $h $fx $fy) 2
  Shot $h "select-word"
  Key 0x43 @($VK_CONTROL, $VK_SHIFT)
  "clipboard after double-click: |$(Get-Clipboard)|"
  Click (At $h $fx $fy) 3
  Shot $h "select-line"
  Key 0x43 @($VK_CONTROL, $VK_SHIFT)
  "clipboard after triple-click: |$(Get-Clipboard)|"
  $p.Kill()
}

function Scenario-Scroll {
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"powershell.exe -NoLogo -NoProfile`"")
  $h = $p.MainWindowHandle
  Click (At $h 0.5 0.5)
  TypeText "1..400 | % { 'scrollback line ' + `$_ }"; Key $VK_RETURN
  Start-Sleep -Milliseconds 1000
  Shot $h "scroll-bottom"
  Wheel (At $h 0.5 0.5) 10
  Shot $h "scroll-wheel-up"
  Key $VK_PRIOR @($VK_SHIFT)
  Shot $h "scroll-shift-pgup"
  TypeText "x"   # typing jumps back to the bottom
  Start-Sleep -Milliseconds 300
  Shot $h "scroll-after-typing"
  $p.Kill()
}

function Scenario-Vim {
  $file = Join-Path $env:TEMP "pod-vim-demo.txt"
  (1..60 | ForEach-Object { "line $_ : the quick brown fox jumps over the lazy dog" }) | Set-Content -Encoding utf8 $file
  $vimrc = Join-Path $env:TEMP "pod-vim-mouse.vim"
  "set mouse=a number" | Set-Content -Encoding ascii $vimrc
  # MSYS vim treats backslashes as escapes: pass forward slashes.
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"vim -S $($vimrc.Replace('\','/')) $($file.Replace('\','/'))`"")
  $h = $p.MainWindowHandle
  Start-Sleep -Milliseconds 1500
  Click (At $h 0.20 0.30)
  Shot $h "vim-click"
  Drag (At $h 0.10 0.40) (At $h 0.35 0.55)
  Shot $h "vim-drag-visual"
  Wheel (At $h 0.5 0.5) -3
  Shot $h "vim-wheel"
  Key $VK_ESCAPE
  $p.Kill()
}

function Scenario-Ime {
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"powershell.exe -NoLogo -NoProfile`"")
  $h = $p.MainWindowHandle
  Click (At $h 0.5 0.5)
  [void][D]::SetForegroundWindow($h)
  TypeText "echo "
  # Open the IME in Hangul mode via the window's default IME window.
  $ime = [D]::ImmGetDefaultIMEWnd($h)
  $WM_IME_CONTROL = 0x283; $IMC_SETOPENSTATUS = 6; $IMC_SETCONVERSIONMODE = 2
  [void][D]::SendMessage($ime, $WM_IME_CONTROL, [IntPtr]$IMC_SETOPENSTATUS, [IntPtr]1)
  [void][D]::SendMessage($ime, $WM_IME_CONTROL, [IntPtr]$IMC_SETCONVERSIONMODE, [IntPtr]1)
  "ime wnd=$ime foreground=$([D]::GetForegroundWindow() -eq $h)"
  # 2-beolsik: g k s r m f = 한글 ; stop while 글 is still composing.
  foreach ($vk in 0x47, 0x4B, 0x53, 0x52, 0x4D, 0x46) { Key ([byte]$vk) }
  Start-Sleep -Milliseconds 300
  Shot $h "ime-composing"
  Key 0x20   # space commits 글
  Start-Sleep -Milliseconds 300
  Shot $h "ime-committed"
  [void][D]::SendMessage($ime, $WM_IME_CONTROL, [IntPtr]$IMC_SETCONVERSIONMODE, [IntPtr]0)
  $p.Kill()
}

function Scenario-Diagonals {
  $p = Start-Spike @("--terminals", "1", "--cmd", "`"powershell.exe -NoLogo -NoProfile`"")
  $h = $p.MainWindowHandle
  Click (At $h 0.5 0.5)
  TypeText "cls; 1..3 | % { ([string][char]0x2571 + [char]0x2572) * 12 }; ([string][char]0x2573) * 24; ([string][char]0x2572 + [char]0x2571) * 12"; Key $VK_RETURN
  Start-Sleep -Milliseconds 800
  Shot $h "diagonals"
  $p.Kill()
}

switch ($Scenario) {
  "diag" { Scenario-Diagonals }
  "select" { Scenario-Select; Scenario-SelectWord 0.13 0.109 }
  "drag" { Scenario-Select }
  "scroll" { Scenario-Scroll }
  "vim" { Scenario-Vim }
  "ime" { Scenario-Ime }
  "all" { Scenario-Select; Scenario-SelectWord 0.13 0.109; Scenario-Scroll; Scenario-Vim; Scenario-Ime }
}
