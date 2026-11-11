# -Captures <dir>: also check every tree listed by the capture.ps1 manifests under <dir>.
param([string]$Captures = '')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'uia-required.ps1')
$baseline = Join-Path $PSScriptRoot 'baseline'
# 1. Every committed baseline tree passes its scene's check.
$trees = @(Get-ChildItem $baseline -Filter '*.tree.txt')
if (-not $trees.Count) { throw 'No baseline trees' }
foreach ($file in $trees) {
 $scene = $file.Name -replace '^(dark|light)-', '' -replace '\.tree\.txt$', ''
 $problems = Test-PodUiaTree $scene ([IO.File]::ReadAllText($file.FullName))
 if ($problems.Count) { throw "Baseline $($file.Name) fails: $($problems -join '; ')" }
}
# 2. A title-bar-only dump fails for every known scene (and for unknown ones).
$titleOnly = Get-Content (Join-Path $baseline 'dark-workspace.tree.txt') -TotalCount 10 | Out-String
foreach ($scene in @($script:PodUiaRequired.Keys) + 'editor-qa-only') {
 if (-not (Test-PodUiaTree $scene $titleOnly).Count) { throw "Title-bar-only tree accepted for '$scene'" }
}
# 3. Retry: two empty dumps (one throwing), then the full tree, is accepted on attempt 3.
$full = [IO.File]::ReadAllText((Join-Path $baseline 'dark-settings.tree.txt'))
$script:calls = 0
$got = Get-PodUiaTree 'settings' { $script:calls++; if ($script:calls -eq 1) { $titleOnly } elseif ($script:calls -eq 2) { throw 'snapshot failed' } else { $full } } -Attempts 4 -DelayMs 1
if ($script:calls -ne 3 -or $got -ne $full) { throw "Retry returned after $($script:calls) calls" }
# 4. Still incomplete after N attempts: throws, names the missing node, calls exactly N times.
$script:calls = 0
$failed = $false
try { [void](Get-PodUiaTree 'settings' { $script:calls++; $titleOnly } -Attempts 3 -DelayMs 1) }
catch { $failed = $_.Exception.Message -like "*incomplete after 3 attempts*설정 분류*" }
if (-not $failed -or $script:calls -ne 3) { throw "Incomplete tree did not fail clearly after 3 attempts (calls=$($script:calls))" }
# 5. Every layout mode of the app has its own table, and no table names a mode that is gone.
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../../crates/pod-app/src/layout_mode.rs'))
$fnId = [regex]::Match($source, '(?s)pub fn id\(self\).*?\n    \}').Value
$ids = @([regex]::Matches($fnId, 'Self::\w+ => "([a-z-]+)"') | ForEach-Object { $_.Groups[1].Value })
if ($ids.Count -lt 7 -or $ids -notcontains 'workbench') { throw "Layout ids not found in layout_mode.rs: $($ids -join ', ')" }
foreach ($id in $ids | Where-Object { $_ -ne 'workbench' }) {
 if (-not $script:PodUiaLayoutRequired.ContainsKey($id)) { throw "No UIA requirements for layout '$id'" }
 # Each mode's own body is checked, never only the size.
 if (-not @((Get-PodUiaRequired 'workspace' $id) | Where-Object { $_ }).Count) { throw "Layout '$id' has no 'workspace' node" }
}
foreach ($id in $script:PodUiaLayoutRequired.Keys) { if ($ids -notcontains $id) { throw "UIA table for unknown layout '$id'" } }
# 6. Kanban's workspace has no workbench sidebar: its column replaces that node, it is not dropped.
$workbench = [IO.File]::ReadAllText((Join-Path $baseline 'dark-workspace.tree.txt'))
$kanban = (@($workbench -split "`r?`n" | Where-Object { -not $_.Contains('저장소와 작업 탐색') }) + "`t11 LIST 응답 필요, 카드 99개") -join "`n"
if ((Test-PodUiaTree 'workspace' $kanban 'kanban').Count) { throw 'Kanban workspace rejected in the kanban layout' }
if (-not (Test-PodUiaTree 'workspace' $kanban).Count) { throw 'Kanban workspace accepted for the workbench' }
if (-not (Test-PodUiaTree 'workspace' $kanban 'workbench').Count) { throw "Kanban workspace accepted for layout 'workbench'" }
if (-not (Test-PodUiaTree 'workspace' $workbench 'kanban').Count) { throw 'Workbench workspace accepted in the kanban layout' }
# Dialog scenes keep the workbench node in every layout.
if ((Get-PodUiaRequired 'settings' 'kanban') -notcontains '설정 분류') { throw 'settings lost its node in the kanban layout' }
# 7. Title-bar-only dumps fail for every layout scene too.
foreach ($id in $script:PodUiaLayoutRequired.Keys) {
 foreach ($scene in $script:PodUiaLayoutRequired[$id].Keys) {
  if (-not (Test-PodUiaTree $scene $titleOnly $id).Count) { throw "Title-bar-only tree accepted for '$scene' in '$id'" }
 }
}
# 8. Unknown layout: fails at once, the dump is never called.
$script:calls = 0
$failed = $false
try { [void](Get-PodUiaTree 'workspace' { $script:calls++; $workbench } -Attempts 3 -DelayMs 1 -Layout 'no-such-layout') }
catch { $failed = $_.Exception.Message -like "*no-such-layout*" }
if (-not $failed -or $script:calls) { throw "Unknown layout not refused up front (calls=$($script:calls))" }
# 9. The retry loop checks the given layout: kanban tree accepted on attempt 1.
$script:calls = 0
$got = Get-PodUiaTree 'workspace' { $script:calls++; $kanban } -Attempts 3 -DelayMs 1 -Layout 'kanban'
if ($script:calls -ne 1 -or $got -ne $kanban) { throw "Kanban tree not accepted at once (calls=$($script:calls))" }
# 10. Optional: real captures (manifest.json Scene/Layout per image).
$captured = 0
if ($Captures) {
 foreach ($manifest in @(Get-ChildItem $Captures -Recurse -Filter 'manifest.json')) {
  # PowerShell 5.1 passes a JSON array on as one object: foreach over the variable unrolls it.
  $rows = Get-Content -Raw -Encoding UTF8 $manifest.FullName | ConvertFrom-Json
  foreach ($row in $rows) {
   $tree = Join-Path $manifest.DirectoryName ($row.Image -replace '\.png$', '.tree.txt')
   $problems = Test-PodUiaTree $row.Scene ([IO.File]::ReadAllText($tree)) ([string]$row.Layout)
   if ($problems.Count) { throw "$tree fails: $($problems -join '; ')" }
   $captured++
  }
 }
 if (-not $captured) { throw "No captures under $Captures" }
}
"PASS: $($trees.Count) baseline trees complete, title-bar-only rejected, retry accepts attempt 3, gives up after N with the missing node; $($ids.Count) layouts have tables, kanban replaces the sidebar node, unknown layout refused; $captured captured trees complete"
