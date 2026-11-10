# Apache-2.0 4(b) check for vendor/gpui-component: every file listed under
# "Modified files" in POD-PATCHES.md must start with a `Pod patch:` notice.
# With -Registry <gpui-component-0.9.0 source dir> it also fails on modified
# files missing from that list.
#   powershell -ExecutionPolicy Bypass -File tools/check-vendor-notices.ps1 [-Registry <dir>]
param([string]$Registry = "")
$ErrorActionPreference = "Stop"
$vendor = Resolve-Path (Join-Path $PSScriptRoot "..\vendor\gpui-component")
$listed = @(Get-Content (Join-Path $vendor "POD-PATCHES.md") | ForEach-Object {
  if ($_ -match '^- `([^`]+)`') { $Matches[1] } })
if ($listed.Count -eq 0) { throw "POD-PATCHES.md lists no modified files" }
$bad = @()
foreach ($f in $listed) {
  $path = Join-Path $vendor $f
  if (-not (Test-Path $path)) { $bad += "listed but missing: $f"; continue }
  $head = (Get-Content $path -TotalCount 3) -join "`n"
  if ($head -notmatch 'Pod patch:') { $bad += "no change notice: $f" }
}
if ($Registry) {
  $root = (Resolve-Path $Registry).Path
  Get-ChildItem $vendor -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($vendor.Path.Length + 1).Replace('\', '/')
    $orig = Join-Path $root $rel
    if ($rel -eq 'POD-PATCHES.md' -or -not (Test-Path $orig)) { return }
    $a = (Get-Content $orig -Encoding UTF8) -join "`n"
    $b = ((Get-Content $_.FullName -Encoding UTF8) | Where-Object { $_ -notmatch '^(//|#) (Pod patch:|.*Change notice per Apache-2\.0)' }) -join "`n"
    if ($a.TrimEnd() -ne $b.TrimEnd() -and $listed -notcontains $rel) { $bad += "modified but not listed: $rel" }
  }
}
if ($bad.Count -gt 0) { $bad | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "vendor notices ok ($($listed.Count) files)"
