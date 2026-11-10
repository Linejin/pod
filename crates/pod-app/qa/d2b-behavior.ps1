# Synthetic/semantic checks only, never a native keyboard gate.
param([int]$AppPid, [string]$Theme='dark')
. (Join-Path $PSScriptRoot 'd2b-screens.ps1') -AppPid $AppPid -Phase after -Theme $Theme -OnlyHelpers
function SetField([string]$Pattern,[string]$Value) {
    $tree=Observe
    $match=[regex]::Match($tree,$Pattern,[Text.RegularExpressions.RegexOptions]::Multiline)
    if (-not $match.Success) { throw "Missing input: $Pattern" }
    $r=orca computer set-value --app "pid:$AppPid" --element-index $match.Groups[1].Value --value $Value --no-screenshot --json | ConvertFrom-Json
    if (-not $r.ok) { throw ($r.error | ConvertTo-Json -Compress) }
}
SetField '^\s*(\d+) 편집 저장소, 작업, 터미널 검색,' 'auth'
Capture 'filter'
SetField '^\s*(\d+) 편집 저장소, 작업, 터미널 검색,' ' '
Activate '^\s*(\d+) 단추 명령·터미널 검색$'
SetField '^\s*(\d+) 편집 작업, 터미널 검색 · > 명령만,' '새비그'
Capture 'fuzzy'
Close-Dialog
Jump 'Open repository' '저장소 열기'; Close-Dialog
Activate '^\s*(\d+) 단추 명령·터미널 검색$'; Capture 'recents'; Close-Dialog
Activate '^\s*(\d+) 단추 설정$'
Activate '^\s*(\d+) 단추 18$'
Activate '^\s*(\d+) 단추 Consolas$' -Last
Activate '^\s*(\d+) 단추 ctrl-shift-k$'
Capture 'settings-live'
Jump 'sample/auth' 'sample/auth'; Capture 'font-live'

Jump 'exit-seven' 'exit-seven'; Capture 'inspector-exit'
