# UIA tree checks for capture.ps1 (dot-sourced). A dump sometimes comes back
# with only the title bar (10 lines): retry until each scene's required nodes
# are present, or fail the capture with the missing names.

# Substrings of `<role> <name>` lines in `orca computer get-app-state` treeText
# (Korean UIA role names on this machine; the title bar keeps English roles).
# A title-bar-only tree contains none of them. Scenes not listed here (QA-only
# editor-* scenes) only get the size check.
$script:PodUiaRequired = @{
 'workspace'     = @('저장소와 작업 탐색')
 'palette'       = @('작업, 터미널 검색 · > 명령만')
 'settings'      = @('설정 분류', '스타일')
 'agents'        = @('Claude 에이전트, Codex 에이전트')
 'inspect'       = @('패널 메뉴')
 'open-repo'     = @('로컬 Git 저장소 경로')
 'new-task'      = @('작업 이름 (예: fix-login')
 'terminal'      = @('이름 (선택, 예: review-bot)')
 'new-group'     = @('Claude 늘리기')
 'gates'         = @('승인 요청 새로고침')
 'notifications' = @('Worker 1 · 승인 필요')
 'review'        = @('다음 변경 (F7')
 'shortcuts'     = @('대화 상자')
 'editor'        = @('열린 파일')
 'orch-board'    = @('작업 항목 완료')
 'orch-inbox'    = @('받은 편지함')
 'orch-cleanup'  = @('끝난 워커 정리')
 'orch-settings' = @('오케스트레이션 설정')
}
# The table above is the workbench layout's (settings.json `layout` empty or
# 'workbench'). The other layout modes (crates/pod-app/src/layout_mode.rs)
# draw their own body: a scene listed under a layout replaces the workbench
# entry (@() = size check only); unlisted scenes keep it (dialogs, palette).
# Every layout id needs an entry here (uia-selftest.ps1 checks the source).
# A dock-panel scene the layout does not show (kanban/review keep the panel
# behind their 패널 tab, block-terminal draws no dock) checks the layout's
# own body instead (A-F0b: every scene captured in every layout).
$script:PodUiaLayoutRequired = @{
 'focus-rail'     = @{
  'workspace'      = @('레일 탐색')
  'rail'           = @('레일 탐색', '작업 탭')
  'rail-attention' = @('레일 탐색', '응답이 필요한 다른 작업')
  'rail-switcher'  = @('레일 탐색', '모든 작업')
  'rail-workspace' = @('레일 탐색', '저장소와 작업 탐색')
 }
 'queue'          = @{
  'workspace' = @('응답 대기열')
  'queue'     = @('응답 대기열', '이번만 허용: ')
 }
 'kanban'         = @{
  'workspace'     = @('응답 필요, 카드')
  'orch-board'    = @('응답 필요, 카드')
  'orch-inbox'    = @('응답 필요, 카드')
  'orch-cleanup'  = @('응답 필요, 카드')
  'orch-settings' = @('응답 필요, 카드')
 }
 'block-terminal' = @{
  'workspace'     = @('블록 피드')
  'settings'      = @('블록 피드')
  'agents'        = @('블록 피드')
  'inspect'       = @('블록 피드')
  'gates'         = @('블록 피드')
  'review'        = @('블록 피드')
  'editor'        = @('블록 피드')
  'orch-board'    = @('블록 피드')
  'orch-inbox'    = @('블록 피드')
  'orch-cleanup'  = @('블록 피드')
  'orch-settings' = @('블록 피드')
 }
 'review'         = @{
  'workspace'            = @('리뷰 화면')
  'review-editor-layout' = @('리뷰 화면', '다음 변경 (F7')
  'orch-board'           = @('리뷰 화면')
  'orch-inbox'           = @('리뷰 화면')
  'orch-cleanup'         = @('리뷰 화면')
  'orch-settings'        = @('리뷰 화면')
 }
 'dense-details'  = @{
  'workspace' = @('저장소와 작업 탐색', '출력 · 이벤트 기록')
  'dense'     = @('저장소와 작업 탐색', '출력 · 이벤트 기록', '세부 정보: ')
 }
}

# Required substrings for $Scene in layout mode $Layout ('' = workbench).
# An unknown layout id throws: a new mode needs its own entry first.
function Get-PodUiaRequired([string]$Scene, [string]$Layout = '') {
 if ($Layout -and $Layout -ne 'workbench') {
  $table = $script:PodUiaLayoutRequired[$Layout]
  if ($null -eq $table) { throw "No UIA requirements for layout '$Layout' (uia-required.ps1)" }
  if ($table.ContainsKey($Scene)) { return ,@($table[$Scene]) }
 }
 return ,@($script:PodUiaRequired[$Scene])
}

# A title-bar-only dump is 10 lines (header, blank, 6 title-bar nodes).
$script:PodUiaMinLines = 20

# Returns the list of problems (empty = complete).
function Test-PodUiaTree([string]$Scene, [string]$Text, [string]$Layout = '') {
 $missing = @()
 $lines = @(($Text -split "`r?`n") | Where-Object { $_.Trim() })
 if ($lines.Count -lt $script:PodUiaMinLines) { $missing += "only $($lines.Count) lines (< $($script:PodUiaMinLines))" }
 $required = Get-PodUiaRequired $Scene $Layout
 foreach ($needle in @($required)) {
  if ($needle -and -not $Text.Contains($needle)) { $missing += "missing '$needle'" }
 }
 return ,$missing
}

# The native activation probe (qa/ui-wave0/dock-activation.ps1) supplies .NET
# UIA ControlType/Name rows. Check each partial frame, without retrying away a
# missing cached dock subtree. The status-bar Button is expected; its TabItem
# is not: window activation must never open the inspector.
function Test-PodDockActivationTree([string]$Text) {
 $problems = Test-PodUiaTree 'settings' $Text
 if ($Text -match '(?m)^ControlType\.TabItem 상태\r?$') {
  $problems += 'activation opened 상태 inspector'
 }
 return ,$problems
}

# Calls $Dump (returns tree text) up to $Attempts times, $DelayMs apart, until
# Test-PodUiaTree passes for $Layout. Throws with the last problems if it never does.
function Get-PodUiaTree([string]$Scene, [scriptblock]$Dump, [int]$Attempts = 6, [int]$DelayMs = 1000, [string]$Layout = '') {
 $what = if ($Layout) { "'$Scene' (layout $Layout)" } else { "'$Scene'" }
 [void](Get-PodUiaRequired $Scene $Layout)
 $problems = @()
 for ($i = 1; $i -le $Attempts; $i++) {
  try {
   $text = [string](& $Dump)
   $problems = Test-PodUiaTree $Scene $text $Layout
  } catch { $problems = @("dump failed: $($_.Exception.Message)") }
  if (-not $problems.Count) {
   if ($i -gt 1) { Write-Host "UIA tree for $what complete on attempt $i" }
   return $text
  }
  if ($i -lt $Attempts) { Start-Sleep -Milliseconds $DelayMs }
 }
 throw "UIA tree for $what incomplete after $Attempts attempts: $($problems -join '; ')"
}
