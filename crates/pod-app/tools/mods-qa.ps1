# Real-app checks for docs/16 §8.2 (Windows toasts) and §7.2 (layout presets).
#   . crates/pod-app/tools/mods-qa.ps1   (repo root; debug or release binaries)
#   Qa-Layout | Qa-InvalidPreset | Qa-Toast | Qa-ToastFallback
# Isolated like mods-demo.ps1 (own daemon pipe, app config, mods folder under
# target/podapp-demo). The window opens on the secondary monitor.
#
# Input never goes through the user's keyboard or mouse: keys are posted to
# Pod's window. Ctrl+K needs Ctrl in Pod's key state (GPUI reads GetKeyState),
# so the script briefly attaches to Pod's input thread and sets only that
# state (SetKeyboardState), then restores it.
. (Join-Path $PSScriptRoot "mods-demo.ps1")
if ($env:POD_QA_BIN) { $Rel = $env:POD_QA_BIN } elseif (-not (Test-Path (Join-Path $Rel "pod-app.exe"))) { $Rel = Join-Path $Root "target\debug" }

if (-not ("QaInput2" -as [type])) {
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class QaInput2 {
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] static extern bool GetKeyboardState(byte[] s);
  [DllImport("user32.dll")] static extern bool SetKeyboardState(byte[] s);
  // W: WM_CHAR carries UTF-16 (the A version would go through the ANSI code page).
  [DllImport("user32.dll", EntryPoint = "PostMessageW")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern uint MapVirtualKey(uint code, uint type);
  static IntPtr L(uint vk, bool up) { uint scan = MapVirtualKey(vk, 0); return (IntPtr)(1 | (scan << 16) | (up ? (3u << 30) : 0)); }
  public static void Key(IntPtr h, uint vk) {
    PostMessage(h, 0x100, (IntPtr)vk, L(vk, false)); System.Threading.Thread.Sleep(40);
    PostMessage(h, 0x101, (IntPtr)vk, L(vk, true)); System.Threading.Thread.Sleep(150);
  }
  // Ctrl+<vk> in Pod's input state only.
  public static void CtrlKey(IntPtr h, uint vk) {
    uint me = GetCurrentThreadId(), pod = GetWindowThreadProcessId(h, IntPtr.Zero);
    AttachThreadInput(me, pod, true);
    try {
      var s = new byte[256]; GetKeyboardState(s); var old = (byte[])s.Clone();
      s[0x11] = 0x80; s[0xA2] = 0x80; SetKeyboardState(s);
      PostMessage(h, 0x100, (IntPtr)0x11, L(0x11, false));
      Key(h, vk);
      PostMessage(h, 0x101, (IntPtr)0x11, L(0x11, true)); System.Threading.Thread.Sleep(150);
      SetKeyboardState(old);
    } finally { AttachThreadInput(me, pod, false); }
  }
  public static void Chars(IntPtr h, string text) {
    foreach (char c in text) { PostMessage(h, 0x102, (IntPtr)c, (IntPtr)1); System.Threading.Thread.Sleep(15); }
    System.Threading.Thread.Sleep(200);
  }
}
"@
}

function Palette([string]$Query) {
  $h = Hwnd
  [QaInput2]::CtrlKey($h, 0x4B)          # Ctrl+K
  Start-Sleep -Milliseconds 400
  [QaInput2]::Chars($h, $Query)
  Start-Sleep -Milliseconds 300
  [QaInput2]::Key($h, 0x0D)              # Enter
  Start-Sleep -Milliseconds 1200
}

# Minimize Pod's window (posted, like the title bar button): the user is not looking.
function Minimize-Pod { [void][QaInput2]::PostMessage((Hwnd), 0x0112, [IntPtr]0xF020, [IntPtr]0); Start-Sleep -Milliseconds 800 }

function Center-Shape {
  $f = Join-Path $env:POD_APP_CONFIG_DIR "layout.json"
  if (-not (Test-Path $f)) { return "(no layout.json)" }
  $j = Get-Content -Raw -Encoding utf8 $f | ConvertFrom-Json
  function S($p) {
    if ($p.info.stack) { $ax = if ($p.info.stack.axis -eq 0) { "h" } else { "v" }; return "$ax[" + (($p.children | ForEach-Object { S $_ }) -join ",") + "]" }
    if ($p.info.tabs) { return "tabs(" + (($p.children | ForEach-Object { S $_ }) -join ",") + ")" }
    if ($p.info.panel.terminal) { return "terminal:" + $p.info.panel.terminal }
    return $p.panel_name
  }
  return S $j.dock.center
}

# Four shell terminals in the grid, then "리뷰 집중" from the palette; restart.
function Qa-Layout {
  Reset-ModsDemo "dark"
  1..4 | ForEach-Object { Pod run --name "shell-$_" '--' cmd.exe /k "echo pane $_" | Out-Null }
  $null = Start-PodApp @("--display", "secondary", "--open-all") "qa-layout.out"
  Start-Sleep -Seconds 2
  $before = Shot "qa-layout-1-grid"
  Palette "리뷰 집중"
  $after = Shot "qa-layout-2-review-focus"
  Stop-PodApp                       # WM_CLOSE: the app saves layout.json on quit
  $saved = Center-Shape
  $null = Start-PodApp @("--display", "secondary") "qa-layout-restart.out"
  Start-Sleep -Seconds 3
  $restart = Shot "qa-layout-3-after-restart"
  Stop-PodApp
  Stop-ModsDaemon
  return @($before, $after, $restart, "saved center: $saved")
}

# A mod whose preset file is invalid: the 확장 page shows the Korean error.
function Qa-InvalidPreset {
  Reset-ModsDemo "dark"
  $d = Join-Path $Mods "broken-layout"
  New-Item -ItemType Directory -Force (Join-Path $d "layouts") | Out-Null
  '{"pod":1,"name":"broken-layout","version":"0.1.0","title":"깨진 프리셋","layouts":["layouts/bad.json"]}' | Write-NoBom (Join-Path $d "mod.json")
  '{"id":"bad","title":"한 칸 나누기","center":{"split":"h","children":[{"panel":"review"}]}}' | Write-NoBom (Join-Path $d "layouts\bad.json")
  # Only the layout mods, so both cards fit on the page.
  Remove-Item -Recurse -Force (Join-Path $Mods "aider-status"), (Join-Path $Mods "demo-notify"), (Join-Path $Mods "group-done-notify")
  Stop-ModsDaemon                 # it scanned the folder already; the app starts a fresh one
  Start-Sleep -Milliseconds 500
  $null = Start-PodApp @("--display", "secondary", "--extensions") "qa-invalid.out"
  Start-Sleep -Seconds 3
  $f = Shot "qa-invalid-preset"
  Stop-PodApp
  Stop-ModsDaemon
  return $f
}

# A mod notice while Pod is not in front, with a temporary registered AUMID.
# The toast is read back from Windows' notification history (no screenshot of
# the primary monitor). The key and the toast are removed after.
function Qa-Toast {
  $id = "Pod.QA.Toast"; $key = "HKCU:\Software\Classes\AppUserModelId\$id"
  $had = Test-Path $key
  if (-not $had) { New-Item -Path $key -Force | Out-Null; New-ItemProperty -Path $key -Name DisplayName -Value "Pod (QA)" -Force | Out-Null }
  $env:POD_TOAST_AUMID = $id
  try {
    Reset-ModsDemo "dark"
    Pod run --name aider-demo '--' powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Fixtures "aider.ps1") | Out-Null
    $null = Start-PodApp @("--display", "secondary", "--open-all") "qa-toast.out"
    Minimize-Pod
    Start-Sleep -Seconds 12              # the fake Aider asks its question; demo-notify notifies
    # No screen capture: toasts appear on the primary monitor next to the
    # user's own windows. Windows' notification history holds what was shown.
    [void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
    $hist = @([Windows.UI.Notifications.ToastNotificationManager]::History.GetHistory($id))
    $n = $hist.Count
    $xml = ($hist | ForEach-Object { $_.Content.GetXml() }) -join "`n"
    [IO.File]::WriteAllText((Join-Path $Captures "qa-toast-history.xml"), $xml, (New-Object Text.UTF8Encoding $false))
    $log = Get-Content -Raw -Encoding utf8 (Join-Path $Captures "qa-toast.out")
    Stop-PodApp
    Stop-ModsDaemon
    return @("toasts in notification history for ${id}: $n", $xml, "OS toast used: $($log -match 'Windows 알림으로도')", "fallback logged: $($log -match 'OS 알림을 쓸 수 없어')")
  } finally {
    try { [Windows.UI.Notifications.ToastNotificationManager]::History.Clear($id) } catch {}
    Remove-Item Env:POD_TOAST_AUMID -ErrorAction SilentlyContinue
    if (-not $had) { Remove-Item -Path $key -Recurse -Force -ErrorAction SilentlyContinue }
  }
}

# Same notice with the default id and no registration (a portable build).
function Qa-ToastFallback {
  Remove-Item Env:POD_TOAST_AUMID -ErrorAction SilentlyContinue
  $registered = Test-Path "HKCU:\Software\Classes\AppUserModelId\Pod.App"
  Reset-ModsDemo "dark"
  Pod run --name aider-demo '--' powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Fixtures "aider.ps1") | Out-Null
  $null = Start-PodApp @("--display", "secondary", "--open-all") "qa-toast-fallback.out"
  Minimize-Pod
  Start-Sleep -Seconds 12
  [void][QaInput2]::PostMessage((Hwnd), 0x0112, [IntPtr]0xF120, [IntPtr]0); Start-Sleep -Milliseconds 800   # SC_RESTORE for the shot
  $f = Shot "qa-toast-fallback-inapp"
  $alive = [bool](App)
  Stop-PodApp
  Stop-ModsDaemon
  $log = Get-Content -Raw -Encoding utf8 (Join-Path $Captures "qa-toast-fallback.out")
  return @($f, "Pod.App registered: $registered", "app alive: $alive", "fallback logged: $($log -match 'OS 알림을 쓸 수 없어')")
}
