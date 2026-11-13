# Strict license gate for the mobile LAN closure (docs/23 section 7, TM-04;
# cards F0 / M2-F0b).
#
# cargo-deny (deny.toml) keeps checking the whole workspace. This gate is
# stricter and only covers what `pod-mobile --features mobile-lan` (and so
# `podd --features mobile-lan`) adds, per target. The TLS stack is the
# platform's (user decision 2026-11-13): SChannel + CNG on Windows, the
# system OpenSSL 3 on Linux, dynamically linked and never vendored.
# - Classes per target: `shipped` = linked into the product (cargo tree
#   -e normal,no-proc-macro); `host` = build scripts and proc-macros with
#   their dependencies, run on the build machine and never distributed;
#   `dev` = test-only.
# - Licenses: only Apache-2.0, MIT and ISC. SPDX OR takes an allowed branch
#   whose text the crate ships (Apache-2.0, then MIT, then ISC), AND needs
#   every term and its text, WITH never passes. A shipped crate gets no
#   exception. A host-only term outside the list must be pinned to one crate
#   version in $hostOnly with its reason (it is never distributed).
#   Compiled native files of shipped crates need an allowed license too;
#   OpenSSL/SSLeay, GPL-family, MPL or BSD-style headers and unreviewed
#   binaries fail.
# - Excluded crates fail anywhere in the closure: rustls and its family,
#   ring, aws-lc, subtle, RustCrypto crypto crates and openssl-src (a
#   vendored OpenSSL).
# - The expected stack: Windows ships windows-sys (SSPI/CNG bindings) and no
#   openssl; Linux ships openssl + openssl-sys.
# - Off means off: `pod-mobile` without features has no third-party
#   dependency and `podd` with default features has no TLS crate.
# - The reviewed closure is pinned in docs/mobile-m2/sbom.json (Cargo.lock
#   checksum, SPDX, the branch chosen, SHA-256 of every license file, native
#   files of shipped crates, the OS libraries used). The gate rebuilds it
#   from cargo tree, Cargo.lock and the crate sources and fails on any
#   difference, so a lock update or a new crate needs a reviewed -Write.
# Needs cargo and the crate sources (`cargo fetch --locked` first).
#   powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1 -Write    # after a reviewed change
#   powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1 -SelfTest # no cargo
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$Write, [switch]$SelfTest)
$ErrorActionPreference = "Stop"

$sbomPath = "docs/mobile-m2/sbom.json"
$windows = "x86_64-pc-windows-msvc"
$linux = "x86_64-unknown-linux-gnu"
$targets = [ordered]@{
  $windows = "Windows x64: podd daemon and the desktop double (M2: built and tested)"
  $linux = "Linux x64: planned daemon target (M2: resolved and compile-checked in CI; runtime unverified)"
}
# The OS libraries each target calls instead of bundling crypto. Recorded,
# never distributed by Pod.
$system = [ordered]@{
  $windows = @(
    [ordered]@{ name = "secur32.dll (SSPI / SChannel)"; license = "LicenseRef-Windows-OS-component"; note = "TLS handshake and records" },
    [ordered]@{ name = "ncrypt.dll, bcrypt.dll (CNG)"; license = "LicenseRef-Windows-OS-component"; note = "listener key (per-user software KSP), SHA-256, RNG, P-256 verify" },
    [ordered]@{ name = "crypt32.dll (CryptoAPI)"; license = "LicenseRef-Windows-OS-component"; note = "certificate and DER encoding/decoding" }
  )
  $linux = @(
    [ordered]@{ name = "libssl.so.3, libcrypto.so.3 (OpenSSL >= 3.0)"; license = "Apache-2.0"; note = "from the distribution, dynamically linked; pod-mobile refuses older versions at run time" }
  )
}
$allow = @("Apache-2.0", "MIT", "ISC")
# Host-only license terms outside $allow, one crate version each, with the
# reason. Never valid for a shipped crate. Adding one needs the user's
# confirmation (docs/mobile-m2/dependencies.md).
$hostOnly = @(
  @{ Crate = "unicode-ident"; Version = "1.0.27"; License = "Unicode-3.0"
     Why = "Unicode data tables inside proc-macro2/syn, which run only while compiling the openssl-macros proc-macro on the build host (Linux target); already in the workspace build through serde_derive" }
)
# Excluded by the platform TLS decision, in any class.
$excluded = '^(rustls(-.*)?|tokio-rustls|hyper-rustls|ring|aws-lc-.*|subtle|openssl-src|boring(-sys)?|sha1|sha2|digest|p256|p384|ecdsa|elliptic-curve|crypto-bigint|primeorder)$'
$tlsCrates = '^(openssl(-sys)?|schannel|native-tls|tokio-native-tls|rustls(-.*)?|tokio-rustls|ring|aws-lc-.*|boring(-sys)?)$'
# The text each chosen license needs in the crate's license files.
$licenseText = @{
  "Apache-2.0" = 'Apache License,? Version 2\.0'
  "MIT" = 'Permission is hereby granted, free of charge, to any person obtaining a copy'
  "ISC" = 'Permission to use, copy, modify, and(/or)? distribute this software for any purpose with or without fee'
  "Unicode-3.0" = 'UNICODE LICENSE V3'
}
# Header markers of a native file. Any of $badMarkers makes a compiled file fail.
$goodMarkers = [ordered]@{
  "Apache-2.0" = 'Licensed under the Apache License|Apache License,? Version 2\.0'
  "ISC" = $licenseText["ISC"]
  "MIT" = $licenseText["MIT"]
}
$badMarkers = [ordered]@{
  "OpenSSL" = 'OpenSSL License|THIS SOFTWARE IS PROVIDED BY THE OpenSSL PROJECT'
  "SSLeay" = 'This package is an SSL implementation written|SSLeay License'
  "GPL" = 'GNU (Lesser |Library |Affero )?General Public License'
  "MPL" = 'Mozilla Public License'
  "BSD-style" = 'Redistribution and use in source and binary forms'
}
$nativeExt = @(".c", ".h", ".s", ".asm", ".inc", ".pl", ".cc", ".cpp", ".hpp", ".o", ".obj", ".a", ".lib", ".dll", ".so", ".dylib")
$binaryExt = @(".o", ".obj", ".a", ".lib", ".dll", ".so", ".dylib")
$licenseFileRe = '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)'
# How a shipped crate's native files are used, where that is not "compiled in".
$nativeUse = @{
  "openssl-sys" = "build.rs runs build/expando.c through the C preprocessor (cc try_expand) to read the system OpenSSL version; nothing is compiled into the product, libssl/libcrypto are linked dynamically"
}

function Sort-Ordinal($items) {
  $a = [string[]]@($items | Where-Object { $null -ne $_ })
  [Array]::Sort($a, [StringComparer]::Ordinal)
  return $a
}

# ---------- JSON (deterministic, LF, objects of scalars inside arrays on one line)

function ConvertTo-Str([string]$s) {
  if ($s -notmatch '["\\\x00-\x1f]') { return '"' + $s + '"' }
  $sb = New-Object Text.StringBuilder
  [void]$sb.Append('"')
  foreach ($c in $s.ToCharArray()) {
    switch ($c) {
      '"' { [void]$sb.Append('\"') } '\' { [void]$sb.Append('\\') }
      "`n" { [void]$sb.Append('\n') } "`r" { [void]$sb.Append('\r') } "`t" { [void]$sb.Append('\t') }
      default { if ([int]$c -lt 32) { [void]$sb.Append(('\u{0:x4}' -f [int]$c)) } else { [void]$sb.Append($c) } }
    }
  }
  [void]$sb.Append('"')
  return $sb.ToString()
}

function ConvertTo-PodJson($v, [string]$pad = "") {
  if ($null -eq $v) { return "null" }
  if ($v -is [bool]) { if ($v) { return "true" } else { return "false" } }
  if ($v -is [int]) { return "$v" }
  if ($v -is [string]) { return (ConvertTo-Str $v) }
  $in = "$pad  "
  if ($v -is [Collections.IDictionary]) {
    if ($v.Count -eq 0) { return "{}" }
    $parts = foreach ($k in $v.Keys) { "$in$(ConvertTo-Str $k): $(ConvertTo-PodJson $v[$k] $in)" }
    return "{`n" + ($parts -join ",`n") + "`n$pad}"
  }
  $items = @($v)
  if ($items.Count -eq 0) { return "[]" }
  $parts = foreach ($x in $items) {
    $flat = $x -is [Collections.IDictionary] -and -not @($x.Values | Where-Object { $_ -is [Collections.IDictionary] -or $_ -is [Array] })
    if ($flat) {
      $kv = foreach ($k in $x.Keys) { "$(ConvertTo-Str $k): $(ConvertTo-PodJson $x[$k])" }
      "$in{" + ($kv -join ", ") + "}"
    } else { "$in$(ConvertTo-PodJson $x $in)" }
  }
  return "[`n" + ($parts -join ",`n") + "`n$pad]"
}

# ---------- SPDX

function Read-SpdxNode {
  if ($script:pos -ge $script:tok.Count) { return @{ T = "bad" } }
  $t = $script:tok[$script:pos]; $script:pos++
  if ($t -eq "(") {
    $n = Read-SpdxOr
    if ($script:pos -lt $script:tok.Count -and $script:tok[$script:pos] -eq ")") { $script:pos++; return $n }
    return @{ T = "bad" }
  }
  if (@(")", "AND", "OR", "WITH") -contains $t) { return @{ T = "bad" } }
  if ($script:pos + 1 -lt $script:tok.Count -and $script:tok[$script:pos] -eq "WITH") {
    $script:pos += 2; return @{ T = "id"; Id = "$t WITH $($script:tok[$script:pos - 1])" }
  }
  return @{ T = "id"; Id = $t }
}
function Read-SpdxAnd {
  $l = Read-SpdxNode
  while ($script:pos -lt $script:tok.Count -and $script:tok[$script:pos] -eq "AND") { $script:pos++; $l = @{ T = "and"; L = $l; R = (Read-SpdxNode) } }
  return $l
}
function Read-SpdxOr {
  $l = Read-SpdxAnd
  while ($script:pos -lt $script:tok.Count -and $script:tok[$script:pos] -eq "OR") { $script:pos++; $l = @{ T = "or"; L = $l; R = (Read-SpdxAnd) } }
  return $l
}
function Read-Spdx([string]$expr) {
  $script:tok = @([regex]::Matches(($expr -replace '/', ' OR '), '\(|\)|[^\s()]+') | ForEach-Object { $_.Value })
  $script:pos = 0
  if ($script:tok.Count -eq 0) { return @{ T = "bad" } }
  $n = Read-SpdxOr
  if ($script:pos -ne $script:tok.Count) { return @{ T = "bad" } }
  return $n
}

# An allowed id for this crate version in its class; with $text, also one
# whose license text the crate ships (SPDX OR picks a branch whose text is kept).
function Test-Allowed([string]$id, [string]$name, [string]$version, [string]$class, $text) {
  $ok = ($allow -contains $id) -or ($class -eq "host" -and [bool]@($hostOnly | Where-Object { $_.Crate -eq $name -and $_.Version -eq $version -and $_.License -eq $id }))
  if (-not $ok -or $null -eq $text) { return $ok }
  return $licenseText.ContainsKey($id) -and $text -match $licenseText[$id]
}
function Get-Rank($set) {
  $r = 0
  foreach ($id in $set) { $i = [array]::IndexOf($allow, $id); if ($i -lt 0) { $i = 9 }; if ($i -gt $r) { $r = $i } }
  return $r
}
# The license ids to comply with, or $null when no allowed choice exists.
function Select-Spdx($n, [string]$name, [string]$version, [string]$class, $text = $null) {
  switch ($n.T) {
    "id" { if (Test-Allowed $n.Id $name $version $class $text) { return , @($n.Id) } else { return $null } }
    "and" {
      $l = Select-Spdx $n.L $name $version $class $text; $r = Select-Spdx $n.R $name $version $class $text
      if ($null -eq $l -or $null -eq $r) { return $null }
      return , @(@($l) + @($r) | Select-Object -Unique)
    }
    "or" {
      $l = Select-Spdx $n.L $name $version $class $text; $r = Select-Spdx $n.R $name $version $class $text
      if ($null -eq $l) { return $r }; if ($null -eq $r) { return $l }
      if ((Get-Rank $r) -lt (Get-Rank $l)) { return $r }; return $l
    }
    default { return $null }
  }
}

# ---------- sources

function Read-Head([string]$path, [int]$chars = 8192) {
  $r = New-Object IO.StreamReader($path, $true)
  try { $buf = New-Object char[] $chars; $n = $r.Read($buf, 0, $chars); return [string]::new($buf, 0, $n) } finally { $r.Dispose() }
}
# Header text without comment leaders, on one line.
function Get-NormText([string]$text) {
  $lines = foreach ($l in ($text -split "\r?\n")) { ($l -replace '^\s*(/\*+|\*+/|\*+|//+|#+|;+|--+|!)?\s*', '') -replace '\s*\*+/\s*$', '' }
  return (($lines -join " ") -replace '\s+', ' ')
}
function Get-Sha256([string]$path) { return (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }

# The license a native file's header states, "NONE" or "BINARY".
function Get-Scan([string]$path) {
  if ($binaryExt -contains [IO.Path]::GetExtension($path).ToLowerInvariant()) { return "BINARY" }
  $head = Read-Head $path
  $t = Get-NormText $head
  $ids = @()
  foreach ($k in $badMarkers.Keys) { if ($t -match $badMarkers[$k]) { $ids += "LicenseRef-$k" } }
  $spdx = [regex]::Match($head, 'SPDX-License-Identifier:\s*([^\r\n*]+)')
  if ($spdx.Success) { $ids += "(" + $spdx.Groups[1].Value.Trim() + ")" }
  foreach ($k in $goodMarkers.Keys) { if ($t -match $goodMarkers[$k]) { $ids += $k } }
  if ($ids.Count -eq 0) { return "NONE" }
  return ((Sort-Ordinal ($ids | Select-Object -Unique)) -join " AND ")
}

# Every native file of one shipped crate: hash, header scan and the license
# it is under (its header, else the nearest LICENSE directory, else the
# crate's SPDX). Binaries are never accepted.
function Get-Native([string]$dir, [string]$crateLicense, [string]$name) {
  $all = @(Get-ChildItem -LiteralPath $dir -Recurse -File | Where-Object { $nativeExt -contains $_.Extension.ToLowerInvariant() })
  if ($all.Count -eq 0) { return $null }
  $files = foreach ($f in ($all | Sort-Object { $_.FullName.Substring($dir.Length + 1).Replace('\', '/') } -CaseSensitive)) {
    $p = $f.FullName.Substring($dir.Length + 1).Replace('\', '/')
    $scan = Get-Scan $f.FullName; $license = $scan; $basis = "header"
    if ($scan -eq "BINARY") { $license = "LicenseRef-unreviewed-binary"; $basis = "binary without a reviewed source" }
    elseif ($scan -eq "NONE") {
      $d = Split-Path $f.FullName -Parent; $lic = $null
      while ($d.Length -gt $dir.Length -and -not $lic) {
        $lic = @(Get-ChildItem -LiteralPath $d -File | Where-Object { $_.Name -match $licenseFileRe })[0]
        $d = Split-Path $d -Parent
      }
      if ($lic) { $basis = $lic.FullName.Substring($dir.Length + 1).Replace('\', '/'); $license = Get-Scan $lic.FullName }
      else { $basis = "crate license (Cargo.toml)"; $license = $crateLicense }
    }
    [ordered]@{ path = $p; sha256 = (Get-Sha256 $f.FullName); scan = $scan; license = $license; basis = $basis }
  }
  $use = if ($nativeUse.ContainsKey($name)) { $nativeUse[$name] } else { "every native file counts as compiled into the product" }
  return [ordered]@{ use = $use; files = @($files) }
}

function Find-Source([string[]]$registries, [string]$name, [string]$version) {
  foreach ($r in $registries) {
    if (-not (Test-Path -LiteralPath $r)) { continue }
    foreach ($idx in Get-ChildItem -LiteralPath $r -Directory) {
      $d = Join-Path $idx.FullName "$name-$version"
      if (Test-Path -LiteralPath (Join-Path $d "Cargo.toml")) { return $d }
    }
  }
  return $null
}

function Read-Lock([string]$text) {
  $sums = @{}
  foreach ($blk in ($text -split '\[\[package\]\]')) {
    $n = [regex]::Match($blk, '(?m)^name = "([^"]+)"'); $v = [regex]::Match($blk, '(?m)^version = "([^"]+)"')
    $c = [regex]::Match($blk, '(?m)^checksum = "([^"]+)"')
    if ($n.Success -and $v.Success) { $sums["$($n.Groups[1].Value) $($v.Groups[1].Value)"] = if ($c.Success) { $c.Groups[1].Value } else { "" } }
  }
  return $sums
}

# ---------- graph (cargo tree)

function Read-Tree([string[]]$lines) {
  $pkgs = @{}
  foreach ($raw in $lines) {
    # CI sets CARGO_TERM_COLOR=always; --color never below, and strip any SGR left.
    $l = (($raw -replace "$([char]27)\[[0-9;]*m", '') -replace '\s*\(\*\)\s*$', '').Trim()
    if (-not $l) { continue }
    $m = [regex]::Match($l, '^(\S+) v(\S+)( \(.+\))?\|(.*)$')
    if (-not $m.Success) { throw "unexpected cargo tree line: $l" }
    # A workspace crate prints its path; "(proc-macro)" or a URL is not local.
    $local = $m.Groups[3].Value -match '^ \((?:[A-Za-z]:\\|/)'
    $pkgs["$($m.Groups[1].Value) $($m.Groups[2].Value)"] = @{ Name = $m.Groups[1].Value; Version = $m.Groups[2].Value; License = $m.Groups[4].Value.Trim(); Local = $local }
  }
  return $pkgs
}
function Invoke-Tree([string]$root, [string[]]$a) {
  $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  Push-Location $root
  try { $out = @(& cargo tree @a --locked --color never --prefix none --format "{p}|{l}" 2>$null) } finally { Pop-Location; $ErrorActionPreference = $prev }
  if ($LASTEXITCODE -ne 0) { throw "cargo tree $($a -join ' ') failed ($LASTEXITCODE)" }
  return (Read-Tree $out)
}
# @{ Targets = @{ triple = @{ shipped; host; dev } }; Off = @{ "label" = pkgs } }
function Get-Graph([string]$root) {
  $g = @{ Targets = @{}; Off = [ordered]@{} }
  foreach ($t in $targets.Keys) {
    $base = @("-p", "pod-mobile", "--features", "mobile-lan", "--target", $t, "-e")
    $ship = Invoke-Tree $root ($base + "normal,no-proc-macro"); $nb = Invoke-Tree $root ($base + "normal,build"); $all = Invoke-Tree $root ($base + "normal,build,dev")
    $host_ = @{} + $nb; $dev = @{} + $all
    foreach ($k in $ship.Keys) { $host_.Remove($k) }
    foreach ($k in $nb.Keys) { $dev.Remove($k) }
    $g.Targets[$t] = @{ shipped = $ship; host = $host_; dev = $dev }
    $g.Off["pod-mobile without features ($t)"] = Invoke-Tree $root @("-p", "pod-mobile", "--target", $t, "-e", "normal,build")
  }
  $g.Off["podd default features ($windows)"] = Invoke-Tree $root @("-p", "podd", "--target", $windows, "-e", "normal")
  return $g
}

# ---------- SBOM and policy

# A package's strictest class over all targets: shipped > host > dev.
function Get-Class($kinds) {
  foreach ($c in "shipped", "host", "dev") { if (@($kinds.Keys | Where-Object { $_ -like "* $c" }).Count -gt 0) { return $c } }
  return "dev"
}

# @{ Doc = ordered SBOM; Fail = @() } from a graph, Cargo.lock text and registries.
function New-Sbom($graph, [string]$lockText, [string[]]$registries) {
  $fail = @()
  $lock = Read-Lock $lockText
  $pkgs = @{}; $tdocs = @()
  foreach ($t in $targets.Keys) {
    $kinds = [ordered]@{}
    foreach ($kind in "shipped", "host", "dev") {
      $set = $graph.Targets[$t][$kind]
      $ids = foreach ($k in $set.Keys) {
        $p = $set[$k]
        if ($p.Local) { continue }
        if (-not $pkgs.ContainsKey($k)) { $pkgs[$k] = @{ P = $p; Kinds = @{} } }
        $pkgs[$k].Kinds["$t $kind"] = $true
        $k
      }
      $kinds[$kind] = @(Sort-Ordinal $ids)
    }
    $local = @(foreach ($kind in "shipped", "host", "dev") { foreach ($p in $graph.Targets[$t][$kind].Values) { if ($p.Local) { $p.Name } } })
    $tdocs += [ordered]@{ triple = $t; role = $targets[$t]; workspace = @(Sort-Ordinal ($local | Select-Object -Unique)); shipped = $kinds.shipped; host = $kinds.host; dev = $kinds.dev; system = @($system[$t]) }
  }
  $docs = foreach ($k in (Sort-Ordinal $pkgs.Keys)) {
    $p = $pkgs[$k].P
    $class = Get-Class $pkgs[$k].Kinds
    $sum = $lock[$k]
    if ($null -eq $sum) { $fail += "$k is in the graph but not in Cargo.lock" } elseif (-not $sum) { $fail += "$k has no checksum in Cargo.lock (not from crates.io)" }
    $dir = Find-Source $registries $p.Name $p.Version
    $files = @(); $native = $null; $text = $null
    if (-not $dir) { $fail += "$k`: crate sources not found (run cargo fetch --locked)" }
    else {
      $lic = @(Get-ChildItem -LiteralPath $dir -Recurse -File | Where-Object { $_.Name -match $licenseFileRe } | ForEach-Object { $_.FullName.Substring($dir.Length + 1).Replace('\', '/') })
      $files = foreach ($f in (Sort-Ordinal $lic)) { [ordered]@{ path = $f; sha256 = (Get-Sha256 (Join-Path $dir $f)) } }
      $text = (@($files) | ForEach-Object { Get-NormText (Get-Content -Raw -LiteralPath (Join-Path $dir $_.path)) }) -join " "
      if ($class -eq "shipped") { $native = Get-Native $dir $p.License $p.Name }
    }
    $ast = Read-Spdx $p.License
    $allowed = Select-Spdx $ast $p.Name $p.Version $class
    $sel = if ($null -ne $allowed -and $null -ne $text) { Select-Spdx $ast $p.Name $p.Version $class $text } else { $null }
    $why = if ($null -eq $allowed) { "choice" } elseif ($null -ne $text -and $null -eq $sel) { "text" } else { "" }
    $isException = [bool]@($sel | Where-Object { $allow -notcontains $_ })
    [ordered]@{
      name = $p.Name; version = $p.Version; checksum = $sum; license = $p.License
      selected = if ($null -eq $sel) { $null } else { (@($sel) -join " AND ") }
      class = $class; host_only_exception = $isException
      used = @(Sort-Ordinal $pkgs[$k].Kinds.Keys)
      license_files = @($files)
      native = $native
      _dir = $dir; _why = $why
    }
  }
  $doc = [ordered]@{
    schema = "pod-mobile-sbom/2"
    root = "pod-mobile"
    features = @("mobile-lan")
    decision = "platform TLS only (user, 2026-11-13): SChannel + CNG on Windows, the system OpenSSL 3 on Linux; no rustls, ring, aws-lc or RustCrypto"
    regenerate = "powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1 -Write"
    policy = [ordered]@{
      allow = $allow
      shipped_exceptions = @()
      host_only = @($hostOnly | ForEach-Object { "$($_.Crate) $($_.Version) $($_.License): $($_.Why)" })
      excluded = $excluded
    }
    targets = $tdocs
    packages = @($docs)
  }
  return @{ Doc = $doc; Fail = $fail }
}

function Test-Policy($doc, $graph) {
  $fail = @()
  foreach ($p in $doc.packages) {
    $k = "$($p.name) $($p.version)"
    if ($p.name -match $excluded) { $fail += "$k`: excluded from the mobile-lan closure (platform TLS only, user decision 2026-11-13)" }
    if ($p._why -eq "choice") { $fail += "$k`: license '$($p.license)' has no allowed choice for a $($p.class) crate (allowed: $($allow -join ', '); host-only pins: $(@($hostOnly | ForEach-Object { "$($_.Crate) $($_.Version)" }) -join ', '))"; continue }
    if (-not $p._dir) { continue }
    if (@($p.license_files).Count -eq 0) { $fail += "$k`: ships no license file" }
    if ($p._why -eq "text") { $fail += "$k`: no license text for an allowed choice of '$($p.license)' in its license files" }
    if ($p.native) {
      foreach ($f in $p.native.files) {
        if ($null -eq (Select-Spdx (Read-Spdx $f.license) $p.name $p.version "shipped")) {
          $fail += "$k`: native file $($f.path) of a shipped crate is under '$($f.license)' ($($f.basis)), not an allowed license"
        }
      }
    }
  }
  foreach ($t in $targets.Keys) {
    $ship = @($graph.Targets[$t].shipped.Values | Where-Object { -not $_.Local } | ForEach-Object { $_.Name })
    if ($t -eq $windows) {
      if ($ship -notcontains "windows-sys") { $fail += "$t ships no windows-sys: the Windows TLS is SChannel/CNG through windows-sys" }
      foreach ($n in $ship) { if ($n -match '^openssl') { $fail += "$t ships $n`: Windows uses SChannel/CNG, not OpenSSL" } }
    }
    if ($t -eq $linux) {
      foreach ($n in "openssl", "openssl-sys") { if ($ship -notcontains $n) { $fail += "$t ships no $n`: the Linux TLS is the system OpenSSL 3" } }
    }
  }
  foreach ($label in $graph.Off.Keys) {
    $tp = @($graph.Off[$label].Values | Where-Object { -not $_.Local })
    if ($label -like "pod-mobile*") { foreach ($p in $tp) { $fail += "$label pulls in $($p.Name) $($p.Version); the default build must stay free of third-party crates" } }
    else { foreach ($p in $tp) { if ($p.Name -match $tlsCrates) { $fail += "$label pulls in $($p.Name) $($p.Version); TLS belongs behind mobile-lan" } } }
  }
  return $fail
}

function Format-Sbom($doc) {
  $copy = [ordered]@{}
  foreach ($k in $doc.Keys) { $copy[$k] = $doc[$k] }
  $copy.packages = @(foreach ($p in $doc.packages) { $q = [ordered]@{}; foreach ($k in $p.Keys) { if (-not $k.StartsWith("_")) { $q[$k] = $p[$k] } }; $q })
  return (ConvertTo-PodJson $copy) + "`n"
}

# Lines that differ between the recorded and the rebuilt SBOM (first few).
function Compare-Text([string]$have, [string]$want) {
  $a = @($have -replace "`r`n", "`n" -split "`n"); $b = @($want -split "`n")
  $out = @()
  for ($i = 0; $i -lt [Math]::Max($a.Count, $b.Count) -and $out.Count -lt 6; $i++) {
    $x = if ($i -lt $a.Count) { $a[$i] } else { "<end>" }; $y = if ($i -lt $b.Count) { $b[$i] } else { "<end>" }
    if ($x -cne $y) { $out += "line $($i + 1): recorded [$($x.Trim())] rebuilt [$($y.Trim())]" }
  }
  return $out
}

# All failures for one closure: rebuild, compare with the recorded text, policy.
function Test-Closure($graph, [string]$lockText, [string[]]$registries, $recorded) {
  $r = New-Sbom $graph $lockText $registries
  $want = Format-Sbom $r.Doc
  $fail = @($r.Fail)
  if ($null -ne $recorded -and ($recorded -replace "`r`n", "`n") -cne $want) {
    $fail += "$sbomPath does not match the lock, graph and sources; review the change, then run -Write: " + ((Compare-Text $recorded $want) -join "; ")
  }
  $fail += Test-Policy $r.Doc $graph
  return @{ Fail = $fail; Text = $want; Doc = $r.Doc }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-mobile-licenses-" + [Guid]::NewGuid().ToString("N"))
  $apache = "Apache License`n Version 2.0, January 2004"
  $mit = "Permission is hereby granted, free of charge, to any person obtaining a copy of this software"
  $isc = "Permission to use, copy, modify, and/or distribute this software for any`npurpose with or without fee is hereby granted"
  $bsd = "Redistribution and use in source and binary forms, with or without modification ... 3. Neither the name of the copyright holder"
  $unicode = "UNICODE LICENSE V3`n COPYRIGHT AND PERMISSION NOTICE"
  # A small closure like the real one: windows-sys on Windows; openssl,
  # openssl-sys (with its preprocessor probe) on Linux; a host-only
  # proc-macro chain ending in unicode-ident; a build-only crate.
  function New-Crates {
    return @{
      "windows-sys 0.61.2" = @{ "Cargo.toml" = "[package]"; "license-apache-2.0" = $apache; "license-mit" = $mit }
      "openssl 0.10.82" = @{ "Cargo.toml" = "[package]"; "LICENSE" = "Licensed under the Apache License, Version 2.0" }
      "openssl-sys 0.9.118" = @{ "Cargo.toml" = "[package]"; "LICENSE-MIT" = $mit; "build/expando.c" = "#include <openssl/opensslv.h>" }
      "unicode-ident 1.0.27" = @{ "Cargo.toml" = "[package]"; "LICENSE-APACHE" = $apache; "LICENSE-MIT" = $mit; "LICENSE-UNICODE" = $unicode }
      "cc 1.11.1" = @{ "Cargo.toml" = "[package]"; "LICENSE-MIT" = $mit; "LICENSE-APACHE" = $apache; "src/probe.c" = "#ifdef __clang__" }
    }
  }
  function New-Pkg([string]$k, [string]$lic, [bool]$local = $false) { $nv = $k -split ' '; return @{ Name = $nv[0]; Version = $nv[1]; License = $lic; Local = $local } }
  # $extra: @{ "triple class" = @{ "name ver" = "SPDX" } }
  function New-Graph($extra) {
    $me = New-Pkg "pod-mobile 0.1.0" "Apache-2.0" $true
    $g = @{ Targets = @{}; Off = [ordered]@{ "pod-mobile without features ($windows)" = @{ "pod-mobile 0.1.0" = $me }
                                              "podd default features ($windows)" = @{ "serde 1.0.0" = (New-Pkg "serde 1.0.0" "MIT OR Apache-2.0") } } }
    $g.Targets[$windows] = @{ shipped = @{ "pod-mobile 0.1.0" = $me; "windows-sys 0.61.2" = (New-Pkg "windows-sys 0.61.2" "MIT OR Apache-2.0") }; host = @{}; dev = @{} }
    $g.Targets[$linux] = @{ shipped = @{ "pod-mobile 0.1.0" = $me; "openssl 0.10.82" = (New-Pkg "openssl 0.10.82" "Apache-2.0"); "openssl-sys 0.9.118" = (New-Pkg "openssl-sys 0.9.118" "MIT") }
                            host = @{ "unicode-ident 1.0.27" = (New-Pkg "unicode-ident 1.0.27" "(MIT OR Apache-2.0) AND Unicode-3.0"); "cc 1.11.1" = (New-Pkg "cc 1.11.1" "MIT OR Apache-2.0") }; dev = @{} }
    foreach ($tk in $extra.Keys) {
      $tc = $tk -split ' '
      foreach ($k in $extra[$tk].Keys) {
        if ($null -eq $extra[$tk][$k]) { $g.Targets[$tc[0]][$tc[1]].Remove($k) } else { $g.Targets[$tc[0]][$tc[1]][$k] = New-Pkg $k $extra[$tk][$k] }
      }
    }
    return $g
  }
  function Write-Crates([string]$reg, $crates) {
    foreach ($k in $crates.Keys) {
      $dir = Join-Path $reg ("index.test\" + ($k -replace ' ', '-'))
      foreach ($f in $crates[$k].Keys) {
        $path = Join-Path $dir $f
        [void](New-Item -ItemType Directory -Force (Split-Path $path -Parent))
        [IO.File]::WriteAllText($path, $crates[$k][$f])
      }
    }
  }
  function New-Lock($pkgs) { return (($pkgs.Keys | ForEach-Object { $nv = $_ -split ' '; "[[package]]`nname = `"$($nv[0])`"`nversion = `"$($nv[1])`"`nchecksum = `"$($pkgs[$_])`"`n" }) -join "`n") }
  $lockGood = @{ "windows-sys 0.61.2" = "a1"; "openssl 0.10.82" = "a2"; "openssl-sys 0.9.118" = "a3"; "unicode-ident 1.0.27" = "a4"; "cc 1.11.1" = "a5"
                 "x 1.0.0" = "b1"; "ring 0.17.14" = "b2"; "rustls 0.23.46" = "b3"; "subtle 2.6.1" = "b4"; "unicode-ident 1.0.28" = "b5"; "openssl-src 300.5.0" = "b6" }
  $L = "$linux shipped"; $LH = "$linux host"; $W = "$windows shipped"
  # Each case: graph changes, crates to add/replace, a tampered recorded SBOM
  # or leaking off builds, and substrings every expected FAIL must contain.
  $cases = @(
    @{ Name = "good"; Expect = @() },
    @{ Name = "GPL crate"; Graph = @{ $L = @{ "x 1.0.0" = "GPL-3.0-only" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "COPYING" = "GNU GENERAL PUBLIC LICENSE" } }
       Expect = @("x 1.0.0: license 'GPL-3.0-only' has no allowed choice") },
    @{ Name = "OpenSSL license in an AND"; Graph = @{ $L = @{ "x 1.0.0" = "ISC AND OpenSSL" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE" = $isc } }
       Expect = @("x 1.0.0: license 'ISC AND OpenSSL' has no allowed choice") },
    @{ Name = "AND with a copyleft term"; Graph = @{ $W = @{ "x 1.0.0" = "(MIT OR Apache-2.0) AND LGPL-2.1-or-later" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE" = $mit } }
       Expect = @("has no allowed choice") },
    @{ Name = "OR without an allowed branch, WITH exception"; Graph = @{ $L = @{ "x 1.0.0" = "GPL-2.0-only WITH Classpath-exception-2.0 OR MPL-2.0" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE" = $mit } }
       Expect = @("has no allowed choice") },
    @{ Name = "OR picks the allowed branch (MIT), legacy slash"; Graph = @{ $L = @{ "x 1.0.0" = "GPL-3.0-only/MIT" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE-MIT" = $mit } }
       Expect = @(); Selected = @{ "x" = "MIT" } },
    @{ Name = "BSD without an exception (rustls's subtle) is refused twice"; Graph = @{ $W = @{ "subtle 2.6.1" = "BSD-3-Clause" } }; Crates = @{ "subtle 2.6.1" = @{ "Cargo.toml" = ""; "LICENSE" = $bsd } }
       Expect = @("subtle 2.6.1: license 'BSD-3-Clause' has no allowed choice", "subtle 2.6.1: excluded from the mobile-lan closure") },
    @{ Name = "OR picks the branch whose text the crate ships"; Graph = @{ $L = @{ "x 1.0.0" = "MIT OR Apache-2.0" } }; Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE-MIT" = $mit } }
       Expect = @(); Selected = @{ "x" = "MIT"; "windows-sys" = "Apache-2.0"; "openssl" = "Apache-2.0"; "unicode-ident" = "Apache-2.0 AND Unicode-3.0"; "cc" = "Apache-2.0" } },
    @{ Name = "the allowed license's text is missing; no license file at all"; Graph = @{ $L = @{ "x 1.0.0" = "Apache-2.0" } }
       Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE-MIT" = $mit }; "openssl-sys 0.9.118" = @{ "Cargo.toml" = "" } }
       Expect = @("x 1.0.0: no license text for an allowed choice", "openssl-sys 0.9.118: ships no license file", "openssl-sys 0.9.118: no license text for an allowed choice") },
    @{ Name = "native file under OpenSSL and an unreviewed binary in a shipped crate"; Graph = @{ $L = @{ "x 1.0.0" = "MIT" } }
       Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE" = $mit; "c/bn.c" = "/* $mit`n * This product includes software written by Eric Young. THIS SOFTWARE IS PROVIDED BY THE OpenSSL PROJECT */"; "c/blob.o" = "OBJ" } }
       Expect = @("x 1.0.0: native file c/blob.o of a shipped crate", "x 1.0.0: native file c/bn.c of a shipped crate") },
    @{ Name = "binaries in a host-only crate are not shipped"; Graph = @{ $LH = @{ "x 1.0.0" = "MIT" } }
       Crates = @{ "x 1.0.0" = @{ "Cargo.toml" = ""; "LICENSE" = $mit; "test-data/a.dll" = "MZ" } }
       Expect = @() },
    @{ Name = "ring, rustls and a vendored OpenSSL are excluded"; Graph = @{ $W = @{ "ring 0.17.14" = "Apache-2.0 AND ISC"; "rustls 0.23.46" = "Apache-2.0 OR ISC OR MIT" }; $L = @{ "openssl-src 300.5.0" = "MIT OR Apache-2.0" } }
       Crates = @{ "ring 0.17.14" = @{ "Cargo.toml" = ""; "LICENSE" = "$apache $isc" }; "rustls 0.23.46" = @{ "Cargo.toml" = ""; "LICENSE-APACHE" = $apache }; "openssl-src 300.5.0" = @{ "Cargo.toml" = ""; "LICENSE-MIT" = $mit } }
       Expect = @("ring 0.17.14: excluded", "rustls 0.23.46: excluded", "openssl-src 300.5.0: excluded") },
    @{ Name = "the host-only pin is not a shipped exception and fits one version only"; Graph = @{ $LH = @{ "unicode-ident 1.0.27" = $null; "unicode-ident 1.0.28" = "(MIT OR Apache-2.0) AND Unicode-3.0" }; $W = @{ "unicode-ident 1.0.27" = "(MIT OR Apache-2.0) AND Unicode-3.0" } }
       Crates = @{ "unicode-ident 1.0.28" = @{ "Cargo.toml" = ""; "LICENSE-APACHE" = $apache; "LICENSE-UNICODE" = $unicode } }
       Expect = @("unicode-ident 1.0.27: license '(MIT OR Apache-2.0) AND Unicode-3.0' has no allowed choice for a shipped crate", "unicode-ident 1.0.28: license '(MIT OR Apache-2.0) AND Unicode-3.0' has no allowed choice for a host crate") },
    @{ Name = "the wrong TLS stack per target"; Graph = @{ $W = @{ "windows-sys 0.61.2" = $null; "openssl 0.10.82" = "Apache-2.0" }; $L = @{ "openssl-sys 0.9.118" = $null } }
       Expect = @("$windows ships no windows-sys", "$windows ships openssl: Windows uses SChannel/CNG", "$linux ships no openssl-sys") },
    @{ Name = "off builds pull in TLS or third-party crates"; Off = $true
       Expect = @("podd default features ($windows) pulls in openssl 0.10.82", "pod-mobile without features ($windows) pulls in windows-sys 0.61.2") },
    @{ Name = "lock and sources missing"; Graph = @{ $L = @{ "y 1.0.0" = "MIT" } }
       Expect = @("y 1.0.0 is in the graph but not in Cargo.lock", "y 1.0.0: crate sources not found") },
    @{ Name = "recorded SBOM differs: tampered native file, changed lock checksum"; Tamper = $true
       Expect = @("docs/mobile-m2/sbom.json does not match") }
  )
  $bad = @()
  try {
    foreach ($c in $cases) {
      $reg = Join-Path $tmp ([Guid]::NewGuid().ToString("N"))
      $crates = New-Crates
      if ($c.Crates) { foreach ($k in $c.Crates.Keys) { $crates[$k] = $c.Crates[$k] } }
      Write-Crates $reg $crates
      $extra = if ($c.Graph) { $c.Graph } else { @{} }
      $g = New-Graph $extra
      if ($c.Off) {
        $g.Off["podd default features ($windows)"]["openssl 0.10.82"] = New-Pkg "openssl 0.10.82" "Apache-2.0"
        $g.Off["pod-mobile without features ($windows)"]["windows-sys 0.61.2"] = New-Pkg "windows-sys 0.61.2" "MIT OR Apache-2.0"
      }
      $lock = New-Lock $lockGood
      $recorded = $null
      if ($c.Tamper) {
        $recorded = (Test-Closure $g $lock @($reg) $null).Text
        [IO.File]::WriteAllText((Join-Path $reg "index.test\openssl-sys-0.9.118\build\expando.c"), "#include <openssl/ssl.h>")
        $lock = $lock -replace 'checksum = "a2"', 'checksum = "a0"'
      }
      $r = Test-Closure $g $lock @($reg) $recorded
      $got = @($r.Fail)
      $miss = @($c.Expect | Where-Object { $e = $_; -not @($got | Where-Object { $_.Contains($e) }) })
      $extraFail = @($got | Where-Object { $f = $_; -not @($c.Expect | Where-Object { $f.Contains($_) }) })
      if ($miss.Count -gt 0 -or $extraFail.Count -gt 0) { $bad += "case '$($c.Name)': missing [$($miss -join ' | ')], unexpected [$($extraFail -join ' | ')]" }
      if ($c.Selected) {
        foreach ($n in $c.Selected.Keys) {
          $p = @($r.Doc.packages | Where-Object { $_.name -eq $n })[0]
          if ($p.selected -ne $c.Selected[$n]) { $bad += "case '$($c.Name)': $n selected [$($p.selected)], want [$($c.Selected[$n])]" }
        }
      }
      if ($c.Name -eq "good") {
        $want = @{ "windows-sys" = "shipped"; "openssl" = "shipped"; "openssl-sys" = "shipped"; "unicode-ident" = "host"; "cc" = "host" }
        foreach ($n in $want.Keys) {
          $p = @($r.Doc.packages | Where-Object { $_.name -eq $n })[0]
          if ($p.class -ne $want[$n]) { $bad += "case 'good': $n class [$($p.class)], want [$($want[$n])]" }
        }
        $ui = @($r.Doc.packages | Where-Object { $_.name -eq "unicode-ident" })[0]
        if (-not $ui.host_only_exception -or $ui.selected -ne "Apache-2.0 AND Unicode-3.0") { $bad += "case 'good': unicode-ident should pass as the pinned host-only term" }
        $sys = @($r.Doc.packages | Where-Object { $_.name -eq "openssl-sys" })[0]
        $exp = @($sys.native.files | Where-Object { $_.path -eq "build/expando.c" })[0]
        if ((-not $exp) -or ($exp.license -ne "MIT") -or ($exp.basis -ne "crate license (Cargo.toml)") -or ($sys.native.use -notmatch 'preprocessor')) { $bad += "case 'good': openssl-sys build/expando.c should be listed under MIT with its preprocessor use" }
        $cc = @($r.Doc.packages | Where-Object { $_.name -eq "cc" })[0]
        if ($null -ne $cc.native) { $bad += "case 'good': a host-only crate's native files are not listed" }
        $again = Test-Closure $g $lock @($reg) $r.Text
        if (@($again.Fail).Count -gt 0) { $bad += "case 'good': the rebuilt SBOM does not match itself: $($again.Fail -join ' | ')" }
        if ((Select-Spdx (Read-Spdx "(MIT OR Apache-2.0) AND ISC") "z" "1" "shipped").Count -ne 2) { $bad += "SPDX: (MIT OR Apache-2.0) AND ISC should need two licenses" }
        if ($null -ne (Select-Spdx (Read-Spdx "MIT AND (") "z" "1" "shipped")) { $bad += "SPDX: a malformed expression should not pass" }
      }
    }
  } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
  if ($bad.Count -gt 0) { $bad | ForEach-Object { Write-Output "SELFTEST FAIL: $_" }; exit 1 }
  Write-Output "check-mobile-licenses selftest ok ($($cases.Count) cases)"
  exit 0
}

$rootPath = (Resolve-Path $Root).Path
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$registries = @(Join-Path $cargoHome "registry\src")
$graph = Get-Graph $rootPath
$lockText = Get-Content -Raw -LiteralPath (Join-Path $rootPath "Cargo.lock")
$file = Join-Path $rootPath $sbomPath
$recorded = if ($Write) { $null } elseif (Test-Path -LiteralPath $file) { [IO.File]::ReadAllText($file) } else { "" }
$r = Test-Closure $graph $lockText $registries $recorded
if ($Write) {
  [void](New-Item -ItemType Directory -Force (Split-Path $file -Parent))
  [IO.File]::WriteAllText($file, $r.Text, (New-Object Text.UTF8Encoding $false))
  Write-Output "wrote $sbomPath; review the diff"
}
if (@($r.Fail).Count -gt 0) { $r.Fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
$pk = @($r.Doc.packages)
$byClass = foreach ($c in "shipped", "host", "dev") { "$(@($pk | Where-Object { $_.class -eq $c }).Count) $c" }
$excl = @($pk | Where-Object { $_.host_only_exception } | ForEach-Object { "$($_.name) $($_.version)" })
Write-Output "mobile license gate ok ($($pk.Count) crates over $($targets.Count) targets match $sbomPath`: $($byClass -join ', '); shipped exceptions: none; host-only pins: $(if ($excl) { $excl -join ', ' } else { 'none' }); excluded crates absent (rustls, ring, aws-lc, subtle, RustCrypto, openssl-src); Windows ships windows-sys, Linux ships openssl; off builds TLS-free)"
