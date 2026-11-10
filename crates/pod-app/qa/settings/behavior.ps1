# Settings category column / select / switch behaviour on the real release app (fixture, temp config).
#   powershell -NoProfile -File crates/pod-app/qa/settings/behavior.ps1
# Input is posted to the window (qa/input.ps1): no foreground or pointer change. Writes PNGs and
# behavior.txt next to this script under after/. Coordinates are logical, window-relative (1291x805).
param([string]$Theme = 'dark')
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$root = (Resolve-Path (Join-Path $here '../../../..')).Path
$out = Join-Path $here 'after'
New-Item -ItemType Directory -Force $out | Out-Null
$cfg = Join-Path $root ('target/visual/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $cfg | Out-Null
"{`"theme`":`"$Theme`",`"reduced_motion`":true,`"font`":`"Malgun Gothic`",`"terminal_font`":`"Cascadia Mono`"}" | Set-Content -Encoding ascii (Join-Path $cfg 'settings.json')
$env:POD_APP_CONFIG_DIR = $cfg
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene settings' -PassThru
$log = @()
function Input($a) { powershell -NoProfile -File (Join-Path $root 'crates/pod-app/qa/input.ps1') -AppPid $app.Id @a; Start-Sleep -Milliseconds 700 }
function Snap($n) { powershell -NoProfile -File (Join-Path $here 'snap.ps1') -AppPid $app.Id -Path (Join-Path $out "$Theme-$n.png") }
function Probe($label) { $script:log += "--- $label"; $script:log += (powershell -NoProfile -File (Join-Path $here 'uia-probe.ps1') -AppPid $app.Id) }
function Saved($k) { ((Get-Content (Join-Path $cfg 'settings.json') -Raw | ConvertFrom-Json).$k) }
try {
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  do { Start-Sleep -Milliseconds 300; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
  Start-Sleep 8
  Probe 'fresh'
  # Category column: click row 0, then Down -> row 1.
  Input @{ X = 360; Y = 92 }
  Input @{ Key = 0x28 }
  Snap 'category-down'; Probe 'after Down on category 0'
  # Select: click the terminal font select, Down, Enter -> Consolas, list closed.
  Input @{ X = 683; Y = 219 }
  Snap 'select-open'; Probe 'select open'
  Input @{ Key = 0x28 }
  Input @{ Key = 0x0D }
  Snap 'select-picked'; Probe 'after Down+Enter'
  $log += "saved terminal_font=$(Saved 'terminal_font')"
  # Closed select: Down changes the value directly.
  Input @{ Key = 0x28 }
  $log += "after closed Down: terminal_font=$(Saved 'terminal_font')"
  # Category Down -> row 2, switch with Space toggles once.
  Input @{ X = 360; Y = 122 }
  Input @{ Key = 0x28 }
  Snap 'general'; Probe 'category 2'
  $before = Saved 'os_notifications'
  Input @{ X = 599; Y = 173 }   # click the os-notifications switch
  $afterClick = Saved 'os_notifications'
  Input @{ Key = 0x20 }         # Space toggles back exactly once
  $afterSpace = Saved 'os_notifications'
  Snap 'switch'; Probe 'switch'
  $log += "os_notifications: start=$before click=$afterClick space=$afterSpace; reduced_motion=$(Saved 'reduced_motion')"
} finally {
  if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
}
$log | Set-Content -Encoding UTF8 (Join-Path $out "$Theme-behavior.txt")
$log
