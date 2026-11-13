# Crate boundary check (no cargo run needed), docs/20-refactor/libs.md section 2
# and rescore.md step 3:
# - pod-test-support is a dev-dependency only: no crate lists it under
#   [dependencies], [build-dependencies] or a target's (build-)dependencies;
# - the leaf crates pod-process, pod-storage and pod-test-support depend on no
#   other workspace crate (no reverse edge into a domain crate) and not on
#   GPUI; process and storage may use pod-test-support for their own tests;
# - the spikes stay workspace members (`cargo build -p pod-spike` keeps
#   working) but out of default-members, and no other crate depends on them,
#   so plain `cargo build/test/clippy` never builds them (quality-14);
# - forbidden edges stay absent, in every dependency table (normal, dev,
#   build, target-specific):
#   - pod-editor takes bounded runs from pod-process, not pod-forge (card T1);
#   - pod-app, pod-cli and pod-remote reach the daemon through pod-client
#     (their `podd` alias), never the podd package (daemon card D-T1);
#   - pod-protocol and pod-client stay free of the daemon (podd), GPUI
#     (gpui, gpui-kit, gpui-component, ...), the terminal emulator
#     (alacritty_terminal) and tokio's server side: the runtime and its OS
#     drivers (features in $tokioServer), whether the dependency entry,
#     [workspace.dependencies] or a [features] `tokio/x` entry turns them on;
#   - the mobile endpoint pod-mobile (docs/23 section 1, card F0) reaches neither
#     the daemon, the full pipe client nor the app (podd, pod-client,
#     pod-app, pod-cli, pod-remote), GPUI or the terminal emulator; podd
#     hands it the six mobile operations as typed callbacks. Its TLS is the
#     platform's (SChannel / system OpenSSL 3, user decision 2026-11-13), so
#     it lists no Rust TLS or crypto provider (rustls, ring, aws-lc,
#     RustCrypto, a vendored OpenSSL);
#   - pod-protocol and pod-client stay free of the mobile runtime and TLS
#     (pod-mobile, openssl, schannel, native-tls, rustls, ring, aws-lc);
#   - every crate a rule names is a workspace member, so renaming one cannot
#     switch its rule off silently.
# A dependency counts under its package name (`x = { package = "..." }`, and
# `x.workspace = true` resolved through [workspace.dependencies]), in inline,
# dotted-key (`x.package = "..."`) and [dependencies.x] table form; values may
# span lines. Reads manifests only.
#   powershell -ExecutionPolicy Bypass -File tools/check-boundaries.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-boundaries.ps1 -SelfTest
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$SelfTest)
$ErrorActionPreference = "Stop"

$spikes = @("crates/pod-spike", "crates/diff-spike")
$leaves = @("pod-process", "pod-storage", "pod-test-support")
$testSupport = "pod-test-support"
# Forbidden edges: a package in From must not depend on a package matching To.
$forbidden = @(
  @{ From = @("pod-editor"); To = '^pod-forge$'; Why = "removed edge" },
  @{ From = @("pod-app", "pod-cli", "pod-remote"); To = '^podd$'; Why = "clients reach the daemon through pod-client" },
  @{ From = @("pod-protocol", "pod-client"); To = '^podd$'; Why = "wire and client crates stay free of the daemon" },
  @{ From = @("pod-protocol", "pod-client"); To = '^gpui([-_].*)?$'; Why = "wire and client crates stay free of GPUI" },
  @{ From = @("pod-protocol", "pod-client"); To = '^alacritty'; Why = "wire and client crates stay free of the terminal emulator" },
  @{ From = @("pod-mobile"); To = '^(podd|pod-client|pod-app|pod-cli|pod-remote)$'; Why = "the mobile endpoint gets typed callbacks, not the daemon, the full pipe client or the app" },
  @{ From = @("pod-mobile"); To = '^gpui([-_].*)?$'; Why = "the mobile endpoint stays free of GPUI" },
  @{ From = @("pod-mobile"); To = '^alacritty'; Why = "the mobile endpoint stays free of the terminal emulator" },
  @{ From = @("pod-mobile"); To = '^(rustls.*|tokio-rustls|ring|aws-lc.*|subtle|p256|ecdsa|sha2|openssl-src|boring.*|native-tls)$'; Why = "the mobile endpoint uses the platform TLS and crypto only" },
  @{ From = @("pod-protocol", "pod-client"); To = '^(pod-mobile|openssl.*|schannel|native-tls|tokio-native-tls|rustls.*|tokio-rustls|ring|aws-lc.*)$'; Why = "wire and client crates stay free of the mobile runtime and TLS" }
)
# Packages that must not turn on tokio's runtime or OS drivers (what podd's
# `server` feature needs); pure helpers such as sync or io-util stay allowed.
$tokioFrom = @("pod-protocol", "pod-client")
$tokioServer = @("full", "rt", "rt-multi-thread", "net", "process", "signal", "fs", "io-std", "test-util")

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

# Brackets and braces a line opens minus those it closes, outside strings.
function Get-Depth([string]$line) {
  $quote = [char]0; $depth = 0
  for ($i = 0; $i -lt $line.Length; $i++) {
    $c = $line[$i]
    if ($quote -ne [char]0) { if ($c -eq $quote) { $quote = [char]0 } }
    elseif ($c -eq '"' -or $c -eq "'") { $quote = $c }
    elseif ($c -eq '[' -or $c -eq '{') { $depth++ }
    elseif ($c -eq ']' -or $c -eq '}') { $depth-- }
  }
  return $depth
}

# Logical lines of a manifest: comments dropped, a value spread over several
# lines (multi-line array or inline table) joined into one line, the inside of
# a multi-line string skipped.
function Get-Lines([string]$text) {
  $out = New-Object System.Collections.ArrayList
  $buf = ""; $depth = 0; $ml = $null
  foreach ($raw in ($text -split "\r?\n")) {
    if ($ml) { if ($raw.Contains($ml)) { $ml = $null }; continue }
    $line = (Remove-Comment $raw).Trim()
    if (-not $line) { continue }
    foreach ($t in '"""', "'''") {
      if (([regex]::Matches($line, [regex]::Escape($t))).Count % 2 -eq 1) { $ml = $t }
    }
    $buf = if ($buf) { "$buf $line" } else { $line }
    $depth += Get-Depth $line
    if ($depth -le 0 -or $ml) { [void]$out.Add($buf); $buf = ""; $depth = 0 }
  }
  if ($buf) { [void]$out.Add($buf) }
  return @($out)
}

# Strings of a TOML array value (`["a", 'b']`).
function Get-Strings([string]$value) {
  foreach ($m in [regex]::Matches($value, '"([^"]*)"|''([^'']*)''')) {
    if ($m.Groups[1].Success) { $m.Groups[1].Value } else { $m.Groups[2].Value }
  }
}

function New-Dep([string]$kind, [string]$section, [string]$key) {
  $key = $key.Trim().Trim('"', "'")
  return @{ Kind = $kind; Section = $section; Key = $key; Package = $key; Features = @(); Inherit = $false }
}

# One `attr = value` of a dependency (dotted key or [dependencies.x] table).
function Set-DepAttr($dep, [string]$attr, [string]$value) {
  switch ($attr) {
    "package" { $dep.Package = $value.Trim().Trim('"', "'") }
    "features" { foreach ($f in (Get-Strings $value)) { $dep.Features += $f } }
    "workspace" { if ($value.Trim() -eq "true") { $dep.Inherit = $true } }
  }
}

# The value of `x = ...`: a version string, or an inline table.
function Set-DepValue($dep, [string]$value) {
  $v = $value.Trim()
  if (-not $v.StartsWith("{")) { return }
  $p = [regex]::Match($v, '(?<![\w-])package\s*=\s*("[^"]*"|''[^'']*'')')
  if ($p.Success) { Set-DepAttr $dep "package" $p.Groups[1].Value }
  $f = [regex]::Match($v, '(?<![\w-])features\s*=\s*(\[[^\]]*\])')
  if ($f.Success) { Set-DepAttr $dep "features" $f.Groups[1].Value }
  if ($v -match '(?<![\w-])workspace\s*=\s*true') { $dep.Inherit = $true }
}

# Package name, dependencies and [features] `dep/feature` entries of one
# manifest. Each dependency is @{ Kind = dependencies|dev-dependencies|
# build-dependencies (workspace for [workspace.dependencies]); Section; Key;
# Package; Features; Inherit (`workspace = true`) }.
function Read-Manifest([string]$text) {
  $name = $null; $deps = New-Object System.Collections.ArrayList; $refs = New-Object System.Collections.ArrayList
  $section = ""; $kind = $null; $table = $null
  $kinds = "dev-dependencies|build-dependencies|dependencies"
  $keyRe = '"[^"]+"|''[^'']+''|[A-Za-z0-9_-]+'
  foreach ($line in (Get-Lines $text)) {
    $h = [regex]::Match($line, '^\[([^\[\]]+)\]$')
    if ($h.Success) {
      $section = $h.Groups[1].Value.Trim(); $kind = $null; $table = $null
      $s = [regex]::Match($section, "^(?:target\..+\.)?($kinds)$")
      $t = [regex]::Match($section, "^(?:target\..+\.)?($kinds)\.(.+)$")
      $w = [regex]::Match($section, '^workspace\.dependencies\.(.+)$')
      if ($section -eq "workspace.dependencies") { $kind = "workspace" }
      elseif ($w.Success) { $table = New-Dep "workspace" $section $w.Groups[1].Value; [void]$deps.Add($table) }
      elseif ($s.Success) { $kind = $s.Groups[1].Value }
      elseif ($t.Success) { $table = New-Dep $t.Groups[1].Value $section $t.Groups[2].Value; [void]$deps.Add($table) }
      continue
    }
    if ($line.StartsWith("[")) { $section = ""; $kind = $null; $table = $null; continue }
    $e = [regex]::Match($line, "^($keyRe)(?:\.($keyRe))?\s*=\s*(.*)$")
    if (-not $e.Success) { continue }
    $key = $e.Groups[1].Value.Trim('"', "'"); $value = $e.Groups[3].Value
    $attr = if ($e.Groups[2].Success) { $e.Groups[2].Value.Trim('"', "'") } else { $null }
    if ($section -eq "package" -and $key -eq "name" -and -not $attr) { $name = $value.Trim().Trim('"', "'") }
    elseif ($table) { if (-not $attr) { Set-DepAttr $table $key $value } }
    elseif ($kind) {
      $dep = @($deps | Where-Object { $_.Section -eq $section -and $_.Key -eq $key })[0]
      if (-not $dep) { $dep = New-Dep $kind $section $key; [void]$deps.Add($dep) }
      if ($attr) { Set-DepAttr $dep $attr $value } else { Set-DepValue $dep $value }
    }
    elseif ($section -eq "features") {
      foreach ($f in (Get-Strings $value)) {
        $m = [regex]::Match($f, '^([^/:?]+)\??/(.+)$')
        if ($m.Success) { [void]$refs.Add(@{ Dep = $m.Groups[1].Value; Feature = $m.Groups[2].Value }) }
      }
    }
  }
  return @{ Name = $name; Deps = @($deps); FeatureRefs = @($refs) }
}

# Every boundary violation under $root, as "FAIL" message bodies.
function Test-Boundaries([string]$root) {
  $toml = Get-Content (Join-Path $root "Cargo.toml") -Raw
  $members = Get-List $toml "members"
  $default = Get-List $toml "default-members"
  $inherited = @{}
  foreach ($d in (Read-Manifest $toml).Deps) { if ($d.Kind -eq "workspace") { $inherited[$d.Key] = $d } }
  $fail = @()
  $crates = @{}
  foreach ($m in $members) {
    $path = Join-Path $root "$m\Cargo.toml"
    if (-not (Test-Path $path)) { $fail += "$m has no Cargo.toml"; continue }
    $man = Read-Manifest (Get-Content $path -Raw)
    if (-not $man.Name) { $fail += "$m has no [package] name"; continue }
    foreach ($d in $man.Deps) {
      if ($d.Inherit -and $inherited.ContainsKey($d.Key)) {
        $w = $inherited[$d.Key]
        $d.Package = $w.Package; $d.Features = @($w.Features) + @($d.Features)
      }
    }
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
      foreach ($r in $forbidden) {
        if ($r.From -contains $man.Name -and $d.Package -match $r.To) {
          $fail += "$what`: $($man.Name) must not depend on $($d.Package) ($($r.Why))"
        }
      }
      if ($tokioFrom -contains $man.Name -and $d.Package -eq "tokio") {
        $on = @($d.Features) + @($man.FeatureRefs | Where-Object { $_.Dep -eq $d.Key } | ForEach-Object { $_.Feature })
        $server = @($on | Where-Object { $tokioServer -contains $_ } | Sort-Object -Unique)
        if ($server.Count -gt 0) { $fail += "$what`: $($man.Name) must not use tokio's server-side features ($($server -join ', '))" }
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
  $ruled = @(@($forbidden | ForEach-Object { $_.From }) + $tokioFrom | Sort-Object -Unique)
  foreach ($c in $ruled) { if ($names -notcontains $c) { $fail += "boundary rule names $c, which is not a workspace member" } }
  return @{ Fail = $fail; Members = $members.Count }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-boundaries-" + [Guid]::NewGuid().ToString("N"))
  $all = @("crates/pod-spike", "crates/diff-spike", "crates/app", "crates/pod-process", "crates/pod-storage", "crates/pod-test-support",
           "crates/pod-editor", "crates/podd", "crates/pod-protocol", "crates/pod-client", "crates/pod-app", "crates/pod-cli", "crates/pod-remote",
           "crates/pod-mobile")
  $shipped = @($all | Where-Object { $spikes -notcontains $_ })
  # A root manifest; the app crates inherit their `podd` alias from it.
  function New-Ws([string[]]$members, [string[]]$defaults, [string]$deps = "") {
    $m = ($members | ForEach-Object { "`"$_`"" }) -join ", "
    $d = ($defaults | ForEach-Object { "`"$_`"" }) -join ", "
    return "[workspace]`nmembers = [$m]`ndefault-members = [$d]`n[workspace.dependencies]`npodd = { package = `"pod-client`", path = `"crates/pod-client`" }`n$deps"
  }
  $wsGood = New-Ws $all $shipped
  $good = @{
    "crates/pod-spike" = "[package]`nname = `"pod-spike`"`n[dependencies]`napp = { path = `"../app`" }"
    "crates/diff-spike" = "[package]`nname = `"diff-spike`""
    "crates/app" = "[package]`nname = `"app`" # main`n[dependencies]`npod-process = { path = `"../pod-process`" }`n# pod-test-support = { path = `"../pod-test-support`" }`ngpui = { package = `"gpui-pre`", version = `"1`" }`n[dev-dependencies]`npod-test-support = { path = `"../pod-test-support`" }`n[target.'cfg(windows)'.dev-dependencies.pod-test-support]`npath = `"../pod-test-support`""
    "crates/pod-process" = "[package]`nname = `"pod-process`"`n[target.'cfg(windows)'.dependencies]`nwindows-sys = `"0.61`"`n[dev-dependencies]`npod-test-support = { path = `"../pod-test-support`" }"
    "crates/pod-storage" = "[package]`nname = `"pod-storage`"`n[dependencies]`nserde = `"1`" # not pod-process`n[features]`nx = [`"serde/std`"]"
    "crates/pod-test-support" = "[package]`nname = `"pod-test-support`"`n[dev-dependencies]`ntempfile = `"3`""
    "crates/pod-editor" = "[package]`nname = `"pod-editor`"`n[dependencies]`npod-process = { path = `"../pod-process`" }"
    # The daemon itself may use everything the client crates must not.
    "crates/podd" = "[package]`nname = `"podd`"`n[dependencies]`npod-client.path = `"../pod-client`"`ntokio = { version = `"1`", features = [`"rt-multi-thread`", `"net`"], optional = true }`nalacritty_terminal = `"0.25`"`n[features]`nserver = [`"dep:tokio`", `"tokio/process`"]"
    "crates/pod-protocol" = "[package]`nname = `"pod-protocol`"`ndescription = `"`"`"`nnot a [dependencies] table {`n`"`"`"`n[dependencies]`nserde = `"1`"`nmods = { package = `"pod-mods`", path = `"../pod-mods`", default-features = false }"
    # tokio without its runtime or drivers (a multi-line array, a feature) is fine.
    "crates/pod-client" = "[package]`nname = `"pod-client`"`n[dependencies]`npod-protocol.path = `"../pod-protocol`"`ntokio = { version = `"1`", default-features = false, features = [`n    `"sync`", # channels`n    `"time`",`n] }`n[features]`nasync = [`"dep:tokio`", `"tokio/io-util`"]"
    # The `podd` alias of the client crate, inline, as a table and inherited.
    "crates/pod-app" = "[package]`nname = `"pod-app`"`n[dependencies]`npodd = { package = `"pod-client`", path = `"../pod-client`" }`ngpui-kit = `"0.9`"`nalacritty_terminal = `"0.25`""
    "crates/pod-cli" = "[package]`nname = `"pod-cli`"`n[dependencies.podd]`npackage = `"pod-client`"`npath = `"../pod-client`"`n[dev-dependencies]`ntokio = { version = `"1`", features = [`"full`"] }"
    "crates/pod-remote" = "[package]`nname = `"pod-remote`"`n[dependencies]`npodd.workspace = true"
    # Platform TLS behind a feature per target; the protocol types are fine.
    "crates/pod-mobile" = "[package]`nname = `"pod-mobile`"`n[dependencies]`npod-protocol.path = `"../pod-protocol`"`n[target.'cfg(windows)'.dependencies.windows-sys]`nversion = `"0.61`"`noptional = true`n[target.'cfg(target_os = `"linux`")'.dependencies]`nopenssl = { version = `"0.10`", optional = true }`n[features]`nmobile-lan = [`"dep:windows-sys`", `"dep:openssl`"]"
  }
  $client = "clients reach the daemon through pod-client"
  $wire = "wire and client crates stay free of"
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
       Ws = New-Ws (@("crates/pod-spike") + $shipped) @("crates/pod-spike", "crates/app")
       Set = @{ "crates/app" = "[package]`nname = `"app`"`n[dev-dependencies]`nspike = { package = `"pod-spike`", path = `"../pod-spike`" }" }
       Expect = @("crates/diff-spike is not a workspace member (cargo build -p diff-spike would stop working)",
                  "crates/pod-spike is a spike but a default member",
                  "crates/app [dev-dependencies] spike (package pod-spike): depends on a spike, which plain cargo build/test would then build") },
    @{ Name = "a removed edge comes back, renamed or as a table"; Ws = $wsGood
       Set = @{ "crates/pod-editor" = "[package]`nname = `"pod-editor`"`n[dependencies]`npod-forge = { path = `"../pod-forge`" }`n[target.'cfg(windows)'.dev-dependencies]`nforge = { package = `"pod-forge`", path = `"../pod-forge`" }`n[build-dependencies.pod-forge]`npath = `"../pod-forge`"" }
       Expect = @("crates/pod-editor [dependencies] pod-forge: pod-editor must not depend on pod-forge (removed edge)",
                  "crates/pod-editor [target.'cfg(windows)'.dev-dependencies] forge (package pod-forge): pod-editor must not depend on pod-forge (removed edge)",
                  "crates/pod-editor [build-dependencies.pod-forge] pod-forge: pod-editor must not depend on pod-forge (removed edge)") },
    @{ Name = "app, cli and remote depend on podd: plain, renamed, dotted, as a table, inherited"
       Ws = New-Ws $all $shipped "server = { package = `"podd`", path = `"crates/podd`" }"
       Set = @{ "crates/pod-app" = "[package]`nname = `"pod-app`"`n[dependencies]`npodd = { path = `"../podd`" }`n[target.'cfg(windows)'.dev-dependencies]`nd = { package = `"podd`", path = `"../podd`" }"
                "crates/pod-cli" = "[package]`nname = `"pod-cli`"`n[build-dependencies.podd]`npath = `"../podd`"`n[dependencies]`nd.package = `"podd`"`nd.path = `"../podd`""
                "crates/pod-remote" = "[package]`nname = `"pod-remote`"`n[dependencies]`nserver.workspace = true`n[dev-dependencies]`nserver = { workspace = true, features = [`"x`"] }" }
       Expect = @("crates/pod-app [dependencies] podd: pod-app must not depend on podd ($client)",
                  "crates/pod-app [target.'cfg(windows)'.dev-dependencies] d (package podd): pod-app must not depend on podd ($client)",
                  "crates/pod-cli [build-dependencies.podd] podd: pod-cli must not depend on podd ($client)",
                  "crates/pod-cli [dependencies] d (package podd): pod-cli must not depend on podd ($client)",
                  "crates/pod-remote [dependencies] server (package podd): pod-remote must not depend on podd ($client)",
                  "crates/pod-remote [dev-dependencies] server (package podd): pod-remote must not depend on podd ($client)") },
    @{ Name = "protocol and client depend on the daemon, GPUI or the terminal emulator"; Ws = $wsGood
       Set = @{ "crates/pod-protocol" = "[package]`nname = `"pod-protocol`"`ndescription = '''`nnot a { table`n'''`n[dependencies]`ndaemon ={ package = `"podd`", path = `"../podd`" }`ngpui = `"0.2`"`n[dev-dependencies.ui]`npackage = `"gpui-component`"`nversion = `"0.5`""
                "crates/pod-client" = "[package]`nname = `"pod-client`"`n[target.'cfg(windows)'.build-dependencies]`nkit = { package = `"gpui-kit`", version = `"0.9`" }`n[dependencies]`nvt.package = `"alacritty_terminal`"`nvt.version = `"0.25`"`npodd.path = `"../podd`"" }
       Expect = @("crates/pod-protocol [dependencies] daemon (package podd): pod-protocol must not depend on podd ($wire the daemon)",
                  "crates/pod-protocol [dependencies] gpui: pod-protocol must not depend on gpui ($wire GPUI)",
                  "crates/pod-protocol [dev-dependencies.ui] ui (package gpui-component): pod-protocol must not depend on gpui-component ($wire GPUI)",
                  "crates/pod-client [target.'cfg(windows)'.build-dependencies] kit (package gpui-kit): pod-client must not depend on gpui-kit ($wire GPUI)",
                  "crates/pod-client [dependencies] vt (package alacritty_terminal): pod-client must not depend on alacritty_terminal ($wire the terminal emulator)",
                  "crates/pod-client [dependencies] podd: pod-client must not depend on podd ($wire the daemon)") },
    @{ Name = "protocol and client turn on tokio's server side: inline, multi-line, dotted, as a table, via [features], inherited"
       Ws = New-Ws $all $shipped "tokio = { version = `"1`", features = [`"rt-multi-thread`"] }"
       Set = @{ "crates/pod-client" = "[package]`nname = `"pod-client`"`n[dependencies]`nrt = { package = `"tokio`", version = `"1`", features = [`n  `"sync`",`n  `"net`",`n] }`n[dev-dependencies.tokio]`nversion = `"1`"`nfeatures = [`"full`", `"macros`"]`n[target.'cfg(windows)'.dependencies]`ntokio.version = `"1`"`ntokio.features = [`"signal`"]"
                "crates/pod-protocol" = "[package]`nname = `"pod-protocol`"`n[dependencies]`ntokio = { workspace = true, optional = true }`n[features]`nasync = [`"dep:tokio`", `"tokio?/process`", `"tokio/sync`"]" }
       Expect = @("crates/pod-client [dependencies] rt (package tokio): pod-client must not use tokio's server-side features (net)",
                  "crates/pod-client [dev-dependencies.tokio] tokio: pod-client must not use tokio's server-side features (full)",
                  "crates/pod-client [target.'cfg(windows)'.dependencies] tokio: pod-client must not use tokio's server-side features (signal)",
                  "crates/pod-protocol [dependencies] tokio: pod-protocol must not use tokio's server-side features (process, rt-multi-thread)") },
    @{ Name = "mobile reaches the daemon, the pipe client, the app, GPUI, the terminal or a Rust TLS stack; protocol and client reach mobile or TLS"; Ws = $wsGood
       Set = @{ "crates/pod-mobile" = "[package]`nname = `"pod-mobile`"`n[dependencies]`ndaemon = { package = `"podd`", path = `"../podd`" }`npodd.workspace = true`n[target.'cfg(windows)'.dev-dependencies]`npod-app = { path = `"../pod-app`" }`nui = { package = `"gpui-kit`", version = `"0.9`" }`n[build-dependencies.alacritty_terminal]`nversion = `"0.25`"`n[target.'cfg(target_os = `"linux`")'.dependencies]`nrustls = { version = `"0.23`", optional = true }`nring = `"0.17`"`nopenssl-src = `"300`""
                "crates/pod-protocol" = "[package]`nname = `"pod-protocol`"`n[dependencies]`nmobile = { package = `"pod-mobile`", path = `"../pod-mobile`", optional = true }`n[dev-dependencies]`nopenssl = `"0.10`""
                "crates/pod-client" = "[package]`nname = `"pod-client`"`n[target.'cfg(windows)'.dependencies]`nschannel = `"0.1`"`ntls = { package = `"tokio-native-tls`", version = `"0.3`" }" }
       Expect = @("crates/pod-mobile [dependencies] daemon (package podd): pod-mobile must not depend on podd (the mobile endpoint gets typed callbacks, not the daemon, the full pipe client or the app)",
                  "crates/pod-mobile [dependencies] podd (package pod-client): pod-mobile must not depend on pod-client (the mobile endpoint gets typed callbacks, not the daemon, the full pipe client or the app)",
                  "crates/pod-mobile [target.'cfg(windows)'.dev-dependencies] pod-app: pod-mobile must not depend on pod-app (the mobile endpoint gets typed callbacks, not the daemon, the full pipe client or the app)",
                  "crates/pod-mobile [target.'cfg(windows)'.dev-dependencies] ui (package gpui-kit): pod-mobile must not depend on gpui-kit (the mobile endpoint stays free of GPUI)",
                  "crates/pod-mobile [build-dependencies.alacritty_terminal] alacritty_terminal: pod-mobile must not depend on alacritty_terminal (the mobile endpoint stays free of the terminal emulator)",
                  "crates/pod-mobile [target.'cfg(target_os = `"linux`")'.dependencies] rustls: pod-mobile must not depend on rustls (the mobile endpoint uses the platform TLS and crypto only)",
                  "crates/pod-mobile [target.'cfg(target_os = `"linux`")'.dependencies] ring: pod-mobile must not depend on ring (the mobile endpoint uses the platform TLS and crypto only)",
                  "crates/pod-mobile [target.'cfg(target_os = `"linux`")'.dependencies] openssl-src: pod-mobile must not depend on openssl-src (the mobile endpoint uses the platform TLS and crypto only)",
                  "crates/pod-protocol [dependencies] mobile (package pod-mobile): pod-protocol must not depend on pod-mobile ($wire the mobile runtime and TLS)",
                  "crates/pod-protocol [dev-dependencies] openssl: pod-protocol must not depend on openssl ($wire the mobile runtime and TLS)",
                  "crates/pod-client [target.'cfg(windows)'.dependencies] schannel: pod-client must not depend on schannel ($wire the mobile runtime and TLS)",
                  "crates/pod-client [target.'cfg(windows)'.dependencies] tls (package tokio-native-tls): pod-client must not depend on tokio-native-tls ($wire the mobile runtime and TLS)") },
    @{ Name = "a crate a rule names is no longer a member"
       Ws = New-Ws @($all | Where-Object { $_ -ne "crates/pod-client" -and $_ -ne "crates/pod-remote" }) @($shipped | Where-Object { $_ -ne "crates/pod-client" -and $_ -ne "crates/pod-remote" })
       Set = @{}
       Expect = @("boundary rule names pod-client, which is not a workspace member",
                  "boundary rule names pod-remote, which is not a workspace member") }
  )
  $bad = @()
  try {
    foreach ($c in $cases) {
      foreach ($k in $c.Set.Keys) { if (-not $good.ContainsKey($k)) { $bad += "case '$($c.Name)': $k is not a fixture crate" } }
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
Write-Output "crate boundaries ok ($($r.Members) members; test-support dev-only, leaf crates without workspace deps, spikes outside defaults, forbidden edges absent: editor->forge, app/cli/remote->podd, protocol/client->podd/GPUI/alacritty/tokio server/mobile/TLS, mobile->podd/client/app/GPUI/alacritty/Rust TLS)"
