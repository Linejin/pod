Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'capture.ps1') -HelpersOnly
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$out = Join-Path $root 'crates/pod-app/qa/d3/restoration'
New-Item -ItemType Directory -Force $out | Out-Null
$config = Join-Path $root ('target/visual/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $config | Out-Null
$old = $env:POD_APP_CONFIG_DIR
$env:POD_APP_CONFIG_DIR = $config
[IO.File]::WriteAllText((Join-Path $config 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic"}')
function Open-Fixture([string]$scene) {
 & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
 $p = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList "--ui-fixture --display secondary --no-sound --fixture-scene $scene" -PassThru
 for ($i=0;$i -lt 100;$i++) { $p.Refresh(); if ($p.MainWindowHandle -ne 0) { break }; Start-Sleep -Milliseconds 100 }
 if ($p.MainWindowHandle -eq 0) { throw 'No fixture HWND' }
 Start-Sleep -Seconds 2
 return $p
}
function Close-Fixture($p) {
 if ($p -and -not $p.HasExited) { [void]$p.CloseMainWindow(); if (-not $p.WaitForExit(5000)) { Stop-Process -Id $p.Id } }
}
function Snapshot($p, [string]$name) {
 $state = orca computer get-app-state --app "pid:$($p.Id)" --no-screenshot --json | ConvertFrom-Json
 if (-not $state.ok) { throw 'UIA snapshot failed' }
 $state.result.snapshot.treeText | Set-Content -Encoding UTF8 (Join-Path $out "$name.tree.txt")
 powershell -NoProfile -File (Join-Path $root 'crates/pod-app/qa/capture.ps1') -AppPid $p.Id -Name "d3/restoration/$name" | Out-Null
 return $state.result.snapshot.treeText
}
$app = $null
try {
 $app = Open-Fixture 'settings'
 $window = [Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
 $condition = New-Object Windows.Automation.AndCondition (
  (New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ControlTypeProperty),([Windows.Automation.ControlType]::Button)),
  (New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::NameProperty),'상태'))
 $status = $window.FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
 if (-not $status) { throw 'Status button missing' }
 $invoke = $status.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
 $invoke.Invoke()
 Start-Sleep -Seconds 1
 $before = Snapshot $app 'before'
 Close-Fixture $app; $app = $null
 $layout = Get-Content (Join-Path $config 'layout.json') -Raw
 if ($layout -notmatch 'PodSettings' -or $layout -notmatch 'PodInspector') { throw 'Panels were not saved' }
 [IO.File]::WriteAllText((Join-Path $out 'saved-layout.json'),$layout)
 $app = Open-Fixture 'workspace'
 $after = Snapshot $app 'after'
 if ($after -notmatch '탭 항목 설정' -or $after -notmatch '탭 항목 상태') { throw "Restored named tabs not found: $after" }
 'PASS: settings center tab and status right inspector saved and restored; both tabs have UIA names' | Set-Content -Encoding UTF8 (Join-Path $out 'result.txt')
} finally { Close-Fixture $app; $env:POD_APP_CONFIG_DIR = $old }
Get-Content (Join-Path $out 'result.txt')
