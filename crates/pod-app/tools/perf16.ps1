# 16 podd terminals each flooding colored output (pod-spike's stress script),
# all attached in pod-app, fps measured like pod-spike (docs/02 §6).
#   & crates/pod-app/tools/perf16.ps1 [-Count 16] [-Duration 30]
param([int]$Count = 16, [int]$Duration = 30, [int]$Batch = 50, [int]$SleepMs = 16)
. (Join-Path $PSScriptRoot "drive.ps1")
$ErrorActionPreference = "Continue"
Stop-PodApp
# Clean slate on the demo daemon. Careful: a `pod` call that auto-starts podd
# while its stdout is a pipe hangs the pipe reader, because the detached podd
# inherits that handle. So only stop a running daemon here, and start the
# fresh one with Start-Process (no pipes).
if ((Pod server status | Out-String) -match "running") { Pod server stop --kill | Out-Null; Start-Sleep 1 }
# (Not `-Wait`: that also waits for descendants, i.e. the detached podd.)
$starter = Start-Process -FilePath (Join-Path $Rel "pod.exe") -ArgumentList "ls" -WindowStyle Hidden -PassThru
$null = $starter.WaitForExit(15000)
Remove-Item -Recurse -Force $env:POD_APP_CONFIG_DIR -ErrorAction SilentlyContinue
$stress = 'while($true){$e=[char]27;$b=[System.Text.StringBuilder]::new();for($k=0;$k -lt __BATCH__;$k++){$script:i++;[void]$b.Append("$e[3$($i%7+1)m$($i.ToString().PadLeft(8))$e[0m $e[1mstep$e[0m tool_call read_file src/lib.rs:$($i%997) $e[2m$([guid]::NewGuid())$e[0m`n")};[Console]::Out.Write($b.ToString());Start-Sleep -Milliseconds __SLEEP__}'
$stress = $stress.Replace("__BATCH__", "$Batch").Replace("__SLEEP__", "$SleepMs")
# A script file: Windows PowerShell 5.1 strips the inner quotes of a -Command argument.
$flood = Join-Path $Demo "flood.ps1"
Set-Content -Path $flood -Value $stress -Encoding UTF8
for ($n = 1; $n -le $Count; $n++) {
  Pod run --name "flood$n" '--' powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $flood | Out-Null
}
Pod ls | Select-Object -First 3
$log = "perf$Count.out"
$p = Start-Process -FilePath (Join-Path $Rel "pod-app.exe") -ArgumentList @("--open-all", "--no-throttle", "--stats", "--duration", "$Duration") -PassThru -RedirectStandardOutput (Join-Path $Captures $log)
$t0 = Get-Date
while (((Get-Date) - $t0).TotalSeconds -lt 12) { Start-Sleep -Milliseconds 200 }
Shot "perf$Count-t12" | Out-Null
$null = $p.WaitForExit(($Duration + 30) * 1000)
Get-Content (Join-Path $Captures $log) | Select-String "stats|summary"
Pod ls --json | ConvertFrom-Json | ForEach-Object { $_.terminals } | Measure-Object -Property attached -Sum | Select-Object Count, Sum
# Stop the floods so they do not load the machine after the measurement.
Pod server stop --kill | Out-Null
