# P3 editor integration QA (docs/13 §8). Fixture only: no daemon, no real
# agent; rust-analyzer only against the generated small fixture repository.
# Run from the repository root after `cargo build --workspace --release`
# (needs target/release/pod-editor-fake-lsp.exe for the cap check).
#
#   powershell -NoProfile -File crates/pod-app/qa/editor/run-editor-qa.ps1 [-Skip lsp,cap,restore,shots]
param([string[]]$Skip = @())
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
Set-Location $root
$out = Join-Path $root 'crates/pod-app/qa/editor'
$log = Join-Path $out 'qa-result.txt'
function Note($s) { $s | Tee-Object -FilePath $log -Append | Out-Host }
"P3 editor QA $(Get-Date -Format s)" | Set-Content -Encoding UTF8 $log

# 1. Screenshots of deterministic scenes (both themes) and LSP scenes (dark).
if ($Skip -notcontains 'shots') {
  & tools/visual/capture.ps1 -Output $out -Scenes @('editor-tree','editor-quickopen','editor-new-file','editor-close','editor-agent-edit','review-editor','review-editor-open') | Out-Null
  Note 'shots: editor-tree, editor-quickopen, editor-new-file, editor-close, editor-agent-edit, review-editor, review-editor-open (dark+light)'
}
if ($Skip -notcontains 'lsp') {
  foreach ($scene in @('editor-lsp','editor-hover','editor-completion')) {
    & tools/visual/capture.ps1 -Output (Join-Path $out "lsp-$scene") -Scenes @($scene) -Themes @('dark') -SettleSeconds 40 | Out-Null
    Move-Item -Force (Join-Path $out "lsp-$scene/dark-$scene.png") (Join-Path $out "dark-$scene.png")
    Move-Item -Force (Join-Path $out "lsp-$scene/dark-$scene.tree.txt") (Join-Path $out "dark-$scene.tree.txt")
    Remove-Item -Recurse -Force (Join-Path $out "lsp-$scene")
    Note "lsp scene $scene captured (rust-analyzer on the fixture repo)"
  }
  Start-Sleep -Seconds 3
  $ra = @(Get-CimInstance Win32_Process -Filter "Name like 'rust-analyzer%'")
  Note "after LSP scenes (apps closed): rust-analyzer processes on the machine: $($ra.Count)"
}

. tools/visual/capture.ps1 -HelpersOnly
$exe = Join-Path $root 'target/release/pod-app.exe'
$monitor = [Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
function Start-Fixture($config, $scene, $worktree) {
  $env:POD_APP_CONFIG_DIR = $config
  if ($worktree) { $env:POD_APP_FIXTURE_WORKTREE = $worktree } else { Remove-Item Env:POD_APP_FIXTURE_WORKTREE -ErrorAction SilentlyContinue }
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 | Out-Null
  $argLine = "--ui-fixture --display secondary --no-sound"
  if ($scene) { $argLine += " --fixture-scene $scene" }
  $app = Start-Process $exe -ArgumentList $argLine -PassThru
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
  [void][PodCapture]::ShowWindow($app.MainWindowHandle, 4)
  [void][PodCapture]::SetWindowPos($app.MainWindowHandle, [IntPtr]::Zero, $monitor.WorkingArea.X + 24, $monitor.WorkingArea.Y + 24, 1440, 960, 0x14)
  $app
}
function Shot($app, $name) {
  $r = New-Object PodCapture+RECT
  [void][PodCapture]::GetWindowRect($app.MainWindowHandle, [ref]$r)
  $bmp = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
  try { [void][PodCapture]::PrintWindow($app.MainWindowHandle, $dc, 2) } finally { $g.ReleaseHdc($dc); $g.Dispose() }
  $bmp.Save((Join-Path $out "$name.png")); $bmp.Dispose()
  $tree = orca computer get-app-state --app "pid:$($app.Id)" --no-screenshot --json | ConvertFrom-Json
  if ($tree.ok) { $tree.result.snapshot.treeText | Set-Content -Encoding UTF8 (Join-Path $out "$name.tree.txt") }
}
function Stop-Fixture($app) {
  if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(8000)) { Stop-Process -Id $app.Id -Force } }
}
function Fake-Count { @(Get-Process pod-editor-fake-lsp -ErrorAction SilentlyContinue).Count }
$settings = { param($dir) New-Item -ItemType Directory -Force $dir | Out-Null; [IO.File]::WriteAllText((Join-Path $dir 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic","terminal_font":"Cascadia Mono"}') }
$oldConfig = $env:POD_APP_CONFIG_DIR

try {
  # 2. LSP cap (1 in this scene) with the fake server: never more than one
  #    process, LRU suspended, all gone after the app closes.
  if ($Skip -notcontains 'cap') {
    $cfg = Join-Path $root ('target/visual/' + [Guid]::NewGuid().ToString('N')); & $settings $cfg
    $before = Fake-Count
    $app = Start-Fixture $cfg 'editor-lsp-cap' $null
    $max = 0
    for ($i = 0; $i -lt 16; $i++) { Start-Sleep -Milliseconds 250; $max = [Math]::Max($max, (Fake-Count) - $before) }
    Start-Sleep -Seconds 1
    $now = (Fake-Count) - $before
    Shot $app 'dark-editor-lsp-cap'
    Stop-Fixture $app
    Start-Sleep -Seconds 3
    $after = (Fake-Count) - $before
    Note "cap: max concurrent fake servers=$max (cap 1; a suspended one may overlap for <=2 s while shutting down), running at capture=$now, after close=$after"
  }

  # 2b. Closing the window with unsaved editor files asks first (the
  #     window must stay open with the dialog).
  if ($Skip -notcontains 'quit') {
    $cfg = Join-Path $root ('target/visual/' + [Guid]::NewGuid().ToString('N')); & $settings $cfg
    $app = Start-Fixture $cfg 'editor-dirty' $null
    Start-Sleep -Seconds 3
    [void]$app.CloseMainWindow()
    Start-Sleep -Seconds 2
    $app.Refresh()
    Note "quit with unsaved files: window still open after WM_CLOSE = $(-not $app.HasExited)"
    if (-not $app.HasExited) { Shot $app 'dark-quit-unsaved' }
    Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
  }

  # 3. Restore: open tabs + cursor in run 1; run 2 (same config, no scene)
  #    restores the editor panel from layout.json.
  if ($Skip -notcontains 'restore') {
    $cfg = Join-Path $root ('target/visual/' + [Guid]::NewGuid().ToString('N')); & $settings $cfg
    $app = Start-Fixture $cfg 'editor' $null
    Start-Sleep -Seconds 4
    Shot $app 'dark-restore-before'
    Stop-Fixture $app
    $layout = Get-Content -Raw (Join-Path $cfg 'layout.json')
    $wt = Join-Path $cfg 'editor-fixture'
    Start-Sleep -Seconds 1
    $app = Start-Fixture $cfg $null $wt
    Start-Sleep -Seconds 5
    Shot $app 'dark-restore-after'
    Stop-Fixture $app
    $saved = ($layout | ConvertFrom-Json)
    $ed = $null
    function Walk($n) { if ($n.panel_name -eq 'PodEditor') { $script:ed = $n }; foreach ($c in $n.children) { Walk $c } }
    Walk $saved.dock.center
    Note "restore: saved PodEditor info = $($ed.info | ConvertTo-Json -Compress -Depth 6)"
  }
} finally {
  $env:POD_APP_CONFIG_DIR = $oldConfig
  Remove-Item Env:POD_APP_FIXTURE_WORKTREE -ErrorAction SilentlyContinue
  Get-Process pod-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$root*" } | Stop-Process -Force
}
Note "left running: pod-app=$(@(Get-Process pod-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$root*" }).Count) fake-lsp=$(Fake-Count)"
