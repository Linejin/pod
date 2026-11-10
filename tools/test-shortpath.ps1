# Run a command with TEMP/TMP spelled as a Windows 8.3 short name, the way
# GitHub's windows-latest runners do (C:\Users\RUNNER~1\AppData\Local\Temp).
# Paths built from the temp folder then reach Pod in the short spelling while
# git, canonicalize and most tools print the long one.
#
#   tools\test-shortpath.ps1                                   # cargo test --workspace --no-fail-fast
#   tools\test-shortpath.ps1 cargo test -p podd --test tasks
#   tools\test-shortpath.ps1 cargo test -p pod-editor '--' --nocapture
#   tools\test-shortpath.ps1 -Root D:\scratch\ShortNameTempRoot cargo test -p pod-cli
#
# 1. Creates the long-named folder (-Root, default <temp>\PodShortPathTemp).
# 2. Resolves its 8.3 short name and stops when the volume has none
#    (8.3 names off: `fsutil 8dot3name query C:`).
# 3. Runs the command with TEMP and TMP set to the short spelling, then
#    restores them. Exit code is the command's.
#
# No param() block on purpose: arguments such as -p go to the command
# unchanged. PowerShell drops a bare `--`; quote it ('--').
$ErrorActionPreference = 'Stop'

$root = Join-Path ([IO.Path]::GetTempPath()) 'PodShortPathTemp'
$cmd = @($args)
if ($cmd.Count -ge 2 -and $cmd[0] -eq '-Root') {
  $root = [string]$cmd[1]
  $cmd = @($cmd | Select-Object -Skip 2)
}
if ($cmd.Count -eq 0) {
  $cmd = @('cargo', 'test', '--workspace', '--no-fail-fast')
}

New-Item -ItemType Directory -Force -Path $root | Out-Null
$long = (Resolve-Path -LiteralPath $root).ProviderPath
$short = (New-Object -ComObject Scripting.FileSystemObject).GetFolder($long).ShortPath
if (-not $short -or $short -eq $long -or (Split-Path -Leaf $short) -notmatch '~') {
  $drive = (Split-Path -Qualifier $long)
  [Console]::Error.WriteLine(("test-shortpath: {0} has no 8.3 short name (8.3 names may be off on {1}: " +
      "fsutil 8dot3name query {1}). Pass -Root with a folder on a volume that has them.") -f $long, $drive)
  exit 2
}
Write-Host "test-shortpath: TEMP/TMP = $short (long: $long)"

$oldTemp, $oldTmp = $env:TEMP, $env:TMP
$env:TEMP = $short
$env:TMP = $short
try {
  # Windows PowerShell wraps a native command's stderr in error records when
  # output is redirected; under 'Stop' cargo's progress lines would abort us,
  # and printed as records they get "At ..." noise. Pass them on as text.
  $ErrorActionPreference = 'Continue'
  $exe = $cmd[0]
  $rest = @($cmd | Select-Object -Skip 1)
  & $exe @rest 2>&1 | ForEach-Object {
    if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { $_ }
  }
  $code = $LASTEXITCODE
} finally {
  $env:TEMP, $env:TMP = $oldTemp, $oldTmp
}
Write-Host "test-shortpath: exit $code"
exit $code
