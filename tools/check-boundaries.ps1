# Crate boundary check (no cargo run needed), docs/20-refactor/libs.md section 2:
# - pod-test-support is a dev-dependency only: no crate lists it under
#   [dependencies], [build-dependencies] or a target's (build-)dependencies;
# - the leaf crates pod-process, pod-storage and pod-test-support depend on no
#   other workspace crate (no reverse edge into a domain crate) and not on
#   GPUI; process and storage may use pod-test-support for their own tests;
# - the spikes stay workspace members (`cargo build -p pod-spike` keeps
#   working) but out of default-members, and no other crate depends on them,
#   so plain `cargo build/test/clippy` never builds them (quality-14);
# - edges the refactor removed stay removed: pod-editor takes bounded runs
#   from pod-process, not pod-forge (card T1), in any dependency table.
# A dependency counts under its package name (`x = { package = "..." }` too),
# in inline and [dependencies.x] table form. Reads manifests only.
#   powershell -ExecutionPolicy Bypass -File tools/check-boundaries.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-boundaries.ps1 -SelfTest
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$SelfTest)
$ErrorActionPreference = "Stop"

$spikes = @("crates/pod-spike", "crates/diff-spike")
$leaves = @("pod-process", "pod-storage", "pod-test-support")
$testSupport = "pod-test-support"
# Package -> packages it must not depend on (removed edges).
$forbidden = @{ "pod-editor" = @("pod-forge") }

function Get-List([string]$toml, [string]$key) {
  $m = [regex]::Match($toml, "(?m)^$key\s*=\s*\[([^\]]*)\]")
  if (-not $m.Success) { return @() }
  return @([regex]::Matches($m.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
}

# Drops a trailing `# comment` that is outside a string.
function Remove-Comment([string]$line) {
  $quote = [char]0
  for ($i = 0; $i -lt $line.Length; $i++) {
    $c = $line[$i]
    if ($quote -ne [char]0) { if ($c -eq $quote) { $quote = [char]0 } }
    elseif ($c -eq '"' -or $c -eq "'") { $quote = $c }
    elseif ($c -eq '#') { return $line.Substring(0, $i) }
  }
  return $line
}

# Package name and dependencies of one manifest. Each dependency is
# @{ Kind = dependencies|dev-dependencies|build-dependencies; Section; Key; Package }.
function Read-Manifest([string]$text) {
  $name = $null; $deps = New-Object System.Collections.ArrayList
  $section = ""; $kind = $null; $table = $null
  $kinds = "dev-dependencies|build-dependencies|dependencies"
  foreach ($raw in ($text -split "\r?\n")) {
    $line = (Remove-Comment $raw).Trim()
    if (-not $line) { continue }
    $h = [regex]::Match($line, '^\[([^\[\]]+)\]$')
    if ($h.Success) {
      $section = $h.Groups[1].Value.Trim(); $kind = $null; $table = $null
      $s = [regex]::Match($section, "^(?:target\..+\.)?($kinds)$")
      $t = [regex]::Match($section, "^(?:target\..+\.)?($kinds)\.(.+)$")
      if ($s.Success) { $kind = $s.Groups[1].Value }
      elseif ($t.Success) {
        $key = $t.Groups[2].Value.Trim('"', "'")
        $table = @{ Kind = $t.Groups[1].Value; Section = $section; Key = $key; Package = $key }
        [void]$deps.Add($table)
      }
      continue
    }
    if ($line.StartsWith("[")) { $section = ""; $kind = $null; $table = $null; continue }
    $e = [regex]::Match($line, '^("[^"]+"|''[^'']+''|[A-Za-z0-9_-]+)(\.workspace)?\s*=\s*(.*)$')
    if (-not $e.Success) { continue }
    $key = $e.Groups[1].Value.Trim('"', "'"); $value = $e.Groups[3].Value
    if ($section -eq "package" -and $key -eq "name") { $name = $value.Trim().Trim('"', "'") }
    elseif ($table -and $key -eq "package") { $table.Package = $value.Trim().Trim('"', "'") }
    elseif ($kind) {
      $p = [regex]::Match($value, 'package\s*=\s*["'']([^"'']+)["'']')
      $pkg = if ($p.Success) { $p.Groups[1].Value } else { $key }
      [void]$deps.Add(@{ Kind = $kind; Section = $section; Key = $key; Package = $pkg })
    }
  }
  return @{ Name = $name; Deps = @($deps) }
}

# Every boundary violation under $root, as "FAIL" message bodies.
function Test-Boundaries([string]$root) {
  $toml = Get-Content (Join-Path $root "Cargo.toml") -Raw
  $members = Get-List $toml "members"
  $default = Get-List $toml "default-members"
  $fail = @()
  $crates = @{}
  foreach ($m in $members) {
    $path = Join-Path $root "$m\Cargo.toml"
    if (-not (Test-Path $path)) { $fail += "$m has no Cargo.toml"; continue }
    $man = Read-Manifest (Get-Content $path -Raw)
    if (-not $man.Name) { $fail += "$m has no [package] name"; continue }
    $crates[$m] = $man
  }
  $names = @($crates.Values | ForEach-Object { $_.Name })
  $spikeNames = @($spikes | Where-Object { $crates.ContainsKey($_) } | ForEach-Object { $crates[$_].Name })
  foreach ($s in $spikes) {
    if ($members -notcontains $s) { $fail += "$s is not a workspace member (cargo build -p $(Split-Path $s -Leaf) would stop working)" }
    if ($default -contains $s) { $fail += "$s is a spike but a default member" }
  }
  foreach ($m in $crates.Keys | Sort-Object) {
    $man = $crates[$m]
    foreach ($d in $man.Deps) {
      $what = "$m [$($d.Section)] $($d.Key)"
      if ($d.Package -ne $d.Key) { $what += " (package $($d.Package))" }
      if ($d.Package -eq $testSupport -and $d.Kind -ne "dev-dependencies") {
        $fail += "$what`: $testSupport is a dev-dependency only"
      }
      if ($forbidden.ContainsKey($man.Name) -and $forbidden[$man.Name] -contains $d.Package) {
        $fail += "$what`: $($man.Name) must not depend on $($d.Package) (removed edge)"
      }
      if ($spikeNames -contains $d.Package -and $spikes -notcontains $m) {
        $fail += "$what`: depends on a spike, which plain cargo build/test would then build"
      }
      if ($leaves -contains $man.Name) {
        $devTestSupport = $d.Package -eq $testSupport -and $d.Kind -eq "dev-dependencies" -and $man.Name -ne $testSupport
        if ($names -contains $d.Package -and -not $devTestSupport) {
          $fail += "$what`: leaf crate $($man.Name) depends on workspace crate $($d.Package)"
        }
        if ($d.Package -match '^gpui' -or $d.Key -match '^gpui') {
          $fail += "$what`: leaf crate $($man.Name) depends on GPUI"
        }
      }
    }
  }
  foreach ($l in $leaves) { if ($names -notcontains $l) { $fail += "leaf crate $l is not a workspace member" } }
  return @{ Fail = $fail; Members = $members.Count }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-boundaries-" + [Guid]::NewGuid().ToString("N"))
  $wsGood = @'
[workspace]
members = ["crates/pod-spike", "crates/diff-spike", "crates/app", "crates/pod-process", "crates/pod-storage", "crates/pod-test-support"]
default-members = ["crates/app", "crates/pod-process", "crates/pod-storage", "crates/pod-test-support"]
'@
  $good = @{
    "crates/pod-spike" = "[package]`nname = `"pod-spike`"`n[dependencies]`napp = { path = `"../app`" }"
    "crates/diff-spike" = "[package]`nname = `"diff-spike`""
    "crates/app" = "[package]`nname = `"app`" # main`n[dependencies]`npod-process = { path = `"../pod-process`" }`n# pod-test-support = { path = `"../pod-test-support`" }`ngpui = { package = `"gpui-pre`", version = `"1`" }`n[dev-dependencies]`npod-test-support = { path = `"../pod-test-support`" }`n[target.'cfg(windows)'.dev-dependencies.pod-test-support]`npath = `"../pod-test-support`""
    "crates/pod-process" = "[package]`nname = `"pod-process`"`n[target.'cfg(windows)'.dependencies]`nwindows-sys = `"0.61`"`n[dev-dependencies]`npod-test-support = { path = `"../pod-test-support`" }"
    "crates/pod-storage" = "[package]`nname = `"pod-storage`"`n[dependencies]`nserde = `"1`" # not pod-process`n[features]`nx = [`"serde/std`"]"
    "crates/pod-test-support" = "[package]`nname = `"pod-test-support`"`n[dev-dependencies]`ntempfile = `"3`""
  }
  # Each case: changes to the good fixture and the FAIL texts it must produce.
  $cases = @(
    @{ Name = "good"; Ws = $wsGood; Set = @{}; Expect = @() },
    @{ Name = "test-support as a normal dependency"; Ws = $wsGood
       Set = @{ "crates/app" = "[package]`nname = `"app`"`n[dependencies]`npod-test-support = { path = `"../pod-test-support`" }" }
       Expect = @("crates/app [dependencies] pod-test-support: pod-test-support is a dev-dependency only") },
    @{ Name = "test-support as a build or target dependency, renamed or as a table"; Ws = $wsGood
       Set = @{ "crates/app" = "[package]`nname = `"app`"`n[build-dependencies]`npod-test-support.workspace = true`n[target.'cfg(windows)'.dependencies]`nts = { package = `"pod-test-support`", path = `"../pod-test-support`" }`n[dependencies.pod-test-support]`npath = `"../pod-test-support`"" }
       Expect = @("crates/app [build-dependencies] pod-test-support: pod-test-support is a dev-dependency only",
                  "crates/app [target.'cfg(windows)'.dependencies] ts (package pod-test-support): pod-test-support is a dev-dependency only",
                  "crates/app [dependencies.pod-test-support] pod-test-support: pod-test-support is a dev-dependency only") },
    @{ Name = "leaf depends on a domain crate, on another leaf, on GPUI"; Ws = $wsGood
       Set = @{ "crates/pod-process" = "[package]`nname = `"pod-process`"`n[dependencies]`napp = { path = `"../app`" }`nui = { package = `"gpui-pre`", version = `"1`" }"
                "crates/pod-storage" = "[package]`nname = `"pod-storage`"`n[dev-dependencies.pod-process]`npath = `"../pod-process`""
                "crates/pod-test-support" = "[package]`nname = `"pod-test-support`"`n[dev-dependencies]`napp = { path = `"../app`" }`nme = { package = `"pod-test-support`", path = `".`" }" }
       Expect = @("crates/pod-process [dependencies] app: leaf crate pod-process depends on workspace crate app",
                  "crates/pod-process [dependencies] ui (package gpui-pre): leaf crate pod-process depends on GPUI",
                  "crates/pod-storage [dev-dependencies.pod-process] pod-process: leaf crate pod-storage depends on workspace crate pod-process",
                  "crates/pod-test-support [dev-dependencies] app: leaf crate pod-test-support depends on workspace crate app",
                  "crates/pod-test-support [dev-dependencies] me (package pod-test-support): leaf crate pod-test-support depends on workspace crate pod-test-support") },
    @{ Name = "spikes in defaults, depended on, or not members"
       Ws = "[workspace]`nmembers = [`"crates/pod-spike`", `"crates/app`", `"crates/pod-process`", `"crates/pod-storage`", `"crates/pod-test-support`"]`ndefault-members = [`"crates/pod-spike`", `"crates/app`"]"
       Set = @{ "crates/app" = "[package]`nname = `"app`"`n[dev-dependencies]`nspike = { package = `"pod-spike`", path = `"../pod-spike`" }" }
       Expect = @("crates/diff-spike is not a workspace member (cargo build -p diff-spike would stop working)",
                  "crates/pod-spike is a spike but a default member",
                  "crates/app [dev-dependencies] spike (package pod-spike): depends on a spike, which plain cargo build/test would then build") },
    @{ Name = "a removed edge comes back, renamed or as a table"; Ws = $wsGood
       Set = @{ "crates/app" = "[package]`nname = `"pod-editor`"`n[dependencies]`npod-forge = { path = `"../pod-forge`" }`n[target.'cfg(windows)'.dev-dependencies]`nforge = { package = `"pod-forge`", path = `"../pod-forge`" }`n[build-dependencies.pod-forge]`npath = `"../pod-forge`"" }
       Expect = @("crates/app [dependencies] pod-forge: pod-editor must not depend on pod-forge (removed edge)",
                  "crates/app [target.'cfg(windows)'.dev-dependencies] forge (package pod-forge): pod-editor must not depend on pod-forge (removed edge)",
                  "crates/app [build-dependencies.pod-forge] pod-forge: pod-editor must not depend on pod-forge (removed edge)") }
  )
  $bad = @()
  try {
    foreach ($c in $cases) {
      $dir = Join-Path $tmp ([Guid]::NewGuid().ToString("N"))
      [void](New-Item -ItemType Directory -Force $dir)
      Set-Content (Join-Path $dir "Cargo.toml") $c.Ws -Encoding UTF8
      foreach ($k in $good.Keys) {
        $text = if ($c.Set.ContainsKey($k)) { $c.Set[$k] } else { $good[$k] }
        [void](New-Item -ItemType Directory -Force (Join-Path $dir $k))
        Set-Content (Join-Path $dir "$k\Cargo.toml") $text -Encoding UTF8
      }
      $got = @((Test-Boundaries $dir).Fail | Sort-Object) -join " | "
      $want = @($c.Expect | Sort-Object) -join " | "
      if ($got -cne $want) { $bad += "case '$($c.Name)': got [$got], want [$want]" }
    }
  } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
  if ($bad.Count -gt 0) { $bad | ForEach-Object { Write-Output "SELFTEST FAIL: $_" }; exit 1 }
  Write-Output "check-boundaries selftest ok ($($cases.Count) cases)"
  exit 0
}

$r = Test-Boundaries (Resolve-Path $Root).Path
if ($r.Fail.Count -gt 0) { $r.Fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "crate boundaries ok ($($r.Members) members; test-support dev-only, leaf crates without workspace deps, spikes outside defaults, removed edges absent)"
