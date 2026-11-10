# Wide glyphs side by side: Pod's `editor-terminal-cjk` fixture scene and the same
# lines in Windows Terminal, both on the secondary monitor, captured DPI-aware.
#   & tools/visual/cjk-compare.ps1 [-Out target/cjk] [-Theme dark]
# Pod uses Cascadia Mono 16 px (= Windows Terminal's default 12 pt) so the cell
# sizes match. Windows Terminal is started with its own defaults; its settings are
# not touched. Outputs are not tracked (no third-party UI screenshots in the repo):
# pod.png, wt.png, manifest.json under -Out. Measure with cjk-measure.py.
param([string]$Out = 'target/cjk', [string]$Theme = 'dark', [int]$SettleSeconds = 3, [switch]$SkipWt)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location $root
. (Join-Path $PSScriptRoot 'capture.ps1') -HelpersOnly
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class CjkWin {
 public delegate bool EnumProc(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
 public static IntPtr Find(string title) {
  IntPtr found = IntPtr.Zero;
  EnumWindows((h, l) => { var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
   if (IsWindowVisible(h) && sb.ToString().Contains(title)) { found = h; return false; } return true; }, IntPtr.Zero);
  return found;
 }
}
'@
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$monitor = [Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
if (-not $monitor) { throw 'Secondary monitor required' }

function Save-Window([IntPtr]$h, [string]$path) {
 $r = New-Object PodCapture+RECT; [void][PodCapture]::GetWindowRect($h, [ref]$r)
 $bmp = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
 $g = [Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
 try { [void][PodCapture]::PrintWindow($h, $dc, 2) } finally { $g.ReleaseHdc($dc); $g.Dispose() }
 $bmp.Save($path); $bmp.Dispose()
 [pscustomobject]@{ Image = (Split-Path $path -Leaf); Dpi = [PodCapture]::GetDpiForWindow($h); Rect = @($r.L, $r.T, $r.R, $r.B) }
}

$manifest = @()
# Pod (fixture data only, isolated config).
$config = Join-Path $root ('target/visual/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $config | Out-Null
$oldConfig = $env:POD_APP_CONFIG_DIR
$env:POD_APP_CONFIG_DIR = $config
[IO.File]::WriteAllText((Join-Path $config 'settings.json'), (@{ theme = $Theme; reduced_motion = $true; font = 'Malgun Gothic'; terminal_font = 'Cascadia Mono'; terminal_font_size = 16 } | ConvertTo-Json -Compress))
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene editor-terminal-cjk' -PassThru
try {
 $deadline = [DateTime]::UtcNow.AddSeconds(30)
 do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
 $h = $app.MainWindowHandle
 if ($h -eq 0) { throw 'Pod did not open a window' }
 [void][PodCapture]::ShowWindow($h, 4)
 [void][PodCapture]::SetWindowPos($h, [IntPtr]::Zero, $monitor.WorkingArea.X + 24, $monitor.WorkingArea.Y + 24, 1440, 960, 0x14)
 Start-Sleep -Seconds $SettleSeconds
 $manifest += Save-Window $h (Join-Path $Out 'pod.png')
} finally {
 $env:POD_APP_CONFIG_DIR = $oldConfig
 if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
}

if (-not $SkipWt) {
 # Same lines as CJK_OUTPUT in crates/pod-app/src/fixture_scenes.rs.
 $lines = @(
  '0123456789012345678901234567890123456789',
  'ひらがな あいうえお かきくけこ さしすせそ',
  'カタカナ アイウエオ カキクケコ サシスセソ',
  '半角 ｶﾀｶﾅ ｱｲｳｴｵ ｶｷｸｹｺ ﾊﾟﾋﾟ',
  '漢字 日本語 東京 中文 简体字 繁體字',
  '한글 가나다라마바사 문서 확인',
  ('emoji ' + [char]::ConvertFromUtf32(0x1F600) + [char]::ConvertFromUtf32(0x1F44D) + [char]::ConvertFromUtf32(0x1F389) + ' end'),
  '┌──┬──┬──┐ |あ|ア|가|漢|',
  '│あ│가│漢│ |い|イ|나|字|',
  '└──┴──┴──┘ 0123456789')
 $sample = Join-Path $Out 'cjk-sample.txt'
 [IO.File]::WriteAllText($sample, (($lines -join "`r`n") + "`r`n"), [Text.Encoding]::Unicode)  # UTF-16 LE + BOM for cmd `type`
 $title = 'PodCjkCompare'
 $x = $monitor.Bounds.X + 40
 Start-Process wt.exe -ArgumentList @('-w', 'new', '--pos', "$x,300", '--size', '60,14', '--title', $title, '--suppressApplicationTitle', 'cmd', '/k', "cls & type `"$sample`"")
 $w = [IntPtr]::Zero
 for ($i = 0; $i -lt 50 -and $w -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 200; $w = [CjkWin]::Find($title) }
 if ($w -eq [IntPtr]::Zero) { throw 'Windows Terminal window not found' }
 try { Start-Sleep -Seconds $SettleSeconds; $manifest += Save-Window $w (Join-Path $Out 'wt.png') }
 finally { [void][CjkWin]::PostMessage($w, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }  # WM_CLOSE
}
$manifest | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $Out 'manifest.json')
$manifest | Format-Table
