# Empty-diff regression with the right dock visible, using fixture data only.
param([ValidateSet('dark','light')][string]$Theme='dark')
. (Join-Path $PSScriptRoot 'd2b-launch.ps1') -Phase after -Theme $Theme -Task 'sample/compare-shell-2' -CapturePass 'empty-final'
$fixturePid=[int](Get-Content (Join-Path $D2Demo 'app.pid'))
. (Join-Path $PSScriptRoot 'd2b-screens.ps1') -AppPid $fixturePid -Phase after -Theme $Theme -OnlyHelpers
Start-Sleep -Seconds 2
Jump 'Open agent tree' '에이전트 트리 열기'
Activate '^\s*(\d+) 단추 상태$'
Capture 'review-empty'
