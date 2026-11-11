# Workspace policy check (no cargo run needed):
# - every member except the spikes is a default member (quality-14), so plain
#   `cargo build/test/clippy` and CI cover every shipped crate;
# - every non-spike crate opts into the workspace lints (quality-10);
# - every crate inherits rust-version (quality-11);
# - a new crate gets the same rules: every crates/<name>/Cargo.toml is a member
#   (or listed in [workspace] exclude) and members are listed one by one, so no
#   crate stays outside both CI and this check.
#   powershell -ExecutionPolicy Bypass -File tools/check-workspace.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-workspace.ps1 -SelfTest
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$SelfTest)
$ErrorActionPreference = "Stop"
$spikes = @("crates/pod-spike", "crates/diff-spike")

function Test-Workspace([string]$root) {
  $toml = Get-Content (Join-Path $root "Cargo.toml") -Raw
  function List([string]$key) {
    $m = [regex]::Match($toml, "(?m)^$key\s*=\s*\[([^\]]*)\]")
    if (-not $m.Success) { return @() }
    return @([regex]::Matches($m.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
  }
  $members = List "members"
  $default = List "default-members"
  $exclude = List "exclude"
  $fail = @()
  if ($members.Count -eq 0) { $fail += "no [workspace] members found" }
  if ($toml -notmatch '(?m)^rust-version\s*=\s*"\d+\.\d+') { $fail += "[workspace.package] has no rust-version" }
  foreach ($m in $members) {
    if ($m -match '[*?\[]') { $fail += "$m is a glob: list each member so this check reads its Cargo.toml"; continue }
    $isSpike = $spikes -contains $m
    if ($isSpike -and $default -contains $m) { $fail += "$m is a spike but a default member" }
    if (-not $isSpike -and $default -notcontains $m) { $fail += "$m is missing from default-members" }
    $path = Join-Path $root "$m\Cargo.toml"
    if (-not (Test-Path $path)) { $fail += "$m has no Cargo.toml"; continue }
    $ct = Get-Content $path -Raw
    if ($ct -notmatch '(?m)^rust-version\.workspace\s*=\s*true') { $fail += "$m does not inherit rust-version" }
    if (-not $isSpike -and $ct -notmatch '(?m)^\[lints\]\s*\r?\n\s*workspace\s*=\s*true') { $fail += "$m does not opt into [lints] workspace = true" }
  }
  foreach ($d in $default) { if ($members -notcontains $d) { $fail += "default member $d is not a member" } }
  $cratesDir = Join-Path $root "crates"
  if (Test-Path $cratesDir) {
    foreach ($c in Get-ChildItem $cratesDir -Directory | Sort-Object Name) {
      $rel = "crates/$($c.Name)"
      if ((Test-Path (Join-Path $c.FullName "Cargo.toml")) -and $members -notcontains $rel -and $exclude -notcontains $rel) {
        $fail += "$rel has a Cargo.toml but is not a workspace member (add it to members, or to exclude)"
      }
    }
  }
  return @{ Fail = $fail; Members = $members.Count; Default = $default.Count }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-workspace-" + [Guid]::NewGuid().ToString("N"))
  $ok = "[package]`nname = `"x`"`nrust-version.workspace = true`n`n[lints]`nworkspace = true"
  $pkg = "[workspace.package]`nrust-version = `"1.99`""
  $ws = "[workspace]`nmembers = [`"crates/pod-spike`", `"crates/app`", `"crates/new`"]`ndefault-members = [`"crates/app`", `"crates/new`"]`nexclude = [`"crates/old`"]`n$pkg"
  $good = @{ "crates/pod-spike" = "[package]`nname = `"pod-spike`"`nrust-version.workspace = true"; "crates/app" = $ok; "crates/new" = $ok; "crates/old" = "[package]`nname = `"old`"" }
  # Each case: workspace manifest, crate manifest changes, expected FAIL texts.
  $cases = @(
    @{ Name = "good (spike without lints, excluded crate, folder without a manifest)"; Ws = $ws; Set = @{ "crates/notes" = $null }; Expect = @() },
    @{ Name = "new member without lints or rust-version"; Ws = $ws
       Set = @{ "crates/new" = "[package]`nname = `"new`"`nrust-version = `"1.80`"`n[lints.clippy]`ntodo = `"warn`"" }
       Expect = @("crates/new does not inherit rust-version", "crates/new does not opt into [lints] workspace = true") },
    @{ Name = "new member outside default-members, spike inside"
       Ws = "[workspace]`nmembers = [`"crates/pod-spike`", `"crates/app`", `"crates/new`"]`ndefault-members = [`"crates/pod-spike`", `"crates/app`", `"crates/gone`"]`nexclude = [`"crates/old`"]`n$pkg"
       Set = @{}
       Expect = @("crates/pod-spike is a spike but a default member", "crates/new is missing from default-members", "default member crates/gone is not a member") },
    @{ Name = "new crate folder that is not a member"; Ws = "[workspace]`nmembers = [`"crates/pod-spike`", `"crates/app`"]`ndefault-members = [`"crates/app`"]`n$pkg"; Set = @{}
       Expect = @("crates/new has a Cargo.toml but is not a workspace member (add it to members, or to exclude)",
                  "crates/old has a Cargo.toml but is not a workspace member (add it to members, or to exclude)") },
    @{ Name = "glob member, member without a manifest, no rust-version"
       Ws = "[workspace]`nmembers = [`"crates/pod-spike`", `"crates/app`", `"crates/new`", `"crates/*`", `"crates/notes`"]`ndefault-members = [`"crates/app`", `"crates/new`", `"crates/*`", `"crates/notes`"]`nexclude = [`"crates/old`"]"
       Set = @{ "crates/notes" = $null }
       Expect = @("[workspace.package] has no rust-version", "crates/* is a glob: list each member so this check reads its Cargo.toml", "crates/notes has no Cargo.toml") }
  )
  $bad = @()
  try {
    foreach ($c in $cases) {
      $dir = Join-Path $tmp ([Guid]::NewGuid().ToString("N"))
      [void](New-Item -ItemType Directory -Force $dir)
      Set-Content (Join-Path $dir "Cargo.toml") $c.Ws -Encoding UTF8
      $files = @{}; foreach ($k in $good.Keys) { $files[$k] = $good[$k] }; foreach ($k in $c.Set.Keys) { $files[$k] = $c.Set[$k] }
      foreach ($k in $files.Keys) {
        [void](New-Item -ItemType Directory -Force (Join-Path $dir $k))
        if ($null -ne $files[$k]) { Set-Content (Join-Path $dir "$k\Cargo.toml") $files[$k] -Encoding UTF8 }
      }
      $got = @((Test-Workspace $dir).Fail | Sort-Object) -join " | "
      $want = @($c.Expect | Sort-Object) -join " | "
      if ($got -cne $want) { $bad += "case '$($c.Name)': got [$got], want [$want]" }
    }
  } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
  if ($bad.Count -gt 0) { $bad | ForEach-Object { Write-Output "SELFTEST FAIL: $_" }; exit 1 }
  Write-Output "check-workspace selftest ok ($($cases.Count) cases)"
  exit 0
}

$r = Test-Workspace (Resolve-Path $Root).Path
if ($r.Fail.Count -gt 0) { $r.Fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "workspace policy ok ($($r.Members) members, $($r.Default) default)"
