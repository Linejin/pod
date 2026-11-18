param(
  [Parameter(Mandatory=$true)][ValidatePattern('^[a-zA-Z0-9_.-]+$')][string]$Name,
  [Parameter(Mandatory=$true)][string]$File,
  [string[]]$Arguments = @(),
  [string]$WorkingDirectory = (Get-Location).Path,
  [Parameter(Mandatory=$true)][string]$OutputRoot,
  [switch]$Serialize,
  [ValidateSet('main','core-dependency')][string]$SerializationGroup = 'main',
  [int]$LockTimeoutMinutes = 30
)
$ErrorActionPreference = 'Stop'
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$WorkingDirectory = [IO.Path]::GetFullPath($WorkingDirectory)
$null = New-Item -ItemType Directory -Force -Path $OutputRoot
$started = [DateTime]::UtcNow
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$logPath = Join-Path $OutputRoot ($Name + '-' + $stamp + '.log')
$resultPath = Join-Path $OutputRoot ($Name + '-' + $stamp + '.json')
$mutex = $null
$acquired = $false
$exitCode = $null
$failure = $null
$sourceCommit = $null
$branch = $null
$dirty = @()
$sourceBefore = $null
$sourceAfter = $null
$process = $null
$outFile = $null
$errFile = $null
$stdoutPath = $logPath + '.stdout'
$stderrPath = $logPath + '.stderr'
$resources = $null

function Quote-NativeArgument([string]$Value) {
  $builder = New-Object System.Text.StringBuilder
  [void]$builder.Append([char]34)
  $slashes = 0
  foreach ($character in $Value.ToCharArray()) {
    if ($character -eq [char]92) { $slashes++; continue }
    if ($character -eq [char]34) {
      [void]$builder.Append([char]92, 2 * $slashes + 1)
      [void]$builder.Append([char]34)
    } else {
      if ($slashes -gt 0) { [void]$builder.Append([char]92, $slashes) }
      [void]$builder.Append($character)
    }
    $slashes = 0
  }
  if ($slashes -gt 0) { [void]$builder.Append([char]92, 2 * $slashes) }
  [void]$builder.Append([char]34)
  $builder.ToString()
}
function Source-Fingerprint {
  $patch = (& git diff --binary HEAD | Out-String)
  $untracked = @(& git ls-files --others --exclude-standard | ForEach-Object {
    if (Test-Path -LiteralPath $_ -PathType Leaf) { $_ + ':' + (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }
  })
  $hasher = [Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($patch + ($untracked -join "`n"))
    ([BitConverter]::ToString($hasher.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
  } finally { $hasher.Dispose() }
}
try {
  if ($SerializationGroup -eq 'core-dependency') {
    if (-not $Serialize -or $env:CARGO_BUILD_JOBS -ne '1') { throw 'Core dependency lane requires -Serialize and CARGO_BUILD_JOBS=1' }
    $os = Get-CimInstance Win32_OperatingSystem
    $cpu = Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average
    $resources = [pscustomobject]@{ free_gib=($os.FreePhysicalMemory / 1MB); cpu_percent=$cpu.Average }
    if ($resources.free_gib -lt 8 -or $resources.cpu_percent -gt 80) { throw 'Core dependency lane blocked by resource floor (8 GiB free, CPU <=80%)' }
  }
  if ($Serialize) {
    $mutexName = 'Local\PodFeatureValidation20261118'
    if ($SerializationGroup -eq 'core-dependency') { $mutexName += '-CoreDependency' }
    $mutex = New-Object System.Threading.Mutex($false, $mutexName)
    $deadline = [DateTime]::UtcNow.AddMinutes($LockTimeoutMinutes)
    while (-not $acquired) {
      try { $acquired = $mutex.WaitOne(10000) }
      catch [System.Threading.AbandonedMutexException] { $acquired = $true }
      if (-not $acquired) {
        Write-Host ('[qa] waiting for shared execution lock: ' + $Name)
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Shared execution lock timed out; command not started' }
      }
    }
  }
  Push-Location -LiteralPath $WorkingDirectory
  try {
    $sourceCommit = (& git rev-parse HEAD | Out-String).Trim()
    $branch = (& git branch --show-current | Out-String).Trim()
    $dirty = @(& git status --porcelain)
    $sourceBefore = Source-Fingerprint
    $resolved = Get-Command -Name $File -CommandType Application -ErrorAction Stop | Select-Object -First 1
    if ([IO.Path]::GetExtension($resolved.Source) -ne '.exe') { throw 'Use an .exe launcher; run PowerShell scripts through powershell.exe -File' }
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $resolved.Source
    $info.Arguments = (($Arguments | ForEach-Object { Quote-NativeArgument $_ }) -join ' ')
    $info.WorkingDirectory = $WorkingDirectory
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    $outFile = [IO.File]::Create($stdoutPath)
    $errFile = [IO.File]::Create($stderrPath)
    if (-not $process.Start()) { throw 'Child process did not start' }
    $outCopy = $process.StandardOutput.BaseStream.CopyToAsync($outFile)
    $errCopy = $process.StandardError.BaseStream.CopyToAsync($errFile)
    $process.WaitForExit()
    $exitCode = $process.ExitCode
    [void]$outCopy.GetAwaiter().GetResult()
    [void]$errCopy.GetAwaiter().GetResult()
    $outFile.Dispose(); $outFile = $null
    $errFile.Dispose(); $errFile = $null
    [IO.File]::WriteAllText($logPath, "[stdout]`n" + [IO.File]::ReadAllText($stdoutPath) + "`n[stderr]`n" + [IO.File]::ReadAllText($stderrPath), [Text.Encoding]::UTF8)
    $sourceAfter = Source-Fingerprint
    if ($sourceBefore -ne $sourceAfter -or $sourceCommit -ne (& git rev-parse HEAD | Out-String).Trim()) {
      $failure = 'Source changed during execution; preserve evidence and rerun with frozen source'
    }
  } finally { Pop-Location }
} catch {
  $failure = $_.Exception.Message
  $failure | Out-File -Append -Encoding utf8 $logPath
} finally {
  if ($outFile) { $outFile.Dispose() }
  if ($errFile) { $errFile.Dispose() }
  if ($process) { $process.Dispose() }
  if ($acquired) { $mutex.ReleaseMutex() }
  if ($mutex) { $mutex.Dispose() }
  $result = [ordered]@{
    executor_version = 3
    executor_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
    check_name = $Name
    source_commit = $sourceCommit
    branch = $branch
    dirty_files = $dirty
    source_fingerprint_before = $sourceBefore
    source_fingerprint_after = $sourceAfter
    working_directory = $WorkingDirectory
    file = $File
    arguments = $Arguments
    started_at = $started.ToString('o')
    finished_at = [DateTime]::UtcNow.ToString('o')
    exit_code = $exitCode
    check_status = $(if ($failure) { 'BLOCKED' } elseif ($exitCode -eq 0) { 'PASS' } else { 'FAIL' })
    error = $failure
    evidence = $logPath
    stdout_evidence = $stdoutPath
    stderr_evidence = $stderrPath
    serialized = [bool]$Serialize
    serialization_group = $SerializationGroup
    resource_snapshot = $resources
  }
  $result | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 $resultPath
  [pscustomobject]$result | Select-Object check_name,check_status,exit_code,source_commit,evidence | ConvertTo-Json -Compress
  Write-Host ('[qa] result: ' + $resultPath)
}
if ($failure) { exit 2 }
exit $exitCode
