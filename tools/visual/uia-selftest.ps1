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
"PASS: $($trees.Count) baseline trees complete, title-bar-only rejected, retry accepts attempt 3, gives up after N with the missing node"
