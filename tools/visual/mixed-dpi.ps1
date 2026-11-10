Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'capture.ps1') -HelpersOnly
$out = Join-Path $root 'crates/pod-app/qa/d3/mixed-dpi'
New-Item -ItemType Directory -Force $out | Out-Null
$config = Join-Path $root ('target/visual/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $config | Out-Null
$old = $env:POD_APP_CONFIG_DIR
$env:POD_APP_CONFIG_DIR = $config
[IO.File]::WriteAllText((Join-Path $config 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic"}')
$screens = [Windows.Forms.Screen]::AllScreens
$primary = $screens | Where-Object Primary | Select-Object -First 1
$secondary = $screens | Where-Object {-not $_.Primary} | Select-Object -First 1
if (-not $primary -or -not $secondary) { throw 'Mixed-DPI test needs two monitors' }
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--ui-fixture --display secondary --no-sound --fixture-scene settings' -PassThru
$results = @()
try {
 for ($i=0; $i -lt 100; $i++) { $app.Refresh(); if ($app.MainWindowHandle -ne 0) { break }; Start-Sleep -Milliseconds 100 }
 $h = $app.MainWindowHandle
 if ($h -eq 0) { throw 'No fixture window' }
 [void][PodCapture]::ShowWindow($h,4)
 $step = 0
 foreach ($screen in @($secondary,$primary,$secondary)) {
  # Move without activation, then measure actual target DPI before sizing.
  [void][PodCapture]::SetWindowPos($h,[IntPtr]::Zero,$screen.WorkingArea.X+24,$screen.WorkingArea.Y+24,1000,700,0x14)
  Start-Sleep -Milliseconds 500
  $dpi = [PodCapture]::GetDpiForWindow($h)
  $width = [int](1024*$dpi/96); $height = [int](700*$dpi/96)
  [void][PodCapture]::SetWindowPos($h,[IntPtr]::Zero,$screen.WorkingArea.X+24,$screen.WorkingArea.Y+24,$width,$height,0x14)
  Start-Sleep -Seconds 2
  $r = New-Object PodCapture+RECT
  [void][PodCapture]::GetWindowRect($h,[ref]$r)
  $bitmap = New-Object Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
  $graphics = [Drawing.Graphics]::FromImage($bitmap); $dc = $graphics.GetHdc()
  try { $ok = [PodCapture]::PrintWindow($h,$dc,2) } finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
  if (-not $ok) { $bitmap.Dispose(); throw 'PrintWindow failed' }
  $name = "$step-$dpi-dpi.png"; $bitmap.Save((Join-Path $out $name)); $bitmap.Dispose()
  $results += [pscustomobject]@{Image=$name;DPI=[PodCapture]::GetDpiForWindow($h);Primary=$screen.Primary;Rect=@($r.L,$r.T,$r.R,$r.B);LogicalSize=@(1024,700)}
  $step++
 }
} finally {
 if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
 $env:POD_APP_CONFIG_DIR = $old
}
$results | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $out 'manifest.json')
$results | Format-Table
