# Tests for tools/publish-public.ps1 in a throwaway repository under %TEMP%:
# no network, no real pattern list, only synthetic strings (security-06, quality-12).
#   powershell -ExecutionPolicy Bypass -File tools/test-publish-public.ps1
$ErrorActionPreference = "Stop"
$script = Join-Path $PSScriptRoot "publish-public.ps1"
$root = Join-Path $env:TEMP ("pod-publish-test-" + [guid]::NewGuid().ToString("N"))
$failures = @()

function InRepo([string]$dir) { $a = $args; Push-Location $dir; try { & git.exe -c core.autocrlf=false @a 2>&1 | Out-Null } finally { Pop-Location } }

# Fresh repo: main = one public commit, master = main + $files; $block = pattern list (null = no file).
function New-Repo([hashtable]$files, $block) {
  $dir = Join-Path $root ([guid]::NewGuid().ToString("N").Substring(0, 8))
  New-Item -ItemType Directory -Force (Join-Path $dir "tools") | Out-Null
  Copy-Item $script (Join-Path $dir "tools\publish-public.ps1")
  InRepo $dir init -q -b main
  InRepo $dir config user.name test; InRepo $dir config user.email test@example.invalid
  Set-Content (Join-Path $dir "README.md") "public" -Encoding ASCII
  InRepo $dir add README.md tools; InRepo $dir commit -q -m public
  InRepo $dir checkout -q -b master
  foreach ($k in $files.Keys) { Set-Content (Join-Path $dir $k) $files[$k] -Encoding ASCII; InRepo $dir add $k }
  InRepo $dir commit -q -m change
  if ($null -ne $block) {
    New-Item -ItemType Directory -Force (Join-Path $dir "private") | Out-Null
    Set-Content (Join-Path $dir "private\publish-block.txt") $block -Encoding UTF8
  }
  return $dir
}

function Run([string]$dir, [switch]$Real, [string[]]$Extra = @()) {
  $a = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $dir "tools\publish-public.ps1"), "-Message", "test") + $Extra
  if (-not $Real) { $a += "-DryRun" }
  $old = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try { $out = (& powershell @a 2>&1 | Out-String) } finally { $ErrorActionPreference = $old }
  return @{ Code = $LASTEXITCODE; Out = $out }
}

function Expect([string]$name, $r, [bool]$ok, [string]$text) {
  $pass = (($r.Code -eq 0) -eq $ok) -and ($r.Out -match [regex]::Escape($text))
  if ($pass) { Write-Output "ok   $name" } else { Write-Output "FAIL $name (exit $($r.Code))`n$($r.Out)"; $script:failures += $name }
}

try {
  $clean = @{ "a.txt" = "hello" }
  Expect "missing list refuses" (Run (New-Repo $clean $null)) $false "missing private"
  Expect "empty list refuses" (Run (New-Repo $clean "")) $false "has no patterns"
  Expect "comment-only list refuses" (Run (New-Repo $clean @("# only a comment", "   ", "#another"))) $false "has no patterns"
  Expect "invalid regex refuses" (Run (New-Repo $clean @("foo[", "bar"))) $false "failed (exit"
  Expect "private pattern blocks" (Run (New-Repo @{ "a.txt" = "see SECRETMARKER here" } @("SECRET[M]ARKER"))) $false "BLOCKED:"
  $fake = "ghp_" + ("A" * 36)
  Expect "generic token blocks" (Run (New-Repo @{ "a.txt" = "token=$fake" } @("unused-pattern-xyz"))) $false "BLOCKED:"
  Expect "clean tree dry run passes" (Run (New-Repo $clean @("# list", "unused-pattern-xyz"))) $true "dry run: would commit tree"

  # Real run whose push fails (origin points nowhere): must fail and leave local main alone.
  $dir = New-Repo $clean @("unused-pattern-xyz")
  InRepo $dir remote add origin (Join-Path $root "no-such-remote.git")
  $before = (& git -C $dir rev-parse main).Trim()
  $r = Run $dir -Real
  Expect "failed push is an error" $r $false "failed (exit"
  if ($r.Out -match "published ") { Write-Output "FAIL failed push printed 'published'"; $failures += "push-published" }
  if ((& git -C $dir rev-parse main).Trim() -ne $before) { Write-Output "FAIL local main moved after failed push"; $failures += "main-moved" }
  # -Root: a parentless commit into an empty bare remote, authored by the noreply identity.
  $dir = New-Repo $clean @("unused-pattern-xyz")
  $bare = Join-Path $root "empty-remote.git"; & git init -q --bare $bare 2>&1 | Out-Null
  InRepo $dir remote add pub $bare
  $r = Run $dir -Real -Extra @("-Root", "-Remote", "pub")
  Expect "root publish into an empty remote" $r $true "published "
  $log = (& git -C $bare log --format="%P|%an|%ae" main 2>&1 | Out-String).Trim()
  if ($log -ne "|Linejin|15919807+Linejin@users.noreply.github.com") { Write-Output "FAIL root commit parents/author: $log"; $failures += "root-commit" }
  $r = Run $dir -Real -Extra @("-Root", "-Remote", "pub")
  Expect "second root publish is refused (no force push)" $r $false "failed (exit"
  Expect "non-noreply author refuses" (Run $dir -Extra @("-AuthorEmail", "someone@example.com")) $false "noreply"
} finally {
  Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}
if ($failures.Count -gt 0) { Write-Output "$($failures.Count) failed"; exit 1 }
Write-Output "all publish-public tests passed"
