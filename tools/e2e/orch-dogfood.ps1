# SPDX-License-Identifier: Apache-2.0
# docs/19 S8 dogfooding, DRY RUN by default: a Pod coordinator terminal starts
# two workers in new worktrees on a fixture repository; worker a1 asks a
# question and the coordinator answers; both workers commit a change and
# report `pod orch done`; the coordinator reads its mail and merges both
# branches into its own worktree. All agents are `fake-agent orch-loop` TUIs
# driven turn by turn (no Claude/Codex, no prompts, no network).
#
#   powershell -NoProfile -File tools/e2e/orch-dogfood.ps1 [-NoBuild] [-Keep]
#
# -RealAgents (needs the user's approval for the usage; first run docs/19
# §17): the coordinator is a real Claude Code (`pod task new --agent
# claude`) given the objective as a brief; it starts one Codex and one
# Claude worker itself, following `pod guide orchestration`. The script only
# sets up, answers Claude's folder-trust dialog for temp-root paths (logged),
# dumps `pod orch` state every minute, waits (up to -TimeoutMin) and checks
# the result. Real agents use the user's own agent login and configuration
# (Pod never writes them); Pod state, worktrees, the app config and mods stay
# under the temp root.
#   -Repo <path> [-Ref master]: work on `git clone --local` of that repository
#     (origin removed: nothing can be pushed) instead of the fixture. A setup
#     commit adds `.claude/settings.json` allowing `pod`/`git`/`cargo test -p`
#     in the clone only, so Claude does not stop on every command.
#   -Objective <file>: the coordinator's brief (default: the fixture brief).
#     Evidence (status/mail dumps, screens, diff stat) goes to -Out.
#
# Everything lives under a temp root and is removed at the end (-Keep keeps
# it); the user's real podd, %APPDATA%\Pod and agent configs are never
# written. The scenario log goes to -Out.
param(
  [string]$Out = 'target/e2e/orch-dogfood',
  [switch]$NoBuild,
  [switch]$Keep,
  [switch]$RealAgents,
  [string]$Repo,
  [string]$Ref = 'master',
  [string]$Objective,
  [int]$TimeoutMin = 45
)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '2'
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $RepoRoot
$Bin = Join-Path $RepoRoot 'target\debug'
if (-not $NoBuild) {
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 | Out-Host
  cargo build -p podd --bins --example fake-agent -p pod-cli --bin pod
  if ($LASTEXITCODE -ne 0) { throw 'build failed' }
}
$Evidence = Join-Path $RepoRoot $Out
New-Item -ItemType Directory -Force $Evidence | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Root = Join-Path $env:TEMP "pod-dogfood-$stamp"
$Name = "pod-dogfood-$stamp"
New-Item -ItemType Directory -Force $Root | Out-Null
$Log = Join-Path $Evidence 'scenario.log'
Set-Content -Encoding UTF8 $Log "orch-dogfood $stamp mode=$(if ($RealAgents) { 'real' } else { 'dry' })"
function Note([string]$m) { $line = "[$((Get-Date).ToString('HH:mm:ss'))] $m"; $line; Log $line }
# A reader (tail, an editor) may hold the log for a moment: retry the write.
function Log([string]$text) { foreach ($i in 1..20) { try { Add-Content -Encoding UTF8 $Log $text -ErrorAction Stop; return } catch { Start-Sleep -Milliseconds 100 } } }

# Isolation: own pipe, state, worktrees, app config, mods and discovery dirs.
$env:POD_SOCKET = $Name
$env:POD_APP_CONFIG_DIR = Join-Path $Root 'config'
$env:POD_MODS_DIR = Join-Path $Root 'mods'
$env:POD_DISCOVER_CLAUDE_DIR = Join-Path $Root 'stores\claude'
$env:POD_DISCOVER_CODEX_DIR = Join-Path $Root 'stores\codex'
$env:POD_DISCOVER_PROCESSES = 'off'
$env:POD_ORCH_RELEASE_AFTER = 'off'
$env:NO_COLOR = $null

$pod = Join-Path $Bin 'pod.exe'
$fake = Join-Path $Bin 'examples\fake-agent.exe'
function Pod { $o = & $pod @args 2>&1; $code = $LASTEXITCODE; Log ("pod " + ($args -join ' ') + " -> $code`n" + ($o -join "`n")); if ($code -ne 0) { throw "pod $($args -join ' ') failed ($code)" }; $o -join "`n" }
function Until([string]$what, [int]$secs, [scriptblock]$cond) {
  $deadline = (Get-Date).AddSeconds($secs)
  while (-not (& $cond)) { if ((Get-Date) -gt $deadline) { throw "timeout: $what" }; Start-Sleep -Milliseconds 250 }
  Note "ok: $what"
}
function Check([bool]$ok, [string]$what) { if (-not $ok) { throw "check failed: $what" }; Note "ok: $what" }
$Base = Join-Path $Root 'agents'
New-Item -ItemType Directory -Force $Base | Out-Null
Set-Content -Encoding ASCII (Join-Path $Base 'pod-exe.txt') $pod
function Turn([string]$who, [object[]]$cmds) {
  # One fake-agent turn: it runs these argv lists (`pod ...`, `!git ...`,
  # `!write file text`) with its own terminal env, in its worktree.
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
function OutOf([string]$who, [int]$k, [int]$secs = 90) {
  $p = Join-Path (Join-Path $Base $who) "out-$k.json"
  Until "$who out-$k" $secs { Test-Path $p }
  $o = Get-Content -Raw -Encoding UTF8 $p | ConvertFrom-Json
  Log ("  $who> " + ($o.args -join ' ') + " -> $($o.code)")
  $o
}
function Ok([string]$who, [int]$k) { $o = OutOf $who $k; if ($o.code -ne 0) { throw "$who $($o.args -join ' ') failed: $($o.stderr)" }; $o }
function Mail([string]$run) { (Pod orch check --run $run --no-ack --json | ConvertFrom-Json).messages }

$SpecA = @"
In the fixture repository, add the file greeting.txt with one line: hello from worker A.
Commit it on your branch with the message "add greeting".
Done when: greeting.txt is committed. Ask the coordinator (pod orch ask) whether the greeting should end with a period before you commit.
"@
$SpecB = @"
In the fixture repository, add docs/usage.md with a two-line usage note for greeting.txt.
Commit it on your branch with the message "add usage".
Done when: docs/usage.md is committed. Do not touch other files.
"@
$FixtureObjective = @"
You coordinate two Pod workers on this fixture repository (it is a throwaway test repository).
1. Read `pod guide orchestration --lang en` and follow it.
2. Start worker A with codex and worker B with claude (`pod orch worker start --agent codex|claude --spec-file ...`), using these specs:
--- spec A ---
$SpecA
--- spec B ---
$SpecB
3. Answer their questions (`pod orch check`, `pod orch reply`). End your turn while waiting; do not poll.
4. When both reported done, merge both branches into your worktree with `git merge --no-edit pod/<name>` and report what you merged.
"@

$podd = $null
try {
  # podd (isolated) and the fixture repository.
  $podd = Start-Process -FilePath (Join-Path $Bin 'podd.exe') -NoNewWindow -PassThru -ArgumentList @('--pipe', "\\.\pipe\$Name", '--holder', (Join-Path $Bin 'pod-holder.exe'), '--state-dir', (Join-Path $Root 'state'), '--worktrees-root', (Join-Path $Root 'worktrees')) -RedirectStandardOutput (Join-Path $Root 'podd.out') -RedirectStandardError (Join-Path $Root 'podd.err')
  Until 'podd pipe' 20 { Test-Path "\\.\pipe\$Name" }
  $RepoDir = Join-Path $Root 'repo'
  $setup = $null
  if ($Repo) {
    # A throwaway clone: origin removed so nothing can be pushed; the source
    # repository and its branches are never touched.
    git clone -q --local --branch $Ref $Repo $RepoDir
    if ($LASTEXITCODE -ne 0) { throw "clone of $Repo failed" }
    git -C $RepoDir remote remove origin
    git -C $RepoDir config user.name 'Pod Test'; git -C $RepoDir config user.email 'pod-test@example.invalid'; git -C $RepoDir config commit.gpgsign false
    # Claude's project permissions for the clone only (never ~/.claude).
    New-Item -ItemType Directory -Force (Join-Path $RepoDir '.claude') | Out-Null
    $allow = @('pod', 'pod.exe', 'git', 'cargo test -p') | ForEach-Object { "Bash($_`:*)"; "PowerShell($_`:*)" }
    $settings = ConvertTo-Json -Depth 4 @{ permissions = @{ allow = @($allow) + @('Edit', 'Write') } }
    [IO.File]::WriteAllText((Join-Path $RepoDir '.claude\settings.json'), $settings, (New-Object Text.UTF8Encoding $false))
    git -C $RepoDir add .claude/settings.json; git -C $RepoDir commit -q -m 'dogfood setup: agent permissions in this clone (not part of the change)'
    $setup = (git -C $RepoDir rev-parse HEAD).Trim()
    $repoName = 'pod'
    Note "clone of $Repo at $Ref, setup commit $setup"
  } else {
    New-Item -ItemType Directory -Force (Join-Path $RepoDir 'docs') | Out-Null
    git -C $RepoDir init -q -b main; git -C $RepoDir config user.name 'Pod Test'; git -C $RepoDir config user.email 'pod-test@example.invalid'; git -C $RepoDir config commit.gpgsign false
    Set-Content -Encoding UTF8 (Join-Path $RepoDir 'README.md') 'fixture repository for the Pod dogfood scenario'
    Set-Content -Encoding UTF8 (Join-Path $RepoDir 'docs\index.md') '# docs'
    git -C $RepoDir add -A; git -C $RepoDir commit -q -m init
    $repoName = 'fixture'
  }
  Pod repo add $RepoDir --name $repoName | Out-Null
  $guide = Pod guide orchestration --lang en
  Check ($guide -match 'pod orch done' -and $guide -match 'protocol v') 'the guide prints, version-stamped'

  if ($RealAgents) {
    Note 'REAL AGENTS: Claude Code coordinator, Codex + Claude workers (uses the user''s agent subscriptions)'
    $brief = Join-Path $Root 'objective.md'
    if ($Objective) { Copy-Item $Objective $brief } else { Set-Content -Encoding UTF8 $brief $FixtureObjective }
    $t0 = Get-Date
    $coord = Pod task new $repoName coord --agent claude --brief-file $brief --json | ConvertFrom-Json
    Note "coordinator task $($coord.id) terminal $($coord.terminals[0].id) worktree $($coord.worktree)"
    $deadline = $t0.AddMinutes($TimeoutMin)
    $wt = $coord.worktree
    $leaf = Split-Path -Leaf $Root
    $answered = @{}
    $mergedAt = $null
    $quiet = 0
    $lastSummary = ''
    $n = 0
    $passed = $false
    while ($true) {
      if ((Get-Date) -gt $deadline) { Note "TIMEOUT after $TimeoutMin min"; break }
      Start-Sleep -Seconds 8
      $n++
      # Claude's folder-trust dialog, answered only for temp-root folders.
      $terms = @((& $pod ls --json 2>$null | ConvertFrom-Json).terminals)
      foreach ($t in $terms) {
        $screen = (& $pod read $t.id 2>$null) -join "`n"
        if ($screen -match 'Yes, I trust this folder' -and -not $answered[$t.id]) {
          if ($screen -match [regex]::Escape($leaf) -and $t.cwd -like "$Root*") {
            # The dialog's default is "No, exit": move the cursor to "Yes"
            # (checked on screen) before Enter, never Enter on "No".
            foreach ($i in 1..3) {
              if (((& $pod read $t.id 2>$null) -join "`n") -match '❯ Yes, I trust this folder') { break }
              & $pod keys $t.id down | Out-Null; Start-Sleep -Milliseconds 300
            }
            if (((& $pod read $t.id 2>$null) -join "`n") -match '❯ Yes, I trust this folder') {
              & $pod keys $t.id enter | Out-Null
              $answered[$t.id] = $true
              Note "answered Claude's folder-trust dialog in $($t.id) ($($t.cwd)): Yes"
            } else { Note "trust dialog in $($t.id): could not select Yes, NOT answered" }
          } else { Note "trust dialog in $($t.id) outside the temp root: NOT answered ($($t.cwd))" }
        }
        if ($screen -match 'Do you want to proceed\?|Allow command|approve this') { Note "permission prompt waiting in $($t.id): $((($screen -split "`n") | Select-String 'proceed|Allow|approve' | Select-Object -First 2) -join ' | ')" }
      }
      $runs = @((& $pod orch run ls --json 2>$null | ConvertFrom-Json).runs)
      $summary = ($runs | ForEach-Object { "$($_.run.id): " + (@($_.items | ForEach-Object { "$($_.id)=$($_.state)" }) -join ' ') }) -join '; '
      if ($summary -ne $lastSummary) { Note "runs: $summary"; $lastSummary = $summary }
      if ($n % 8 -eq 0) { & $pod orch workers 2>&1 | Out-File -Encoding utf8 (Join-Path $Evidence 'workers-latest.txt') }
      # Done: two completed items, both worker branches merged into the
      # coordinator's worktree, and the coordinator's turn over (it still
      # runs the test and reports after merging): not working for 2 polls.
      $done = @($runs | ForEach-Object { $_.items } | Where-Object { $_.state -eq 'completed' })
      if ($done.Count -ge 2) {
        $merged = @(git -C $wt branch --merged HEAD --format '%(refname:short)' 2>$null | Where-Object { $_ -like 'pod/*' -and $_ -ne "pod/coord" })
        if ($merged.Count -ge 2) {
          if (-not $mergedAt) { $mergedAt = Get-Date; Note "merged: $($merged -join ', ')" }
          $ct = $terms | Where-Object { $_.id -eq $coord.terminals[0].id }
          $busy = $ct -and $ct.status -and ($ct.status.state -in @('working', 'blocked'))
          if ($busy) { $quiet = 0 } else { $quiet++ }
          if ($quiet -ge 2) { Note "coordinator turn over ($([math]::Round(((Get-Date) - $mergedAt).TotalMinutes, 1)) min after the merge)"; $passed = $true; break }
        }
      }
    }
    $mins = [math]::Round(((Get-Date) - $t0).TotalMinutes, 1)
    Note "elapsed $mins min"
    # Evidence: run, mail (bodies trimmed), workers, screens, diff stat.
    # All mail of a run, read or not, from its log (`check` shows unread only).
    function AllMail([string]$run) {
      $f = Join-Path $Root "state\orch\runs\$run\mail.jsonl"
      if (-not (Test-Path $f)) { return @() }
      Get-Content -Encoding UTF8 $f | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.kind -eq 'mail' } | ForEach-Object { $_.mail }
    }
    foreach ($r in $runs) {
      $id = $r.run.id
      & $pod orch run show $id 2>&1 | Out-File -Encoding utf8 (Join-Path $Evidence "run-$id.txt")
      AllMail $id | ForEach-Object {
        $b = ([string]$_.body) -replace '\s+', ' '
        "$([DateTimeOffset]::FromUnixTimeMilliseconds($_.created_ms).ToLocalTime().ToString('HH:mm:ss')) $($_.id) $($_.from)->$($_.to) [$($_.type)] $($_.subject)`n    " + $b.Substring(0, [math]::Min(240, $b.Length))
      } | Out-File -Encoding utf8 (Join-Path $Evidence "mail-$id.txt")
    }
    & $pod orch workers 2>&1 | Out-File -Encoding utf8 (Join-Path $Evidence 'workers.txt')
    foreach ($t in @((& $pod ls --all --json 2>$null | ConvertFrom-Json).terminals)) {
      & $pod read $t.id --history --lines 120 2>&1 | Out-File -Encoding utf8 (Join-Path $Evidence "screen-$($t.id).txt")
    }
    $diffBase = if ($setup) { $setup } else { (git -C $wt rev-list --max-parents=0 HEAD | Select-Object -First 1) }
    "coordinator branch log:`n$(git -C $wt log --oneline --graph -12 | Out-String)`ndiff --stat $diffBase..HEAD:`n$(git -C $wt diff --stat $diffBase HEAD | Out-String)" | Out-File -Encoding utf8 (Join-Path $Evidence 'diffstat.txt')
    $m = @($runs | ForEach-Object { AllMail $_.run.id })
    Note "mail: $(@($m | Where-Object { $_.type -eq 'question' }).Count) question(s), $(@($m | Where-Object { $_.type -eq 'answer' }).Count) answer(s), $(@($m | Where-Object { $_.type -eq 'done' }).Count) done, $(@($m | Where-Object { $_.type -eq 'escalation' }).Count) escalation(s)"
    if (-not $passed) { throw "real-agent scenario did not finish (see $Evidence)" }
    Check (@($m | Where-Object { $_.type -eq 'done' }).Count -ge 2) 'two done reports'
    Check (@($m | Where-Object { $_.type -eq 'question' }).Count -ge 1) 'a worker asked a question'
    Note 'scenario passed (real agents)'
    return
  }

  # DRY RUN with fake agents.
  $agent = ConvertTo-Json -Compress @($fake, "orch-loop:$Base")
  $coord = Pod task new fixture coord --json '--' $fake "orch-loop:$Base" | ConvertFrom-Json
  Note "coordinator task $($coord.id) terminal $($coord.terminals[0].id)"
  Until 'coordinator agent' 30 { Test-Path (Join-Path $Base 'coord') }
  $specA = Join-Path $Root 'spec-a.md'; Set-Content -Encoding UTF8 $specA $SpecA
  $specB = Join-Path $Root 'spec-b.md'; Set-Content -Encoding UTF8 $specB $SpecB

  # 1. The coordinator reads the guide and starts two workers in new worktrees.
  $k = Turn 'coord' @(
    @('guide', 'orchestration', '--lang', 'en'),
    @('orch', 'run', 'new', '--objective', 'greeting + usage (dogfood dry run)', '--json'),
    @('orch', 'worker', 'start', '--spec-file', $specA, '--title', 'add greeting', '--agent', $agent, '--json'),
    @('orch', 'worker', 'start', '--spec-file', $specB, '--title', 'add usage', '--agent', $agent, '--json'))
  [void](Ok 'coord' $k); [void](Ok 'coord' ($k + 1))
  $wa = (Ok 'coord' ($k + 2)).stdout | ConvertFrom-Json
  $wb = (Ok 'coord' ($k + 3)).stdout | ConvertFrom-Json
  Check ($wa.stage -eq 'started' -and $wb.stage -eq 'started') 'both workers started (new worktrees)'
  Until 'worker agents' 30 { (Test-Path (Join-Path $Base $wa.attempt.id)) -and (Test-Path (Join-Path $Base $wb.attempt.id)) }
  $a1 = $wa.attempt.id; $a2 = $wb.attempt.id
  $taskA = Pod task show $wa.task --json | ConvertFrom-Json
  $taskB = Pod task show $wb.task --json | ConvertFrom-Json
  Check ($taskA.worktree -ne $taskB.worktree -and $taskA.worktree -ne $coord.worktree) 'separate worktrees'

  # 2. a1 asks (blocks); b works and reports.
  $qa = Turn $a1 @(, @('orch', 'ask', '--question', 'Should the greeting end with a period?', '--json'))
  $kb = Turn $a2 @(
    @('!write', 'docs/usage.md', "Usage`ncat greeting.txt`n"),
    @('!git', 'add', '-A'),
    @('!git', 'commit', '-q', '-m', 'add usage'),
    @('orch', 'done', '--outcome', 'succeeded', '--body', 'did: docs/usage.md / found: nothing / left: nothing', '--files', 'docs/usage.md', '--json'))
  foreach ($i in 0..3) { [void](Ok $a2 ($kb + $i)) }
  Until 'question recorded' 30 { @(Mail 'r1' | Where-Object { $_.type -eq 'question' }).Count -ge 1 }

  # 3. The coordinator is woken (a `pod mail:` line), reads and answers.
  Until 'coordinator woken' 30 { (Get-Content -Raw -ErrorAction SilentlyContinue (Join-Path $Base 'coord\submits.log')) -match 'pod mail: ' }
  $c = Turn 'coord' @(, @('orch', 'check', '--json'))
  $mail = ((Ok 'coord' $c).stdout | ConvertFrom-Json).messages
  $q = $mail | Where-Object { $_.type -eq 'question' } | Select-Object -First 1
  Check ($null -ne $q) 'the question reached the coordinator'
  $r = Turn 'coord' @(, @('orch', 'reply', '--id', $q.id, '--body', 'yes, end it with a period', '--json'))
  [void](Ok 'coord' $r)
  $ans = Ok $a1 $qa
  Check ((($ans.stdout | ConvertFrom-Json).answer.body) -match 'period') 'worker A got the answer'

  # 4. a1 does the work and reports.
  $ka = Turn $a1 @(
    @('!write', 'greeting.txt', "hello from worker A.`n"),
    @('!git', 'add', '-A'),
    @('!git', 'commit', '-q', '-m', 'add greeting'),
    @('orch', 'done', '--outcome', 'succeeded', '--body', 'did: greeting.txt (with a period, as answered) / found: nothing / left: nothing', '--files', 'greeting.txt', '--json'))
  foreach ($i in 0..3) { [void](Ok $a1 ($ka + $i)) }
  Until 'both items completed' 30 { $run = Pod orch run show r1 --json | ConvertFrom-Json; @($run.items | Where-Object { $_.state -eq 'completed' }).Count -eq 2 }

  # 5. The coordinator reads the reports and merges both branches.
  $c = Turn 'coord' @(
    @('orch', 'check', '--json'),
    @('task', 'diff', $wa.task, '--json'),
    @('!git', 'merge', '--no-edit', "pod/$($taskA.name)"),
    @('!git', 'merge', '--no-edit', "pod/$($taskB.name)"))
  # Worker B reported before the coordinator's first check: count both reads.
  $reports = @($mail) + @(((Ok 'coord' $c).stdout | ConvertFrom-Json).messages) | Where-Object { $_.type -eq 'done' }
  Check (@($reports).Count -eq 2) 'two done reports'
  Check (@($reports | Where-Object { $_.text.kind -eq 'done.report' }).Count -eq 2) 'done reports carry structured text'
  [void](Ok 'coord' ($c + 1)); [void](Ok 'coord' ($c + 2)); [void](Ok 'coord' ($c + 3))
  Check ((Test-Path (Join-Path $coord.worktree 'greeting.txt')) -and (Test-Path (Join-Path $coord.worktree 'docs\usage.md'))) 'both changes merged into the coordinator worktree'
  $history = git -C $coord.worktree log --oneline -5 | Out-String
  Note "coordinator branch:`n$history"
  $esc = @(Mail 'r1' | Where-Object { $_.type -eq 'escalation' })
  Note "escalations: $($esc.Count) ($(@($esc | ForEach-Object { $_.text.kind }) -join ', '))"
  $workers = Pod orch workers --run r1
  Note "workers:`n$workers"
  Note 'scenario passed (dry run)'
}
finally {
  # Only this scenario's processes (by path and start time), never another podd.
  # Real agents run under pod-holder: end each holder's whole process tree.
  if ($RealAgents) {
    Get-Process pod-holder -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($Bin) -and ($_.StartTime -ge (Get-Item $Root).CreationTime) } | ForEach-Object { taskkill /T /F /PID $_.Id 2>&1 | Out-Null }
  }
  Get-Process podd, pod-holder, fake-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($Bin) -and ($_.StartTime -ge (Get-Item $Root).CreationTime) } | Stop-Process -Force -ErrorAction SilentlyContinue
  if (-not $Keep) { Start-Sleep -Milliseconds 500; Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue }
}
