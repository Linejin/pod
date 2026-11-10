# SPDX-License-Identifier: Apache-2.0
. (Join-Path $PSScriptRoot 'session.ps1')
if ($env:POD_SOCKET -ne 'pod-e2e-rerun-c9cf81') { throw 'Wrong daemon' }
$a = App
if ($a) { $a.CloseMainWindow() | Out-Null; if (!$a.WaitForExit(5000)) { $a.Kill() } }
$tasks = (Pod task ls --json | ConvertFrom-Json).tasks
foreach ($task in $tasks) {
    $full = [System.IO.Path]::GetFullPath($task.worktree)
    $expected = [System.IO.Path]::GetFullPath((Join-Path $env:POD_WORKTREES_ROOT 'sample')) + '\'
    if (!$full.StartsWith($expected,[System.StringComparison]::OrdinalIgnoreCase)) { throw "Unexpected worktree: $full" }
    Pod task rm $task.id --force --delete-branch --json
    if ($LASTEXITCODE -ne 0) { throw "Cleanup failed for $($task.id)" }
}
$remaining = (Pod ls --all --json | ConvertFrom-Json).terminals
foreach ($terminal in $remaining) {
    Pod rm $terminal.id --force --json
    if ($LASTEXITCODE -ne 0) { throw "Cleanup failed for $($terminal.id)" }
}
Pod group rm sample/duel --json
Pod repo rm sample --json
Pod ls --all --json
Pod server stop --kill --json
& git -C (Join-Path $E2eRoot 'sample') worktree list --porcelain
& git -C (Join-Path $E2eRoot 'sample') branch --list 'pod/*'
$watcherFile = Join-Path $E2eRoot 'watcher.pid'
if (Test-Path $watcherFile) {
    $watcherId = [int](Get-Content $watcherFile)
    $watcher = Get-CimInstance Win32_Process -Filter "ProcessId=$watcherId"
    if ($watcher -and $watcher.CommandLine -like "*$RepoRoot*watch-status.ps1*") {
        Stop-Process -Id $watcherId -Force
    }
}
# Delete only this rerun's disposable state, after Pod removed its worktrees.
$privateState = Join-Path $env:LOCALAPPDATA 'pod\state\pod-e2e-rerun-c9cf81'
foreach ($target in @($E2eRoot, $privateState)) {
    $resolved = [System.IO.Path]::GetFullPath($target)
    $allowed = @('C:\ai\pod-e2e\rerun-c9cf81', [System.IO.Path]::GetFullPath($privateState))
    if ($resolved -notin $allowed) { throw "Unexpected cleanup path: $resolved" }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
