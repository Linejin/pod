# SPDX-License-Identifier: Apache-2.0
# docs/19 S7: the orchestration screens against a real, isolated podd with two
# fake-agent workers (no Claude/Codex). A fake coordinator (`fake-agent
# orch-loop`) starts two workers with `pod orch worker start`; worker a1 asks a
# question, worker a2 reports progress; the board and inbox are captured; the
# coordinator answers, both workers report `done`; captured again; then the
# cleanup screen. Windows open on the SECONDARY monitor; captures are DPI-aware
# PrintWindow (no focus taken, no input injected: the app opens its screens
# with `--command`). Everything lives under a temp root and is removed at the
# end; the user's real podd, %APPDATA%\Pod and agent configs are never used.
#
#   powershell -NoProfile -File tools/e2e/orch-s7.ps1 [-NoBuild] [-Keep]
param(
  [string]$Out = 'crates/pod-app/qa/orch/e2e',
  [switch]$NoBuild,
  [switch]$Keep
)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $RepoRoot
$Bin = Join-Path $RepoRoot 'target\debug'
if (-not $NoBuild) {
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
  cargo build -p podd --bins --example fake-agent -p pod-cli --bin pod -p pod-app
  if ($LASTEXITCODE -ne 0) { throw 'build failed' }
}
$Evidence = Join-Path $RepoRoot $Out
New-Item -ItemType Directory -Force $Evidence | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Root = Join-Path $env:TEMP "pod-orch-s7-$stamp"
$Name = "pod-orch-s7-$stamp"
New-Item -ItemType Directory -Force $Root | Out-Null
$Log = Join-Path $Evidence 'scenario.log'
Set-Content -Encoding UTF8 $Log "orch-s7 scenario $stamp"
function Note([string]$m) { $line = "[$((Get-Date).ToString('HH:mm:ss'))] $m"; $line; Add-Content -Encoding UTF8 $Log $line }

# Isolation: own pipe, state, worktrees, app config, mods and discovery dirs.
$env:POD_SOCKET = $Name
$env:POD_APP_CONFIG_DIR = Join-Path $Root 'config'
$env:POD_MODS_DIR = Join-Path $Root 'mods'
$env:POD_DISCOVER_CLAUDE_DIR = Join-Path $Root 'stores\claude'
$env:POD_DISCOVER_CODEX_DIR = Join-Path $Root 'stores\codex'
$env:POD_DISCOVER_PROCESSES = 'off'
# Keep finished workers' terminals for the captures (released on cleanup).
$env:POD_ORCH_RELEASE_AFTER = 'off'
$env:NO_COLOR = $null

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (-not ('OrchShot' -as [type])) {
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class OrchShot {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
}
'@
}
[void][OrchShot]::SetProcessDpiAwarenessContext([IntPtr](-4))

$pod = Join-Path $Bin 'pod.exe'
$fake = Join-Path $Bin 'examples\fake-agent.exe'
function Pod { $o = & $pod @args 2>&1; $code = $LASTEXITCODE; Add-Content -Encoding UTF8 $Log ("pod " + ($args -join ' ') + " -> $code`n" + ($o -join "`n")); if ($code -ne 0) { throw "pod $($args -join ' ') failed ($code)" }; $o -join "`n" }
function Until([string]$what, [int]$secs, [scriptblock]$cond) {
  $deadline = (Get-Date).AddSeconds($secs)
  while (-not (& $cond)) { if ((Get-Date) -gt $deadline) { throw "timeout: $what" }; Start-Sleep -Milliseconds 250 }
  Note "ok: $what"
}
$Base = Join-Path $Root 'agents'
New-Item -ItemType Directory -Force $Base | Out-Null
Set-Content -Encoding ASCII (Join-Path $Base 'pod-exe.txt') $pod
$n = @{}
function Turn([string]$who, [object[]]$cmds) {
  # One fake-agent turn: it runs these `pod` argv lists with its own env.
  $dir = Join-Path $Base $who
  $first = (Get-ChildItem $dir -Filter 'out-*.json' -ErrorAction SilentlyContinue | Measure-Object).Count + 1
  # PowerShell 5.1's ConvertTo-Json flattens nested arrays: build it by hand.
  $parts = foreach ($c in $cmds) { '[' + ((@($c) | ForEach-Object { ConvertTo-Json -Compress ([string]$_) }) -join ',') + ']' }
  $json = '[' + ($parts -join ',') + ']'
  $tmp = Join-Path $dir 'cmd.tmp'
  [IO.File]::WriteAllText($tmp, $json, (New-Object Text.UTF8Encoding $false))
  Move-Item -Force $tmp (Join-Path $dir 'cmd.json')
  $first
}
function OutOf([string]$who, [int]$k, [int]$secs = 60) {
  $p = Join-Path (Join-Path $Base $who) "out-$k.json"
  Until "$who out-$k" $secs { Test-Path $p }
  Get-Content -Raw -Encoding UTF8 $p | ConvertFrom-Json
}
function Shot([System.Diagnostics.Process]$app, [string]$name) {
  $app.Refresh(); $h = $app.MainWindowHandle
  if ($h -eq [IntPtr]::Zero) { throw 'no app window' }
  $r = New-Object OrchShot+RECT; [void][OrchShot]::GetWindowRect($h, [ref]$r)
  $b = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($b); $dc = $g.GetHdc()
  try { [void][OrchShot]::PrintWindow($h, $dc, 2) } finally { $g.ReleaseHdc($dc) }
  $path = Join-Path $Evidence "$name.png"; $b.Save($path); $g.Dispose(); $b.Dispose()
  Note "capture $path rect=$($r.L),$($r.T),$($r.R),$($r.B) dpi=$([OrchShot]::GetDpiForWindow($h))"
  # UIA names of the orchestration screens (accessible names are Korean).
  $el = [System.Windows.Automation.AutomationElement]::FromHandle($h)
  $all = $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  $names = foreach ($e in $all) { $nm = $e.Current.Name; if ($nm -match '워커|실행|받은 편지함|질문|답장|정리|코디네이터') { "$($e.Current.ControlType.ProgrammaticName)`t$nm" } }
  Set-Content -Encoding UTF8 (Join-Path $Evidence "$name.uia.txt") $names
  Note "uia: $(@($names).Count) named orchestration elements"
}
function StartApp([string[]]$commands) {
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 | Out-Host
  $args2 = @('--pipe', $Name, '--display', 'secondary', '--no-sound')
  foreach ($c in $commands) { $args2 += @('--command', $c) }
  # -NoNewWindow: the debug build's console attaches to this (hidden) one.
  $p = Start-Process -FilePath (Join-Path $Bin 'pod-app.exe') -ArgumentList $args2 -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Root "app-$($commands -join '-').out") -RedirectStandardError (Join-Path $Root "app-$($commands -join '-').err")
  return $p
}

$podd = $null; $app = $null
try {
  # podd (isolated) and a repository.
  $podd = Start-Process -FilePath (Join-Path $Bin 'podd.exe') -NoNewWindow -PassThru -ArgumentList @('--pipe', "\\.\pipe\$Name", '--holder', (Join-Path $Bin 'pod-holder.exe'), '--state-dir', (Join-Path $Root 'state'), '--worktrees-root', (Join-Path $Root 'worktrees')) -RedirectStandardOutput (Join-Path $Root 'podd.out') -RedirectStandardError (Join-Path $Root 'podd.err')
  Until 'podd pipe' 20 { Test-Path "\\.\pipe\$Name" }
  $repo = Join-Path $Root 'repo'
  New-Item -ItemType Directory -Force $repo | Out-Null
  git -C $repo init -q -b main; git -C $repo config user.name 'Pod Test'; git -C $repo config user.email 'pod-test@example.invalid'; git -C $repo config commit.gpgsign false
  Set-Content -Encoding UTF8 (Join-Path $repo 'README.md') 'demo'
  git -C $repo add -A; git -C $repo commit -q -m init
  Pod repo add $repo --name demo | Out-Null
  $agent = ConvertTo-Json -Compress @($fake, "orch-loop:$Base")
  $coord = Pod task new demo coord --json '--' $fake "orch-loop:$Base" | ConvertFrom-Json
  Note "coordinator task $($coord.id) terminal $($coord.terminals[0].id)"
  Until 'coordinator agent' 30 { Test-Path (Join-Path $Base 'coord') }

  # The coordinator starts two workers.
  $k = Turn 'coord' @(
    @('orch', 'worker', 'start', '--spec', "세션 만료 처리를 고친다.`n완료 기준: 테스트 통과.", '--title', '세션 만료 처리 고치기', '--agent', $agent, '--json'),
    @('orch', 'worker', 'start', '--spec', '로그인 회귀 테스트를 추가한다.', '--title', '회귀 테스트 추가', '--agent', $agent, '--json'))
  $s1 = OutOf 'coord' $k; $s2 = OutOf 'coord' ($k + 1)
  if ($s1.code -ne 0 -or $s2.code -ne 0) { throw "worker start failed: $($s1.stderr) $($s2.stderr)" }
  Until 'worker agents' 30 { (Test-Path (Join-Path $Base 'a1')) -and (Test-Path (Join-Path $Base 'a2')) }
  # The app starts after the workers (their auto-opened tabs would cover the board).
  $app = StartApp @('orch-board', 'orch-inbox')
  Start-Sleep -Seconds 4
  # a1 asks (blocks until answered); a2 notes progress.
  $qa = Turn 'a1' @(, @('orch', 'ask', '--question', '세션 만료를 401로 둘까요, 403으로 바꿀까요?', '--json'))
  $pa = Turn 'a2' @(, @('orch', 'send', '--type', 'heartbeat', '--subject', '테스트 12/30', '--json'))
  [void](OutOf 'a2' $pa)
  Until 'question recorded' 30 { (Pod orch workers --run r1 --json | ConvertFrom-Json).workers | Where-Object { $_.waiting_answer } }
  Start-Sleep -Seconds 2
  Shot $app '01-board-inbox-question'

  # The coordinator reads its mail and answers; then both workers finish.
  $c = Turn 'coord' @(, @('orch', 'check', '--json'))
  $mail = ((OutOf 'coord' $c).stdout | ConvertFrom-Json).messages
  $q = $mail | Where-Object { $_.type -eq 'question' } | Select-Object -First 1
  if (-not $q) { throw 'no question in the coordinator mailbox' }
  $r = Turn 'coord' @(, @('orch', 'reply', '--id', $q.id, '--body', '401 유지: 기존 클라이언트 호환', '--json'))
  [void](OutOf 'coord' $r)
  $ans = OutOf 'a1' $qa
  if ($ans.code -ne 0) { throw "ask failed: $($ans.stderr)" }
  $d1 = Turn 'a1' @(, @('orch', 'done', '--outcome', 'succeeded', '--body', '무엇을: 401 유지 / 찾은 것: 갱신 경쟁 / 남은 것: 없음', '--json'))
  $d2 = Turn 'a2' @(, @('orch', 'done', '--outcome', 'succeeded', '--body', '무엇을: 회귀 테스트 3개 / 찾은 것: 없음 / 남은 것: 없음', '--json'))
  [void](OutOf 'a1' $d1); [void](OutOf 'a2' $d2)
  Until 'both attempts succeeded' 30 { $run = Pod orch run show r1 --json | ConvertFrom-Json; @($run.attempts | Where-Object { $_.state -eq 'succeeded' }).Count -eq 2 }
  Start-Sleep -Seconds 2
  Shot $app '02-board-inbox-done'
  Stop-Process -Id $app.Id -Force; $app = $null

  # The cleanup screen (finished workers, S3 tiers).
  $app = StartApp @('orch-cleanup')
  Start-Sleep -Seconds 5
  Shot $app '03-cleanup-restored-layout'
  Note 'scenario passed'
}
finally {
  if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force }
  # Only this scenario's processes (by path), never another podd.
  Get-Process podd, pod-holder, fake-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($Bin) -and ($_.StartTime -ge (Get-Item $Root).CreationTime) } | Stop-Process -Force -ErrorAction SilentlyContinue
  if (-not $Keep) { Start-Sleep -Milliseconds 500; Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue }
}
