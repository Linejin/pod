# Wait for CPU headroom before a heavy step (cargo build/test, app runs).
#
#   & C:\ai\ide\tools\cpu-gate.ps1                 # defaults: 1-min avg < 70%, < 2 cargo processes
#   & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 60 -MaxCargo 1
#
# Reads the status written by tools/cpu-governor.ps1. If the governor is not
# running (no fresh status), it measures CPU itself. Gives up waiting after
# -TimeoutMin and proceeds with a warning, so work is delayed, never blocked forever.
# Exit code is always 0.
param(
  [int]$MaxPct = 70,
  [int]$MaxCargo = 2,
  [int]$TimeoutMin = 30,
  [int]$PollSec = 10,
  [string]$StatusPath = "C:\ai\pod-target\cpu\status.json"
)
$ErrorActionPreference = "SilentlyContinue"
# Count cargo builds, but not the background `cargo check` that an editor's
# rust-analyzer (e.g. a Claude Code LSP plugin in a worker session) runs on save:
# those are short, low-priority-able and would otherwise block the gate forever.
function Get-BuildCargoCount {
  $procs = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name)
  $byId = @{}; foreach ($p in $procs) { $byId[[int]$p.ProcessId] = $p }
  $n = 0
  foreach ($p in $procs | Where-Object { $_.Name -eq 'cargo.exe' }) {
    $cur = $p; $fromRa = $false
    for ($i = 0; $i -lt 6 -and $cur; $i++) {
      $cur = $byId[[int]$cur.ParentProcessId]
      if ($cur -and $cur.Name -eq 'rust-analyzer.exe') { $fromRa = $true; break }
    }
    if (-not $fromRa) { $n++ }
  }
  $n
}
$deadline = (Get-Date).AddMinutes($TimeoutMin)
$waited = $false
while ($true) {
  $s = $null
  if (Test-Path $StatusPath) {
    $s = Get-Content $StatusPath -Raw | ConvertFrom-Json
    if ($s -and ((Get-Date) - [datetime]$s.time).TotalSeconds -gt 30) { $s = $null }  # stale
  }
  if (-not $s) {
    $v = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime
    $s = [pscustomobject]@{ cpu_avg_1m = [int]$v; cargo_processes = 0; verdict = "unknown (no governor)" }
  }
  $s.cargo_processes = Get-BuildCargoCount
  if ($s.cpu_avg_1m -lt $MaxPct -and $s.cargo_processes -lt $MaxCargo) {
    if ($waited) { Write-Output "[cpu-gate] go: cpu 1m=$($s.cpu_avg_1m)% cargo=$($s.cargo_processes)" }
    exit 0
  }
  if ((Get-Date) -gt $deadline) {
    Write-Output "[cpu-gate] timeout after $TimeoutMin min, proceeding anyway: cpu 1m=$($s.cpu_avg_1m)% cargo=$($s.cargo_processes)"
    exit 0
  }
  if (-not $waited) { Write-Output "[cpu-gate] waiting: cpu 1m=$($s.cpu_avg_1m)% (max $MaxPct) cargo=$($s.cargo_processes) (max $($MaxCargo - 1) others) ..." }
  $waited = $true
  Start-Sleep -Seconds $PollSec
}
