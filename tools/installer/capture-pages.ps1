# Screenshots of the Korean installer/uninstaller pages, driven through UI Automation
# (no real mouse/keyboard input). Every window is moved to the secondary monitor.
#
#   powershell -ExecutionPolicy Bypass -File tools/installer/capture-pages.ps1 [-Setup ...] [-OutDir ...]
#
# Same isolation as test-install.ps1 (prefix install dir, private pipe and data dirs).
# Flow: fresh install (welcome, dir, tasks, ready, finished) -> start a terminal ->
# upgrade (podd/terminal dialogs) -> uninstall (terminal dialog, hooks dialog
# answered "아니요", data dialog answered "예" = prefix data only) -> checks
# nothing is left.
param(
  [string]$Setup = "",
  [string]$Prefix = (Join-Path $env:TEMP "pod-install-shots"),
  [string]$OutDir = ""
)
$ErrorActionPreference = "Stop"
function Gate {
  $g = if ($env:POD_CPU_GATE) { $env:POD_CPU_GATE } else { Join-Path $Root "tools\cpu-gate.ps1" }
  & $g -MaxPct 70 -MaxCargo 1 | Out-Host
}
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Setup) { $Setup = (Get-ChildItem (Join-Path $Root "dist") -Filter "Pod-Setup-*-x64.exe" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName }
if (-not $OutDir) { $OutDir = Join-Path $Root "dist\shots" }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Setup = (Resolve-Path $Setup).Path
$StartLnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Pod.lnk"
$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8C7541A9-1FDB-43FC-B2EB-44131622A0CF}_is1"
if (Test-Path $UninstKey) { throw "Pod is already installed for this user; refusing" }
# Recursive cleanup is restricted to a child of the OS temporary directory.
$Prefix = [IO.Path]::GetFullPath($Prefix)
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
if (-not $Prefix.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or $Prefix.TrimEnd('\') -eq $tempRoot.TrimEnd('\')) { throw "Unsafe test prefix: $Prefix" }
$Dir = Join-Path $Prefix "app"
if (Test-Path $Prefix) { Remove-Item -Recurse -Force $Prefix }
New-Item -ItemType Directory -Force "$Prefix\localappdata", "$Prefix\appdata", "$Prefix\worktrees" | Out-Null
$env:POD_SOCKET = "podd-installer-shots"
$env:LOCALAPPDATA = "$Prefix\localappdata"
$env:APPDATA = "$Prefix\appdata"
$env:POD_WORKTREES_ROOT = "$Prefix\worktrees"
$PathBefore = (Get-Item "HKCU:\Environment").GetValue("Path", $null, "DoNotExpandEnvironmentNames")

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Wn {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
  [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m, int t, out uint x, out uint y);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
}
"@
[void][Wn]::SetProcessDpiAwarenessContext([IntPtr](-4))
$A = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$sec = [System.Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
if (-not $sec) { $sec = [System.Windows.Forms.Screen]::PrimaryScreen }

function TopWindows([int[]]$pids) {
  $A::RootElement.FindAll($TS::Children, [System.Windows.Automation.Condition]::TrueCondition) |
    Where-Object { $pids -contains $_.Current.ProcessId }
}
function SetupPids {
  @(Get-Process | Where-Object { $_.ProcessName -like "Pod-Setup*" -or $_.ProcessName -like "_unins*" -or $_.ProcessName -like "_iu*" -or $_.ProcessName -like "unins*" } | ForEach-Object Id)
}
# Windows to match, innermost first: UIA lists a message box under the wizard that owns it.
function Candidates {
  $winCond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Window)
  foreach ($w in TopWindows (SetupPids)) {
    $inner = @($w.FindAll($TS::Descendants, $winCond))
    [array]::Reverse($inner)
    $inner
    $w
  }
}
# Text of the controls currently shown (wizard pages keep hidden pages' controls).
function VisibleText($w) {
  ($w.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
    Where-Object { -not $_.Current.IsOffscreen } | ForEach-Object { $_.Current.Name }) -join "`n"
}
function WaitButton($w, [string]$pattern, [int]$timeout = 180) {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
  $deadline = (Get-Date).AddSeconds($timeout)
  while ((Get-Date) -lt $deadline) {
    $b = $w.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -match 'Button' -and $_.Current.Name -match $pattern -and $_.Current.IsEnabled -and -not $_.Current.IsOffscreen }
    if ($b) { return }
    Start-Sleep -Milliseconds 200
  }
  throw "no button /$pattern/"
}
# Wait for a top-level window of the setup/uninstall processes whose text matches $pattern.
function WaitWindow([string]$pattern, [int]$timeout = 60) {
  $deadline = (Get-Date).AddSeconds($timeout)
  while ((Get-Date) -lt $deadline) {
    try {
      foreach ($w in Candidates) {
        if (($w.Current.Name + "`n" + (VisibleText $w)) -match $pattern) { return $w }
      }
    } catch {}  # a window closed while being read
    Start-Sleep -Milliseconds 150
  }
  throw "no window matching /$pattern/"
}
function Place($w) {
  $h = [IntPtr]$w.Current.NativeWindowHandle
  $r = New-Object Wn+RECT; [void][Wn]::GetWindowRect($h, [ref]$r)
  $b = $sec.WorkingArea
  $x = $b.X + [int](($b.Width - ($r.R - $r.L)) / 2); $y = $b.Y + [int](($b.Height - ($r.B - $r.T)) / 2)
  [void][Wn]::SetWindowPos($h, [IntPtr]::Zero, $x, $y, 0, 0, 0x0001 -bor 0x0004 -bor 0x0010)  # NOSIZE|NOZORDER|NOACTIVATE
}
function Grab([IntPtr]$h, [uint32]$flags) {
  $r = New-Object Wn+RECT; [void][Wn]::GetWindowRect($h, [ref]$r)
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][Wn]::PrintWindow($h, $hdc, $flags); $g.ReleaseHdc($hdc); $g.Dispose()
  return $bmp
}
function Shot($w, [string]$name) {
  Place $w; Start-Sleep -Milliseconds 1500
  $h = [IntPtr]$w.Current.NativeWindowHandle
  # PW_RENDERFULLCONTENT (2) copies DWM's surface, which went stale while the session was
  # locked (check-list items, the ready memo and Latin runs of labels missing). Flag 0 makes
  # the window paint itself synchronously into our DC (complete, deterministic) but draws a
  # classic title bar. So: frame from flag 2, client area from flag 0.
  $mx = [uint32]0; $my = [uint32]0
  [void][Wn]::GetDpiForMonitor([Wn]::MonitorFromWindow($h, 2), 0, [ref]$mx, [ref]$my)
  # A system-DPI-aware window on a higher-DPI monitor is stretched by DWM; PrintWindow
  # renders it unscaled in the top-left part of the (physical) window rect.
  $scale = [double]$mx / [double][Wn]::GetDpiForWindow($h)
  if ($scale -lt 1) { $scale = 1 }
  $frame = Grab $h 2
  $client = Grab $h 0
  $wr = New-Object Wn+RECT; [void][Wn]::GetWindowRect($h, [ref]$wr)
  $cr = New-Object Wn+RECT; [void][Wn]::GetClientRect($h, [ref]$cr)
  $pt = New-Object Wn+POINT; [void][Wn]::ClientToScreen($h, [ref]$pt)
  $cx = [int](($pt.X - $wr.L) / $scale); $cy = [int](($pt.Y - $wr.T) / $scale)
  $src = New-Object System.Drawing.Rectangle $cx, $cy, ($cr.R - $cr.L), ($cr.B - $cr.T)
  $g = [System.Drawing.Graphics]::FromImage($frame)
  $g.DrawImage($client, $src, $src, [System.Drawing.GraphicsUnit]::Pixel); $g.Dispose(); $client.Dispose()
  $out = $frame
  if ($scale -gt 1.01) { $out = $frame.Clone((New-Object System.Drawing.Rectangle 0, 0, ([int]($frame.Width / $scale)), ([int]($frame.Height / $scale))), $frame.PixelFormat) }
  $out.Save((Join-Path $OutDir "$name.png")); $frame.Dispose(); if ($out -ne $frame) { $out.Dispose() }
  Write-Host "shot $name"
}# Inno 6.7 buttons are TNewButton windows without a UIA Invoke pattern: click with BM_CLICK
# (posted to the button's own HWND; the real mouse and keyboard are never used).
function Press($w, [string]$pattern) {
  $b = $w.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
    Where-Object { $_.Current.ClassName -match 'Button' -and $_.Current.Name -match $pattern -and $_.Current.IsEnabled -and -not $_.Current.IsOffscreen -and $_.Current.NativeWindowHandle -ne 0 } |
    Select-Object -First 1
  if (-not $b) { throw "no button /$pattern/ in '$($w.Current.Name)'" }
  [void][Wn]::PostMessage([IntPtr]$b.Current.NativeWindowHandle, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)  # BM_CLICK
  Start-Sleep -Milliseconds 800
}
$Next = '^다음'; $Install = '^설치'; $Finish = '^마침'; $Yes = '^예'; $No = '^아니요'; $Ok = '^확인'

try {
# ------------------------------------------------ fresh install
Gate
$p = Start-Process -FilePath $Setup -ArgumentList "/DIR=`"$Dir`"", "/LOG=`"$OutDir\install.log`"" -PassThru
$w = WaitWindow 'Pod'; Shot $w "01-welcome"; Press $w $Next
$w = WaitWindow '설치할 위치|폴더'; Shot $w "02-dir"; Press $w $Next
$w = WaitWindow '추가 작업'; Shot $w "03-tasks"
Press $w $Next
$w = WaitWindow '준비'; Shot $w "04-ready"; Press $w $Install
$w = WaitWindow 'Pod' 30; WaitButton $w $Finish; Shot $w "05-finished"; Gate; Press $w $Finish
$p.WaitForExit()

# ------------------------------------------------ upgrade with podd + a terminal running
Gate
& "$Dir\pod.exe" run --name shots -- cmd.exe /k "echo upgrade-demo" | Out-Null
Start-Sleep -Seconds 2
Gate
$p = Start-Process -FilePath $Setup -ArgumentList "/LOG=`"$OutDir\upgrade.log`"" -PassThru
$w = WaitWindow 'Pod'
# Upgrade skips the dir page; go to the ready page.
for ($i = 0; $i -lt 4; $i++) {
  if ((VisibleText $w) -match '준비') { break }
  if ($i -eq 0) { Shot $w "06-upgrade-welcome" }
  Press $w $Next
}
Press $w $Install
try { $m = WaitWindow 'Pod 앱이 실행 중' 20; Shot $m "07-upgrade-app"; Press $m $Yes } catch { Write-Host "no app dialog: $_" }
$m = WaitWindow '데몬\(podd\)'; Shot $m "08-upgrade-daemon"; Press $m $Yes
$m = WaitWindow '그대로 유지'; Shot $m "09-upgrade-terminals"; Press $m $Ok
$w = WaitWindow 'Pod' 30; WaitButton $w $Finish; Shot $w "10-upgrade-finished"; Gate; Press $w $Finish
$p.WaitForExit()
$aside = @(Get-ChildItem $Dir, "$Dir\x64" -Filter "*.old-*" | ForEach-Object Name)
Write-Host "moved aside: $($aside -join ', ')"
Gate
Write-Host (& "$Dir\pod.exe" ls | Out-String)

# ------------------------------------------------ uninstall
Gate
$u = Start-Process -FilePath "$Dir\unins000.exe" -ArgumentList "/LOG=`"$OutDir\uninstall.log`"" -PassThru
# The order of the uninstaller's dialogs depends on the phase that shows them; handle any order.
$handlers = [ordered]@{
  '제거하시겠습니까' = @("11-uninstall-confirm", $Yes)
  '터미널이 \d+개' = @("12-uninstall-terminals", $Yes)
  'pod hooks uninstall' = @("13-uninstall-hooks", $No)
  '사용자 데이터' = @("14-uninstall-data", $Yes)
  '제거되었습니다' = @("15-uninstall-done", $Ok)
}
$done = @{}
$deadline = (Get-Date).AddSeconds(120)
while (-not $done.ContainsKey('제거되었습니다') -and (Get-Date) -lt $deadline) {
  try {
    foreach ($w in Candidates) {
      $txt = $w.Current.Name + "`n" + (VisibleText $w)
      foreach ($k in $handlers.Keys) {
        if (-not $done.ContainsKey($k) -and $txt -match $k) {
          Shot $w $handlers[$k][0]; Press $w $handlers[$k][1]; $done[$k] = $true; break
        }
      }
    }
  } catch { Write-Host "(uia, window closed while reading: $($_.Exception.Message))" }
  Start-Sleep -Milliseconds 300
}
Write-Host "uninstall dialogs seen: $($done.Keys -join ', ')"
Start-Sleep -Seconds 2
} finally {
  # Never leave the screenshot install behind, whatever failed above.
  Get-Process | Where-Object { $_.ProcessName -like "Pod-Setup*" -or $_.ProcessName -like "_unins*" -or $_.ProcessName -like "_iu*" -or $_.ProcessName -like "unins*" } | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  if (Test-Path $UninstKey) {
    Write-Host "cleanup: silent uninstall"
    if (Test-Path "$Dir\unins000.exe") { Gate; Start-Process -WindowStyle Hidden "$Dir\unins000.exe" -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/STOPTERMINALS", "/PURGEDATA" -Wait; for ($i = 0; $i -lt 120 -and (Get-Process | Where-Object { $_.ProcessName -like "_unins*" -or $_.ProcessName -like "_iu*" }); $i++) { Start-Sleep -Milliseconds 500 } }
  }
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Prefix, [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  if (Test-Path $UninstKey) { Remove-Item $UninstKey }
  if (Test-Path $StartLnk) { Remove-Item $StartLnk }
  if (((Get-Item 'HKCU:\Environment').GetValue('Path', $null, 'DoNotExpandEnvironmentNames')) -ne $PathBefore) { Set-ItemProperty "HKCU:\Environment" -Name Path -Value $PathBefore -Type ExpandString }
  $script:leftDir = Test-Path $Dir
  Remove-Item -Recurse -Force $Prefix -ErrorAction SilentlyContinue
}
$left = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Dir, [StringComparison]::OrdinalIgnoreCase) })
Write-Host "processes left: $($left.Count); app dir existed after uninstall: $script:leftDir; data exists: $(Test-Path "$Prefix\localappdata\pod"); uninstall key: $(Test-Path $UninstKey); PATH same: $(((Get-Item 'HKCU:\Environment').GetValue('Path', $null, 'DoNotExpandEnvironmentNames')) -eq $PathBefore)"
Remove-Item -Recurse -Force $Prefix -ErrorAction SilentlyContinue
