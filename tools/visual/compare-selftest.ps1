Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$test = Join-Path $root ('target/visual-compare-test/'+[Guid]::NewGuid().ToString('N'))
$baseline = Join-Path $test 'baseline'; $actual = Join-Path $test 'actual'
New-Item -ItemType Directory -Force $baseline,$actual | Out-Null
function Write-Sample($path, [int]$red) {
 $bitmap = New-Object Drawing.Bitmap 2,2
 for ($x=0;$x -lt 2;$x++) { for ($y=0;$y -lt 2;$y++) { $bitmap.SetPixel($x,$y,[Drawing.Color]::FromArgb(255,20,30,40)) } }
 $bitmap.SetPixel(0,0,[Drawing.Color]::FromArgb(255,$red,30,40))
 $bitmap.Save($path); $bitmap.Dispose()
}
Write-Sample (Join-Path $baseline 'sample.png') 20
Write-Sample (Join-Path $actual 'sample.png') 28
& powershell -NoProfile -File (Join-Path $PSScriptRoot 'capture.ps1') -Mode compare -Baseline $baseline -Output $actual -Tolerance 8
if ($LASTEXITCODE -ne 0) { throw 'Tolerance boundary should pass' }
Write-Sample (Join-Path $actual 'sample.png') 29
& powershell -NoProfile -File (Join-Path $PSScriptRoot 'capture.ps1') -Mode compare -Baseline $baseline -Output $actual -Tolerance 8
if ($LASTEXITCODE -ne 1) { throw 'One pixel above tolerance should fail' }
$result = Get-Content (Join-Path $actual 'compare.json') -Raw | ConvertFrom-Json
if ($result.ChangedPixels -ne 1 -or -not (Test-Path (Join-Path $actual 'sample.diff.png'))) { throw 'Diff count/image incorrect' }
'PASS: tolerance boundary 8, one changed pixel at 9, nonzero failure exit, diff PNG'
