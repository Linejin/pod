# ui-05: 800 logical (1200x960 physical at 150%) review layout with the comments drawer.
# -Start launches the fixture app on the secondary monitor (isolated config) and prints its pid;
# -Shot captures that window as <Name>.png (+ UIA tree) here. Drive the app in between with
# `orca computer` (click 의견 / press Escape). Fixture data only.
param([switch]$Start, [int]$AppPid = 0, [string]$Name = '')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
. (Join-Path $root 'tools/visual/capture.ps1') -HelpersOnly
if ($Start) {
 $monitor = [Windows.Forms.Screen]::AllScreens | Where-Object {-not $_.Primary} | Select-Object -First 1
 if (-not $monitor) { throw 'Secondary monitor required' }
 $config = Join-Path $root ('target/visual/'+[Guid]::NewGuid().ToString('N'))
 New-Item -ItemType Directory -Force $config | Out-Null
 $env:POD_APP_CONFIG_DIR = $config
 $settings = @{theme='dark';reduced_motion=$true;font='Malgun Gothic';terminal_font='Cascadia Mono';layout='review'}
 [IO.File]::WriteAllText((Join-Path $config 'settings.json'), ($settings | ConvertTo-Json -Compress))
 & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
 $app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene review' -PassThru
 $deadline = [DateTime]::UtcNow.AddSeconds(30)
 do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
 [void][PodCapture]::ShowWindow($app.MainWindowHandle,4)
 [void][PodCapture]::SetWindowPos($app.MainWindowHandle,[IntPtr]::Zero,$monitor.WorkingArea.X+24,$monitor.WorkingArea.Y+24,1200,960,0x14)
 Start-Sleep -Seconds 2
 $app.Id
 return
}
$h = (Get-Process -Id $AppPid).MainWindowHandle
$r = New-Object PodCapture+RECT
[void][PodCapture]::GetWindowRect($h,[ref]$r)
$bitmap = New-Object Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
$graphics = [Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
try { $ok = [PodCapture]::PrintWindow($h,$dc,2) } finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
if (-not $ok) { throw 'PrintWindow failed' }
$bitmap.Save((Join-Path $PSScriptRoot "$Name.png")); $bitmap.Dispose()
$tree = orca computer get-app-state --app "pid:$AppPid" --no-screenshot --json | ConvertFrom-Json
$tree.result.snapshot.treeText | Set-Content -Encoding UTF8 (Join-Path $PSScriptRoot "$Name.tree.txt")
"$Name dpi=$([PodCapture]::GetDpiForWindow($h)) rect=$($r.L),$($r.T),$($r.R),$($r.B)"
