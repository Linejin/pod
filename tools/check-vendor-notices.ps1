# Apache-2.0 4(b) check for the three patched GPUI packages: every file listed under
# "Modified files" in POD-PATCHES.md must start with a `Pod patch:` notice.
# With -Registry <gpui-component-0.9.0 source dir> it also fails on modified
# files missing from that list.
#   powershell -ExecutionPolicy Bypass -File tools/check-vendor-notices.ps1 [-Registry <dir>]
param(
  [string]$Registry = "",
  [ValidateSet('all', 'gpui-component', 'gpui-base', 'gpui-pre')][string]$Package = 'all'
)
$ErrorActionPreference = "Stop"
$packages = @('gpui-component', 'gpui-base', 'gpui-pre')
$versions = @{ 'gpui-component' = '0.9.0'; 'gpui-base' = '0.9.0'; 'gpui-pre' = '0.3.11' }
if ($Package -ne 'all') { $packages = @($Package) }
$bad = @()
$total = 0
for ($i = 0; $i -lt $packages.Count; $i++) {
$packageName = $packages[$i]
$vendor = Resolve-Path (Join-Path $PSScriptRoot "..\vendor\$packageName")
$listed = @(Get-Content (Join-Path $vendor "POD-PATCHES.md") | ForEach-Object {
  if ($_ -match '^- `([^`]+)`') { $Matches[1] } })
if ($listed.Count -eq 0) { throw "$packageName/POD-PATCHES.md lists no modified files" }
$total += $listed.Count
if (-not ((Test-Path (Join-Path $vendor 'LICENSE')) -or (Test-Path (Join-Path $vendor 'LICENSE-APACHE')))) {
  $bad += "missing upstream license: $packageName"
}
foreach ($f in $listed) {
  $path = Join-Path $vendor $f
  if (-not (Test-Path $path)) { $bad += "listed but missing: $f"; continue }
  $head = (Get-Content $path -TotalCount 3) -join "`n"
  if ($head -notmatch 'Pod patch:') { $bad += "no change notice: $f" }
}
if ($Registry) {
  # Preserve the existing -Registry gpui-component-0.9.0 interface; sibling
  # published package directories supply originals for the new patches.
  $root = (Resolve-Path $Registry).Path
  if ($packageName -ne 'gpui-component') {
    if ($Package -eq 'all') {
      $root = (Resolve-Path (Join-Path (Split-Path $root) ($packageName + '-' + $versions[$packageName]))).Path
    }
  }
  Get-ChildItem $vendor -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($vendor.Path.Length + 1).Replace('\', '/')
    $orig = Join-Path $root $rel
    if ($rel -eq 'POD-PATCHES.md') { return }
    if (-not (Test-Path $orig)) { $bad += "no upstream original: $packageName/$rel"; return }
    if ((Get-FileHash $orig).Hash -eq (Get-FileHash $_.FullName).Hash) { return }
    $a = (Get-Content $orig -Encoding UTF8) -join "`n"
    $b = ((Get-Content $_.FullName -Encoding UTF8) | Where-Object { $_ -notmatch '^(//|#) (Pod patch:|.*Change notice per Apache-2\.0)' }) -join "`n"
    if ($a.TrimEnd() -ne $b.TrimEnd() -and $listed -notcontains $rel) { $bad += "modified but not listed: $packageName/$rel" }
  }
}
}
if ($bad.Count -gt 0) { $bad | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "vendor notices ok ($total modified files across $($packages.Count) packages)"
