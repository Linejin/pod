# Publish the current master tree to the public repository as ONE new commit on
# `main`. Local master keeps the full private history and is never pushed.
#
#   powershell -ExecutionPolicy Bypass -File tools/publish-public.ps1 -Message "..." [-DryRun]
#   first publish into a new, empty repository: add -Root (a parentless commit;
#   the push is not forced, so it fails if the remote branch already exists).
#
# Checks before publishing: no private/ or vendor/conpty files in the tree, no
# match for the private blocked-pattern list (third-party reverse-engineering
# markers, private identifiers) and no match for the built-in generic secret
# patterns below (key headers, common token prefixes). This is a pattern scan,
# not a proof that nothing secret is in the tree. The commit uses the GitHub
# noreply identity so no personal or work e-mail becomes public.
# Tests: tools/test-publish-public.ps1 (temporary repository, no network).
param(
  [Parameter(Mandatory = $true)][string]$Message,
  [string]$Source = "master",
  [string]$Remote = "origin",
  [string]$AuthorName = "Linejin",
  [string]$AuthorEmail = "15919807+Linejin@users.noreply.github.com",
  [switch]$Root,
  [switch]$DryRun
)
$ErrorActionPreference = "Stop"
Set-Location (Resolve-Path (Join-Path $PSScriptRoot ".."))

# Runs git and returns its stdout lines; any exit code outside $Ok throws, so a
# failed git step (bad regex, missing ref, rejected push) never reads as success.
function Invoke-Git([string[]]$GitArgs, [int[]]$Ok = @(0)) {
  $old = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try { $out = & git @GitArgs 2>$null; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
  if ($Ok -notcontains $code) { throw "git $($GitArgs -join ' ') failed (exit $code)" }
  return $out
}

# Lines in $tree matching extended regex $pattern ("<tree>:<path>:<line>:<text>").
function Find-Matches([string]$tree, [string]$pattern) {
  return Invoke-Git @("grep", "-n", "-I", "-E", "-e", $pattern, $tree, "--", ":!tools/publish-public.ps1", ":!tools/test-publish-public.ps1") -Ok @(0, 1)
}

$tree = (Invoke-Git @("rev-parse", "$Source^{tree}")).Trim()
if ($AuthorEmail -notmatch '@users\.noreply\.github\.com$') { throw "author e-mail must be a GitHub noreply address" }
if ($Root) {
  $parent = $null
} else {
  $parent = (Invoke-Git @("rev-parse", "main")).Trim()
  if ((Invoke-Git @("rev-parse", "$parent^{tree}")).Trim() -eq $tree) { Write-Output "main already has this tree; nothing to publish."; exit 0 }
}

# Blocked patterns live in the git-ignored private/publish-block.txt (one extended
# regex per line) so the list itself, which names private identifiers, is never
# published. An empty or comment-only list is an error, not "nothing to check".
$patternFile = Join-Path "private" "publish-block.txt"
if (-not (Test-Path $patternFile)) { throw "missing $patternFile (blocked-pattern list); refusing to publish without it" }
$patterns = @(Get-Content $patternFile -Encoding UTF8 | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('#') })
if ($patterns.Count -eq 0) { throw "$patternFile has no patterns (only blank or comment lines); refusing to publish" }

# Generic secret shapes, independent of the private list.
$generic = @(
  '-----BEGIN [A-Z ]*PRIVATE KEY-----',
  'gh[pousr]_[A-Za-z0-9]{36}',
  'github_pat_[A-Za-z0-9_]{40,}',
  'sk-ant-[A-Za-z0-9_-]{20,}',
  'sk-(proj-)?[A-Za-z0-9]{32,}',
  'AKIA[0-9A-Z]{16}',
  'xox[abprs]-[A-Za-z0-9-]{10,}'
)

# Canary: the scanner must find a synthetic token in a throwaway tree, or the
# scan below proves nothing (e.g. git grep cannot read trees here).
# No pipes into git: PowerShell 5.1 may prepend a BOM to piped native input.
$canaryText = "token = " + "ghp_" + ("C" * 36)
$canaryDir = Join-Path ([IO.Path]::GetTempPath()) ("pod-canary-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $canaryDir | Out-Null
try {
  $canaryFile = Join-Path $canaryDir "canary.txt"
  [IO.File]::WriteAllText($canaryFile, $canaryText, (New-Object System.Text.UTF8Encoding $false))
  $canaryBlob = (Invoke-Git @("hash-object", "-w", $canaryFile)).Trim()
  $env:GIT_INDEX_FILE = Join-Path $canaryDir "index"
  Invoke-Git @("update-index", "--add", "--cacheinfo", "100644,$canaryBlob,canary.txt") | Out-Null
  $canaryTree = (Invoke-Git @("write-tree")).Trim()
} finally {
  Remove-Item Env:GIT_INDEX_FILE -ErrorAction SilentlyContinue
  Remove-Item -Recurse -Force $canaryDir -ErrorAction SilentlyContinue
}
if (-not (Find-Matches $canaryTree $generic[1])) { throw "scanner canary was not detected; refusing to publish" }

$bad = @()
$bad += Invoke-Git @("ls-tree", "-r", "--name-only", $tree) | Select-String -Pattern '^private/|^vendor/conpty/'
foreach ($p in $patterns + $generic) { $bad += Find-Matches $tree $p }
if ($bad.Count -gt 0) { $bad | Select-Object -First 20 | ForEach-Object { Write-Output "BLOCKED: $_" }; throw "publish blocked: fix the findings above" }

$env:GIT_AUTHOR_NAME = $AuthorName; $env:GIT_AUTHOR_EMAIL = $AuthorEmail
$env:GIT_COMMITTER_NAME = $env:GIT_AUTHOR_NAME; $env:GIT_COMMITTER_EMAIL = $env:GIT_AUTHOR_EMAIL
$body = "$Message`n`nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
$base = if ($parent) { $parent } else { "4b825dc642cb6eb9a060e54bf8d69288fbee4904" } # empty tree
if ($DryRun) { Write-Output "dry run: would commit tree $tree on top of $(if ($parent) { $parent } else { '(root)' }) as $AuthorName to $Remote ($($patterns.Count) private + $($generic.Count) generic patterns checked)"; git diff --stat $base $tree | Select-Object -Last 1; exit 0 }
$ctArgs = @("commit-tree", $tree, "-m", $body); if ($parent) { $ctArgs += @("-p", $parent) }
$sha = (Invoke-Git $ctArgs).Trim()
Invoke-Git @("push", $Remote, "${sha}:refs/heads/main") | Out-Null
# Move local main only after the remote accepted the commit.
Invoke-Git @("branch", "-f", "main", $sha) | Out-Null
Write-Output "published $sha"
