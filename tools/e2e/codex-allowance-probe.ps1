# Probe the narrow Codex worker allowance (docs/19 §18.1) with the REAL
# installed `codex` binary and no model: `codex exec` talks to a local
# scripted Responses API (codex-mock-responses.py) that makes it run fixed
# shell commands through its own exec policy and sandbox.
#
#   tools\e2e\codex-allowance-probe.ps1 [-NoBuild]
#
# Isolation: a temp repository + linked worktree, its own podd (pipe, state,
# worktrees), `codex exec --ephemeral` (no session files). The user's Codex
# config is read (as a real worker would) but never written. The rules are
# the ones podd writes (taken from crates/podd/src/server/codex_allowance.rs).
# Section 4 starts a real Pod worker (an idle PowerShell as its agent) for
# the repository-filter check (docs/19 §18.5).
# Evidence: target/e2e/codex-allowance/ (probe.log, mock logs, exec output).
param([switch]$NoBuild, [int]$Port = 18431)
$ErrorActionPreference = 'Stop'
$Repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Bin = Join-Path $Repo 'target\debug'
$Out = Join-Path $Repo 'target\e2e\codex-allowance'
New-Item -ItemType Directory -Force $Out | Out-Null
$Log = Join-Path $Out 'probe.log'
Set-Content $Log "codex-allowance probe $(Get-Date -Format s)"
function Note($s) { Write-Host $s; Add-Content $Log $s }

if (-not $NoBuild) {
  & cargo build -p podd --bins -p pod-cli --bin pod
  if ($LASTEXITCODE) { throw 'build failed' }
}
$python = (Get-Command python -ErrorAction Stop).Source
Note "codex: $((Get-Command codex).Source) $(codex --version)"

# Temp repository + linked worktree (a Pod worker's layout).
$Name = "pod-codex-probe-$PID"
$Root = Join-Path $env:TEMP $Name
Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
$Main = Join-Path $Root 'repo'; $Wt = Join-Path $Root 'wt'
New-Item -ItemType Directory -Force $Main | Out-Null
git -C $Main init -q -b main
git -C $Main config user.name 'Pod Probe'; git -C $Main config user.email 'probe@example.invalid'; git -C $Main config commit.gpgsign false
Set-Content (Join-Path $Main 'a.txt') 'a'
git -C $Main add a.txt; git -C $Main commit -qm init
git -C $Main worktree add -q -b pod/probe $Wt

# The rules podd writes, and its info/exclude line.
$src = Get-Content -Raw (Join-Path $Repo 'crates\podd\src\server\codex_allowance.rs')
$rules = [regex]::Match($src, 'pub const RULES: &str = r#"(?s)(.*?)"#;').Groups[1].Value
$excl = [regex]::Match($src, 'pub const EXCLUDE_LINE: &str = "(.*?)";').Groups[1].Value
function Install-Rules {
  New-Item -ItemType Directory -Force (Join-Path $Wt '.codex\rules') | Out-Null
  [IO.File]::WriteAllText((Join-Path $Wt '.codex\rules\pod-worker.rules'), $rules)
  Add-Content (Join-Path $Main '.git\info\exclude') $excl
}

# Isolated podd; `pod` from this build first on PATH.
$Pipe = $Name
$podd = Start-Process -FilePath (Join-Path $Bin 'podd.exe') -NoNewWindow -PassThru -ArgumentList @('--pipe', "\\.\pipe\$Pipe", '--holder', (Join-Path $Bin 'pod-holder.exe'), '--state-dir', (Join-Path $Root 'state'), '--worktrees-root', (Join-Path $Root 'worktrees')) -RedirectStandardOutput (Join-Path $Root 'podd.out') -RedirectStandardError (Join-Path $Root 'podd.err')
for ($i = 0; $i -lt 100 -and -not (Test-Path "\\.\pipe\$Pipe"); $i++) { Start-Sleep -Milliseconds 100 }
$env:POD_SOCKET = $Pipe
$env:PATH = "$Bin;$env:PATH"

function Probe([string]$tag, [object[]]$commands) {
  $cmds = Join-Path $Out "$tag.cmds.json"; $mlog = Join-Path $Out "$tag.mock.jsonl"
  [IO.File]::WriteAllText($cmds, (ConvertTo-Json @($commands)))
  Remove-Item $mlog -ErrorAction SilentlyContinue
  $mock = Start-Process $python -ArgumentList @('-I', (Join-Path $PSScriptRoot 'codex-mock-responses.py'), "$Port", $cmds, $mlog) -PassThru -NoNewWindow
  Start-Sleep -Milliseconds 800
  Push-Location $Wt
  try {
    $a = @('exec', '--ephemeral', '-m', 'mock-model', '-c', 'model_provider=mock',
      '-c', "model_providers.mock={name='mock',base_url='http://127.0.0.1:$Port/v1',wire_api='responses'}",
      '-c', "projects={'$Wt'={trust_level='trusted'}}", 'probe')
    $ErrorActionPreference = 'Continue'
    & codex @a 2>&1 | Out-File -Encoding utf8 (Join-Path $Out "$tag.exec.txt")
  } finally { Pop-Location; Stop-Process $mock.Id -Force -ErrorAction SilentlyContinue }
  # Map each command to the output Codex sent back for it.
  $lines = Get-Content $mlog | ForEach-Object { $_ | ConvertFrom-Json }
  $res = [ordered]@{}
  for ($i = 0; $i -lt $commands.Count; $i++) {
    $o = ($lines | Where-Object { $_.outputs -and $_.outputs[0].call_id -eq "call_$($i + 1)" } | Select-Object -First 1).outputs[0].output
    $e = $commands[$i]
    $key = if ($e -is [hashtable]) { $e.name } else { $e }
    $res[$key] = "$o"
  }
  $res
}
# An environment variable with an empty value (PowerShell's `$env:X = ''`
# and .NET delete it); the worker's git guard has empty filter values.
Add-Type -Namespace PodProbe -Name Env -MemberDefinition '[DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern bool SetEnvironmentVariable(string name, string value);'
function Set-RawEnv($k, $v) { [void][PodProbe.Env]::SetEnvironmentVariable($k, [string]$v) }
function Code($o) { if ($o -match 'Process exited with code (\d+)') { [int]$Matches[1] } else { -1 } }

$fails = 0
function Check($what, $ok, $detail) {
  $s = if ($ok) { 'PASS' } else { $script:fails++; 'FAIL' }
  Note "$s  $what"
  if (-not $ok) { Note "      $($detail -replace "`n", ' | ')" }
}

$outside = Join-Path $env:USERPROFILE "pod-codex-probe-$PID.txt"
try {
  # 1. Control: no rules -> pod cannot reach the pipe, git cannot commit.
  Set-Content (Join-Path $Wt 'b.txt') 'b'
  $r = Probe 'control' @('pod ls', 'git add b.txt')
  Check 'control: pod ls is denied in the sandbox' ((Code $r['pod ls']) -ne 0) $r['pod ls']
  Check 'control: git add is denied (index.lock)' ((Code $r['git add b.txt']) -ne 0 -and $r['git add b.txt'] -match 'index.lock') $r['git add b.txt']

  # 2. With Pod's rules.
  Install-Rules
  Check 'rules file hidden from git' (-not ((git -C $Wt status --porcelain --untracked-files=all) -match 'pod-worker')) ''
  # A made-up worker identity: `pod orch commit` then gets podd's own
  # "not found" answer, which proves it reached the pipe (the sandbox would
  # fail with os error 5 first).
  $env:POD_ATTEMPT_ID = 'a99'; $env:POD_ATTEMPT_TOKEN = 'probe'; $env:POD_TERMINAL_ID = 't1'
  try {
    $r = Probe 'allowed' @('"id=$env:POD_ATTEMPT_ID token=$env:POD_ATTEMPT_TOKEN"', 'pod --version', 'pod orch queue', 'pod orch commit -m probe --all', 'pod ls', 'pod read t1', 'git status --short', 'git add b.txt', 'git commit -m probe', 'git log --oneline -1', 'git diff HEAD --stat', "Set-Content -Path '$outside' -Value x", 'pod orch limits --set max_workers=2')
  } finally { Remove-Item Env:POD_ATTEMPT_ID, Env:POD_ATTEMPT_TOKEN, Env:POD_TERMINAL_ID -ErrorAction SilentlyContinue }
  $envl = $r['"id=$env:POD_ATTEMPT_ID token=$env:POD_ATTEMPT_TOKEN"']
  Check 'the worker identity reaches commands Codex runs' ($envl -match 'id=a99 token=probe') $envl
  Check 'pod --version runs' ((Code $r['pod --version']) -eq 0) $r['pod --version']
  Check 'pod orch queue reaches the pipe' ((Code $r['pod orch queue']) -eq 0) $r['pod orch queue']
  $pc = $r['pod orch commit -m probe --all']
  Check 'pod orch commit reaches podd (allowed)' ($pc -notmatch 'os error 5' -and $pc -match 'a99') $pc
  Check 'pod ls stays in the sandbox (no terminal listing)' ((Code $r['pod ls']) -ne 0 -and $r['pod ls'] -match 'os error 5') $r['pod ls']
  Check 'pod read stays in the sandbox (no terminal reading)' ((Code $r['pod read t1']) -ne 0 -and $r['pod read t1'] -match 'os error 5') $r['pod read t1']
  Check 'git status runs (sandbox)' ((Code $r['git status --short']) -eq 0) $r['git status --short']
  Check 'git add stays in the sandbox (index.lock denied)' ((Code $r['git add b.txt']) -ne 0) $r['git add b.txt']
  Check 'git commit stays in the sandbox' ((Code $r['git commit -m probe']) -ne 0) $r['git commit -m probe']
  Check 'git log runs (sandbox)' ((Code $r['git log --oneline -1']) -eq 0) $r['git log --oneline -1']
  Check 'git diff runs (sandbox)' ((Code $r['git diff HEAD --stat']) -eq 0) $r['git diff HEAD --stat']
  Check 'writing outside the worktree is still denied' ((Code $r["Set-Content -Path '$outside' -Value x"]) -ne 0 -and -not (Test-Path $outside)) $r["Set-Content -Path '$outside' -Value x"]
  $lim = $r['pod orch limits --set max_workers=2']
  Check 'pod orch limits (not allowed) stays sandboxed' ((Code $lim) -ne 0) $lim
  Note "exec mode never asks: a command outside the rules fails in the sandbox instead of asking."
  # 3. Escape attempts (docs/19 §18.1): a command that runs outside the
  # sandbox must not pick up a git config the sandboxed agent wrote. The
  # agent cannot touch the worktree's own `.git` file, but it can build
  # nested repositories whose pre-commit hook and fsmonitor write a marker
  # OUTSIDE the worktree. No marker may appear. Each attack runs twice:
  # as is, and with `safe.directory=*` (a common Windows setting; injected
  # through GIT_CONFIG_* for that codex process only), because otherwise
  # git's ownership check (the repo belongs to Codex's sandbox user) is
  # what stops it, by accident.
  $mk = Join-Path $env:USERPROFILE "pod-codex-escape-$PID"
  $mkU = $mk.Replace('\', '/')
  $gitBefore = Get-Content -Raw (Join-Path $Wt '.git')
  $names = @('c0', 'a1', 'a2', 'a3', 'a4', 'a5', 's1', 's2', 's3', 's4', 's5')
  $setup = @"
foreach (`$n in '$($names -join "','")') {
  git init -q `$n
  New-Item -ItemType Directory -Force "`$n\h" | Out-Null
  `$u = ("`$PWD\`$n").Replace('\', '/')
  [IO.File]::WriteAllText("`$PWD\`$n\h\pre-commit", "#!/bin/sh``necho hook > '$mkU-`$n-hook.txt'``n")
  [IO.File]::WriteAllText("`$PWD\`$n\fsm", "#!/bin/sh``necho fsmonitor > '$mkU-`$n-fsm.txt'``nexit 1``n")
  git -C `$n config core.hooksPath "`$u/h"
  git -C `$n config core.fsmonitor "`$u/fsm"
  git -C `$n config user.name x; git -C `$n config user.email x@example.invalid
  Set-Content "`$n\x.txt" x
  git -C `$n add x.txt
}
'evil repos ready'
"@
  $r0 = Probe 'escape-setup' @(
    "Set-Content -Path .git -Value 'gitdir: a1/.git'",
    'Remove-Item -Force .git',
    $setup
  )
  Check '(a) the worktree .git file cannot be rewritten' ((Get-Content -Raw (Join-Path $Wt '.git')) -eq $gitBefore) $r0["Set-Content -Path .git -Value 'gitdir: a1/.git'"]
  Check '(a) the worktree .git file cannot be deleted' (Test-Path (Join-Path $Wt '.git')) $r0['Remove-Item -Force .git']
  Check 'nested repos with hooks were set up in the sandbox' ($r0[$setup] -match 'evil repos ready') $r0[$setup]
  Check 'nested repo payloads cannot fire inside the sandbox' (@(Get-ChildItem "$mk-*" -ErrorAction SilentlyContinue).Count -eq 0) ''
  function Set-SafeAll([bool]$on) {
    if ($on) { $env:GIT_CONFIG_COUNT = '1'; $env:GIT_CONFIG_KEY_0 = 'safe.directory'; $env:GIT_CONFIG_VALUE_0 = '*' }
    else { Remove-Item Env:GIT_CONFIG_COUNT, Env:GIT_CONFIG_KEY_0, Env:GIT_CONFIG_VALUE_0 -ErrorAction SilentlyContinue }
  }
  # Positive control: the same commit run unsandboxed by this script (with
  # safe.directory=*) fires both payloads, so a missing marker below means
  # "blocked", not "broken".
  Set-SafeAll $true
  Push-Location (Join-Path $Wt 'c0'); $ErrorActionPreference = 'Continue'; $ctl = (git commit -qm control 2>&1 | Out-String); $ErrorActionPreference = 'Stop'; Pop-Location
  Set-SafeAll $false
  Check 'control: the payloads fire when git runs unsandboxed' ((Test-Path "$mk-c0-hook.txt") -and (Test-Path "$mk-c0-fsm.txt")) ((@(Get-ChildItem "$mk-*" -ErrorAction SilentlyContinue).Name -join ', ') + " | $ctl")
  Remove-Item "$mk-c0-*" -ErrorAction SilentlyContinue
  foreach ($pass in @(@{ p = 'a'; safe = $false; what = 'as is' }, @{ p = 's'; safe = $true; what = 'safe.directory=*' })) {
    $p = $pass.p
    $attacks = @(
      @{ name = "commit in a nested repo (workdir) [$($pass.what)]"; cmd = "git commit -m ${p}1"; workdir = (Join-Path $Wt "${p}1"); repo = "${p}1" },
      @{ name = "add in a nested repo (workdir) [$($pass.what)]"; cmd = 'git add -A'; workdir = (Join-Path $Wt "${p}2"); repo = "${p}2" },
      @{ name = "cd into a nested repo, commit [$($pass.what)]"; cmd = "cd ${p}3; git commit -m ${p}3"; repo = "${p}3" },
      @{ name = "GIT_DIR=<nested> git commit [$($pass.what)]"; cmd = "`$env:GIT_DIR = '${p}4\.git'; `$env:GIT_WORK_TREE = '${p}4'; git commit -m ${p}4"; repo = "${p}4" },
      @{ name = "git --git-dir=<nested> commit [$($pass.what)]"; cmd = "git --git-dir=${p}5/.git --work-tree=${p}5 commit -m ${p}5"; repo = "${p}5" }
    )
    Set-SafeAll $pass.safe
    try { $r = Probe "escape-$p" $attacks } finally { Set-SafeAll $false }
    foreach ($t in $attacks) {
      $hit = @(Get-ChildItem "$mk-$($t.repo)-*" -ErrorAction SilentlyContinue)
      Check "(b/c) $($t.name): no hook/fsmonitor outside the sandbox" ($hit.Count -eq 0) ("markers: $($hit.Name -join ', ') | " + $r[$t.name])
    }
  }
  $r = Probe 'outside-paths' @("git add -- '$env:USERPROFILE\NTUSER.DAT'")
  $add = $r["git add -- '$env:USERPROFILE\NTUSER.DAT'"]
  Check '(c) git add of a path outside the worktree is refused' ((Code $add) -ne 0) $add
  Remove-Item "$mk-*" -ErrorAction SilentlyContinue

  # 4. Repository filters (docs/19 §18.5, re-score daemon-security-01): the
  # repository config routes *.dat through `sh clean.sh`, a script tracked
  # in the worktree. A real Pod worker (its own task worktree and attempt
  # token; the agent is an idle PowerShell, Codex runs the commands) rewrites
  # the script to write a marker OUTSIDE the worktree and commits through
  # `pod orch commit`. No marker may appear; a path the filter handles is
  # refused.
  $fm = Join-Path $env:USERPROFILE "pod-codex-filter-$PID.txt"
  $fmU = $fm.Replace('\', '/')
  [IO.File]::WriteAllText((Join-Path $Main 'clean.sh'), "echo clean >> `"$fmU`"; cat")
  [IO.File]::WriteAllText((Join-Path $Main '.gitattributes'), "*.dat filter=demo`n")
  [IO.File]::WriteAllText((Join-Path $Main 'a.dat'), "data`n")
  git -C $Main config filter.demo.clean 'sh clean.sh'; git -C $Main config filter.demo.smudge cat
  $ErrorActionPreference = 'Continue'
  git -C $Main add clean.sh .gitattributes a.dat 2>&1 | Out-Null; git -C $Main commit -qm filter 2>&1 | Out-Null
  $ErrorActionPreference = 'Stop'
  Check 'control: the filter runs on a plain git add' (Test-Path $fm) ''
  Remove-Item $fm -ErrorAction SilentlyContinue
  $pod = Join-Path $Bin 'pod.exe'
  & $pod repo add $Main --name probe | Out-Null
  # This isolated podd only: start the worker even on a busy machine.
  & $pod orch limits --set cpu_max_pct=off | Out-Null
  $coord = (& $pod task new probe coord --json -- powershell -NoProfile -Command 'Start-Sleep 3600') | ConvertFrom-Json
  $envFile = Join-Path $Root 'worker-env.txt'
  $workerPs1 = Join-Path $Root 'worker.ps1'
  [IO.File]::WriteAllText($workerPs1, "Get-ChildItem env:POD_*, env:GIT_CONFIG* | ForEach-Object { `$_.Name + '=' + `$_.Value } | Set-Content '$envFile'; Start-Sleep 3600")
  $env:POD_TERMINAL_ID = $coord.terminals[0].id
  try {
    # Windows PowerShell drops bare quotes from native arguments: escape them.
    $agent = (ConvertTo-Json -Compress @('powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $workerPs1)).Replace('"', '\"')
    $ErrorActionPreference = 'Continue'
    $wsRaw = (& $pod orch worker start --spec 'filter probe' --agent $agent --place new --json 2>&1 | Out-String)
    $ErrorActionPreference = 'Stop'
    Add-Content $Log "worker start: $wsRaw"
    $ws = $wsRaw | ConvertFrom-Json
  } finally { Remove-Item Env:POD_TERMINAL_ID -ErrorAction SilentlyContinue }
  for ($i = 0; $i -lt 600 -and -not (Test-Path $envFile); $i++) { Start-Sleep -Milliseconds 100 }
  $we = @{}; Get-Content $envFile | ForEach-Object { $k, $v = $_ -split '=', 2; $we[$k] = $v }
  $W2 = ((& $pod task show $ws.task --json) | ConvertFrom-Json).worktree
  Check 'a real worker with its own worktree started' ($we['POD_ATTEMPT_TOKEN'] -and (Test-Path (Join-Path $W2 'clean.sh'))) "$($ws | ConvertTo-Json -Compress)"
  # Creating the task worktrees checks out the base commit, which may run
  # the base commit's own (reviewed) filter: the §18.5 residual, not the
  # worker's code. Start the check from here.
  if (Test-Path $fm) { Note "note: task creation ran the base commit's filter: $(Get-Content $fm)" }
  Remove-Item $fm -ErrorAction SilentlyContinue
  $aBytes = [IO.File]::ReadAllBytes((Join-Path $W2 'a.dat'))
  $Wt0 = $Wt; $Wt = $W2; Install-Rules
  foreach ($k in 'POD_ATTEMPT_ID', 'POD_ATTEMPT_TOKEN', 'POD_TERMINAL_ID') { Set-Item "env:$k" $we[$k] }
  # The worker's git guard (docs/19 §18.5, r2-orch): Codex gets it the way a
  # Pod-started Codex would, from the worker's environment.
  $gk = @($we.Keys | Where-Object { $_ -like 'GIT_CONFIG*' })
  Check 'the worker env carries the git guard (filter demo emptied)' ([int]$we['GIT_CONFIG_COUNT'] -ge 6 -and ($we.Keys | Where-Object { $_ -like 'GIT_CONFIG_KEY_*' -and $we[$_] -eq 'filter.demo.clean' })) ($gk -join ',')
  foreach ($k in $gk) { Set-RawEnv $k $we[$k] }
  # Codex alone (no Pod command): Codex's own process runs git in the
  # worktree outside its sandbox; with the guard it runs no filter.
  $r0 = @{}
  try { $r0 = Probe 'filter-codex-only' @('echo hi', 'git config --get core.fsmonitor', 'git status --short') } catch { Note "codex-only probe: $_" }
  $codexRan = if (Test-Path $fm) { (Get-Content $fm) -join ',' } else { '' }
  Check 'first Codex session with the worker env ran no repository filter (no marker)' (-not $codexRan) $codexRan
  Check 'the guard reaches git inside the sandbox (fsmonitor false, git works)' ($r0['git config --get core.fsmonitor'] -match 'false' -and (Code $r0['git status --short']) -eq 0) "$($r0['git config --get core.fsmonitor']) | $($r0['git status --short'])"
  Remove-Item $fm -ErrorAction SilentlyContinue
  $evil = "Set-Content -NoNewline -Path clean.sh -Value 'echo pwned >> `"$fmU`"; cat'"
  try {
    $r = Probe 'filter' @($evil, 'pod orch commit -m filter-script clean.sh', 'Set-Content -Path a.dat -Value changed', 'pod orch commit -m data --all')
  } finally {
    Remove-Item Env:POD_ATTEMPT_ID, Env:POD_ATTEMPT_TOKEN, Env:POD_TERMINAL_ID -ErrorAction SilentlyContinue
    $Wt = $Wt0
  }
  $c1 = $r['pod orch commit -m filter-script clean.sh']; $c2 = $r['pod orch commit -m data --all']
  Check 'the worker rewrote the filter script (sandbox write)' ((Get-Content -Raw (Join-Path $W2 'clean.sh')) -match 'pwned') $r[$evil]
  Check 'pod orch commit of the rewritten script succeeds' ((Code $c1) -eq 0 -and (git -C $W2 log -1 --format=%s) -eq 'filter-script') $c1
  Check 'a path the filter handles is refused' ((Code $c2) -ne 0 -and $c2 -match 'filter') $c2
  # "clean" lines come from the Codex session start (the original script,
  # see the info line above); "pwned" would be the worker's code.
  $seen = (Get-Content $fm -ErrorAction SilentlyContinue) -join ','
  Check '(d) pod orch commit ran none of the worker''s filter code (no "pwned")' ($seen -notmatch 'pwned') $seen
  Remove-Item $fm -ErrorAction SilentlyContinue
  # Codex-side (not Pod's commit path): a new Codex session in this worktree
  # now that the script is the worker's.
  # (the checked-out bytes again: same size, new mtime, so git must compare
  # content, which a worker can arrange too)
  [IO.File]::WriteAllBytes((Join-Path $W2 'a.dat'), $aBytes)
  # (Codex again with the worker's identity and guard.)
  foreach ($k in 'POD_ATTEMPT_ID', 'POD_ATTEMPT_TOKEN', 'POD_TERMINAL_ID') { Set-Item "env:$k" $we[$k] }
  $Wt = $W2
  try { $null = Probe 'filter-codex-next' @('echo hi') } catch { Note "codex-next probe: $_" }
  finally {
    Remove-Item Env:POD_ATTEMPT_ID, Env:POD_ATTEMPT_TOKEN, Env:POD_TERMINAL_ID -ErrorAction SilentlyContinue
    $Wt = $Wt0
  }
  $next = (Get-Content $fm -ErrorAction SilentlyContinue) -join ','
  Check 'next Codex session ran none of the worker''s rewritten filter' (-not $next) $next
  foreach ($k in $gk) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
  Remove-Item $fm -ErrorAction SilentlyContinue
  $ErrorActionPreference = 'Continue'; git -C $W2 add a.dat 2>&1 | Out-Null; $ErrorActionPreference = 'Stop'
  Check 'control: the rewritten script is live for a plain git add' (Test-Path $fm) ''
  Remove-Item $fm -ErrorAction SilentlyContinue

} finally {
  Remove-Item $outside -ErrorAction SilentlyContinue
  Remove-Item (Join-Path $env:USERPROFILE "pod-codex-escape-$PID-*") -ErrorAction SilentlyContinue
  Remove-Item (Join-Path $env:USERPROFILE "pod-codex-filter-$PID.txt") -ErrorAction SilentlyContinue
  try { & (Join-Path $Bin 'pod.exe') server stop 2>$null | Out-Null } catch {}
  Start-Sleep -Milliseconds 500
  if (-not $podd.HasExited) { Stop-Process $podd.Id -Force -ErrorAction SilentlyContinue }
  # Terminal holders outlive podd by design: stop the probe's own (section 4).
  Get-CimInstance Win32_Process -Filter "Name='pod-holder.exe'" | Where-Object { $_.CommandLine -like "*$Name*" } |
    ForEach-Object { taskkill /T /F /PID $_.ProcessId 2>$null | Out-Null }
  Remove-Item Env:POD_SOCKET -ErrorAction SilentlyContinue
  Get-ChildItem env:GIT_CONFIG* -ErrorAction SilentlyContinue | ForEach-Object { Remove-Item "env:$($_.Name)" }
  Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}
Note $(if ($fails) { "RESULT: $fails FAILED" } else { 'RESULT: PASS' })
exit [int]($fails -gt 0)
