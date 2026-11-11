# Run chosen podd integration tests for the daemon refactor
# (docs/20-refactor/daemon.md, card D-F0) without cargo inside the tests and
# without the user's real profile.
#
#   tools\test-daemon-refactor.ps1 -Test protocol_wire
#   tools\test-daemon-refactor.ps1 -Test mobile -Filter rate_limit
#   tools\test-daemon-refactor.ps1 -Test detach,stop_race -SkipBuild
#   tools\test-daemon-refactor.ps1 -List
#
# 1. Build (skipped with -SkipBuild). Before each cargo command: tools\cpu-gate.ps1
#    (-MaxCargo 1) and a free-memory floor. One command builds the fixture
#    binaries (pod, pod-hook, examples\fake-agent), one the chosen test targets
#    (--no-run). Nothing is built while tests run.
# 2. Verify: podd, pod-holder, the fixture binaries and every chosen test
#    executable exist in target\<profile>.
# 3. Run: each test executable directly (no cargo), from crates\podd, one at a
#    time, with -Filter passed to it. Under a fresh temp root: TEMP/TMP (so the
#    tests' own state dirs, worktrees, mods and agent stores), USERPROFILE/HOME
#    (with a fixture git identity), APPDATA, LOCALAPPDATA, POD_WORKTREES_ROOT,
#    POD_MODS_DIR and the session-discovery stores. USERNAME is set to a run id,
#    so a client without --pipe reaches \\.\pipe\podd-<run id>, never the user's
#    daemon. POD_SOCKET, POD_TERMINAL_ID, POD_DAEMON and POD_HOLDER are removed.
#    Limit: podd builds each terminal's environment from the user's registry
#    environment (crates\podd\src\env.rs), so programs inside test terminals see
#    the real profile paths; tests pass their own env for anything they write.
#
# The root is removed after a passing run (-Keep keeps it, a failing run keeps
# it and prints it). podd/pod-holder processes of this target dir started during
# the run and still alive at the end are reported.
#
# Every target uses the prebuilt fixtures (crates\podd\tests\common\fixtures.rs):
# `pr` moved there in P5, `status` in P6.
param(
  [string[]]$Test,
  [string]$Filter,
  [switch]$Exact,
  [switch]$SkipBuild,
  [switch]$Release,
  [switch]$Keep,
  [switch]$List,
  [int]$Threads = 0,
  [int]$MaxCpu = 70,
  [int]$MinFreeGB = 8
)
# Native commands write progress to stderr; Windows PowerShell 5.1 must not
# turn that into errors. Failures are checked through exit codes.
$ErrorActionPreference = 'Continue'
$repo = Split-Path -Parent $PSScriptRoot
$podd = Join-Path $repo 'crates\podd'

$targets = Get-ChildItem (Join-Path $podd 'tests') -Filter *.rs | ForEach-Object { $_.BaseName } | Sort-Object
# Targets that cannot run here yet (name = reason); none since P6.
$blocked = @{}
if ($List) {
  foreach ($t in $targets) {
    if ($blocked.ContainsKey($t)) { "{0,-24} (not runnable: {1})" -f $t, $blocked[$t] } else { $t }
  }
  exit 0
}
if (-not $Test) { throw 'name at least one test target with -Test (see -List)' }
$Test = @($Test | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
foreach ($t in $Test) {
  if ($targets -notcontains $t) { throw "unknown test target '$t' (see -List)" }
  if ($blocked.ContainsKey($t)) { throw "test target '$t' $($blocked[$t])" }
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$profileName = if ($Release) { 'release' } else { 'debug' }
$profileDir = Join-Path $targetDir $profileName
$profileArgs = if ($Release) { @('--release') } else { @() }

function Wait-Resources {
  & (Join-Path $PSScriptRoot 'cpu-gate.ps1') -MaxPct $MaxCpu -MaxCargo 1
  while (((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB) -lt $MinFreeGB) {
    Write-Host "[test-daemon-refactor] waiting for $MinFreeGB GB of free memory"
    Start-Sleep -Seconds 15
  }
}

# --- 1. build ---
$exes = @{}
if (-not $SkipBuild) {
  Push-Location $repo
  try {
    Wait-Resources
    & cargo build -p podd --example fake-agent -p pod-hook -p pod-cli --bin pod --bin pod-hook @profileArgs
    if ($LASTEXITCODE -ne 0) { throw 'building the fixture binaries failed' }
    Wait-Resources
    $testArgs = @('test', '-p', 'podd', '--no-run', '--message-format=json-render-diagnostics') + $profileArgs
    foreach ($t in $Test) { $testArgs += @('--test', $t) }
    $json = & cargo @testArgs
    if ($LASTEXITCODE -ne 0) { throw 'building the test targets failed' }
    foreach ($line in $json) {
      if (-not $line.StartsWith('{')) { continue }
      $m = $line | ConvertFrom-Json
      if ($m.reason -eq 'compiler-artifact' -and $m.executable -and $m.profile.test -and $Test -contains $m.target.name) {
        $exes[$m.target.name] = $m.executable
      }
    }
  } finally { Pop-Location }
} else {
  foreach ($t in $Test) {
    $found = Get-ChildItem (Join-Path $profileDir 'deps') -Filter "$t-*.exe" -ErrorAction SilentlyContinue |
      Where-Object { $_.BaseName -match "^$([regex]::Escape($t))-[0-9a-f]{16}$" } |
      Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($found) { $exes[$t] = $found.FullName }
  }
}

# --- 2. verify ---
$missing = @()
foreach ($rel in 'podd.exe', 'pod-holder.exe', 'pod.exe', 'pod-hook.exe', 'examples\fake-agent.exe') {
  $p = Join-Path $profileDir $rel
  if (Test-Path $p) { Write-Host ("[test-daemon-refactor] {0}  {1:yyyy-MM-dd HH:mm:ss}" -f $p, (Get-Item $p).LastWriteTime) }
  else { $missing += $p }
}
foreach ($t in $Test) { if (-not $exes[$t] -or -not (Test-Path $exes[$t])) { $missing += "test executable of '$t'" } }
if ($missing) { throw "missing (build without -SkipBuild): $($missing -join ', ')" }

# --- 3. run ---
$runId = "pod-rf-{0:yyyyMMddHHmmss}-{1}" -f (Get-Date), $PID
# $env:TEMP as given (not GetTempPath, which expands 8.3 names), so a run under
# tools/test-shortpath.ps1 really gives podd a short-name state directory.
$tempBase = if ($env:TEMP) { $env:TEMP } else { [IO.Path]::GetTempPath() }
$root = Join-Path $tempBase $runId
$dirs = @{
  tmp = 'tmp'; home = 'home'; roaming = 'roaming'; local = 'local'
  worktrees = 'worktrees'; mods = 'mods'; claude = 'agents\claude'; codex = 'agents\codex'
}
$d = @{}
foreach ($k in $dirs.Keys) { $d[$k] = Join-Path $root $dirs[$k]; New-Item -ItemType Directory -Force $d[$k] -ErrorAction Stop | Out-Null }
Set-Content -ErrorAction Stop -Encoding ascii (Join-Path $d.home '.gitconfig') "[user]`n`tname = Pod Test`n`temail = pod-test@example.invalid`n"

$set = @{
  TEMP = $d.tmp; TMP = $d.tmp; USERPROFILE = $d.home; HOME = $d.home
  APPDATA = $d.roaming; LOCALAPPDATA = $d.local; USERNAME = $runId
  POD_WORKTREES_ROOT = $d.worktrees; POD_MODS_DIR = $d.mods
  POD_DISCOVER_CLAUDE_DIR = $d.claude; POD_DISCOVER_CODEX_DIR = $d.codex; POD_DISCOVER_PROCESSES = 'off'
}
$remove = 'POD_SOCKET', 'POD_TERMINAL_ID', 'POD_DAEMON', 'POD_HOLDER'
$saved = @{}
foreach ($k in @($set.Keys) + $remove) { $saved[$k] = [Environment]::GetEnvironmentVariable($k, 'Process') }

$started = Get-Date
$failed = @()
Push-Location $podd
try {
  foreach ($k in $set.Keys) { [Environment]::SetEnvironmentVariable($k, $set[$k], 'Process') }
  foreach ($k in $remove) { [Environment]::SetEnvironmentVariable($k, $null, 'Process') }
  foreach ($t in $Test) {
    $runArgs = @()
    if ($Filter) { $runArgs += $Filter }
    if ($Exact) { $runArgs += '--exact' }
    if ($Threads -gt 0) { $runArgs += "--test-threads=$Threads" }
    Wait-Resources
    Write-Host "[test-daemon-refactor] $t $($runArgs -join ' ')"
    & $exes[$t] @runArgs
    if ($LASTEXITCODE -ne 0) { $failed += $t }
  }
} finally {
  foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], 'Process') }
  Pop-Location
}

$prefix = (Resolve-Path $profileDir).Path
$left = @(Get-CimInstance Win32_Process -Filter "Name='podd.exe' OR Name='pod-holder.exe'" |
  Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, 'OrdinalIgnoreCase') -and $_.CreationDate -ge $started })
foreach ($p in $left) { Write-Warning ("still running after the tests: {0} pid {1}" -f $p.Name, $p.ProcessId) }

if ($failed) {
  Write-Host "[test-daemon-refactor] FAILED: $($failed -join ', '); temp root kept: $root"
  exit 1
}
if ($Keep) { Write-Host "[test-daemon-refactor] temp root kept: $root" }
else { Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue }
Write-Host "[test-daemon-refactor] passed: $($Test -join ', ')"
exit 0
