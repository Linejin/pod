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
# A title-bar-only dump is 10 lines (header, blank, 6 title-bar nodes).
$script:PodUiaMinLines = 20

# Returns the list of problems (empty = complete).
function Test-PodUiaTree([string]$Scene, [string]$Text) {
 $missing = @()
 $lines = @(($Text -split "`r?`n") | Where-Object { $_.Trim() })
 if ($lines.Count -lt $script:PodUiaMinLines) { $missing += "only $($lines.Count) lines (< $($script:PodUiaMinLines))" }
 $required = $script:PodUiaRequired[$Scene]
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
# Test-PodUiaTree passes. Throws with the last problems if it never does.
function Get-PodUiaTree([string]$Scene, [scriptblock]$Dump, [int]$Attempts = 6, [int]$DelayMs = 1000) {
 $problems = @()
 for ($i = 1; $i -le $Attempts; $i++) {
  try {
   $text = [string](& $Dump)
   $problems = Test-PodUiaTree $Scene $text
  } catch { $problems = @("dump failed: $($_.Exception.Message)") }
  if (-not $problems.Count) {
   if ($i -gt 1) { Write-Host "UIA tree for '$Scene' complete on attempt $i" }
   return $text
  }
  if ($i -lt $Attempts) { Start-Sleep -Milliseconds $DelayMs }
 }
 throw "UIA tree for '$Scene' incomplete after $Attempts attempts: $($problems -join '; ')"
}
