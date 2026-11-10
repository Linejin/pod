# Semantic fixture navigation. This is not native keyboard-only evidence.
param([int]$AppPid, [ValidateSet('before','after')][string]$Phase, [string]$Theme, [switch]$OnlyHelpers)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$OutputEncoding = [Text.UTF8Encoding]::new()
function Observe {
    $r = orca computer get-app-state --app "pid:$AppPid" --no-screenshot --json | ConvertFrom-Json
    if (-not $r.ok) { throw ($r.error | ConvertTo-Json -Compress) }
    return $r.result.snapshot.treeText
}
function Activate([string]$Pattern, [switch]$Last) {
    $tree = Observe
    $found = @([regex]::Matches($tree, $Pattern, [Text.RegularExpressions.RegexOptions]::Multiline))
    if ($found.Count -eq 0 -or ($found.Count -gt 1 -and -not $Last)) { throw "Ambiguous/missing target $Pattern`n$tree" }
    $index = $found[-1].Groups[1].Value
    $r = orca computer click --app "pid:$AppPid" --element-index $index --no-screenshot --json | ConvertFrom-Json
    if (-not $r.ok) { throw ($r.error | ConvertTo-Json -Compress) }
}
function Close-Dialog {
    if ((Observe) -match '대화 상자') { Activate '^\s*(\d+) 단추 (Close|닫기)$' -Last }
}
function Jump([string]$English, [string]$Korean) {
    Close-Dialog
    Activate '^\s*(\d+) 단추 (Search commands / terminals Ctrl\+K|명령·터미널 검색)$'
    $title = if ($Phase -eq 'before') { $English } else { $Korean }
    $tree = Observe
    $inputMatch = [regex]::Match($tree, '^\s*(\d+) 편집 (Search tasks, terminals or > commands|작업, 터미널 검색 · > 명령만),', [Text.RegularExpressions.RegexOptions]::Multiline)
    if (-not $inputMatch.Success) { throw "Palette input missing" }
    $r = orca computer set-value --app "pid:$AppPid" --element-index $inputMatch.Groups[1].Value --value $title --no-screenshot --json | ConvertFrom-Json
    if (-not $r.ok) { throw ($r.error | ConvertTo-Json -Compress) }
    powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'input.ps1') -AppPid $AppPid -Key 13
    if ((Observe) -match '편집 (Search tasks, terminals or > commands|작업, 터미널 검색 · > 명령만),') { throw "Palette command did not activate: $title" }
}
function Capture([string]$Screen) {
    $name = "d2b-$Phase-$Theme-$Screen"
    # Let async model changes settle; capture only the target HWND at native DPI.
    Start-Sleep -Milliseconds 500
    powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'capture.ps1') -AppPid $AppPid -Name $name
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot "$name.tree.txt"), (Observe))
}
if ($OnlyHelpers) { return }
Close-Dialog
$ready = $false
for ($attempt = 0; $attempt -lt 15; $attempt++) {
    if ((Observe) -match '(Send 1 open to agent|미해결 1개 보내기)') { $ready = $true; break }
    Start-Sleep -Seconds 1
}
if (-not $ready) { throw 'Review fixture did not finish loading' }
Capture 'review'
Activate '^\s*(\d+) 단추 (Settings|설정)$'; Capture 'settings'; Close-Dialog
Activate '^\s*(\d+) 단추 (Search commands / terminals Ctrl\+K|명령·터미널 검색)$'; Capture 'palette'; Close-Dialog
Jump 'Open repository'  '저장소 열기'; Capture 'repository'; Close-Dialog
Jump 'New task' '새 작업'; Capture 'new-task'; Close-Dialog
Jump 'New terminal' '새 터미널'; Capture 'new-terminal'; Close-Dialog
Jump 'New comparison group' '새 비교 그룹'; Capture 'new-group'; Close-Dialog
Jump 'Open agent tree' '에이전트 트리 열기'; Capture 'agents'
Jump 'Open gates inbox' '승인 요청 열기'; Capture 'gates'
Jump 'Notifications' '알림'; Capture 'notifications'; Close-Dialog

Jump 'Open agent tree' '에이전트 트리 열기'
# Group row position is from the reviewed fixture screenshot at 144 DPI.
powershell -NoProfile -File (Join-Path $PSScriptRoot 'input.ps1') -AppPid $AppPid -X 1020 -Y 134
Start-Sleep -Seconds 2
Capture 'group'
Jump 'sample/auth' 'sample/auth'; Capture 'terminal'
Activate '^\s*(\d+) 단추 (Status|상태)$'; Capture 'inspector'; Close-Dialog
