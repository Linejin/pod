# Docs index check: every numbered doc (docs/NN-*.md) must be linked from the
# README document table (quality-08), and a doc header that states
# "기준: 프로토콜 vN" must match PROTOCOL_VERSION in podd (quality-09).
#   powershell -ExecutionPolicy Bypass -File tools/check-docs.ps1
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$readme = Get-Content (Join-Path $root "README.md") -Raw -Encoding UTF8
$missing = @(Get-ChildItem (Join-Path $root "docs") -Filter "*.md" | Where-Object { $_.Name -match '^\d\d-' } |
  Where-Object { $readme -notmatch [regex]::Escape("(docs/$($_.Name))") } | ForEach-Object { $_.Name })
$fail = @($missing | ForEach-Object { "README.md does not link docs/$_" })
$proto = [regex]::Match((Get-Content (Join-Path $root "crates\pod-protocol\src\protocol.rs") -Raw), 'PROTOCOL_VERSION: u32 = (\d+)').Groups[1].Value
if (-not $proto) { $fail += "PROTOCOL_VERSION not found in crates/pod-protocol/src/protocol.rs" }
Get-ChildItem (Join-Path $root "docs") -Filter "*.md" | ForEach-Object {
  foreach ($m in [regex]::Matches((Get-Content $_.FullName -Raw -Encoding UTF8), '기준: 프로토콜 v(\d+)')) {
    if ($m.Groups[1].Value -ne $proto) { $fail += "docs/$($_.Name) says protocol v$($m.Groups[1].Value), code is v$proto" }
  }
}
if ($fail.Count -gt 0) { $fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "docs index ok"
