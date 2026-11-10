# SPDX-License-Identifier: Apache-2.0
param([int]$Seconds = 180)
. (Join-Path $PSScriptRoot 'session.ps1')
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $env:POD_SOCKET, [System.IO.Pipes.PipeDirection]::InOut)
$pipe.Connect(5000)
$writer = New-Object System.IO.StreamWriter($pipe, (New-Object System.Text.UTF8Encoding($false)))
$reader = New-Object System.IO.StreamReader($pipe)
$writer.AutoFlush = $true
$writer.WriteLine('{"jsonrpc":"2.0","id":1,"method":"status.subscribe","params":{}}')
$until = (Get-Date).AddSeconds($Seconds)
try {
 while ((Get-Date) -lt $until) {
  $pending = $reader.ReadLineAsync()
  while (!$pending.Wait(500)) { if ((Get-Date) -ge $until) { return } }
  if (!$pending.Result) { break }
  $entry = @{observed_utc=[DateTimeOffset]::UtcNow.ToString('o');event=($pending.Result | ConvertFrom-Json)}
  $entry | ConvertTo-Json -Compress -Depth 40 | Add-Content -Encoding UTF8 (Join-Path $Evidence 'status-events.jsonl')
 }
} finally { $pipe.Dispose() }
