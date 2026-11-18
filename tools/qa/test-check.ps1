param([string]$OutputRoot = '')
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path (Split-Path $PSScriptRoot)
if (-not $OutputRoot) { $OutputRoot = Join-Path $repoRoot ('target/qa-executor-selftest/' + [guid]::NewGuid().ToString('N')) }
$null = New-Item -ItemType Directory -Path $OutputRoot -Force
$fixture = Join-Path $OutputRoot 'fixture'
$null = New-Item -ItemType Directory -Path $fixture
& git -C $fixture init -q -b qa-fixture
Set-Content -Encoding ascii (Join-Path $fixture 'source.txt') 'baseline'
& git -C $fixture add -- source.txt
& git -C $fixture -c user.name=QaFixture -c user.email=qa@example.invalid commit -qm baseline
$child = Join-Path $OutputRoot 'child with spaces.ps1'
@'
param([string]$Text, [string]$PathValue, [int]$Result, [string]$Mutate)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
[pscustomobject]@{ text=$Text; path=$PathValue } | ConvertTo-Json -Compress
[Console]::Error.WriteLine('stderr independently preserved')
if ($Mutate) { Add-Content -LiteralPath $Mutate -Value 'changed' }
exit $Result
'@ | Set-Content -LiteralPath $child -Encoding UTF8
$text = '한글 spaces and "quotes"'
$path = 'C:\path with spaces\tail\'
$executor = Join-Path $PSScriptRoot 'invoke-check.ps1'
foreach ($case in @(@{name='pass'; code=0}, @{name='fail'; code=7}, @{name='changed'; code=0})) {
  $arguments = @('-NoProfile','-File',$child,'-Text',$text,'-PathValue',$path,'-Result',[string]$case.code)
  if ($case.name -eq 'changed') { $arguments += @('-Mutate',(Join-Path $fixture 'source.txt')) }
  & $executor -Name $case.name -File powershell.exe -Arguments $arguments -WorkingDirectory $fixture -OutputRoot $OutputRoot
  $recordFile = Get-ChildItem -LiteralPath $OutputRoot -Filter ($case.name + '-*.json') | Sort-Object LastWriteTime -Descending | Select-Object -First 1
  $record = Get-Content -LiteralPath $recordFile.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
  $expectedStatus = if ($case.name -eq 'changed') { 'BLOCKED' } elseif ($case.code -eq 0) { 'PASS' } else { 'FAIL' }
  if ($record.exit_code -ne $case.code -or $record.check_status -ne $expectedStatus) { throw ('Exit/status mismatch: ' + $case.name) }
  $received = Get-Content -LiteralPath $record.stdout_evidence -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($received.text -ne $text -or $received.path -ne $path) { throw ('Argument roundtrip failed: ' + $case.name) }
  if ((Get-Content -LiteralPath $record.stderr_evidence -Raw -Encoding UTF8) -notmatch 'stderr independently preserved') { throw 'Stderr missing' }
  if ($case.name -eq 'changed' -and $record.source_fingerprint_before -eq $record.source_fingerprint_after) { throw 'Changed source was not detected' }
}
$savedJobs = [Environment]::GetEnvironmentVariable('CARGO_BUILD_JOBS', 'Process')
try {
  $env:CARGO_BUILD_JOBS = '2'
  & $executor -Name invalid-core-lane -File powershell.exe -Arguments @('-NoProfile','-Command','exit 0') -WorkingDirectory $fixture -OutputRoot $OutputRoot -Serialize -SerializationGroup core-dependency
  $recordFile = Get-ChildItem -LiteralPath $OutputRoot -Filter 'invalid-core-lane-*.json' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
  $record = Get-Content -LiteralPath $recordFile.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($record.check_status -ne 'BLOCKED' -or $null -ne $record.exit_code -or $record.error -notmatch 'CARGO_BUILD_JOBS=1') { throw 'Unsafe core lane was not blocked before child start' }
} finally { [Environment]::SetEnvironmentVariable('CARGO_BUILD_JOBS', $savedJobs, 'Process') }
Write-Host ('PASS: exit 0/7, Unicode/quoting, streams, changed-source and core-resource guard; evidence: ' + $OutputRoot)
exit 0
