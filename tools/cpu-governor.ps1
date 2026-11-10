# CPU governor for parallel agent work on this PC.
#
# Runs in the background (one instance). Every few seconds it:
#   - samples total CPU and keeps 1-minute / 5-minute averages,
#   - drops build processes (cargo, rustc, linkers, build scripts, and anything
#     running from the shared target dir) to Idle priority so the user's own
#     apps stay responsive,
#   - writes a status file that tools/cpu-gate.ps1 and the coordinator read,
#   - appends a log line when the state changes or every 30 s.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\cpu-governor.ps1
#   (status: C:\ai\pod-target\cpu\status.json, log: ...\cpu\governor.log)
param(
  [int]$IntervalSec = 3,
  [string]$Dir = "C:\ai\pod-target\cpu",
  [int]$BusyPct = 70,
  [int]$OverloadPct = 90,
  [string[]]$TargetDirs = @("C:\ai\pod-target", "C:\ai\ide\target", "$env:USERPROFILE\orca\workspaces")
)
$ErrorActionPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force $Dir | Out-Null
$statusPath = Join-Path $Dir "status.json"
$logPath = Join-Path $Dir "governor.log"
$lockPath = Join-Path $Dir "governor.lock"

# Single instance: hold an exclusive handle on the lock file.
try { $lock = [System.IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None') }
catch { Write-Output "cpu-governor already running"; exit 0 }

$buildNames = @('cargo', 'rustc', 'link', 'lld-link', 'rust-lld', 'cl', 'cc1', 'clang', 'zig')
$samples = New-Object System.Collections.Generic.List[object]
$lastLog = [datetime]::MinValue
$lastVerdict = ""

function Log($msg) { Add-Content -Path $logPath -Value ("{0:yyyy-MM-dd HH:mm:ss} {1}" -f (Get-Date), $msg) -Encoding UTF8 }
Log "governor started (pid $PID, interval ${IntervalSec}s, busy>${BusyPct}%, overload>${OverloadPct}%)"

while ($true) {
  $now = Get-Date
  $cpu = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime
  if ($null -eq $cpu) { $cpu = 0 }
  $samples.Add([pscustomobject]@{ t = $now; v = [double]$cpu })
  while ($samples.Count -gt 0 -and ($now - $samples[0].t).TotalSeconds -gt 300) { $samples.RemoveAt(0) }
  $avg1 = ($samples | Where-Object { ($now - $_.t).TotalSeconds -le 60 } | Measure-Object v -Average).Average
  $avg5 = ($samples | Measure-Object v -Average).Average

  # Lower build processes to Idle.
  $builders = 0; $lowered = 0
  foreach ($p in Get-Process) {
    $isBuild = ($buildNames -contains $p.ProcessName) -or ($p.ProcessName -like 'build-script-*')
    if (-not $isBuild -and $p.Path) {
      foreach ($d in $TargetDirs) { if ($p.Path.StartsWith($d, [StringComparison]::OrdinalIgnoreCase) -and $p.Path -match '\\(debug|release)\\(deps|build)\\') { $isBuild = $true; break } }
    }
    if (-not $isBuild) { continue }
    if ($p.ProcessName -eq 'cargo') { $builders++ }
    try { if ($p.PriorityClass -ne 'Idle') { $p.PriorityClass = 'Idle'; $lowered++ } } catch {}
  }

  $verdict = if ($avg1 -ge $OverloadPct) { "overloaded" } elseif ($avg1 -ge $BusyPct) { "busy" } else { "ok" }
  $status = [ordered]@{
    time = $now.ToString("o"); cpu_now = [int]$cpu; cpu_avg_1m = [int]$avg1; cpu_avg_5m = [int]$avg5
    cargo_processes = $builders; verdict = $verdict; governor_pid = $PID
  }
  ($status | ConvertTo-Json -Compress) | Set-Content -Path $statusPath -Encoding UTF8

  if ($verdict -ne $lastVerdict -or ($now - $lastLog).TotalSeconds -ge 30 -or $lowered -gt 0) {
    $top = ""
    if ($verdict -ne "ok") {
      $top = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $_.Name -notin '_Total', 'Idle' } |
        Sort-Object PercentProcessorTime -Descending | Select-Object -First 5 | ForEach-Object { "$($_.Name)=$($_.PercentProcessorTime)" }) -join " "
    }
    Log ("{0} now={1}% 1m={2}% 5m={3}% cargo={4} lowered={5} {6}" -f $verdict, [int]$cpu, [int]$avg1, [int]$avg5, $builders, $lowered, $top)
    $lastLog = $now; $lastVerdict = $verdict
  }
  Start-Sleep -Seconds $IntervalSec
}
