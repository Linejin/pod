# Writes THIRD-PARTY-NOTICES.txt for the shipped Pod binaries.
#
#   powershell -ExecutionPolicy Bypass -File tools/installer/notices.ps1 -Out dist\THIRD-PARTY-NOTICES.txt
#
# Source of truth: `cargo metadata --locked` (Cargo.lock), filtered to the
# x86_64-pc-windows-msvc target. Starting from the shipped workspace crates it
# follows only *normal* dependencies (build-dependencies and dev-dependencies
# are not in the shipped files). For every crate it lists name, version,
# SPDX license and repository, and copies the license/notice files found in
# the crate's package (LICENSE*, LICENCE*, COPYING*, NOTICE*, COPYRIGHT*,
# UNLICENSE*, plus `license-file`). Identical texts are printed once with the
# list of crates that use them. Also appends the non-crate components
# (Microsoft ConPTY, Lucide icons).
#
# Fails when a shipped crate has a GPL-family license (GPL/LGPL/AGPL) or no
# license at all. cargo-deny (deny.toml) is the policy gate; this is a second,
# shipped-files-only check.
param(
  [Parameter(Mandatory = $true)][string]$Out,
  [string[]]$Roots = @("pod-app", "podd", "pod-cli", "pod-hook"),
  [string]$Target = "x86_64-pc-windows-msvc",
  [string]$Version = ""
)
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Push-Location $Root
try {
  $raw = (& cargo metadata --format-version 1 --locked --filter-platform $Target) -join "`n"
  if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed ($LASTEXITCODE)" }
} finally { Pop-Location }

Add-Type -AssemblyName System.Web.Extensions
$ser = New-Object System.Web.Script.Serialization.JavaScriptSerializer
$ser.MaxJsonLength = [int]::MaxValue
$meta = $ser.DeserializeObject($raw)

$pkgs = @{}
foreach ($p in $meta["packages"]) { $pkgs[$p["id"]] = $p }
$nodes = @{}
foreach ($n in $meta["resolve"]["nodes"]) { $nodes[$n["id"]] = $n }
$ws = @{}
foreach ($id in $meta["workspace_members"]) { $ws[$id] = $true }

# Walk normal dependencies from the shipped roots.
$queue = New-Object System.Collections.Queue
$seen = @{}
foreach ($name in $Roots) {
  $id = @($meta["workspace_members"] | Where-Object { $pkgs[$_]["name"] -eq $name })
  if ($id.Count -ne 1) { throw "workspace crate '$name' not found" }
  $queue.Enqueue($id[0]); $seen[$id[0]] = $true
}
while ($queue.Count -gt 0) {
  $id = $queue.Dequeue()
  foreach ($d in $nodes[$id]["deps"]) {
    $normal = $false
    foreach ($k in $d["dep_kinds"]) { if ($null -eq $k["kind"]) { $normal = $true } }
    if ($normal -and -not $seen.ContainsKey($d["pkg"])) {
      $seen[$d["pkg"]] = $true; $queue.Enqueue($d["pkg"])
    }
  }
}

$ownCrates = @()
$crates = @()
foreach ($id in $seen.Keys) {
  $p = $pkgs[$id]
  if ($ws.ContainsKey($id)) { $ownCrates += $p["name"]; continue }
  $crates += $p
}
$crates = $crates | Sort-Object { $_["name"] }, { $_["version"] }

$texts = [ordered]@{}     # hash -> @{ text; users }
$rows = @()
$bad = @()
$missing = @()
$sha = [System.Security.Cryptography.SHA256]::Create()
$namePattern = '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)'

function Add-Text([string]$text, [string]$user) {
  $norm = ($text -replace "`r`n", "`n").Trim()
  $h = [BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($norm)))
  if (-not $texts.Contains($h)) { $texts[$h] = @{ text = $norm; users = New-Object System.Collections.ArrayList } }
  [void]$texts[$h].users.Add($user)
}

$mitBody = @"
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"@

foreach ($p in $crates) {
  $label = "$($p["name"]) $($p["version"])"
  $lic = $p["license"]
  $repo = $p["repository"]
  if (-not $repo) { $repo = $p["homepage"] }
  if (-not $repo) { $repo = "https://crates.io/crates/$($p["name"])" }
  if (-not $lic -and $p["license_file"]) { $lic = "see license file" }
  if (-not $lic) { $bad += "$label has no license" ; $lic = "(none)" }
  # GPL family anywhere in the expression is refused unless an OR offers another
  # license; only "X OR GPL" style expressions are accepted.
  if ($lic -match 'GPL' -and $lic -notmatch '\bOR\b') { $bad += "$label is $lic" }
  $rows += [pscustomobject]@{ Crate = $p["name"]; Version = $p["version"]; License = $lic; Source = $repo }

  $dir = Split-Path -Parent $p["manifest_path"]
  $files = @(Get-ChildItem -LiteralPath $dir -File -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match $namePattern } | Sort-Object Name)
  if ($p["license_file"]) {
    $lf = Join-Path $dir $p["license_file"]
    if ((Test-Path -LiteralPath $lf) -and -not ($files | Where-Object { $_.FullName -eq (Resolve-Path -LiteralPath $lf).Path })) {
      $files += Get-Item -LiteralPath $lf
    }
  }
  if ($files.Count -gt 0) {
    foreach ($f in $files) {
      Add-Text ("[" + $f.Name + "]`n" + [System.IO.File]::ReadAllText($f.FullName)) $label
    }
  } else {
    $missing += $label
    $authors = ($p["authors"] -join ", ")
    if (-not $authors) { $authors = "the $($p["name"]) authors" }
    $t = "[no license file in the published package; license: $lic; source: $repo]"
    if ($lic -match 'MIT') {
      $t += "`n`nMIT License`n`nCopyright (c) $authors`n`n$mitBody"
    }
    if ($lic -match 'Apache-2.0') {
      $t += "`n`nApache License 2.0: the full text is in LICENSE.txt shipped with Pod (identical license text)."
    }
    Add-Text $t $label
  }
}

if ($bad.Count -gt 0) {
  throw ("license check failed:`n  " + ($bad -join "`n  "))
}

$sb = New-Object System.Text.StringBuilder
$nl = "`r`n"
function W([string]$s = "") { [void]$sb.Append(($s -replace "`r?`n", $nl) + $nl) }

W "Pod $Version - third-party notices"
W "==================================="
W ""
W "Pod is licensed under the Apache License 2.0 (LICENSE.txt). The shipped programs"
W "(pod-app.exe, podd.exe, pod-holder.exe, pod.exe, pod-hook.exe) contain the open"
W "source components listed below. Generated by tools/installer/notices.ps1 from"
W "Cargo.lock (target $Target, normal dependencies of: $($Roots -join ', '))."
W ""
W "Pod's own crates (Apache-2.0): $((($ownCrates | Sort-Object) -join ', '))"
W ""
W "1. Other components"
W "-------------------"
W ""
W "Microsoft.Windows.Console.ConPTY 1.25.260930003 (conpty.dll, x64\OpenConsole.exe)"
W "  License: MIT - https://github.com/microsoft/terminal - text in section 4."
W "Lucide icons 0.468.0 (SVG, embedded in pod-app via the pod-ui crate)"
W "  License: ISC (Feather-derived portions MIT) - https://github.com/lucide-icons/lucide - text in section 4."
W ""
W "2. Rust crates ($($rows.Count))"
W "------------------"
W ""
$w1 = [Math]::Max(5, ($rows | ForEach-Object { $_.Crate.Length } | Measure-Object -Maximum).Maximum)
$w2 = [Math]::Max(7, ($rows | ForEach-Object { $_.Version.Length } | Measure-Object -Maximum).Maximum)
foreach ($r in $rows) {
  W ("{0}  {1}  {2}  {3}" -f $r.Crate.PadRight($w1), $r.Version.PadRight($w2), $r.License, $r.Source)
}
W ""
W "Crates whose package has no license file (a standard text is given instead): $($missing.Count)"
if ($missing.Count -gt 0) { W ("  " + ($missing -join ", ")) }
W ""
W "3. License texts of the crates (each distinct text once)"
W "--------------------------------------------------------"
foreach ($h in $texts.Keys) {
  $e = $texts[$h]
  W ""
  W ("=" * 78)
  W ("Used by: " + (($e.users | Sort-Object -Unique) -join ", "))
  W ("=" * 78)
  W $e.text
}
W ""
W "4. Other component license texts"
W "--------------------------------"
W ""
W ([System.IO.File]::ReadAllText((Join-Path $PSScriptRoot "licenses\ConPTY-LICENSE.txt")))
W ""
W ([System.IO.File]::ReadAllText((Join-Path $Root "crates\pod-ui\THIRD-PARTY.md")))

$dirOut = Split-Path -Parent $Out
if ($dirOut) { New-Item -ItemType Directory -Force $dirOut | Out-Null }
[System.IO.File]::WriteAllText($Out, $sb.ToString(), (New-Object System.Text.UTF8Encoding($true)))
$licSummary = $rows | Group-Object License | Sort-Object Count -Descending | ForEach-Object { "$($_.Count) $($_.Name)" }
Write-Host "notices: $($rows.Count) crates, $($texts.Count) distinct texts, $($missing.Count) without license file -> $Out"
Write-Host ("licenses: " + ($licSummary -join "; "))
