# Orchestration demo data (docs/09 §8) on the isolated demo daemon of drive.ps1.
# Dot-source after drive.ps1 and review-demo.ps1 (PostMessage input helpers):
#   . crates/pod-app/tools/drive.ps1; . crates/pod-app/tools/review-demo.ps1; . crates/pod-app/tools/orch-demo.ps1
#   New-OrchRepo            # repo "orchdemo" with tools/member.ps1 (the scripted "agent")
#   New-LeadTask            # task orchdemo/lead (shell) — the parent terminal for the agent tree
#   Start-SubGroup          # inside the lead terminal: `pod group new orchdemo sub ...` (members get the lead as parent)
#
# No real Claude/Codex turn: every member is a PowerShell "shell" agent whose
# prompt runs tools/member.ps1. The script reports OSC 7501 states like an
# agent, edits files in its worktree, and member 3 asks for approval from
# inside its terminal with `pod gate request ... --wait`.
$OrchRepo = Join-Path $Demo "orchdemo"
$PodExe = Join-Path $Rel "pod.exe"

$MemberScript = @'
# Scripted stand-in for an agent (orchestration demo). Arg: path of pod.exe.
param([string]$Pod = "pod")
$e = [char]27; $bel = [char]7
function Report([string]$p) { [Console]::Out.Write("$e]7501;$p$bel"); [Console]::Out.Flush() }
$me = $env:POD_TASK_ID
$n = [int]($me.Substring($me.LastIndexOf('-') + 1))
Report "state=working:app=member"
"[$me] working on the prompt..."
Start-Sleep (3 + 3 * $n)
Set-Content -Encoding utf8 result.txt "result of $me"
Add-Content -Encoding utf8 src\lib.rs "pub fn answer_$n() -> u32 { $(40 + $n) }"
if ($n -eq 2) {
  Add-Content -Encoding utf8 result.txt "PASS all checks"
  Set-Content -Encoding utf8 src\extra.rs "// helper added by $me`npub fn extra() {}"
}
if ($n -eq 3) {
  Report "state=blocked:kind=permission:app=member"
  "[$me] asking for approval (pod gate request --wait)..."
  & $Pod gate request "Member $n wants to run the migration on the staging DB. OK?" --title "Run migration" --expires 900 --wait
  if ($LASTEXITCODE -eq 0) { "[$me] approved, continuing" ; Add-Content result.txt "migration approved" } else { "[$me] not approved" }
  Report "state=working:app=member"
  Start-Sleep 2
}
"[$me] done: $(Get-Content result.txt -Raw)"
Report "state=done:app=member"
'@

function New-OrchRepo {
  if (Test-Path $OrchRepo) { Remove-Item -Recurse -Force $OrchRepo }
  New-Item -ItemType Directory -Force (Join-Path $OrchRepo "src"), (Join-Path $OrchRepo "tools") | Out-Null
  Set-Content -Encoding utf8 (Join-Path $OrchRepo "README.md") "# orchdemo`n`nFan-out demo repository.`n"
  Set-Content -Encoding utf8 (Join-Path $OrchRepo "src\lib.rs") "//! Demo crate.`npub fn answer() -> u32 { 42 }`n"
  Set-Content -Encoding utf8 (Join-Path $OrchRepo "tools\member.ps1") $MemberScript
  Git-In $OrchRepo init -q -b main
  Git-In $OrchRepo config user.email demo@example.com
  Git-In $OrchRepo config user.name demo
  Git-In $OrchRepo add .
  Git-In $OrchRepo commit -q -m "base"
  Pod repo add $OrchRepo --name orchdemo | Out-Null
}

# The prompt every member gets (typed into its PowerShell).
function Member-Prompt { "powershell -NoProfile -ExecutionPolicy Bypass -File tools\member.ps1 '$PodExe'" }

function New-LeadTask {
  Pod task new orchdemo lead --agent shell | Out-Null
}

# From inside the lead's terminal: a nested group (its members record the lead terminal as parent).
function Start-SubGroup {
  $lead = (Pod task show orchdemo/lead --json | ConvertFrom-Json).terminals[0].id
  # A script file keeps the quoting simple (PowerShell 5.1 mangles embedded
  # double quotes in native arguments).
  $script = Join-Path $Demo "start-sub.ps1"
  Set-Content -Encoding utf8 $script "& '$PodExe' group new orchdemo sub --prompt `"$(Member-Prompt)`" --agent shell --agent shell --agent shell -- cmd /c findstr /c:PASS result.txt"
  & $PodExe send $lead "& '$script'" | Out-Null
  return $lead
}
